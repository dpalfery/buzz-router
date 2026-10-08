//! Task 2.8: `capture --channel ID --since DUR` (design 12.1, requirements 55.1 and 57).
//!
//! The binary runs against an axum mock relay on `127.0.0.1:0`. `router.toml` points `relay_url`
//! at it as `ws://127.0.0.1:<port>` and every bot uses a `file:` key. The mock answers `/query`:
//! kind-39002 membership lists by `#p`, and the channel's events by `#h`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine as _;
use nostr::{Event, JsonUtil};
use serde_json::Value;
use support::{keys, message, raw_event};
use uuid::Uuid;

const CHANNEL: &str = "6a0e2f4c-1b3d-4e5f-8a7b-9c0d1e2f3a4b";
const OTHER_CHANNEL: &str = "7b1f3a5d-2c4e-4f60-9b8c-0d1e2f3a4b5c";

#[derive(Default)]
struct Mock {
    /// Each bot's channel memberships: (bot pubkey hex, channel uuid).
    memberships: Vec<(String, String)>,
    /// The channel's events, served for `#h` queries in this (scrambled) order.
    events: Vec<Event>,
    /// Every channel-event query: its `since` and the signer of its NIP-98 header.
    channel_queries: Vec<(Option<u64>, String)>,
}

type Shared = Arc<Mutex<Mock>>;

fn nip98_signer(headers: &HeaderMap) -> String {
    let value = headers.get("authorization").unwrap().to_str().unwrap();
    let encoded = value.strip_prefix("Nostr ").unwrap();
    let json = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    let event = Event::from_json(json).unwrap();
    event.pubkey.to_hex()
}

