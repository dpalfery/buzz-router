//! RED tests for T2.5: discovery, subscription, backfill and cursors
//! (design §10.2, §10.3, §10.4, §6.2 first run; requirements R47.4, R48.1
//! steps 2-4, R48.2, R48.4, R50.1, R61.2, R61.3; assumption A14).
//!
//! The implementation does not exist yet:
//! `buzz_router::relay::discovery::{discover_channels, DiscoveredChannel}`,
//! `buzz_router::relay::backfill::{backfill_since, OVERLAP_SECS}` and
//! `buzz_router::relay::conn::{spawn_synced_connection, SyncParams, Delivered}`
//! are all unresolved, so `cargo test -p buzz-router --test relay_backfill`
//! must fail with unresolved-import errors (E0432/E0433). Once those items
//! exist as stubs, the subscription, sink-ordering and reconnect assertions
//! below fail against the current connection task, which sends no REQs and
//! delivers nothing to the sink.
//!
//! Planned GREEN API (GREEN worker: implement exactly this):
//!
//! - `discovery::DiscoveredChannel { pub id: Uuid, pub name: String }`
//! - `discovery::discover_channels(rest: &RestClient, bot_pubkey_hex: &str)
//!   -> Result<Vec<DiscoveredChannel>, RelayError>`:
//!   REST-query kind 39002 with `#p = bot pubkey`, take channel UUIDs from the
//!   `d` tags, REST-query kind 39000 with `#d = those UUIDs` for names, and
//!   skip channels whose metadata carries `["archived", "true"]` (design
//!   §10.2, mirroring `buzz-acp` `discover_channels` /
//!   `merge_discovered_channels`).
//! - `backfill::OVERLAP_SECS: u64 = 300`
//! - `backfill::backfill_since(rest: &RestClient, channel: Uuid,
//!   since_secs: u64) -> Result<Vec<Event>, RelayError>`: one filter
//!   `{kinds: [9, 40003], "#h": [uuid], since, limit: 500}`, paged with
//!   `until` + `before_id` from the oldest event of each full page until a
//!   short page (paging rides on `RestClient::query`), returned ascending by
//!   `(created_at, id)` (design §10.4).
//! - `conn::Delivered { pub channel: Uuid, pub source: ingest::Source,
//!   pub event: Event }`
//! - `conn::SyncParams { pub conn: ConnParams, pub rest: RestClient,
//!   pub store: Store, pub bot: BotName,
//!   pub sink: mpsc::UnboundedSender<Delivered>, pub core: CoreHandle,
//!   pub membership_tap: Option<mpsc::UnboundedSender<(BotName,
//!   BTreeSet<ChannelId>)>> }`: after every successful discovery the task
//!   sends `Memberships{bot, channels}` to `core` (design §10.2 step 3) and a
//!   copy to `membership_tap` when present (tests observe the report there).
//! - `conn::spawn_synced_connection(params: SyncParams) -> Connection`
//!   (reuses the existing `Connection` handle: `wait_up`, `publish`,
//!   `attempts`). After authentication, on every (re)connect it discovers,
//!   sends one `["REQ", "ch-<uuid>", {kinds: [9, 40003], "#h": [uuid],
//!   since: connect time}]` per live channel spaced 125 ms apart (design
//!   §10.3), then per channel either skips backfill when there is no cursor
//!   and advances the cursor to connect time (A14, design §6.2 first run) or
//!   backfills from `cursor - OVERLAP_SECS`. Backfill is delivered to `sink`
//!   as `Source::Backfill` ascending before any live event buffered meanwhile,
//!   which follows as `Source::Live`; the cursor key is
//!   `(bot, conn.relay_url)` and never moves backwards. Discovery,
//!   subscription and backfill rerun on every reconnect (R50.1, R61.2).
//!
//! TIMING DEVIATION (documented inline, per T2.4 precedent): the contract says
//! "(paused time)", but this workspace's `tokio` dependency does not enable
//! `test-util` for the test profile in a way that makes
//! `#[tokio::test(start_paused = true)]` usable here (see `relay_conn.rs`),
//! so all timing assertions run on REAL time with widened bands: REQ spacing
//! asserts gaps >= 100 ms (GREEN sleeps 125 ms between REQs), and the
//! reconnect redial relies on the design §10.5 1 s first rung inside a 20 s
//! guard.
//!
//! Conventions: both mocks bind `127.0.0.1:0` (localhost only); the REST mock
//! is an axum server built from `ws://127.0.0.1:<rest-port>` (mapped to http
//! by `RestClient::new`, never dialled as WebSocket); the WebSocket mock is a
//! `tokio-tungstenite` server like `relay_conn.rs`. Every key comes from
//! `support::keys` (deterministic fixture secrets, no real keys); the
//! `since`/cursor windows use real system time with ±10 s guards.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "helpers and mock tasks in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use buzz_router::ingest::Source;
use buzz_router::relay::backfill::{backfill_since, OVERLAP_SECS};
use buzz_router::relay::conn::{spawn_synced_connection, ConnParams, Delivered, SyncParams};
use buzz_router::relay::discovery::{discover_channels, DiscoveredChannel};
use buzz_router::relay::rest::RestClient;
use buzz_router::store::Store;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use nostr::{Event, JsonUtil};
use router_core::ids::{BotName, ChannelId};
use support::{keys, message, raw_event, spawn_test_core};
use tokio::sync::mpsc;
use uuid::Uuid;

/// The overlap backfill subtracts from the cursor (R48.2, design §10.4).
const EXPECTED_OVERLAP: u64 = 300;

/// A connected mock-relay stream (server side of `accept_async`).
type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

// --- Mock REST server -------------------------------------------------------

/// What the mock relay serves on `POST /query`, plus a log of every request.
struct RestState {
    /// (bot pubkey hex, channel uuid) pairs, served as kind-39002 events.
    memberships: Vec<(String, String)>,
    /// (channel uuid, name, archived) triples, served as kind-39000 events.
    metadata: Vec<(String, String, bool)>,
    /// Canned backfill pages for `#h` queries: page N is served on hit N.
    backfill_pages: Vec<serde_json::Value>,
    /// Extra latency before answering `#h` queries (lets a live event arrive
    /// mid-backfill in the buffering test).
    backfill_delay: Duration,
    /// The first this-many `#h` queries fail with HTTP 400 (not retried).
    backfill_failures: usize,
    /// The first this-many kind-39002 discovery queries fail with HTTP 400.
    discovery_failures: usize,
    /// Every request body received, in order.
    requests: Vec<serde_json::Value>,
}

