//! Task 3.6: the HTTP API, its auth, the `/v1/post` precedence, body limits and the admin token
//! (design section 8, DD-22, R28, R42, R43, A13).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use buzz_router::api::{admin_token, loopback_router, tailnet_router, ApiState};
use buzz_router::core::CoreHandle;
use buzz_router::ingest::Source;
use buzz_router::store::halts::HaltScope;
use buzz_router::store::wakes::WakeState;
use buzz_router::store::Store;
use http_body_util::BodyExt;
use nostr::Event;
use router_core::config::{parse_roster, parse_router};
use router_core::ids::BotName;
use serde_json::{json, Value};
use support::{
    base_secs, channel, keys, roster_toml, router_toml, spawn_test_core_with, top_level,
    FakeAdapter, FakeRelay, Step, TestCoreOptions, DEFAULT_ADAPTER_TOML,
};
use tower::ServiceExt;

const ADMIN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const API_ADAPTER_TOML: &str = "type = \"command\"\ncommand = [\"agent\"]\ncwd = \"~/fixture\"\nenv = {}\nprompt_mode = \"stdin\"\nreply_mode = \"api\"\nprompt_template = \"\"\n";

struct Api {
    core: CoreHandle,
    relay: FakeRelay,
    store: Store,
    adapter: FakeAdapter,
    loopback: Router,
    tailnet: Router,
    event: Event,
}

impl Api {
    /// The token of the `n`th dispatched wake.
    fn token(&self, n: usize) -> String {
        self.adapter.dispatches()[n].token.clone()
    }

    fn wake_state(&self, n: usize) -> WakeState {
        let id = self.adapter.dispatches()[n].wake_id;
        self.store.wakes().get(&id).unwrap().unwrap().state
    }
}

/// A core whose bots run `script`, the owner's "@A hi" ingested, and both routers.
async fn api_with(script: Vec<Step>, limits: &str, adapter_toml: &str) -> Api {
    let adapter = FakeAdapter::new(script);
    let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        limits: limits.to_owned(),
        adapter_toml: adapter_toml.to_owned(),
        ..TestCoreOptions::default()
    });
    let roster = parse_roster(&roster_toml(limits)).unwrap();
    let state = ApiState::new(core.clone(), ADMIN.to_owned(), &roster);
    let event = top_level(&keys("owner"), channel(), "@A hi", base_secs());
    core.ingest(BotName::new("A").unwrap(), event.clone(), Source::Live);
    core.flush().await;
    Api {
        core,
        relay,
        store,
        adapter,
        loopback: loopback_router(state.clone()),
        tailnet: tailnet_router(state),
        event,
    }
}

async fn api(script: Vec<Step>) -> Api {
    api_with(script, "", DEFAULT_ADAPTER_TOML).await
}

