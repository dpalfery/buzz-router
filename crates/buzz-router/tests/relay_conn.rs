//! RED tests for T2.4: WebSocket connection, NIP-42 auth, publish acks and
//! reconnect (design §10.1, §10.5, §6.8; requirements R50.1, R61.1).
//!
//! The implementation (`buzz_router::relay::conn::{ConnParams, Connection,
//! spawn_connection}`) does not exist yet, so `cargo test -p buzz-router
//! --test relay_conn` must fail with an unresolved-import error naming
//! `buzz_router::relay::conn` (E0432/E0433). Every other import below resolves
//! against real workspace dependencies.
//!
//! TIMING DEVIATION (documented inline, per task note): the task's test
//! contract says "(paused time)", but tokio's `test-util` feature is NOT
//! enabled by this workspace's `tokio` dependency, so
//! `#[tokio::test(start_paused = true)]` is unavailable. All timing tests run
//! on REAL time with generous windows: the reconnect rungs (1s, 2s) are
//! asserted inside widened bands (600..=1500ms, 1300..=2700ms) that stay
//! centered on the design §10.5 ladder but tolerate real-time CI slop.
//!
//! Conventions: mock relay servers bind `127.0.0.1:0` (localhost only);
//! every test builds fresh keys with `nostr::Keys::generate()` (no real
//! keys); the `auth` tag, when configured, is asserted as exactly
//! `["auth", "test-auth-tag"]`; kind checks use `u16::from(kind) == 22242`
//! because `Kind` has no `PartialEq`.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "helpers and mock tasks in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use buzz_router::relay::conn::{spawn_connection, ConnParams, Connection};
use buzz_router::relay::RelayError;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use nostr::{Event, EventBuilder, JsonUtil, Keys, Kind};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// A connected mock-relay stream (server side of `accept_async`).
type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

/// Compile-time pin: `Connection` is `Clone + Send + Sync`.
fn assert_connection_traits<T: Clone + Send + Sync>() {}

/// Bind a mock relay listener on localhost and return it with its `ws://` URL.
async fn bind_listener() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, format!("ws://127.0.0.1:{port}/"))
}

/// Accept one connection and split it into `(sink, stream)`.
async fn accept_split(listener: &TcpListener) -> (SplitSink<Ws, Message>, SplitStream<Ws>) {
    let (tcp, _) = listener.accept().await.unwrap();
    tokio_tungstenite::accept_async(tcp).await.unwrap().split()
}

