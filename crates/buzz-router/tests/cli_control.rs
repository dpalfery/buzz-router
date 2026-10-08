//! Task 3.8: `stop`, `resume` and `cancel` through the admin API (design 6.7 and 12.1, R33.1).
//!
//! The binary runs against an in-process admin API on `127.0.0.1:0` that records each request.
//! `router.toml` points `api_bind` at it, and `admin.token` is written to the data directory.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{OriginalUri, State};
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use support::{roster_toml, router_toml};

const ADMIN: &str = "feedface00000000feedface00000000feedface00000000feedface00000000";

/// One recorded request: path, authorization header and JSON body.
type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

async fn record(
    State(seen): State<Seen>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    let auth = headers
        .get("authorization")
        .map(|value| value.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let body: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    let scope = body.get("bots").cloned().unwrap_or_else(|| json!("all"));
    seen.lock()
        .unwrap()
        .push((uri.path().to_owned(), auth, body));
    Json(json!({ "scope": scope }))
}

struct Sandbox {
    dir: tempfile::TempDir,
    seen: Seen,
}

async fn sandbox() -> Sandbox {
    let seen: Seen = Arc::default();
    let app = Router::new()
        .route("/v1/stop", post(record))
        .route("/v1/resume", post(record))
        .route("/v1/cancel", post(record))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bind = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(dir.path().join("config/roster.toml"), roster_toml("")).unwrap();
    std::fs::write(
        dir.path().join("config/router.toml"),
        router_toml(1).replace("127.0.0.1:47821", &bind.to_string()),
    )
    .unwrap();
    std::fs::write(dir.path().join("data/admin.token"), format!("{ADMIN}\n")).unwrap();
    Sandbox { dir, seen }
}

impl Sandbox {
    async fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_buzz-router"));
        command
            .args(args)
            .env("BUZZ_ROUTER_CONFIG_DIR", self.dir.path().join("config"))
            .env("BUZZ_ROUTER_DATA_DIR", self.dir.path().join("data"));
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_command_sends_its_admin_body_and_exits_zero() {
    let sandbox = sandbox().await;

    let cases: [(&[&str], &str, Value); 6] = [
        (&["stop"], "/v1/stop", json!({})),
        (
            &["stop", "--bot", "A", "--bot", "B"],
            "/v1/stop",
            json!({ "bots": ["A", "B"] }),
        ),
        (&["resume"], "/v1/resume", json!({})),
        (
            &["resume", "--bot", "A"],
            "/v1/resume",
            json!({ "bots": ["A"] }),
        ),
        (&["cancel"], "/v1/cancel", json!({})),
        (
            &["cancel", "--bot", "C"],
            "/v1/cancel",
            json!({ "bots": ["C"] }),
        ),
    ];
    for (args, path, body) in &cases {
        let output = sandbox.run(args).await;
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        let (seen_path, auth, seen_body) = sandbox.seen.lock().unwrap().last().cloned().unwrap();
        assert_eq!(&seen_path, path, "{args:?}");
        assert_eq!(auth, format!("Bearer {ADMIN}"), "{args:?}");
        assert_eq!(&seen_body, body, "{args:?}");
        let printed: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(printed.get("scope").is_some(), "{args:?}: {printed}");
    }
    assert_eq!(sandbox.seen.lock().unwrap().len(), cases.len());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_bot_is_bad_input_and_sends_nothing() {
    let sandbox = sandbox().await;

    let output = sandbox.run(&["stop", "--bot", "Nobody"]).await;

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(sandbox.seen.lock().unwrap().is_empty());
}