async fn query(State(mock): State<Shared>, headers: HeaderMap, body: Bytes) -> Json<Value> {
    let filters: Vec<Value> = serde_json::from_slice(&body).unwrap();
    let mut mock = mock.lock().unwrap();
    let mut out: Vec<Value> = Vec::new();
    for filter in filters {
        let kinds: Vec<u64> = filter["kinds"]
            .as_array()
            .map(|k| k.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        if kinds.contains(&39002) {
            let wanted: Vec<String> = filter["#p"]
                .as_array()
                .map(|p| {
                    p.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            for (pubkey, channel) in &mock.memberships {
                if wanted.contains(pubkey) {
                    let event = raw_event(
                        &keys("relay"),
                        39002,
                        "",
                        &[&["d", channel], &["p", pubkey]],
                        1_000,
                    );
                    out.push(serde_json::to_value(&event).unwrap());
                }
            }
        } else if filter.get("#h").is_some() {
            let since = filter["since"].as_u64();
            mock.channel_queries.push((since, nip98_signer(&headers)));
            for event in &mock.events {
                out.push(serde_json::to_value(event).unwrap());
            }
        }
    }
    Json(Value::Array(out))
}

async fn start_mock(mock: Shared) -> u16 {
    let app = Router::new().route("/query", post(query)).with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

/// Writes a roster and a `router.toml` serving `bots`, each with a 0600 `file:` key.
fn write_config(dir: &Path, port: u16, bots: &[&str]) {
    let mut roster = String::from(
        r#"version = 1

[owner]
name = "Fixture Owner"
pubkeys = ["1111111111111111111111111111111111111111111111111111111111111111"]
timezone = "UTC"
"#,
    );
    let mut router = format!(
        r#"relay_url = "ws://127.0.0.1:{port}"
api_bind = "127.0.0.1:47821"
tailnet_bind = ""
public_url = ""
roster_path = "roster.toml"
"#
    );
    for name in bots {
        roster.push_str(&format!(
            "\n[[bots]]\nname = \"{name}\"\npubkey = \"{}\"\nchannels = [\"*\"]\nrespond_to = \"owner-only\"\n",
            keys(name).public_key().to_hex()
        ));
        let key_file = dir.join(format!("{name}.key"));
        fs::write(&key_file, keys(name).secret_key().to_secret_hex()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&key_file, fs::Permissions::from_mode(0o600)).unwrap();
        }
        router.push_str(&format!(
            r#"
[[bots]]
name = "{name}"
key = "file:{name}.key"
auth_tag = ""
max_concurrent = 1

[bots.adapter]
type = "command"
command = ["agent"]
cwd = "~/fixture"
env = {{}}
prompt_mode = "stdin"
reply_mode = "stdout"
prompt_template = ""
"#
        ));
    }
    fs::write(dir.join("roster.toml"), roster).unwrap();
    fs::write(dir.join("router.toml"), router).unwrap();
}

async fn capture(config: &Path, args: &[&str]) -> Output {
    let data = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_buzz-router"));
    command
        .arg("capture")
        .args(args)
        .arg("--config-dir")
        .arg(config)
        .arg("--data-dir")
        .arg(data.path())
        .env_remove("BUZZ_ROUTER_CONFIG_DIR")
        .env_remove("BUZZ_ROUTER_DATA_DIR");
    tokio::task::spawn_blocking(move || {
        let output = command.output().unwrap();
        drop(data);
        output
    })
    .await
    .unwrap()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn assert_json_error(output: &Output, category: &str) {
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    let line: Value = serde_json::from_str(stderr.trim_end()).expect("one JSON error line");
    assert_eq!(line["error"], category, "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_prints_the_channel_ascending_as_signed_jsonl() {
    let channel = Uuid::parse_str(CHANNEL).unwrap();
    let owner = keys("owner");
    let now = now_secs();
    let first = message(&owner, channel, "first", None, &[], now - 3_000);
    let second = message(&owner, channel, "second", None, &[], now - 2_000);
    let third = message(&owner, channel, "third", None, &[], now - 1_000);

    let mock: Shared = Arc::default();
    {
        let mut mock = mock.lock().unwrap();
        mock.memberships = vec![
            (keys("A").public_key().to_hex(), OTHER_CHANNEL.into()),
            (keys("B").public_key().to_hex(), CHANNEL.into()),
        ];
        mock.events = vec![third.clone(), first.clone(), second.clone()];
    }
    let port = start_mock(mock.clone()).await;
    let config = tempfile::tempdir().unwrap();
    write_config(config.path(), port, &["A", "B"]);

    let output = capture(config.path(), &["--channel", CHANNEL, "--since", "2h"]).await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");

    let stdout = String::from_utf8(output.stdout).unwrap();
    let events: Vec<Event> = stdout
        .lines()
        .map(|line| Event::from_json(line).expect("each line is an event"))
        .collect();
    for event in &events {
        event.verify().expect("each line is a validly signed event");
    }
    let ids: Vec<_> = events.iter().map(|e| e.id).collect();
    assert_eq!(ids, vec![first.id, second.id, third.id], "ascending order");

    let queries = mock.lock().unwrap().channel_queries.clone();
    assert_eq!(queries.len(), 1, "{queries:?}");
    let (since, signer) = &queries[0];
    let since = since.expect("the query carries since");
    let expected = now_secs() - 7_200;
    assert!(
        since.abs_diff(expected) <= 5,
        "since {since} should be now - 7200 ({expected})"
    );
    assert_eq!(
        signer,
        &keys("B").public_key().to_hex(),
        "the first local bot that is a member of the channel queries"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_since_exits_one_with_a_json_error() {
    let port = start_mock(Arc::default()).await;
    let config = tempfile::tempdir().unwrap();
    write_config(config.path(), port, &["A"]);

    let output = capture(config.path(), &["--channel", CHANNEL, "--since", "7x"]).await;
    assert_eq!(output.status.code(), Some(1));
    assert_json_error(&output, "user_error");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_channel_with_no_member_bot_exits_one() {
    let mock: Shared = Arc::default();
    mock.lock().unwrap().memberships =
        vec![(keys("A").public_key().to_hex(), OTHER_CHANNEL.into())];
    let port = start_mock(mock.clone()).await;
    let config = tempfile::tempdir().unwrap();
    write_config(config.path(), port, &["A"]);

    let output = capture(config.path(), &["--channel", CHANNEL, "--since", "1d"]).await;
    assert_eq!(output.status.code(), Some(1));
    assert_json_error(&output, "user_error");
    assert!(mock.lock().unwrap().channel_queries.is_empty());
}
