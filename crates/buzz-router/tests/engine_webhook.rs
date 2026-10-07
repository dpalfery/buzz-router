//! Task 5.2: webhook wakes through the engine and the tailnet API (design 6.6 endings, 7.2, 8,
//! A6, R1.5, R30.1, R38.2 to R38.4, R39.2, R39.3, R40.6). The webhook is an in-process axum
//! server on loopback; the agent calls back through the tailnet router.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::post;
use axum::Router;
use buzz_router::api::{tailnet_router, ApiState};
use buzz_router::core::{select_adapters, ApiRequest, ApiResponse, CoreHandle};
use buzz_router::ingest::Source;
use buzz_router::store::wakes::WakeState;
use buzz_router::store::Store;
use hmac::{Hmac, KeyInit, Mac};
use http_body_util::BodyExt;
use nostr::Event;
use router_core::config::parse_roster;
use router_core::ids::BotName;
use router_core::route::{Control, Scope};
use serde_json::{json, Value};
use sha2::Sha256;
use support::{
    base_secs, channel, emoji, keys, roster_toml, spawn_test_core_with, top_level, FakeRelay,
    TestCoreOptions,
};
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "test-webhook-secret-placeholder";
const ADMIN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const PUBLIC_URL: &str = "http://router.invalid:47822";
const TAILNET_BIND: &str = "127.0.0.1:47822";

/// How the webhook answers a wake.
#[derive(Clone)]
enum Answer {
    Respond(StatusCode, &'static str),
    Never,
}

#[derive(Debug, Clone)]
struct Received {
    headers: HeaderMap,
    body: Bytes,
}

#[derive(Clone, Default)]
struct Requests {
    hook: Arc<Mutex<Vec<Received>>>,
    cancel: Arc<Mutex<Vec<Received>>>,
}

#[derive(Clone)]
struct Server {
    answer: Answer,
    requests: Requests,
}

async fn hook(
    State(server): State<Server>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    server
        .requests
        .hook
        .lock()
        .unwrap()
        .push(Received { headers, body });
    match server.answer {
        Answer::Respond(status, body) => (status, body.to_owned()),
        Answer::Never => std::future::pending().await,
    }
}

async fn cancel(State(server): State<Server>, headers: HeaderMap, body: Bytes) -> StatusCode {
    server
        .requests
        .cancel
        .lock()
        .unwrap()
        .push(Received { headers, body });
    StatusCode::OK
}

/// Serves `/hook` with `answer` and `/cancel` with 200, returning the base URL.
async fn serve(answer: Answer) -> (String, Requests) {
    let requests = Requests::default();
    let app = Router::new()
        .route("/hook", post(hook))
        .route("/cancel", post(cancel))
        .with_state(Server {
            answer,
            requests: requests.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), requests)
}

/// A unique environment variable holding the secret, so tests can run in parallel.
fn secret_env() -> String {
    let name = format!("BUZZ_ROUTER_TEST_SECRET_{}", Uuid::new_v4().simple());
    std::env::set_var(&name, SECRET);
    name
}

fn signature(body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

struct Harness {
    core: CoreHandle,
    relay: FakeRelay,
    store: Store,
    tailnet: Router,
    requests: Requests,
    trigger: Event,
}

impl Harness {
    /// A core whose bots use a `mode` webhook answering `answer`, with "@A hi" from the owner
    /// ingested.
    async fn start(mode: &str, answer: Answer, limits: &str) -> Self {
        let (url, requests) = serve(answer).await;
        let adapter_toml = format!(
            "type = \"webhook\"\nurl = \"{url}/hook\"\nsecret_env = \"{}\"\nmode = \"{mode}\"\ncancel_url = \"{url}/cancel\"\n",
            secret_env()
        );
        let data_dir = tempfile::tempdir().unwrap().keep();
        let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
            limits: limits.to_owned(),
            adapter_toml,
            tailnet_bind: TAILNET_BIND.to_owned(),
            public_url: PUBLIC_URL.to_owned(),
            adapters: Some(Box::new(move |config| {
                select_adapters(config, &data_dir, &reqwest::Client::new())
            })),
            ..TestCoreOptions::default()
        });
        let roster = parse_roster(&roster_toml(limits)).unwrap();
        let tailnet = tailnet_router(ApiState::new(core.clone(), ADMIN.to_owned(), &roster));
        let trigger = top_level(&keys("owner"), channel(), "@A hi", base_secs());
        core.ingest(BotName::new("A").unwrap(), trigger.clone(), Source::Live);
        core.flush().await;
        Self {
            core,
            relay,
            store,
            tailnet,
            requests,
            trigger,
        }
    }

    /// The first wake request the webhook received, waiting for it.
    async fn wake_request(&self) -> Received {
        eventually(|| self.requests.hook.lock().unwrap().first().cloned()).await
    }

    async fn wake_body(&self) -> Value {
        serde_json::from_slice(&self.wake_request().await.body).unwrap()
    }