/// Send a JSON array as a text message. The `.into()` keeps this compiling
/// whether `Message::Text` carries `String` or `Utf8Bytes`.
async fn send_array(sink: &mut SplitSink<Ws, Message>, value: serde_json::Value) {
    sink.send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

/// Read relay messages until a JSON text array arrives. Client pings are
/// ignored; a close from the peer fails the test.
async fn recv_array(stream: &mut SplitStream<Ws>) -> serde_json::Value {
    loop {
        let msg = stream.next().await.unwrap().unwrap();
        match msg {
            Message::Text(text) => {
                let value: serde_json::Value = serde_json::from_str(text.as_ref()).unwrap();
                if value.is_array() {
                    return value;
                }
            }
            Message::Close(_) => {
                panic!("peer closed the WebSocket while a JSON array was expected")
            }
            _ => {}
        }
    }
}

/// Drive the server side of the NIP-42 handshake (reference:
/// `examples/countdown-bot` `connect_and_authenticate` + `build_auth_event`):
/// send `["AUTH", challenge]`, read the client's `["AUTH", event]`, assert the
/// auth event, then reply `["OK", id, true]`. Returns the client's auth event.
async fn server_complete_auth(
    sink: &mut SplitSink<Ws, Message>,
    stream: &mut SplitStream<Ws>,
    expected_pubkey: nostr::PublicKey,
    relay_url: &str,
    challenge: &str,
    auth_tag: Option<&str>,
) -> Event {
    send_array(sink, serde_json::json!(["AUTH", challenge])).await;
    let msg = recv_array(stream).await;
    assert_eq!(msg[0].as_str().unwrap(), "AUTH");
    let event = Event::from_json(msg[1].to_string()).unwrap();
    assert_eq!(
        u16::from(event.kind),
        22242,
        "auth event must be kind 22242"
    );
    assert!(event.verify().is_ok(), "auth event must verify");
    assert_eq!(event.pubkey, expected_pubkey);
    let tags: Vec<Vec<String>> = event
        .tags
        .iter()
        .map(|tag| tag.as_slice().to_vec())
        .collect();
    let has = |name: &str, second: &str| {
        tags.iter()
            .any(|t| t.len() == 2 && t[0] == name && t[1] == second)
    };
    assert!(
        has("relay", relay_url),
        "auth event is missing [\"relay\", {relay_url:?}]: {tags:?}"
    );
    assert!(
        has("challenge", challenge),
        "auth event is missing [\"challenge\", {challenge:?}]: {tags:?}"
    );
    match auth_tag {
        Some(tag) => assert!(
            has("auth", tag),
            "auth event is missing [\"auth\", {tag:?}]: {tags:?}"
        ),
        None => assert!(
            tags.iter()
                .all(|t| t.first().map(String::as_str) != Some("auth")),
            "auth event must carry no `auth` tag: {tags:?}"
        ),
    }
    send_array(sink, serde_json::json!(["OK", event.id.to_hex(), true])).await;
    event
}

/// An established mock-relay connection: the client handle plus the server
/// halves and the bot keys.
struct Authed {
    conn: Connection,
    sink: SplitSink<Ws, Message>,
    stream: SplitStream<Ws>,
    keys: Keys,
}

/// Spawn a connection to a mock relay, complete the NIP-42 handshake inline,
/// and wait until the connection reports Up.
async fn establish(auth_tag: Option<String>, challenge: &str) -> Authed {
    let (listener, relay_url) = bind_listener().await;
    let keys = Keys::generate();
    let expected_pubkey = keys.public_key();
    let conn: Connection = spawn_connection(ConnParams::new(
        relay_url.clone(),
        keys.clone(),
        auth_tag.clone(),
    ));
    let (mut sink, mut stream) = accept_split(&listener).await;
    server_complete_auth(
        &mut sink,
        &mut stream,
        expected_pubkey,
        &relay_url,
        challenge,
        auth_tag.as_deref(),
    )
    .await;
    let up = tokio::time::timeout(Duration::from_secs(5), conn.wait_up()).await;
    assert!(up.is_ok(), "connection did not report Up within 5s");
    Authed {
        conn,
        sink,
        stream,
        keys,
    }
}

/// Auth handshake shared by the two auth tests.
async fn auth_flow(auth_tag: Option<String>, challenge: &str) {
    let challenge = challenge.to_string();
    let (listener, relay_url) = bind_listener().await;
    let keys = Keys::generate();
    let expected_pubkey = keys.public_key();
    let server_url = relay_url.clone();
    let server_tag = auth_tag.clone();
    let conn: Connection = spawn_connection(ConnParams::new(relay_url, keys, auth_tag));
    let server = tokio::spawn(async move {
        let (mut sink, mut stream) = accept_split(&listener).await;
        server_complete_auth(
            &mut sink,
            &mut stream,
            expected_pubkey,
            &server_url,
            &challenge,
            server_tag.as_deref(),
        )
        .await;
    });
    let up = tokio::time::timeout(Duration::from_secs(5), conn.wait_up()).await;
    assert!(up.is_ok(), "connection did not report Up within 5s");
    server.abort();
}

#[tokio::test]
async fn auth_without_tag() {
    auth_flow(None, "chal-1").await;
}

#[tokio::test]
async fn auth_with_tag() {
    auth_flow(Some("test-auth-tag".to_string()), "chal-2").await;
}

#[tokio::test]
async fn publish_resolves_on_ok() {
    let Authed {
        conn,
        mut sink,
        mut stream,
        keys,
    } = establish(None, "chal-pub-ok").await;
    let event = EventBuilder::new(Kind::Custom(9), "hello")
        .sign_with_keys(&keys)
        .unwrap();
    let id_hex = event.id.to_hex();
    let server = tokio::spawn(async move {
        let msg = recv_array(&mut stream).await;
        assert_eq!(msg[0].as_str().unwrap(), "EVENT");
        let got = Event::from_json(msg[1].to_string()).unwrap();
        assert_eq!(got.id.to_hex(), id_hex);
        send_array(&mut sink, serde_json::json!(["OK", id_hex, true])).await;
    });
    let res: Result<(), RelayError> =
        tokio::time::timeout(Duration::from_secs(5), conn.publish(event))
            .await
            .unwrap();
    assert!(res.is_ok(), "publish did not resolve Ok: {res:?}");
    server.abort();
}

#[tokio::test]
async fn publish_rejected_errors() {
    let Authed {
        conn,
        mut sink,
        mut stream,
        keys,
    } = establish(None, "chal-pub-reject").await;
    let event = EventBuilder::new(Kind::Custom(9), "hello")
        .sign_with_keys(&keys)
        .unwrap();
    let id_hex = event.id.to_hex();
    let server = tokio::spawn(async move {
        let msg = recv_array(&mut stream).await;
        assert_eq!(msg[0].as_str().unwrap(), "EVENT");
        send_array(
            &mut sink,
            serde_json::json!(["OK", id_hex, false, "blocked: spam"]),
        )
        .await;
    });
    let res: Result<(), RelayError> =
        tokio::time::timeout(Duration::from_secs(5), conn.publish(event))
            .await
            .unwrap();
    let err = res.unwrap_err();
    assert!(
        err.to_string().contains("blocked: spam"),
        "unexpected publish error: {err}"
    );
    server.abort();
}

#[tokio::test]
async fn publish_times_out() {
    let (listener, relay_url) = bind_listener().await;
    let keys = Keys::generate();
    let expected_pubkey = keys.public_key();
    let mut params = ConnParams::new(relay_url.clone(), keys.clone(), None);
    params.publish_timeout = Duration::from_millis(300);
    let conn: Connection = spawn_connection(params);
    let server = tokio::spawn(async move {
        let (mut sink, mut stream) = accept_split(&listener).await;
        server_complete_auth(
            &mut sink,
            &mut stream,
            expected_pubkey,
            &relay_url,
            "chal-pub-timeout",
            None,
        )
        .await;
        // Auth completes, then the relay never answers: hold the connection
        // open so only the client's publish timeout can fire.
        let msg = recv_array(&mut stream).await;
        assert_eq!(msg[0].as_str().unwrap(), "EVENT");
        std::future::pending::<()>().await;
    });
    let up = tokio::time::timeout(Duration::from_secs(5), conn.wait_up()).await;
    assert!(up.is_ok(), "connection did not report Up within 5s");
    let event = EventBuilder::new(Kind::Custom(9), "hello")
        .sign_with_keys(&keys)
        .unwrap();
    let start = Instant::now();
    // A 10s default publish timeout would trip this 5s guard and fail the
    // test; the 300ms override must fire first.
    let res: Result<(), RelayError> =
        tokio::time::timeout(Duration::from_secs(5), conn.publish(event))
            .await
            .unwrap();
    assert!(res.is_err(), "publish should have timed out");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "publish took too long to time out"
    );
    server.abort();
}

