//! Task 4.3: `status [--json]` against the running binary (design 12.1, 12.2, R2.16, R52.2,
//! R52.3). The daemon runs against the in-process mock relay of `support::daemon`.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use serde_json::Value;
use support::daemon::{eventually, started, Sandbox};
use support::{pubkey_hex, roster_toml};

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn status_json_prints_exactly_the_body_of_get_status() {
    let (sandbox, _daemon, authed) = started().await;
    eventually("bot B authenticates", async || {
        authed.lock().unwrap().contains(&pubkey_hex("B"))
    })
    .await;
    let http = reqwest::Client::new();
    let get = async || {
        http.get(format!("{}/v1/status", sandbox.api()))
            .bearer_auth(sandbox.admin_token())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    };
    eventually("bot B shows as connected", async || {
        let body: Value = serde_json::from_str(&get().await).unwrap();
        body["bots"][1]["connected"] == true
    })
    .await;

    let output = sandbox.cli(&["status", "--json"]).await;
    let body = get().await;

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(stdout(&output).trim_end(), body);
    let document: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(document["version"], env!("CARGO_PKG_VERSION"));
    let roster = sandbox.cli(&["roster", "check"]).await;
    assert_eq!(roster.status.code(), Some(0), "{roster:?}");
    assert_eq!(document["roster_hash"], stdout(&roster).trim());
    let bots = document["bots"].as_array().unwrap();
    assert_eq!(bots.len(), 2);
    assert_eq!(bots[0]["name"], "A");
    assert_eq!(bots[0]["available"], false);
    assert_eq!(bots[0]["connected"], false);
    assert_eq!(bots[1]["name"], "B");
    assert_eq!(bots[1]["available"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn status_without_json_prints_a_table_naming_each_bot() {
    let (sandbox, _daemon, _) = started().await;

    let output = sandbox.cli(&["status"]).await;

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = stdout(&output);
    assert!(serde_json::from_str::<Value>(&text).is_err(), "{text}");
    let roster = sandbox.cli(&["roster", "check"]).await;
    assert!(text.contains(stdout(&roster).trim()), "{text}");
    assert!(text.lines().any(|line| line.starts_with("A ")), "{text}");
    assert!(text.lines().any(|line| line.starts_with("B ")), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn with_the_daemon_down_status_is_a_network_error() {
    let sandbox = Sandbox::new(&roster_toml(""), "ws://127.0.0.1:9");
    std::fs::create_dir_all(&sandbox.data).unwrap();
    std::fs::write(sandbox.data.join("admin.token"), "a".repeat(64)).unwrap();

    for args in [&["status"][..], &["status", "--json"][..]] {
        let output = sandbox.cli(args).await;

        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        let line: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
        assert_eq!(line["error"], "network_error");
        assert!(output.stdout.is_empty());
    }
}
