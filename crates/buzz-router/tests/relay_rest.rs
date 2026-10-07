//! RED tests for T2.3: relay REST client and `RelayPort` (design §10.4, §6.1).
//!
//! The implementation (`buzz_router::relay::{RelayPort, RelayError}` and
//! `buzz_router::relay::rest::{RestClient, relay_ws_to_http}`) does not exist
//! yet. These tests define its contract, mirroring `buzz-acp`'s `RestClient`
//! (`relay.rs:251-580`):
//!
//! - `RestClient::new(relay_url: &str, keys: nostr::Keys, auth_tag: Option<String>)`
//!   where `relay_url` is the `ws://` form (mapped to http internally).
//! - `POST /query` and `POST /events` with NIP-98 `Authorization: Nostr <b64>`
//!   (kind-27235 event with `u`, `method`, `nonce`, `payload` tags), re-signed
//!   on each attempt; `x-auth-tag` iff an auth tag is configured.
//! - Retries after 500ms, 1s, 2s (±20% jitter) on 429/502/503/504, timeout or
//!   connect errors; other statuses fail at once.
//! - Backfill paging with `until` + `before_id` from the oldest event of each
//!   full page (key names match buzz-acp `query_raw_all`).

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "helpers and mock handlers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use base64::Engine as _;
use buzz_router::relay::rest::{relay_ws_to_http, RestClient};
use buzz_router::relay::{RelayError, RelayPort};
use sha2::{Digest, Sha256};

/// Compile-time pin: `RestClient` implements the `RelayPort` trait.
fn assert_relay_port<T: RelayPort>() {}

/// Compile-time pin: `RelayError` is only ever required to be `Debug + Display`;
/// tests never match on its variants.
fn assert_error_bounds<T: std::fmt::Debug + std::fmt::Display>() {}

/// One request observed by the mock relay.
#[derive(Debug, Clone)]
struct RecordedRequest {
    authorization: Option<String>,
    auth_tag: Option<String>,
    body: Vec<u8>,
    // Wall-clock arrival time (`std` so the retry test can run on real time;
    // tokio's paused clock needs the `test-util` feature, which is off).
    at: std::time::Instant,
}