#[tokio::test]
async fn reconnect_ladder() {
    let (listener, relay_url) = bind_listener().await;
    let keys = Keys::generate();
    let conn: Connection = spawn_connection(ConnParams::new(relay_url.clone(), keys.clone(), None));
    // Arrival instant of every accepted connection; the test needs ≥ 3.
    let arrivals: Arc<Mutex<Vec<Instant>>> = Arc::new(Mutex::new(Vec::new()));
    let arrivals_server = arrivals.clone();
    let server = tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut guard = arrivals_server.lock().unwrap();
            guard.push(Instant::now());
            let n = guard.len();
            drop(guard);
            let keys = keys.clone();
            let relay_url = relay_url.clone();
            tokio::spawn(async move {
                let (mut sink, mut stream) = {
                    let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                    ws.split()
                };
                // Every connection (including retries) must complete auth,
                // else the client keeps failing instead of backing off.
                server_complete_auth(
                    &mut sink,
                    &mut stream,
                    keys.public_key(),
                    &relay_url,
                    "chal-reconnect",
                    None,
                )
                .await;
                if n <= 2 {
                    // Complete auth on the first two connections, then close
                    // each to trigger the reconnect ladder (rungs 1s, 2s).
                    sink.send(Message::Close(None)).await.unwrap();
                } else {
                    // Later connections stay open.
                    std::future::pending::<()>().await;
                }
            });
        }
    });
    let seen_three = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if arrivals.lock().unwrap().len() >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(seen_three.is_ok(), "client did not dial 3 times within 15s");
    let times = arrivals.lock().unwrap().clone();
    // Design §10.5 ladder starts 1s, 2s (±20% jitter); real-time windows are
    // widened for CI slop but still centered on the first two rungs.
    let gap1 = times[1].duration_since(times[0]);
    let gap2 = times[2].duration_since(times[1]);
    assert!(
        (Duration::from_millis(600)..=Duration::from_millis(1500)).contains(&gap1),
        "first redial came after {gap1:?}, expected ~1s"
    );
    assert!(
        (Duration::from_millis(1300)..=Duration::from_millis(2700)).contains(&gap2),
        "second redial came after {gap2:?}, expected ~2s"
    );
    // The third connection completes auth, and the Up latch stays resolved.
    let up = tokio::time::timeout(Duration::from_secs(5), conn.wait_up()).await;
    assert!(up.is_ok(), "connection did not report Up within 5s");
    server.abort();
}

#[tokio::test]
async fn ping_gets_pong() {
    let Authed {
        conn,
        mut sink,
        mut stream,
        keys: _,
    } = establish(None, "chal-ping").await;
    let _ = &conn;
    sink.send(Message::Ping(b"hello".to_vec().into()))
        .await
        .unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(msg, Message::Pong(_)),
        "expected Pong, got {msg:?}"
    );
}

#[tokio::test]
async fn dns_failures_retry_flat() {
    assert_connection_traits::<Connection>();
    let params = ConnParams::new(
        "ws://buzz-router-nonexistent.invalid/".to_string(),
        Keys::generate(),
        None,
    );
    let conn: Connection = spawn_connection(params);
    // DNS failures retry on a flat 2s (design §10.5); `.invalid` never
    // resolves, so attempts accumulate. Poll for ≥ 3 attempts with an overall
    // guard; no tight gap assertions, resolver speed varies.
    let reached = tokio::time::timeout(Duration::from_secs(15), async {
        while conn.attempts() < 3 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .is_ok();
    assert!(reached, "expected >= 3 dial attempts within 15s");
}
