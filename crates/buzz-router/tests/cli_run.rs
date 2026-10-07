//! Task 3.10: `run` wiring (design 6.2 steps 1 to 5 and 7, DD-23, DD-24, R1.1, R1.4, R1.15, R3.5,
//! R43.4, R52.1, R62.6). Each test runs the binary against the in-process mock relay of
//! `support::daemon`. Nothing connects to a real relay.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::time::Duration;

use support::daemon::{eventually, listening, started, Sandbox};
use support::pubkey_hex;

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_roster_exits_1_with_one_json_line_and_listens_on_nothing() {
    let sandbox = Sandbox::new("version = 1\n[owner]\n", "ws://127.0.0.1:9");
    let mut daemon = sandbox.spawn();

    assert_eq!(daemon.exit_code().await, Some(1));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stderr = daemon.stderr();
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 1, "{stderr}");
    let line: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(line["error"], "user_error");
    assert!(!listening(&sandbox.api()).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_valid_config_serves_the_api_and_the_bots_with_a_key() {
    let (sandbox, mut daemon, authed) = started().await;
    let http = reqwest::Client::new();

    let pass = http
        .post(format!("{}/v1/pass", sandbox.api()))
        .send()
        .await
        .unwrap();
    assert_eq!(pass.status().as_u16(), 401);

    let token = sandbox.data.join("admin.token");
    assert!(token.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&token).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let tailnet = sandbox.tailnet();
    eventually("the tailnet API listens", async || {
        listening(&tailnet).await
    })
    .await;
    let pass = http
        .post(format!("{tailnet}/v1/pass"))
        .send()
        .await
        .unwrap();
    assert_eq!(pass.status().as_u16(), 401);
    let status = http
        .get(format!("{tailnet}/v1/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(status.status().as_u16(), 404);

    eventually("bot B authenticates", async || {
        authed.lock().unwrap().contains(&pubkey_hex("B"))
    })
    .await;
    assert!(!authed.lock().unwrap().contains(&pubkey_hex("A")));
    let stderr = daemon.stderr();
    assert!(
        stderr
            .lines()
            .any(|line| line.contains("unavailable") && line.contains("A")),
        "{stderr}"
    );

    #[cfg(unix)]
    {
        daemon.signal(nix::sys::signal::Signal::SIGTERM);
        assert_eq!(daemon.exit_code().await, Some(0));
    }
    #[cfg(not(unix))]
    daemon.child.kill().await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn ctrl_c_exits_0() {
    let (_sandbox, mut daemon, _) = started().await;

    daemon.signal(nix::sys::signal::Signal::SIGINT);

    assert_eq!(daemon.exit_code().await, Some(0));
}