/// Sends a request and returns the status and the JSON body (`Null` when empty).
async fn send(
    router: &Router,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<Vec<u8>>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request
        .body(body.map_or_else(Body::empty, Body::from))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

fn text_body(text: &str) -> Option<Vec<u8>> {
    Some(json!({ "text": text }).to_string().into_bytes())
}

async fn post(router: &Router, token: Option<&str>, text: &str) -> (StatusCode, Value) {
    send(router, "POST", "/v1/post", token, text_body(text)).await
}

#[tokio::test(start_paused = true)]
async fn an_unknown_or_missing_token_is_unauthorized() {
    let api = api(vec![Step::Hang]).await;

    for token in [None, Some("deadbeef")] {
        let (status, body) = post(&api.loopback, token, "hi").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{token:?}");
        assert_eq!(body["error"], "unauthorized");
        let (status, _) = send(&api.loopback, "POST", "/v1/pass", token, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = send(&api.loopback, "POST", "/v1/eta", token, text_body("x")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test(start_paused = true)]
async fn a_successful_post_returns_the_event_id_and_reaches_the_relay() {
    let api = api(vec![Step::Hang]).await;

    let (status, body) = post(&api.loopback, Some(&api.token(0)), "hello").await;

    assert_eq!(status, StatusCode::OK);
    let replies = api.relay.messages("reply");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].content, "hello");
    assert_eq!(body, json!({ "event_id": replies[0].id.to_hex() }));
}

#[tokio::test(start_paused = true)]
async fn an_ended_wake_gets_410_and_a_halted_bot_423_even_when_ended() {
    let api = api(vec![Step::Exit(0)]).await;
    api.core.flush().await;
    assert_eq!(api.wake_state(0), WakeState::Passed);
    let token = api.token(0);

    let (status, body) = post(&api.loopback, Some(&token), "late").await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(body["error"], "wake_ended");
    let (status, _) = send(&api.loopback, "POST", "/v1/pass", Some(&token), None).await;
    assert_eq!(status, StatusCode::GONE);
    let (status, _) = send(
        &api.loopback,
        "POST",
        "/v1/eta",
        Some(&token),
        text_body("x"),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);

    api.store
        .halts()
        .set(&HaltScope::Bot(BotName::new("A").unwrap()), Some("test"), 0)
        .unwrap();
    let (status, body) = post(&api.loopback, Some(&token), "late").await;
    assert_eq!(status, StatusCode::LOCKED);
    assert_eq!(body["error"], "halted");
    assert!(api.relay.messages("reply").is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_post_after_the_deadline_gets_410() {
    let api = api_with(
        vec![Step::Hang],
        "max_wake_minutes = 1",
        DEFAULT_ADAPTER_TOML,
    )
    .await;
    tokio::time::sleep(Duration::from_secs(90)).await;
    api.core.flush().await;
    assert_eq!(api.wake_state(0), WakeState::Timeout);

    let (status, body) = post(&api.loopback, Some(&api.token(0)), "late").await;

    assert_eq!(status, StatusCode::GONE);
    assert_eq!(body["error"], "wake_ended");
}

#[tokio::test(start_paused = true)]
async fn the_fourth_post_gets_429_and_a_pass_after_it_still_gets_200() {
    let api = api_with(vec![Step::Hang], "", API_ADAPTER_TOML).await;
    let token = api.token(0);

    for n in 0..3 {
        let (status, _) = post(&api.loopback, Some(&token), &format!("reply {n}")).await;
        assert_eq!(status, StatusCode::OK, "post {n}");
    }
    let (status, body) = post(&api.loopback, Some(&token), "one too many").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "too_many_posts");
    assert_eq!(api.relay.messages("reply").len(), 3);

    let (status, body) = send(&api.loopback, "POST", "/v1/pass", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({}));
}

#[tokio::test(start_paused = true)]
async fn a_pass_ends_the_wake_as_passed() {
    let api = api_with(vec![Step::Hang], "", API_ADAPTER_TOML).await;

    let (status, body) = send(&api.loopback, "POST", "/v1/pass", Some(&api.token(0)), None).await;
    api.core.flush().await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({}));
    assert_eq!(api.wake_state(0), WakeState::Passed);
}

#[tokio::test(start_paused = true)]
async fn an_eta_is_used_by_the_status_note() {
    let api = api(vec![Step::Hang]).await;

    let (status, body) = send(
        &api.loopback,
        "POST",
        "/v1/eta",
        Some(&api.token(0)),
        text_body("10 minutes"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({}));
    tokio::time::sleep(Duration::from_secs(25)).await;
    api.core.flush().await;

    let notes = api.relay.messages("status");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].content, "On it, about 10 minutes.");
    assert_eq!(notes[0].tags.event_ids().next(), Some(&api.event.id));
}

#[tokio::test(start_paused = true)]
async fn admin_routes_need_the_admin_token() {
    let api = api(vec![Step::Hang]).await;

    let (status, body) = send(&api.loopback, "GET", "/v1/status", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");
    let wake_token = api.token(0);
    for token in [None, Some("wrong"), Some(wake_token.as_str())] {
        let (status, _) = send(&api.loopback, "GET", "/v1/status", token, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{token:?}");
        for path in ["/v1/stop", "/v1/resume", "/v1/cancel"] {
            let (status, _) = send(&api.loopback, "POST", path, token, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path} {token:?}");
        }
    }
}

#[tokio::test(start_paused = true)]
async fn an_admin_control_naming_an_unknown_bot_is_bad_input() {
    let api = api(vec![Step::Hang]).await;

    for path in ["/v1/stop", "/v1/resume", "/v1/cancel"] {
        let body = Some(json!({ "bots": ["A", "Nobody"] }).to_string().into_bytes());
        let (status, body) = send(&api.loopback, "POST", path, Some(ADMIN), body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(body["error"], "bad_request");
    }
}

#[tokio::test(start_paused = true)]
async fn the_tailnet_router_serves_token_routes_only() {
    let api = api(vec![Step::Hang]).await;

    let (status, body) = send(&api.tailnet, "GET", "/v1/status", Some(ADMIN), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");
    for path in ["/v1/stop", "/v1/resume", "/v1/cancel"] {
        let (status, _) = send(&api.tailnet, "POST", path, Some(ADMIN), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _) = send(&api.tailnet, "GET", "/anything", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = post(&api.tailnet, Some(&api.token(0)), "over the tailnet").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(api.relay.messages("reply").len(), 1);
}

#[tokio::test(start_paused = true)]
async fn oversized_bodies_and_texts_are_bad_requests() {
    let api = api(vec![Step::Hang]).await;
    let token = api.token(0);

    let huge = vec![b'x'; 71 * 1024];
    let (status, body) = send(&api.loopback, "POST", "/v1/post", Some(&token), Some(huge)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "bad_request");

    let long = "y".repeat(65 * 1024);
    let (status, body) = post(&api.loopback, Some(&token), &long).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "bad_request");

    let (status, _) = send(
        &api.loopback,
        "POST",
        "/v1/post",
        Some(&token),
        Some(b"not json".to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(api.relay.messages("reply").is_empty());
}

#[test]
fn an_omitted_api_bind_resolves_to_the_default_loopback_port() {
    let roster = parse_roster(&roster_toml("")).unwrap();
    let toml = router_toml(1).replace("api_bind = \"127.0.0.1:47821\"\n", "");
    assert!(!toml.contains("api_bind"));
    let config = parse_router(&toml, &roster).unwrap();
    assert_eq!(config.api_bind.to_string(), "127.0.0.1:47821");
}

#[test]
fn admin_token_ensure_creates_a_private_hex_token_once() {
    let dir = tempfile::tempdir().unwrap();

    let token = admin_token::ensure(dir.path()).unwrap();

    assert_eq!(token.len(), 64);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    let path = dir.path().join("admin.token");
    assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), token);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert_eq!(admin_token::ensure(dir.path()).unwrap(), token);

    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("admin.token"), "existing\n").unwrap();
    assert_eq!(admin_token::ensure(other.path()).unwrap(), "existing");
    assert_eq!(
        std::fs::read_to_string(other.path().join("admin.token")).unwrap(),
        "existing\n"
    );
}
