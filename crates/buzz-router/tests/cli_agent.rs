//! Task 3.9: the agent commands `post`, `pass` and `eta` (design 12.1 and 14, R53.1–R53.3, R57).
//!
//! The binary runs against an in-process API on `127.0.0.1:0`, served by a test core whose bot A
//! has a running `FakeAdapter` wake. Time is real: the binary is a separate process.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::io::Write;
use std::process::{Command, Output, Stdio};

use buzz_router::api::{loopback_router, ApiState};
use buzz_router::core::CoreHandle;
use buzz_router::ingest::Source;
use buzz_router::store::halts::HaltScope;
use buzz_router::store::wakes::WakeState;
use buzz_router::store::Store;
use router_core::config::parse_roster;
use router_core::ids::BotName;
use serde_json::Value;
use support::{
    base_secs, channel, keys, roster_toml, spawn_test_core_with, top_level, FakeAdapter, FakeRelay,
    Step, TestCoreOptions,
};

struct Daemon {
    _core: CoreHandle,
    relay: FakeRelay,
    store: Store,
    url: String,
    token: String,
    dir: tempfile::TempDir,
}

async fn daemon() -> Daemon {
    daemon_with(vec![Step::Hang]).await
}

/// Polls `probe` every 10 ms until it returns a value, for at most 5 s.
async fn wait_for<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    for _ in 0..500 {
        if let Some(value) = probe() {
            return value;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("timed out waiting");
}

async fn daemon_with(script: Vec<Step>) -> Daemon {
    let script_end = script.last().cloned().unwrap();
    let adapter = FakeAdapter::new(script);
    let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });
    let roster = parse_roster(&roster_toml("")).unwrap();
    let state = ApiState::new(core.clone(), "admin".to_owned(), &roster);
    let event = top_level(&keys("owner"), channel(), "@A hi", base_secs());
    core.ingest(BotName::new("A").unwrap(), event, Source::Live);
    let dispatch = wait_for(|| adapter.dispatches().into_iter().next()).await;
    let token = dispatch.token.clone();
    if !matches!(script_end, Step::Hang) {
        wait_for(|| {
            let wake = store.wakes().get(&dispatch.wake_id).unwrap()?;
            (wake.state != WakeState::Running).then_some(())
        })
        .await;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, loopback_router(state)).await });
    Daemon {
        _core: core,
        relay,
        store,
        url,
        token,
        dir: tempfile::tempdir().unwrap(),
    }
}

/// Runs the binary with `args`, the given env (`None` removes the variable) and `stdin`.
async fn run(args: &[&str], url: Option<&str>, token: Option<&str>, stdin: &str) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_buzz-router"));
    command
        .args(args)
        .env("BUZZ_ROUTER_CONFIG_DIR", dir.path())
        .env("BUZZ_ROUTER_DATA_DIR", dir.path())
        .env_remove("BUZZ_ROUTER_URL")
        .env_remove("BUZZ_ROUTER_WAKE_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(url) = url {
        command.env("BUZZ_ROUTER_URL", url);
    }
    if let Some(token) = token {
        command.env("BUZZ_ROUTER_WAKE_TOKEN", token);
    }
    let stdin = stdin.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    })
    .await
    .unwrap()
}

impl Daemon {
    async fn run(&self, args: &[&str]) -> Output {
        run(args, Some(&self.url), Some(&self.token), "").await
    }
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The JSON error line on stderr.
fn error_line(output: &Output) -> Value {
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    let line = stderr.lines().last().unwrap();
    serde_json::from_str(line).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn post_text_prints_the_event_id() {
    let daemon = daemon().await;

    let output = daemon.run(&["post", "--text", "hi"]).await;

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let replies = daemon.relay.messages("reply");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].content, "hi");
    assert_eq!(
        stdout_json(&output),
        serde_json::json!({ "event_id": replies[0].id.to_hex() })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn post_text_file_reads_stdin_or_a_file() {
    let daemon = daemon().await;

    let output = run(
        &["post", "--text-file", "-"],
        Some(&daemon.url),
        Some(&daemon.token),
        "from stdin\n",
    )
    .await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let path = daemon.dir.path().join("reply.txt");
    std::fs::write(&path, "from a file").unwrap();
    let output = daemon
        .run(&["post", "--text-file", path.to_str().unwrap()])
        .await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let texts: Vec<String> = daemon
        .relay
        .messages("reply")
        .into_iter()
        .map(|event| event.content)
        .collect();
    assert_eq!(texts, ["from stdin", "from a file"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn eta_and_pass_work() {
    let daemon = daemon().await;

    let output = daemon.run(&["eta", "--text", "about 10 minutes"]).await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(stdout_json(&output), serde_json::json!({}));

    let output = daemon.run(&["pass"]).await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(stdout_json(&output), serde_json::json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_env_vars_are_user_errors() {
    let daemon = daemon().await;

    for (url, token) in [
        (None, Some(daemon.token.as_str())),
        (Some(daemon.url.as_str()), None),
    ] {
        let output = run(&["pass"], url, token, "").await;
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert_eq!(error_line(&output)["error"], "user_error");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_halted_bot_exits_4_with_halted_in_the_message() {
    let daemon = daemon().await;
    daemon
        .store
        .halts()
        .set(&HaltScope::All, Some("test"), 0)
        .unwrap();

    let output = daemon.run(&["post", "--text", "hi"]).await;

    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let line = error_line(&output);
    assert_eq!(line["error"], "error");
    assert!(
        line["message"].as_str().unwrap().contains("halted"),
        "{line}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn api_errors_map_to_exit_codes() {
    let daemon = daemon().await;

    let output = run(&["pass"], Some(&daemon.url), Some("not-a-token"), "").await;
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_eq!(error_line(&output)["error"], "auth_error");

    let path = daemon.dir.path().join("long.txt");
    std::fs::write(&path, "x".repeat(65 * 1024)).unwrap();
    let output = daemon
        .run(&["post", "--text-file", path.to_str().unwrap()])
        .await;
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(error_line(&output)["error"], "user_error");

    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let output = run(&["pass"], Some(&dead_url), Some(&daemon.token), "").await;
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let line = error_line(&output);
    assert_eq!(line["error"], "network_error");
    assert_eq!(line["retryable"], true);

    let ended = daemon_with(vec![Step::Exit(0)]).await;
    let output = ended.run(&["post", "--text", "late"]).await;
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let line = error_line(&output);
    assert_eq!(line["error"], "error");
    assert!(
        line["message"].as_str().unwrap().contains("wake_ended"),
        "{line}"
    );
}