fn record(headers: &HeaderMap, body: &[u8]) -> RecordedRequest {
    RecordedRequest {
        authorization: headers
            .get("Authorization")
            .or_else(|| headers.get("authorization"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        auth_tag: headers
            .get("x-auth-tag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        body: body.to_vec(),
        at: std::time::Instant::now(),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn test_keys() -> nostr::Keys {
    nostr::Keys::generate()
}

fn signed_kind9(keys: &nostr::Keys, created_at_secs: u64, content: &str) -> nostr::Event {
    nostr::EventBuilder::new(nostr::Kind::Custom(9), content)
        .custom_created_at(nostr::Timestamp::from(created_at_secs))
        .sign_with_keys(keys)
        .unwrap()
}

/// Serve `app` on 127.0.0.1:0. Returns the `ws://` relay URL (the form passed
/// to `RestClient::new`), the port, and the server task.
async fn spawn_mock(app: Router) -> (String, u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("ws://127.0.0.1:{port}"), port, handle)
}

fn query_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/query")
}

// --- Test 1: NIP-98 authorization --------------------------------------------

#[derive(Clone)]
struct Nip98State {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

async fn nip98_query(
    State(state): State<Nip98State>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    state.requests.lock().unwrap().push(record(&headers, &body));
    (StatusCode::OK, "[]".to_string())
}

#[tokio::test]
async fn nip98_authorization() {
    let state = Nip98State {
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let (relay_url, port, _server) = spawn_mock(
        Router::new()
            .route("/query", post(nip98_query))
            .with_state(state.clone()),
    )
    .await;

    let keys = test_keys();
    let client = RestClient::new(&relay_url, keys.clone(), None);
    let events = client.query(vec![nostr::Filter::new()]).await.unwrap();
    assert!(events.is_empty());

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];

    let auth = req
        .authorization
        .as_ref()
        .expect("POST /query sends an Authorization header");
    let b64 = auth
        .strip_prefix("Nostr ")
        .expect("Authorization header starts with `Nostr `");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .expect("Authorization payload is base64");
    let decoded_str = String::from_utf8(decoded).expect("Authorization payload is UTF-8");

    // Decodes to an event with the NIP-98 shape, signed by the bot's key.
    let auth_event: nostr::Event =
        serde_json::from_str(&decoded_str).expect("Authorization payload parses as nostr::Event");
    assert_eq!(u16::from(auth_event.kind), 27235);
    assert_eq!(auth_event.pubkey, keys.public_key());
    auth_event.verify().expect("NIP-98 event verifies");

    // Tags pin the exact request: URL, method, body hash, and a nonce.
    let value: serde_json::Value =
        serde_json::from_str(&decoded_str).expect("Authorization payload is JSON");
    let tags = value["tags"].as_array().expect("NIP-98 event has tags");
    let find = |name: &str| {
        tags.iter()
            .find(|t| t[0] == name)
            .unwrap_or_else(|| panic!("NIP-98 event has a `{name}` tag"))
    };
    assert_eq!(find("u")[1], query_url(port));
    assert_eq!(find("method")[1], "POST");
    assert_eq!(
        find("payload")[1],
        serde_json::Value::String(hex_lower(&Sha256::digest(&req.body)))
    );
    assert!(
        find("nonce")[1].as_str().is_some_and(|n| !n.is_empty()),
        "NIP-98 event has a non-empty `nonce` tag"
    );
}

// --- Test 2: x-auth-tag header -----------------------------------------------

#[derive(Clone)]
struct AuthTagState {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

async fn authtag_query(
    State(state): State<AuthTagState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    state.requests.lock().unwrap().push(record(&headers, &body));
    (StatusCode::OK, "[]".to_string())
}

#[tokio::test]
async fn auth_tag_header() {
    let state = AuthTagState {
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let (relay_url, _port, _server) = spawn_mock(
        Router::new()
            .route("/query", post(authtag_query))
            .with_state(state.clone()),
    )
    .await;

    let with_tag = RestClient::new(&relay_url, test_keys(), Some("delegated-tag".to_string()));
    with_tag.query(vec![nostr::Filter::new()]).await.unwrap();

    let without_tag = RestClient::new(&relay_url, test_keys(), None);
    without_tag.query(vec![nostr::Filter::new()]).await.unwrap();

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].auth_tag.as_deref(), Some("delegated-tag"));
    assert_eq!(requests[1].auth_tag, None);
}

// --- Test 3: retry timing (paused time) --------------------------------------

#[derive(Clone)]
struct RetryState {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    hits: Arc<AtomicUsize>,
}

async fn flaky_query(
    State(state): State<RetryState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let n = state.hits.fetch_add(1, Ordering::SeqCst);
    state.requests.lock().unwrap().push(record(&headers, &body));
    if n < 2 {
        (StatusCode::SERVICE_UNAVAILABLE, "[]".to_string())
    } else {
        (StatusCode::OK, "[]".to_string())
    }
}

// NOTE: the task brief asks for `#[tokio::test(start_paused = true)]` with
// `tokio::time::advance` here, but that needs tokio's `test-util` feature,
// which is not enabled (workspace tokio has no `test-util`, and RED may not
// touch Cargo.toml). This test therefore runs on real time: the 500ms/1s
// ladder dominates localhost overhead by two orders of magnitude, so the
// ±20% windows (400..=600ms, 800..=1200ms) stay deterministic.
#[tokio::test]
async fn retries_then_succeeds() {
    let state = RetryState {
        requests: Arc::new(Mutex::new(Vec::new())),
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let (relay_url, _port, _server) = spawn_mock(
        Router::new()
            .route("/query", post(flaky_query))
            .with_state(state.clone()),
    )
    .await;

    let client = RestClient::new(&relay_url, test_keys(), None);
    let events = client.query(vec![nostr::Filter::new()]).await.unwrap();
    assert!(events.is_empty());

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let gap1 = (requests[1].at - requests[0].at).as_millis();
    let gap2 = (requests[2].at - requests[1].at).as_millis();
    assert!(
        (400..=600).contains(&gap1),
        "first retry gap is {gap1}ms, expected 400..=600ms"
    );
    assert!(
        (800..=1200).contains(&gap2),
        "second retry gap is {gap2}ms, expected 800..=1200ms"
    );
}

// --- Test 4: client errors fail at once --------------------------------------

#[derive(Clone)]
struct FailState {
    hits: Arc<AtomicUsize>,
}

async fn failing_query(State(state): State<FailState>) -> impl IntoResponse {
    state.hits.fetch_add(1, Ordering::SeqCst);
    (
        StatusCode::BAD_REQUEST,
        r#"{"error":"bad filter"}"#.to_string(),
    )
}

#[tokio::test]
async fn client_error_fails_at_once() {
    let state = FailState {
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let (relay_url, _port, _server) = spawn_mock(
        Router::new()
            .route("/query", post(failing_query))
            .with_state(state.clone()),
    )
    .await;

    let client = RestClient::new(&relay_url, test_keys(), None);
    let result = client.query(vec![nostr::Filter::new()]).await;
    assert!(result.is_err(), "a 400 response is an error");
    assert_eq!(
        state.hits.load(Ordering::SeqCst),
        1,
        "a 400 response is not retried"
    );
}

// --- Test 5: paged query -----------------------------------------------------

#[derive(Clone)]
struct PagedState {
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    pages: Arc<Vec<serde_json::Value>>,
    hits: Arc<AtomicUsize>,
}

async fn serve_paged(
    State(state): State<PagedState>,
    _headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let n = state.hits.fetch_add(1, Ordering::SeqCst);
    state.requests.lock().unwrap().push(body.to_vec());
    let page = &state.pages[n.min(state.pages.len() - 1)];
    (StatusCode::OK, page.to_string())
}

/// Extract the `(until, before_id)` cursor from a request body, whether the
/// body is a single filter object or an array of them.
fn cursor_of(body: &[u8]) -> Option<(u64, String)> {
    let text = std::str::from_utf8(body).ok()?;
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let objects: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(_) => vec![&value],
        _ => Vec::new(),
    };
    for object in objects {
        if let (Some(until), Some(before_id)) = (
            object.get("until").and_then(serde_json::Value::as_u64),
            object.get("before_id").and_then(serde_json::Value::as_str),
        ) {
            return Some((until, before_id.to_string()));
        }
    }
    None
}

#[tokio::test]
async fn paged_query() {
    let keys = test_keys();
    let base_secs = 1_700_000_000u64;
    let events: Vec<nostr::Event> = (0..1007)
        .map(|i| signed_kind9(&keys, base_secs + i as u64, &format!("message {i}")))
        .collect();
    let pages = vec![
        serde_json::to_value(&events[0..500]).unwrap(),
        serde_json::to_value(&events[500..1000]).unwrap(),
        serde_json::to_value(&events[1000..1007]).unwrap(),
    ];
    let state = PagedState {
        requests: Arc::new(Mutex::new(Vec::new())),
        pages: Arc::new(pages),
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let (relay_url, _port, _server) = spawn_mock(
        Router::new()
            .route("/query", post(serve_paged))
            .with_state(state.clone()),
    )
    .await;

    let client = RestClient::new(&relay_url, test_keys(), None);
    let fetched = client
        .query(vec![nostr::Filter::new().limit(500)])
        .await
        .unwrap();
    assert_eq!(fetched.len(), 1007, "pages of 500 + 500 + 7 combine");

    let ids: std::collections::HashSet<String> = fetched.iter().map(|e| e.id.to_hex()).collect();
    assert_eq!(ids.len(), 1007, "every returned event is distinct");

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 3, "paging stops after the short page");
    assert!(
        cursor_of(&requests[0]).is_none(),
        "the first request carries no cursor"
    );

    // Requests 2 and 3 page from the oldest event of the previous page.
    let (until2, before2) =
        cursor_of(&requests[1]).expect("the second request carries until/before_id");
    assert_eq!(until2, events[0].created_at.as_secs());
    assert_eq!(before2, events[0].id.to_hex());

    let (until3, before3) =
        cursor_of(&requests[2]).expect("the third request carries until/before_id");
    assert_eq!(until3, events[500].created_at.as_secs());
    assert_eq!(before3, events[500].id.to_hex());
}

// --- Test 6: submit_event posts to /events -----------------------------------

#[derive(Clone)]
struct EventsState {
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

async fn capture_event(State(state): State<EventsState>, body: Bytes) -> impl IntoResponse {
    state.bodies.lock().unwrap().push(body.to_vec());
    (StatusCode::OK, "{}".to_string())
}

#[tokio::test]
async fn submit_event_posts_to_events() {
    let state = EventsState {
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let (relay_url, _port, _server) = spawn_mock(
        Router::new()
            .route("/events", post(capture_event))
            .with_state(state.clone()),
    )
    .await;

    let keys = test_keys();
    let event = signed_kind9(&keys, 1_700_000_000, "hello relay");
    let client = RestClient::new(&relay_url, test_keys(), None);
    client.submit_event(&event).await.unwrap();

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    let received: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
    assert_eq!(
        received,
        serde_json::to_value(&event).unwrap(),
        "the server receives the exact event JSON"
    );
}

// --- Test 7: ws-to-http mapping -----------------------------------------------

#[test]
fn ws_to_http_mapping() {
    assert_relay_port::<RestClient>();
    assert_error_bounds::<RelayError>();

    assert_eq!(
        relay_ws_to_http("wss://example.com/"),
        "https://example.com"
    );
    assert_eq!(relay_ws_to_http("ws://127.0.0.1:9/"), "http://127.0.0.1:9");
    assert_eq!(
        relay_ws_to_http("http://127.0.0.1:9/"),
        "http://127.0.0.1:9"
    );
    assert_eq!(
        relay_ws_to_http("https://example.com/"),
        "https://example.com"
    );
}

/// A relay that accepts the connection but never answers must not hang the
/// client: each attempt times out, and timeouts are retried (review finding
/// #6, R61.5).
#[tokio::test]
async fn a_silent_relay_times_out_and_is_retried() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = accepted.clone();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            held.push(socket);
        }
    });
    let client = RestClient::new(&format!("ws://127.0.0.1:{port}"), test_keys(), None)
        .with_request_timeout(std::time::Duration::from_millis(200));

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.query(vec![nostr::Filter::new().kind(nostr::Kind::Custom(9))]),
    )
    .await
    .expect("the query gives up instead of hanging");

    assert!(result.is_err(), "a silent relay fails the query");
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        4,
        "one attempt plus three timeout retries"
    );
}