impl RestState {
    fn new() -> Self {
        Self {
            memberships: Vec::new(),
            metadata: Vec::new(),
            backfill_pages: Vec::new(),
            backfill_delay: Duration::ZERO,
            backfill_failures: 0,
            discovery_failures: 0,
            requests: Vec::new(),
        }
    }
}

type SharedRest = Arc<Mutex<RestState>>;

fn kinds_of(filter: &serde_json::Value) -> Vec<u64> {
    filter["kinds"]
        .as_array()
        .map(|k| k.iter().filter_map(serde_json::Value::as_u64).collect())
        .unwrap_or_default()
}

async fn query_handler(
    State(state): State<SharedRest>,
    _headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let filters: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    let delay;
    let mut failed = false;
    let mut out: Vec<serde_json::Value> = Vec::new();
    {
        let mut guard = state.lock().unwrap();
        guard.requests.push(serde_json::from_slice(&body).unwrap());
        let hit = guard
            .requests
            .iter()
            .filter(|request| {
                request.as_array().is_some_and(|fs| {
                    fs.iter().any(|f| {
                        let kinds = kinds_of(f);
                        !kinds.contains(&39002) && !kinds.contains(&39000) && f.get("#h").is_some()
                    })
                })
            })
            .count();
        let discovery_hit = guard
            .requests
            .iter()
            .filter(|request| {
                request
                    .as_array()
                    .is_some_and(|fs| fs.iter().any(|f| kinds_of(f).contains(&39002)))
            })
            .count();
        for filter in &filters {
            let kinds = kinds_of(filter);
            if kinds.contains(&39002) && discovery_hit <= guard.discovery_failures {
                failed = true;
            } else if kinds.contains(&39002) {
                let wanted: Vec<String> = filter["#p"]
                    .as_array()
                    .map(|p| {
                        p.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                for (pubkey, channel) in &guard.memberships {
                    if wanted.contains(pubkey) {
                        let event = raw_event(
                            &keys("relay"),
                            39002,
                            "",
                            &[&["d", channel.as_str()], &["p", pubkey.as_str()]],
                            1_000,
                        );
                        out.push(serde_json::to_value(&event).unwrap());
                    }
                }
            } else if kinds.contains(&39000) {
                let wanted: Vec<String> = filter["#d"]
                    .as_array()
                    .map(|p| {
                        p.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                for (channel, name, archived) in &guard.metadata {
                    if wanted.contains(channel) {
                        let event = if *archived {
                            raw_event(
                                &keys("relay"),
                                39000,
                                "",
                                &[&["d", channel], &["name", name], &["archived", "true"]],
                                1_000,
                            )
                        } else {
                            raw_event(
                                &keys("relay"),
                                39000,
                                "",
                                &[&["d", channel], &["name", name]],
                                1_000,
                            )
                        };
                        out.push(serde_json::to_value(&event).unwrap());
                    }
                }
            } else if filter.get("#h").is_some() && hit <= guard.backfill_failures {
                failed = true;
            } else if filter.get("#h").is_some() {
                let page = guard
                    .backfill_pages
                    .get(hit.saturating_sub(1))
                    .cloned()
                    .unwrap_or(serde_json::Value::Array(Vec::new()));
                if let Some(items) = page.as_array() {
                    out.extend(items.iter().cloned());
                }
            }
        }
        delay = guard.backfill_delay;
    }
    if delay > Duration::ZERO {
        tokio::time::sleep(delay).await;
    }
    if failed {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    }
    Json(serde_json::Value::Array(out)).into_response()
}

async fn start_rest_mock(state: SharedRest) -> (String, u16) {
    let app = Router::new()
        .route("/query", post(query_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("ws://127.0.0.1:{port}"), port)
}

/// Every `#h` (backfill) request body seen by the mock, in order.
fn backfill_requests(state: &SharedRest) -> Vec<serde_json::Value> {
    state
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|request| {
            request.as_array().is_some_and(|fs| {
                fs.iter().any(|f| {
                    let kinds = kinds_of(f);
                    !kinds.contains(&39002) && !kinds.contains(&39000) && f.get("#h").is_some()
                })
            })
        })
        .cloned()
        .collect()
}

/// The single filter of a one-filter backfill request.
fn only_filter(body: &serde_json::Value) -> &serde_json::Value {
    let filters = body.as_array().unwrap();
    assert_eq!(filters.len(), 1, "one backfill filter: {body}");
    &filters[0]
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// --- Mock WebSocket server --------------------------------------------------

/// Bind a mock relay listener on localhost and return it with its URL.
async fn bind_ws() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, format!("ws://127.0.0.1:{port}/"))
}

/// Accept one connection and split it.
async fn accept_split(
    listener: &tokio::net::TcpListener,
) -> (
    SplitSink<Ws, tokio_tungstenite::tungstenite::Message>,
    SplitStream<Ws>,
) {
    let (tcp, _) = listener.accept().await.unwrap();
    tokio_tungstenite::accept_async(tcp).await.unwrap().split()
}

async fn send_array(
    sink: &mut SplitSink<Ws, tokio_tungstenite::tungstenite::Message>,
    value: serde_json::Value,
) {
    use tokio_tungstenite::tungstenite::Message;
    sink.send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

/// Read the next JSON array message, waiting up to `timeout`.
async fn recv_array_timeout(
    stream: &mut SplitStream<Ws>,
    timeout: Duration,
) -> Option<serde_json::Value> {
    use tokio_tungstenite::tungstenite::Message;
    tokio::time::timeout(timeout, async {
        loop {
            let msg = stream.next().await.unwrap().unwrap();
            if let Message::Text(text) = msg {
                let value: serde_json::Value = serde_json::from_str(text.as_ref()).unwrap();
                if value.is_array() {
                    return value;
                }
            }
        }
    })
    .await
    .ok()
}

/// Drive the server side of the NIP-42 handshake: send `["AUTH", challenge]`,
/// read the client's `["AUTH", event]`, check it is a kind-22242 event from
/// the bot, then reply `["OK", id, true]`.
async fn server_complete_auth(
    sink: &mut SplitSink<Ws, tokio_tungstenite::tungstenite::Message>,
    stream: &mut SplitStream<Ws>,
    expected_pubkey: nostr::PublicKey,
    relay_url: &str,
    challenge: &str,
) {
    send_array(sink, serde_json::json!(["AUTH", challenge])).await;
    let msg = recv_array_timeout(stream, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(msg[0].as_str().unwrap(), "AUTH");
    let event = Event::from_json(msg[1].to_string()).unwrap();
    assert_eq!(u16::from(event.kind), 22242);
    assert!(event.verify().is_ok());
    assert_eq!(event.pubkey, expected_pubkey);
    let _ = relay_url;
    send_array(sink, serde_json::json!(["OK", event.id.to_hex(), true])).await;
}

/// One REQ observed on the wire: the subscription id, filter and arrival time.
struct ObservedReq {
    sub_id: String,
    filter: serde_json::Value,
    at: std::time::Instant,
}

/// Read one `["REQ", sub_id, filter]` (fails the test on timeout).
async fn recv_req(stream: &mut SplitStream<Ws>) -> ObservedReq {
    let msg = recv_array_timeout(stream, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(msg[0].as_str().unwrap(), "REQ", "expected REQ, got {msg}");
    ObservedReq {
        sub_id: msg[1].as_str().unwrap().to_string(),
        filter: msg[2].clone(),
        at: std::time::Instant::now(),
    }
}

// --- Shared fixtures ---------------------------------------------------------

const CHANNEL_A: &str = "6a0e2f4c-1b3d-4e5f-8a7b-9c0d1e2f3a4b";
const CHANNEL_B: &str = "7b1f3a5d-2c4e-4f60-9b8c-0d1e2f3a4b5c";

fn bot_name() -> BotName {
    BotName::new("A").unwrap()
}

/// A file-backed store pair: one handle to move into the connection, one kept
/// by the test for seeding and assertions (in-memory databases cannot share).
fn file_stores() -> (Store, Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite3");
    let seed = Store::open(&path).unwrap();
    let conn = Store::open(&path).unwrap();
    (seed, conn, dir)
}

fn channel_uuid(raw: &str) -> Uuid {
    Uuid::parse_str(raw).unwrap()
}

// --- Test 1: discovery -------------------------------------------------------

#[tokio::test]
async fn discovery_names_two_channels_and_skips_the_archived_one() {
    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    {
        let mut guard = rest.lock().unwrap();
        let bot_hex = keys("A").public_key().to_hex();
        guard.memberships = vec![
            (bot_hex.clone(), CHANNEL_A.to_string()),
            (bot_hex, CHANNEL_B.to_string()),
        ];
        guard.metadata = vec![
            (CHANNEL_A.to_string(), "live".to_string(), false),
            (CHANNEL_B.to_string(), "dead".to_string(), true),
        ];
    }
    let (relay_url, _port) = start_rest_mock(rest.clone()).await;
    let client = RestClient::new(&relay_url, keys("A"), None);

    let channels: Vec<DiscoveredChannel> =
        discover_channels(&client, &keys("A").public_key().to_hex())
            .await
            .expect("discovery succeeds");

    assert_eq!(channels.len(), 1, "the archived channel is skipped");
    assert_eq!(channels[0].id, channel_uuid(CHANNEL_A));
    assert_eq!(channels[0].name, "live");

    let guard = rest.lock().unwrap();
    let saw_39002 = guard.requests.iter().any(|body| {
        body.as_array()
            .is_some_and(|fs| fs.iter().any(|f| kinds_of(f).contains(&39002)))
    });
    let saw_39000 = guard.requests.iter().any(|body| {
        body.as_array()
            .is_some_and(|fs| fs.iter().any(|f| kinds_of(f).contains(&39000)))
    });
    assert!(saw_39002, "discovery queries kind 39002 by #p");
    assert!(saw_39000, "discovery queries kind 39000 names by #d");
}

// --- Test 2: subscription -----------------------------------------------------

#[tokio::test]
async fn subscription_sends_one_req_per_live_channel_spaced_apart() {
    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    {
        let mut guard = rest.lock().unwrap();
        let bot_hex = keys("A").public_key().to_hex();
        guard.memberships = vec![
            (bot_hex.clone(), CHANNEL_A.to_string()),
            (bot_hex, CHANNEL_B.to_string()),
        ];
        guard.metadata = vec![
            (CHANNEL_A.to_string(), "one".to_string(), false),
            (CHANNEL_B.to_string(), "two".to_string(), false),
        ];
    }
    let (rest_url, _port) = start_rest_mock(rest.clone()).await;
    let (ws_listener, ws_url) = bind_ws().await;

    let (_seed, store, _dir) = file_stores();
    let (sink_tx, _sink_rx) = mpsc::unbounded_channel::<Delivered>();
    let connect_before = now_secs();
    let conn = spawn_synced_connection(SyncParams {
        conn: ConnParams::new(ws_url.clone(), keys("A"), None),
        rest: RestClient::new(&rest_url, keys("A"), None),
        store,
        bot: bot_name(),
        sink: sink_tx,
        core: spawn_test_core().0,
        membership_tap: None,
    });

    let (mut sink, mut stream) = accept_split(&ws_listener).await;
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-sub",
    )
    .await;
    let up = tokio::time::timeout(Duration::from_secs(5), conn.wait_up()).await;
    assert!(up.is_ok(), "the connection reports Up after auth");

    let first = recv_req(&mut stream).await;
    let second = recv_req(&mut stream).await;
    let mut sub_ids = vec![first.sub_id.clone(), second.sub_id.clone()];
    sub_ids.sort();
    assert_eq!(
        sub_ids,
        vec![format!("ch-{CHANNEL_A}"), format!("ch-{CHANNEL_B}"),],
        "one REQ ch-<uuid> per remaining channel"
    );
    for req in [&first, &second] {
        let kinds: Vec<u64> = req.filter["kinds"]
            .as_array()
            .map(|k| k.iter().filter_map(serde_json::Value::as_u64).collect())
            .unwrap_or_default();
        assert_eq!(kinds, vec![9, 40003], "REQ subscribes to kinds [9,40003]");
        let channel = req.sub_id.strip_prefix("ch-").unwrap();
        let tagged: Vec<String> = req.filter["#h"]
            .as_array()
            .map(|h| {
                h.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(tagged, vec![channel.to_string()], "REQ tags #h");
        let since = req.filter["since"].as_u64().expect("REQ carries since");
        assert!(
            since >= connect_before.saturating_sub(10) && since <= now_secs() + 10,
            "REQ since is connect time, got {since}"
        );
    }
    let gap = second.at.duration_since(first.at).as_millis();
    assert!(
        gap >= 100,
        "REQs are spaced at least ~125 ms apart (real-time band >= 100 ms), got {gap} ms"
    );
}

// --- Test 3: backfill paging and order ----------------------------------------

#[tokio::test]
async fn backfill_starts_at_cursor_minus_300_pages_to_completion_ascending() {
    assert_eq!(
        OVERLAP_SECS, EXPECTED_OVERLAP,
        "the backfill overlap is 300 s"
    );
    let connect = now_secs();
    let cursor = (connect - 1_000) as i64;
    let since = (cursor - OVERLAP_SECS as i64) as u64;

    let owner = keys("owner");
    let channel = channel_uuid(CHANNEL_A);
    // 507 events over distinct timestamps, served scrambled across two pages.
    let mut events: Vec<Event> = (0..507)
        .map(|i| {
            message(
                &owner,
                channel,
                &format!("backfill {i}"),
                None,
                &[],
                since + 1 + i,
            )
        })
        .collect();
    events.reverse();
    let pages = vec![
        serde_json::to_value(&events[0..500]).unwrap(),
        serde_json::to_value(&events[500..507]).unwrap(),
    ];
    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    rest.lock().unwrap().backfill_pages = pages;
    let (rest_url, _port) = start_rest_mock(rest.clone()).await;
    let client = RestClient::new(&rest_url, keys("A"), None);

    let fetched = backfill_since(&client, channel, since)
        .await
        .expect("backfill succeeds");

    assert_eq!(fetched.len(), 507, "backfill pages to completion");
    let mut expected = events.clone();
    expected.sort_by_key(|e| (e.created_at.as_secs(), e.id.to_hex()));
    let got: Vec<String> = fetched.iter().map(|e| e.id.to_hex()).collect();
    let want: Vec<String> = expected.iter().map(|e| e.id.to_hex()).collect();
    assert_eq!(got, want, "backfill is ascending by (created_at, id)");

    let queries = backfill_requests(&rest);
    assert_eq!(queries.len(), 2, "a full page is followed once");
    let first = only_filter(&queries[0]);
    assert_eq!(first["since"].as_u64(), Some(since));
    assert_eq!(first["limit"].as_u64(), Some(500));
    let tagged: Vec<String> = first["#h"]
        .as_array()
        .map(|h| {
            h.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(tagged, vec![CHANNEL_A.to_string()]);
    assert!(
        queries[0].to_string().find("\"until\"").is_none(),
        "the first page carries no cursor"
    );
    let second = only_filter(&queries[1]);
    // The oldest event of the full first page drives the paging cursor: the
    // first page holds the newest 500 of the 507 backfill events.
    let oldest_page_one = events[0..500]
        .iter()
        .min_by_key(|e| (e.created_at.as_secs(), e.id.to_hex()))
        .unwrap();
    assert_eq!(
        second["until"].as_u64(),
        Some(oldest_page_one.created_at.as_secs()),
        "the second page continues from the oldest event of the first"
    );
    assert_eq!(
        second["before_id"].as_str(),
        Some(oldest_page_one.id.to_hex()).as_deref(),
        "the second page carries before_id"
    );
}

// --- Test 4: first run ---------------------------------------------------------

#[tokio::test]
async fn first_run_skips_backfill_and_seeds_cursor_at_connect_time() {
    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    {
        let mut guard = rest.lock().unwrap();
        let bot_hex = keys("A").public_key().to_hex();
        guard.memberships = vec![(bot_hex, CHANNEL_A.to_string())];
        guard.metadata = vec![(CHANNEL_A.to_string(), "one".to_string(), false)];
        guard.backfill_pages = vec![serde_json::json!([serde_json::to_value(message(
            &keys("owner"),
            channel_uuid(CHANNEL_A),
            "history",
            None,
            &[],
            now_secs() - 5_000,
        ))
        .unwrap()])];
    }
    let (rest_url, _port) = start_rest_mock(rest.clone()).await;
    let (ws_listener, ws_url) = bind_ws().await;

    // No cursor is seeded: this (bot, relay) pair never connected (A14).
    let (seed, store, _dir) = file_stores();
    let bot = bot_name();
    assert_eq!(seed.cursors().get(&bot, &ws_url).unwrap(), None);

    let (sink_tx, mut sink_rx) = mpsc::unbounded_channel::<Delivered>();
    let (core, _relay, core_store) = spawn_test_core();
    let connect_before = now_secs();
    let conn = spawn_synced_connection(SyncParams {
        conn: ConnParams::new(ws_url.clone(), keys("A"), None),
        rest: RestClient::new(&rest_url, keys("A"), None),
        store,
        bot: bot.clone(),
        sink: sink_tx,
        core,
        membership_tap: None,
    });

    let (mut sink, mut stream) = accept_split(&ws_listener).await;
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-first",
    )
    .await;
    let up = tokio::time::timeout(Duration::from_secs(5), conn.wait_up()).await;
    assert!(up.is_ok(), "the connection reports Up after auth");

    // The channel is still subscribed: only the history fetch is skipped.
    let req = recv_req(&mut stream).await;
    assert_eq!(req.sub_id, format!("ch-{CHANNEL_A}"));

    // A live event flows straight to the sink as Live.
    let live = message(
        &keys("owner"),
        channel_uuid(CHANNEL_A),
        "live now",
        None,
        &[],
        now_secs(),
    );
    send_array(
        &mut sink,
        serde_json::json!(["EVENT", req.sub_id, serde_json::to_value(&live).unwrap()]),
    )
    .await;
    let delivered = tokio::time::timeout(Duration::from_secs(5), sink_rx.recv())
        .await
        .expect("a live event is delivered")
        .expect("the sink stays open");
    assert_eq!(delivered.event.id, live.id);
    assert_eq!(delivered.source, Source::Live);

    assert!(
        backfill_requests(&rest).is_empty(),
        "first run performs no backfill query"
    );
    // The core is the only cursor writer (DD-1): it stores the first-run
    // cursor, and the relay task's own store connection never writes.
    let cursor = eventually_cursor(&core_store, &bot, &ws_url).await;
    assert!(
        cursor as u64 >= connect_before.saturating_sub(10) && cursor as u64 <= now_secs() + 10,
        "the cursor starts at connect time (A14), got {cursor}"
    );
    assert_eq!(
        seed.cursors().get(&bot, &ws_url).unwrap(),
        None,
        "the relay task does not write the cursor itself"
    );
}

/// Polls `store` until the `(bot, relay_url)` cursor exists, up to 5 s.
async fn eventually_cursor(store: &Store, bot: &BotName, relay_url: &str) -> i64 {
    for _ in 0..100 {
        if let Some(cursor) = store.cursors().get(bot, relay_url).unwrap() {
            return cursor;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no cursor was stored for {bot} at {relay_url}");
}

// --- Test 5: buffering ----------------------------------------------------------

#[tokio::test]
async fn sink_receives_backfill_before_buffered_live_events() {
    let connect = now_secs();
    let cursor = (connect - 1_000) as i64;
    let since = (cursor - OVERLAP_SECS as i64) as u64;
    let channel = channel_uuid(CHANNEL_A);

    let owner = keys("owner");
    // Same-timestamp pair pins the (created_at, id) tiebreak.
    let first = message(&owner, channel, "first", None, &[], since + 10);
    let tie_a = message(&owner, channel, "tie a", None, &[], since + 20);
    let tie_b = message(&owner, channel, "tie b", None, &[], since + 20);
    let last = message(&owner, channel, "last", None, &[], since + 30);
    let live = message(&owner, channel, "live", None, &[], connect + 2);

    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    {
        let mut guard = rest.lock().unwrap();
        let bot_hex = keys("A").public_key().to_hex();
        guard.memberships = vec![(bot_hex, CHANNEL_A.to_string())];
        guard.metadata = vec![(CHANNEL_A.to_string(), "one".to_string(), false)];
        // Scrambled; the delay lets the live event arrive mid-backfill.
        guard.backfill_pages = vec![serde_json::to_value(vec![
            last.clone(),
            tie_b.clone(),
            first.clone(),
            tie_a.clone(),
        ])
        .unwrap()];
        guard.backfill_delay = Duration::from_millis(400);
    }
    let (rest_url, _port) = start_rest_mock(rest.clone()).await;
    let (ws_listener, ws_url) = bind_ws().await;

    let (seed, store, _dir) = file_stores();
    let bot = bot_name();
    seed.cursors().advance(&bot, &ws_url, cursor).unwrap();

    let (sink_tx, mut sink_rx) = mpsc::unbounded_channel::<Delivered>();
    let _conn = spawn_synced_connection(SyncParams {
        conn: ConnParams::new(ws_url.clone(), keys("A"), None),
        rest: RestClient::new(&rest_url, keys("A"), None),
        store,
        bot: bot.clone(),
        sink: sink_tx,
        core: spawn_test_core().0,
        membership_tap: None,
    });

    let (mut sink, mut stream) = accept_split(&ws_listener).await;
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-buffer",
    )
    .await;
    let req = recv_req(&mut stream).await;
    assert_eq!(req.sub_id, format!("ch-{CHANNEL_A}"));
    // The relay emits EOSE then a live event while backfill is still flying.
    send_array(&mut sink, serde_json::json!(["EOSE", req.sub_id])).await;
    send_array(
        &mut sink,
        serde_json::json!(["EVENT", req.sub_id, serde_json::to_value(&live).unwrap()]),
    )
    .await;

    let mut got: Vec<Delivered> = Vec::new();
    for _ in 0..5 {
        let next = tokio::time::timeout(Duration::from_secs(10), sink_rx.recv())
            .await
            .expect("backfill plus the buffered live event arrive")
            .expect("the sink stays open");
        got.push(next);
    }
    let mut backfill = [first.clone(), tie_a.clone(), tie_b.clone(), last.clone()];
    backfill.sort_by_key(|e| (e.created_at.as_secs(), e.id.to_hex()));
    let want: Vec<String> = backfill
        .iter()
        .map(|e| e.id.to_hex())
        .chain(std::iter::once(live.id.to_hex()))
        .collect();
    let got_ids: Vec<String> = got.iter().map(|d| d.event.id.to_hex()).collect();
    assert_eq!(
        got_ids, want,
        "backfill arrives ascending before any buffered live event"
    );
    for delivered in &got[..4] {
        assert_eq!(delivered.source, Source::Backfill);
        assert_eq!(delivered.channel, channel);
    }
    assert_eq!(got[4].source, Source::Live);

    let queries = backfill_requests(&rest);
    assert_eq!(queries.len(), 1);
    assert_eq!(only_filter(&queries[0])["since"].as_u64(), Some(since));
    let _ = seed;
    let _ = bot;
}

// --- Test 6: reconnect ------------------------------------------------------------

#[tokio::test]
async fn reconnect_rediscovers_resubscribes_and_backfills_from_cursor() {
    let connect = now_secs();
    let cursor = (connect - 1_000) as i64;
    let since_first = (cursor - OVERLAP_SECS as i64) as u64;
    let channel = channel_uuid(CHANNEL_A);

    let owner = keys("owner");
    let history = message(&owner, channel, "history", None, &[], since_first + 5);
    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    {
        let mut guard = rest.lock().unwrap();
        let bot_hex = keys("A").public_key().to_hex();
        guard.memberships = vec![(bot_hex, CHANNEL_A.to_string())];
        guard.metadata = vec![(CHANNEL_A.to_string(), "one".to_string(), false)];
        // One page per expected backfill query: the mock serves page N on
        // `#h` hit N, and backfill runs once per connection (two here), so
        // both queries must see the history.
        guard.backfill_pages = vec![
            serde_json::to_value(vec![history.clone()]).unwrap(),
            serde_json::to_value(vec![history.clone()]).unwrap(),
        ];
    }
    let (rest_url, _port) = start_rest_mock(rest.clone()).await;
    let (ws_listener, ws_url) = bind_ws().await;

    let (seed, store, _dir) = file_stores();
    let bot = bot_name();
    seed.cursors().advance(&bot, &ws_url, cursor).unwrap();

    let (sink_tx, mut sink_rx) = mpsc::unbounded_channel::<Delivered>();
    let _conn = spawn_synced_connection(SyncParams {
        conn: ConnParams::new(ws_url.clone(), keys("A"), None),
        rest: RestClient::new(&rest_url, keys("A"), None),
        store,
        bot: bot.clone(),
        sink: sink_tx,
        core: spawn_test_core().0,
        membership_tap: None,
    });

    // First connection: auth, REQ, backfill, live.
    let (mut sink, mut stream) = accept_split(&ws_listener).await;
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-re-1",
    )
    .await;
    let req = recv_req(&mut stream).await;
    assert_eq!(req.sub_id, format!("ch-{CHANNEL_A}"));
    let first_batch = tokio::time::timeout(Duration::from_secs(10), sink_rx.recv())
        .await
        .expect("first backfill arrives")
        .expect("the sink stays open");
    assert_eq!(first_batch.event.id, history.id);
    assert_eq!(first_batch.source, Source::Backfill);

    assert_eq!(
        seed.cursors().get(&bot, &ws_url).unwrap(),
        Some(cursor),
        "delivery alone does not move the cursor: the core advances it after applying (DD-1)"
    );
    // Stand in for the core applying a newer event before the drop.
    let cursor_after_first = cursor + 100;
    seed.cursors()
        .advance(&bot, &ws_url, cursor_after_first)
        .unwrap();
    let since_second = (cursor_after_first - OVERLAP_SECS as i64) as u64;

    // The relay drops the socket; the client redials on the §10.5 ladder.
    use tokio_tungstenite::tungstenite::Message;
    sink.send(Message::Close(None)).await.unwrap();
    let (mut sink, mut stream) =
        tokio::time::timeout(Duration::from_secs(20), accept_split(&ws_listener))
            .await
            .expect("the client reconnects");
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-re-2",
    )
    .await;
    let req = recv_req(&mut stream).await;
    assert_eq!(
        req.sub_id,
        format!("ch-{CHANNEL_A}"),
        "resubscription follows the reconnect"
    );
    let second_batch = tokio::time::timeout(Duration::from_secs(10), sink_rx.recv())
        .await
        .expect("backfill runs again after the reconnect")
        .expect("the sink stays open");
    assert_eq!(second_batch.event.id, history.id);
    assert_eq!(second_batch.source, Source::Backfill);

    let guard = rest.lock().unwrap();
    let discovery_39002 = guard
        .requests
        .iter()
        .filter(|body| {
            body.as_array()
                .is_some_and(|fs| fs.iter().any(|f| kinds_of(f).contains(&39002)))
        })
        .count();
    assert_eq!(
        discovery_39002, 2,
        "discovery reruns on the reconnect (R61.2)"
    );
    drop(guard);
    let queries = backfill_requests(&rest);
    assert_eq!(queries.len(), 2, "backfill reruns on the reconnect");
    assert_eq!(
        only_filter(&queries[0])["since"].as_u64(),
        Some(since_first)
    );
    assert_eq!(
        only_filter(&queries[1])["since"].as_u64(),
        Some(since_second),
        "the second backfill starts from cursor - 300 (R50.1)"
    );
}

// --- Test 7: membership reporting -------------------------------------------------

/// Discovery reports `Memberships{bot, channels}` to the core on every
/// (re)connect (design §10.2 step 3, feeding `Snapshot.local_members`).
#[tokio::test]
async fn discovery_reports_memberships_to_core_on_every_connect() {
    let channel = channel_uuid(CHANNEL_A);
    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    {
        let mut guard = rest.lock().unwrap();
        let bot_hex = keys("A").public_key().to_hex();
        guard.memberships = vec![(bot_hex, CHANNEL_A.to_string())];
        guard.metadata = vec![(CHANNEL_A.to_string(), "one".to_string(), false)];
        guard.backfill_pages = vec![
            serde_json::Value::Array(Vec::new()),
            serde_json::Value::Array(Vec::new()),
        ];
    }
    let (rest_url, _port) = start_rest_mock(rest.clone()).await;
    let (ws_listener, ws_url) = bind_ws().await;

    let (seed, store, _dir) = file_stores();
    let bot = bot_name();
    seed.cursors()
        .advance(&bot, &ws_url, now_secs() as i64 - 1_000)
        .unwrap();

    let (sink_tx, _sink_rx) = mpsc::unbounded_channel::<Delivered>();
    let (tap_tx, mut tap_rx) = mpsc::unbounded_channel::<(BotName, BTreeSet<ChannelId>)>();
    let _conn = spawn_synced_connection(SyncParams {
        conn: ConnParams::new(ws_url.clone(), keys("A"), None),
        rest: RestClient::new(&rest_url, keys("A"), None),
        store,
        bot: bot.clone(),
        sink: sink_tx,
        core: spawn_test_core().0,
        membership_tap: Some(tap_tx),
    });

    // First connection: auth, REQ, then the membership report.
    let (mut sink, mut stream) = accept_split(&ws_listener).await;
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-member-1",
    )
    .await;
    let req = recv_req(&mut stream).await;
    assert_eq!(req.sub_id, format!("ch-{CHANNEL_A}"));
    let (got_bot, got_channels) = tokio::time::timeout(Duration::from_secs(5), tap_rx.recv())
        .await
        .expect("the membership report arrives")
        .expect("the tap stays open");
    assert_eq!(got_bot, bot, "the report names the syncing bot");
    assert_eq!(
        got_channels,
        BTreeSet::from([ChannelId::from(channel)]),
        "the report carries the discovered live channels"
    );

    // The relay drops the socket; the reconnect reports again (R61.2).
    use tokio_tungstenite::tungstenite::Message;
    sink.send(Message::Close(None)).await.unwrap();
    let (mut sink, mut stream) =
        tokio::time::timeout(Duration::from_secs(20), accept_split(&ws_listener))
            .await
            .expect("the client reconnects");
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-member-2",
    )
    .await;
    let req = recv_req(&mut stream).await;
    assert_eq!(req.sub_id, format!("ch-{CHANNEL_A}"));
    let (got_bot, got_channels) = tokio::time::timeout(Duration::from_secs(10), tap_rx.recv())
        .await
        .expect("the reconnect reports memberships again")
        .expect("the tap stays open");
    assert_eq!(got_bot, bot);
    assert_eq!(got_channels, BTreeSet::from([ChannelId::from(channel)]));
}

// --- Cursor ownership and backfill failures (review finding #1) ---------------

/// Spawns a synced connection for bot A over `store`, reporting into a
/// throwaway test core.
fn spawn_synced(ws_url: &str, rest_url: &str, store: Store) -> mpsc::UnboundedReceiver<Delivered> {
    let (sink_tx, sink_rx) = mpsc::unbounded_channel::<Delivered>();
    let _conn = spawn_synced_connection(SyncParams {
        conn: ConnParams::new(ws_url.to_owned(), keys("A"), None),
        rest: RestClient::new(rest_url, keys("A"), None),
        store,
        bot: bot_name(),
        sink: sink_tx,
        core: spawn_test_core().0,
        membership_tap: None,
    });
    sink_rx
}

/// A delivered event must not move the cursor until the core has applied it
/// (design §6.3 step 7, DD-1): the relay task only reads the cursor.
#[tokio::test]
async fn the_relay_task_never_advances_the_cursor() {
    let cursor = now_secs() as i64 - 1_000;
    let channel = channel_uuid(CHANNEL_A);
    let owner = keys("owner");
    let history = message(&owner, channel, "history", None, &[], cursor as u64 + 50);
    let live = message(&owner, channel, "live", None, &[], now_secs());
    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    {
        let mut guard = rest.lock().unwrap();
        let bot_hex = keys("A").public_key().to_hex();
        guard.memberships = vec![(bot_hex, CHANNEL_A.to_string())];
        guard.metadata = vec![(CHANNEL_A.to_string(), "one".to_string(), false)];
        guard.backfill_pages = vec![serde_json::to_value(vec![history.clone()]).unwrap()];
    }
    let (rest_url, _port) = start_rest_mock(rest.clone()).await;
    let (ws_listener, ws_url) = bind_ws().await;
    let (seed, store, _dir) = file_stores();
    let bot = bot_name();
    seed.cursors().advance(&bot, &ws_url, cursor).unwrap();
    let mut sink_rx = spawn_synced(&ws_url, &rest_url, store);

    let (mut sink, mut stream) = accept_split(&ws_listener).await;
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-cur",
    )
    .await;
    let req = recv_req(&mut stream).await;
    let backfilled = tokio::time::timeout(Duration::from_secs(10), sink_rx.recv())
        .await
        .expect("the backfill arrives")
        .expect("the sink stays open");
    assert_eq!(backfilled.event.id, history.id);
    send_array(
        &mut sink,
        serde_json::json!(["EVENT", req.sub_id, serde_json::to_value(&live).unwrap()]),
    )
    .await;
    let streamed = tokio::time::timeout(Duration::from_secs(5), sink_rx.recv())
        .await
        .expect("the live event arrives")
        .expect("the sink stays open");
    assert_eq!(streamed.event.id, live.id);
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        seed.cursors().get(&bot, &ws_url).unwrap(),
        Some(cursor),
        "only the core advances the cursor, after the apply commits"
    );
}

/// The core advances the cursor as it applies each event, so backfill from
/// several channels must reach it oldest first across all of them; otherwise
/// a crash mid-batch would skip an older event on a later channel.
#[tokio::test]
async fn backfill_is_ascending_across_channels() {
    let connect = now_secs();
    let since = connect - 1_000 - OVERLAP_SECS;
    let owner = keys("owner");
    let a = channel_uuid(CHANNEL_A);
    let b = channel_uuid(CHANNEL_B);
    let a1 = message(&owner, a, "a1", None, &[], since + 10);
    let a2 = message(&owner, a, "a2", None, &[], since + 30);
    let b1 = message(&owner, b, "b1", None, &[], since + 20);
    let b2 = message(&owner, b, "b2", None, &[], since + 40);
    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    {
        let mut guard = rest.lock().unwrap();
        let bot_hex = keys("A").public_key().to_hex();
        guard.memberships = vec![
            (bot_hex.clone(), CHANNEL_A.to_string()),
            (bot_hex, CHANNEL_B.to_string()),
        ];
        guard.metadata = vec![
            (CHANNEL_A.to_string(), "one".to_string(), false),
            (CHANNEL_B.to_string(), "two".to_string(), false),
        ];
        // Page N answers the Nth `#h` query, whichever channel asks.
        guard.backfill_pages = vec![
            serde_json::to_value(vec![a1.clone(), a2.clone()]).unwrap(),
            serde_json::to_value(vec![b1.clone(), b2.clone()]).unwrap(),
        ];
    }
    let (rest_url, _port) = start_rest_mock(rest.clone()).await;
    let (ws_listener, ws_url) = bind_ws().await;
    let (seed, store, _dir) = file_stores();
    seed.cursors()
        .advance(&bot_name(), &ws_url, (since + OVERLAP_SECS) as i64)
        .unwrap();
    let mut sink_rx = spawn_synced(&ws_url, &rest_url, store);

    let (mut sink, mut stream) = accept_split(&ws_listener).await;
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-asc",
    )
    .await;
    let mut got: Vec<u64> = Vec::new();
    for _ in 0..4 {
        let next = tokio::time::timeout(Duration::from_secs(10), sink_rx.recv())
            .await
            .expect("the backfill arrives")
            .expect("the sink stays open");
        got.push(next.event.created_at.as_secs() - since);
    }
    assert_eq!(
        got,
        vec![10, 20, 30, 40],
        "backfill is oldest first across channels"
    );
}

/// A channel whose backfill fails must not be skipped: the connection drops
/// and redials, delivering nothing from the failed sync, so no later event
/// can move the cursor past the gap.
#[tokio::test]
async fn a_failed_channel_backfill_redials() {
    let cursor = now_secs() as i64 - 1_000;
    let channel = channel_uuid(CHANNEL_A);
    let owner = keys("owner");
    let history = message(&owner, channel, "history", None, &[], cursor as u64 + 50);
    let live = message(&owner, channel, "live", None, &[], now_secs());
    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    {
        let mut guard = rest.lock().unwrap();
        let bot_hex = keys("A").public_key().to_hex();
        guard.memberships = vec![(bot_hex, CHANNEL_A.to_string())];
        guard.metadata = vec![(CHANNEL_A.to_string(), "one".to_string(), false)];
        guard.backfill_failures = 1;
        guard.backfill_delay = Duration::from_millis(400);
        guard.backfill_pages = vec![
            serde_json::Value::Array(Vec::new()),
            serde_json::to_value(vec![history.clone()]).unwrap(),
        ];
    }
    let (rest_url, _port) = start_rest_mock(rest.clone()).await;
    let (ws_listener, ws_url) = bind_ws().await;
    let (seed, store, _dir) = file_stores();
    seed.cursors()
        .advance(&bot_name(), &ws_url, cursor)
        .unwrap();
    let mut sink_rx = spawn_synced(&ws_url, &rest_url, store);

    let (mut sink, mut stream) = accept_split(&ws_listener).await;
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-f1",
    )
    .await;
    let req = recv_req(&mut stream).await;
    // A live event arrives while the failing backfill is in flight.
    send_array(
        &mut sink,
        serde_json::json!(["EVENT", req.sub_id, serde_json::to_value(&live).unwrap()]),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        sink_rx.try_recv().is_err(),
        "nothing from a sync whose backfill failed is delivered"
    );

    let (mut sink, mut stream) =
        tokio::time::timeout(Duration::from_secs(20), accept_split(&ws_listener))
            .await
            .expect("the client redials after the failed backfill");
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-f2",
    )
    .await;
    let _ = recv_req(&mut stream).await;
    let backfilled = tokio::time::timeout(Duration::from_secs(10), sink_rx.recv())
        .await
        .expect("the retried backfill arrives")
        .expect("the sink stays open");
    assert_eq!(backfilled.event.id, history.id);
    assert_eq!(backfilled.source, Source::Backfill);
}