    fn state(&self, wake_id: Uuid) -> WakeState {
        self.store.wakes().get(&wake_id).unwrap().unwrap().state
    }

    async fn state_becomes(&self, wake_id: Uuid, state: WakeState) {
        eventually(|| (self.state(wake_id) == state).then_some(())).await;
        self.core.flush().await;
    }

    async fn call(&self, path: &str, token: &str, body: Option<Value>) -> StatusCode {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let response = self.tailnet.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let _ = response.into_body().collect().await.unwrap();
        status
    }

    async fn post(&self, token: &str, text: &str) -> StatusCode {
        self.call("/v1/post", token, Some(json!({ "text": text })))
            .await
    }
}

/// Polls `check` until it returns `Some`, failing after a while.
async fn eventually<T>(mut check: impl FnMut() -> Option<T>) -> T {
    for _ in 0..400 {
        if let Some(found) = check() {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    check().ok_or("the condition never held").unwrap()
}

fn wake_of(body: &Value) -> (Uuid, String) {
    (
        Uuid::parse_str(body["wake_id"].as_str().unwrap()).unwrap(),
        body["token"].as_str().unwrap().to_owned(),
    )
}

// Async.

#[tokio::test]
async fn the_payload_api_url_is_the_public_url() {
    let h = Harness::start("async", Answer::Respond(StatusCode::ACCEPTED, ""), "").await;

    let body = h.wake_body().await;

    assert_eq!(body["api"]["url"], PUBLIC_URL);
    assert_eq!(body["bot"], "A");
}

#[tokio::test]
async fn a_tailnet_post_publishes_and_ends_the_wake_and_a_second_post_is_gone() {
    let h = Harness::start("async", Answer::Respond(StatusCode::ACCEPTED, ""), "").await;
    let (wake_id, token) = wake_of(&h.wake_body().await);

    assert_eq!(h.post(&token, "done").await, StatusCode::OK);
    h.state_becomes(wake_id, WakeState::Posted).await;

    let replies = h.relay.messages("reply");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].content, "done");
    assert_eq!(h.post(&token, "again").await, StatusCode::GONE);
    assert_eq!(h.relay.messages("reply").len(), 1);
}

#[tokio::test]
async fn a_pass_ends_the_wake_passed_with_a_check_for_an_owner_direct_wake() {
    let h = Harness::start("async", Answer::Respond(StatusCode::ACCEPTED, ""), "").await;
    let (wake_id, token) = wake_of(&h.wake_body().await);

    assert_eq!(h.call("/v1/pass", &token, None).await, StatusCode::OK);
    h.state_becomes(wake_id, WakeState::Passed).await;

    assert_eq!(h.relay.reactions(emoji::CHECK), vec![h.trigger.id]);
}

#[tokio::test]
async fn a_stop_during_an_async_wake_halts_posts_and_calls_cancel_url() {
    let h = Harness::start("async", Answer::Respond(StatusCode::ACCEPTED, ""), "").await;
    let (wake_id, token) = wake_of(&h.wake_body().await);
    h.state_becomes(wake_id, WakeState::Running).await;

    let response = h
        .core
        .api(ApiRequest::Control(Control::Stop(Scope::All)))
        .await;
    assert_eq!(response, ApiResponse::Done);

    assert_eq!(h.post(&token, "late").await, StatusCode::LOCKED);
    assert_eq!(h.state(wake_id), WakeState::Killed);
    let request = eventually(|| h.requests.cancel.lock().unwrap().first().cloned()).await;
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body, json!({ "wake_id": wake_id }));
    assert_eq!(
        request.headers["x-buzz-router-signature"].to_str().unwrap(),
        signature(&request.body)
    );
    assert_eq!(
        request.headers["x-buzz-router-wake"].to_str().unwrap(),
        wake_id.to_string()
    );
    assert!(h.relay.messages("reply").is_empty());
}

// Sync.

#[tokio::test]
async fn a_sync_text_reply_is_published_under_the_reaction_target() {
    let h = Harness::start(
        "sync",
        Answer::Respond(StatusCode::OK, r#"{"text":"hello from the routine"}"#),
        "",
    )
    .await;
    let (wake_id, _) = wake_of(&h.wake_body().await);

    h.state_becomes(wake_id, WakeState::Posted).await;

    let replies = h.relay.messages("reply");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].content, "hello from the routine");
    assert!(replies[0].tags.event_ids().any(|id| *id == h.trigger.id));
}

#[tokio::test(start_paused = true)]
async fn a_sync_wake_at_its_deadline_times_out_with_an_hourglass() {
    let h = Harness::start("sync", Answer::Never, "max_wake_minutes = 1").await;
    let (wake_id, _) = wake_of(&h.wake_body().await);

    tokio::time::sleep(Duration::from_secs(61)).await;
    h.state_becomes(wake_id, WakeState::Timeout).await;

    assert_eq!(h.relay.reactions(emoji::HOURGLASS), vec![h.trigger.id]);
    assert!(
        h.requests.cancel.lock().unwrap().is_empty(),
        "a deadline does not call cancel_url"
    );
}
