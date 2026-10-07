//! Task 5.1: the webhook adapter (design 7.2, DD-16, R36.6, R38.1, R38.2, R39.1 to R39.3, A13).
//! Each test serves the webhook from an in-process axum server on loopback.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use buzz_router::adapter::webhook::WebhookAdapter;
use buzz_router::adapter::{Adapter, AdapterEvent, SyncReply, WakeContext};
use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use router_core::config::{AdapterConfig, Limits, WebhookMode};
use router_core::ids::{BotName, ChannelId, EventId};
use router_core::payload::{ApiRef, ChannelRef, WakePayload};
use router_core::route::Reason;
use router_core::thread::RoundMode;
use sha2::Sha256;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const SECRET: &str = "test-webhook-secret-placeholder";

/// A request the test server received.
#[derive(Debug, Clone)]
struct Received {
    headers: HeaderMap,
    body: Bytes,
}

/// How the test server answers.
#[derive(Clone)]
enum Answer {
    Respond(StatusCode, &'static str),
    Never,
}

#[derive(Clone)]
struct Server {
    answer: Answer,
    received: Arc<Mutex<Vec<Received>>>,
}

async fn handle(
    State(server): State<Server>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    server
        .received
        .lock()
        .unwrap()
        .push(Received { headers, body });
    match server.answer {
        Answer::Respond(status, body) => (status, body.to_owned()),
        Answer::Never => std::future::pending().await,
    }
}

/// Serves `answer` on `/hook` and `/cancel`, returning the base URL and the received requests.
async fn serve(answer: Answer) -> (String, Arc<Mutex<Vec<Received>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let server = Server {
        answer,
        received: received.clone(),
    };
    let app = axum::Router::new()
        .route("/hook", post(handle))
        .route("/cancel", post(handle))
        .with_state(server);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), received)
}

/// A unique environment variable name holding the secret, so tests can run in parallel.
fn secret_env() -> String {
    let name = format!("BUZZ_ROUTER_TEST_SECRET_{}", Uuid::new_v4().simple());
    std::env::set_var(&name, SECRET);
    name
}

fn config(
    url: &str,
    secret_env: String,
    mode: WebhookMode,
    cancel_url: Option<String>,
) -> AdapterConfig {
    AdapterConfig::Webhook {
        url: format!("{url}/hook"),
        secret_env,
        mode,
        cancel_url,
    }
}

fn event_id(n: u8) -> EventId {
    EventId::from_hex(&format!("{n:064x}")).unwrap()
}

fn wake(adapter: AdapterConfig) -> (WakeContext, WakePayload) {
    let wake_id = Uuid::new_v4();
    let token = "ab".repeat(32);
    let limits = Limits::default();
    let channel = ChannelId::from(Uuid::from_u128(7));
    let ctx = WakeContext {
        wake_id,
        token: token.clone(),
        bot: BotName::new("A").unwrap(),
        adapter,
        channel,
        root: event_id(1),
        reply_parent: event_id(1),
        reason: Reason::Mention,
        reason_author: "Owner".to_owned(),
        mode: RoundMode::Direct,
        turns_left: limits.turns_per_round - 1,
        limits,
        deadline: Utc::now(),
        trigger_ids: vec![event_id(1)],
    };
    let payload = WakePayload {
        wake_id,
        token,
        bot: "A".to_owned(),
        channel: ChannelRef {
            id: channel.to_string(),
            name: "general".to_owned(),
        },
        thread_root_id: event_id(1).to_string(),
        reply_parent_id: event_id(1).to_string(),
        reason: Reason::Mention,
        round_mode: RoundMode::Direct,
        turns_left_after_this: limits.turns_per_round - 1,
        turns_per_round: limits.turns_per_round,
        deadline: "2026-10-05T15:20:00Z".to_owned(),
        triggers: vec![event_id(1).to_string()],
        context: Vec::new(),
        api: ApiRef::new("http://127.0.0.1:47821"),
    };
    (ctx, payload)
}

fn adapter() -> WebhookAdapter {
    WebhookAdapter::new(reqwest::Client::new())
}

async fn run(config: AdapterConfig) -> (AdapterEvent, Uuid) {
    let (ctx, payload) = wake(config);
    let wake_id = ctx.wake_id;
    let event = adapter().run(ctx, payload, CancellationToken::new()).await;
    (event, wake_id)
}

fn signature(body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

fn header<'a>(received: &'a Received, name: &str) -> &'a str {
    received.headers.get(name).unwrap().to_str().unwrap()
}

fn is_failed(event: &AdapterEvent) -> bool {
    matches!(event, AdapterEvent::Failed(_))
}

// Signing.

#[tokio::test]
async fn the_request_is_signed_and_names_the_wake() {
    let (url, received) = serve(Answer::Respond(StatusCode::ACCEPTED, "")).await;
    let (event, wake_id) = run(config(&url, secret_env(), WebhookMode::Async, None)).await;

    assert_eq!(event, AdapterEvent::AsyncAccepted);
    let received = received.lock().unwrap();
    let request = &received[0];
    assert_eq!(header(request, "content-type"), "application/json");
    assert_eq!(
        header(request, "x-buzz-router-signature"),
        signature(&request.body)
    );
    assert_eq!(header(request, "x-buzz-router-wake"), wake_id.to_string());
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["wake_id"], wake_id.to_string());
    assert_eq!(body["bot"], "A");
}