// --- Discovery failures (review finding #5) ------------------------------------

/// A failed discovery must not leave the bot connected with no subscriptions:
/// the connection drops and redials, and the retry subscribes.
#[tokio::test]
async fn a_failed_discovery_redials() {
    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    {
        let mut guard = rest.lock().unwrap();
        let bot_hex = keys("A").public_key().to_hex();
        guard.memberships = vec![(bot_hex, CHANNEL_A.to_string())];
        guard.metadata = vec![(CHANNEL_A.to_string(), "one".to_string(), false)];
        guard.discovery_failures = 1;
    }
    let (rest_url, _port) = start_rest_mock(rest.clone()).await;
    let (ws_listener, ws_url) = bind_ws().await;
    let (_seed, store, _dir) = file_stores();
    let _sink_rx = spawn_synced(&ws_url, &rest_url, store);

    let (mut sink, mut stream) = accept_split(&ws_listener).await;
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-d1",
    )
    .await;

    let (mut sink, mut stream) =
        tokio::time::timeout(Duration::from_secs(20), accept_split(&ws_listener))
            .await
            .expect("the client redials after the failed discovery");
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-d2",
    )
    .await;
    let req = recv_req(&mut stream).await;
    assert_eq!(
        req.sub_id,
        format!("ch-{CHANNEL_A}"),
        "the retry subscribes"
    );
}