#[tokio::test]
async fn an_unset_secret_variable_fails_without_calling() {
    let (url, received) = serve(Answer::Respond(StatusCode::OK, "")).await;
    let unset = format!("BUZZ_ROUTER_TEST_UNSET_{}", Uuid::new_v4().simple());
    let (event, _) = run(config(&url, unset, WebhookMode::Async, None)).await;

    assert!(is_failed(&event), "{event:?}");
    assert!(received.lock().unwrap().is_empty());
}

// Async mode.

#[tokio::test]
async fn async_500_fails() {
    let (url, _) = serve(Answer::Respond(StatusCode::INTERNAL_SERVER_ERROR, "")).await;
    let (event, _) = run(config(&url, secret_env(), WebhookMode::Async, None)).await;
    assert!(is_failed(&event), "{event:?}");
}

#[tokio::test]
async fn async_without_an_answer_in_ten_seconds_fails() {
    let (url, received) = serve(Answer::Never).await;
    let (ctx, payload) = wake(config(&url, secret_env(), WebhookMode::Async, None));
    let started = tokio::time::Instant::now();
    let running = tokio::spawn(adapter().run(ctx, payload, CancellationToken::new()));
    // Pausing earlier would let the clock skip ahead before the loopback request arrives.
    while received.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::pause();

    let event = running.await.unwrap();
    assert!(is_failed(&event), "{event:?}");
    assert!(started.elapsed() >= Duration::from_secs(10));
}

// Sync mode.

#[tokio::test]
async fn sync_text_is_a_reply() {
    let (url, _) = serve(Answer::Respond(StatusCode::OK, r#"{"text":"hi"}"#)).await;
    let (event, _) = run(config(&url, secret_env(), WebhookMode::Sync, None)).await;
    assert_eq!(
        event,
        AdapterEvent::SyncReply(SyncReply::Text("hi".to_owned()))
    );
}

#[tokio::test]
async fn sync_pass_is_a_pass() {
    let (url, _) = serve(Answer::Respond(StatusCode::OK, r#"{"pass":true}"#)).await;
    let (event, _) = run(config(&url, secret_env(), WebhookMode::Sync, None)).await;
    assert_eq!(event, AdapterEvent::SyncReply(SyncReply::Pass));
}

#[tokio::test]
async fn sync_unrecognised_body_fails() {
    let (url, _) = serve(Answer::Respond(StatusCode::OK, r#"{"foo":1}"#)).await;
    let (event, _) = run(config(&url, secret_env(), WebhookMode::Sync, None)).await;
    assert!(is_failed(&event), "{event:?}");
}

#[tokio::test]
async fn sync_non_2xx_fails() {
    let (url, _) = serve(Answer::Respond(StatusCode::BAD_GATEWAY, r#"{"text":"hi"}"#)).await;
    let (event, _) = run(config(&url, secret_env(), WebhookMode::Sync, None)).await;
    assert!(is_failed(&event), "{event:?}");
}

#[tokio::test]
async fn cancelling_a_sync_wake_drops_the_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let (ctx, payload) = wake(config(
        &format!("http://{addr}"),
        secret_env(),
        WebhookMode::Sync,
        None,
    ));
    let cancel = CancellationToken::new();
    let running = tokio::spawn(adapter().run(ctx, payload, cancel.clone()));

    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    while !String::from_utf8_lossy(&request).contains("\"wake_id\"") {
        let read = socket.read(&mut buffer).await.unwrap();
        assert!(read > 0, "the connection closed before the request arrived");
        request.extend_from_slice(&buffer[..read]);
    }
    cancel.cancel();

    let event = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert!(is_failed(&event), "{event:?}");
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the request was not dropped");
}

// Cancel.

#[tokio::test]
async fn cancel_url_gets_a_signed_post_naming_the_wake() {
    let (url, received) = serve(Answer::Respond(StatusCode::OK, "")).await;
    let wake_id = Uuid::new_v4();
    let config = config(
        &url,
        secret_env(),
        WebhookMode::Async,
        Some(format!("{url}/cancel")),
    );

    adapter().cancel(&config, wake_id).await;

    let received = received.lock().unwrap();
    let request = &received[0];
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body, serde_json::json!({ "wake_id": wake_id.to_string() }));
    assert_eq!(
        header(request, "x-buzz-router-signature"),
        signature(&request.body)
    );
    assert_eq!(header(request, "x-buzz-router-wake"), wake_id.to_string());
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn cancel_url_errors_are_logged() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let (url, _) = serve(Answer::Respond(StatusCode::INTERNAL_SERVER_ERROR, "")).await;
    let wake_id = Uuid::new_v4();
    let config = config(
        &url,
        secret_env(),
        WebhookMode::Async,
        Some(format!("{url}/cancel")),
    );

    adapter().cancel(&config, wake_id).await;

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("cancel_url"), "{logs}");
    assert!(logs.contains(&wake_id.to_string()), "{logs}");
    assert!(!logs.contains(SECRET), "{logs}");
}