// --- Cursor read failures (review cycle 2, finding C) --------------------------

/// A failed cursor read must not skip backfill and go live, which would let
/// later events move the cursor past history never fetched: the connection
/// drops and redials, and the retry backfills from the cursor.
#[tokio::test]
async fn a_failed_cursor_read_redials_instead_of_skipping_backfill() {
    let cursor = now_secs() as i64 - 1_000;
    let channel = channel_uuid(CHANNEL_A);
    let owner = keys("owner");
    let history = message(&owner, channel, "history", None, &[], cursor as u64 + 50);
    let live = message(&owner, channel, "live", None, &[], now_secs());
    let rest: SharedRest = Arc::new(Mutex::new(RestState::new()));
    {
        let mut guard = rest.lock().unwrap();
        let bot_hex = keys("A").public_key().to_hex();
        guard.memberships = vec![(bot_hex, CHANNEL_A.to_string())];
        guard.metadata = vec![(CHANNEL_A.to_string(), "one".to_string(), false)];
        guard.backfill_pages = vec![serde_json::to_value(vec![history.clone()]).unwrap()];
    }
    let (rest_url, _port) = start_rest_mock(rest.clone()).await;
    let (ws_listener, ws_url) = bind_ws().await;
    let (seed, store, _dir) = file_stores();
    seed.cursors()
        .advance(&bot_name(), &ws_url, cursor)
        .unwrap();
    seed.connection()
        .execute_batch("ALTER TABLE cursors RENAME TO cursors_hidden")
        .unwrap();
    let mut sink_rx = spawn_synced(&ws_url, &rest_url, store);

    let (mut sink, mut stream) = accept_split(&ws_listener).await;
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-c1",
    )
    .await;
    let req = recv_req(&mut stream).await;
    // The cursor read follows the subscription at once; let it fail, then
    // send a live event that a connection gone live would deliver.
    tokio::time::sleep(Duration::from_millis(300)).await;
    send_array(
        &mut sink,
        serde_json::json!(["EVENT", req.sub_id, serde_json::to_value(&live).unwrap()]),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        sink_rx.try_recv().is_err(),
        "a connection whose cursor read failed delivers nothing"
    );
    seed.connection()
        .execute_batch("ALTER TABLE cursors_hidden RENAME TO cursors")
        .unwrap();

    let (mut sink, mut stream) =
        tokio::time::timeout(Duration::from_secs(20), accept_split(&ws_listener))
            .await
            .expect("the client redials after the failed cursor read");
    server_complete_auth(
        &mut sink,
        &mut stream,
        keys("A").public_key(),
        &ws_url,
        "chal-c2",
    )
    .await;
    let _ = recv_req(&mut stream).await;
    let backfilled = tokio::time::timeout(Duration::from_secs(10), sink_rx.recv())
        .await
        .expect("the retried backfill arrives")
        .expect("the sink stays open");
    assert_eq!(backfilled.event.id, history.id);
    assert_eq!(backfilled.source, Source::Backfill);
}
