//! Task 3.10: `run` wiring (design 6.2 steps 1 to 5 and 7, DD-23, DD-24, R1.1, R1.4, R1.15, R3.5,
//! R43.4, R52.1, R62.6). Each test runs the binary against an in-process mock WebSocket relay that
//! completes NIP-42 auth and records who authenticated. Nothing connects to a real relay.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nostr::{Event, JsonUtil};
use support::{keys, pubkey_hex, roster_toml, DEFAULT_ADAPTER_TOML};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::Message;

/// A port nothing listens on right now.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// A mock relay: answers every connection with an AUTH challenge, accepts the auth event and every
/// published event, and records the pubkeys that authenticated.
async fn mock_relay() -> (String, Arc<Mutex<BTreeSet<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let authed = Arc::new(Mutex::new(BTreeSet::new()));
    let seen = authed.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let seen = seen.clone();
            tokio::spawn(async move {
                // REST calls to the same port fail the handshake, which the router tolerates.
                let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
                    return;
                };
                let (mut sink, mut stream) = ws.split();
                let challenge = serde_json::json!(["AUTH", "challenge-1"]).to_string();
                if sink.send(Message::Text(challenge.into())).await.is_err() {
                    return;
                }
                while let Some(Ok(message)) = stream.next().await {
                    let Message::Text(text) = message else {
                        continue;
                    };
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    let verb = value[0].as_str().unwrap_or_default().to_owned();
                    if verb != "AUTH" && verb != "EVENT" {
                        continue;
                    }
                    let Ok(event) = Event::from_json(value[1].to_string()) else {
                        continue;
                    };
                    if verb == "AUTH" {
                        seen.lock().unwrap().insert(event.pubkey.to_hex());
                    }
                    let ok = serde_json::json!(["OK", event.id.to_hex(), true]).to_string();
                    if sink.send(Message::Text(ok.into())).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (url, authed)
}

/// A config and data directory pair.
struct Sandbox {
    _dir: tempfile::TempDir,
    config: PathBuf,
    data: PathBuf,
    api_port: u16,
    tailnet_port: u16,
}

impl Sandbox {
    /// Writes `roster` and a `router.toml` serving bots A and B with file keys. Only B's key file
    /// exists.
    fn new(roster: &str, relay_url: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let data = dir.path().join("data");
        std::fs::create_dir_all(config.join("keys")).unwrap();
        std::fs::write(config.join("roster.toml"), roster).unwrap();
        let api_port = free_port();
        let tailnet_port = free_port();
        let mut router = format!(
            "relay_url = \"{relay_url}\"\napi_bind = \"127.0.0.1:{api_port}\"\ntailnet_bind = \"127.0.0.1:{tailnet_port}\"\npublic_url = \"\"\nroster_path = \"roster.toml\"\n"
        );
        for name in ["A", "B"] {
            router.push_str(&format!(
                "\n[[bots]]\nname = \"{name}\"\nkey = \"file:keys/{name}.key\"\nauth_tag = \"\"\nmax_concurrent = 1\n\n[bots.adapter]\n{DEFAULT_ADAPTER_TOML}"
            ));
        }
        std::fs::write(config.join("router.toml"), router).unwrap();
        write_key(&config.join("keys").join("B.key"), "B");
        Self {
            _dir: dir,
            config,
            data,
            api_port,
            tailnet_port,
        }
    }

    fn spawn(&self) -> Daemon {
        let mut child = Command::new(env!("CARGO_BIN_EXE_buzz-router"))
            .arg("run")
            .env("BUZZ_ROUTER_CONFIG_DIR", &self.config)
            .env("BUZZ_ROUTER_DATA_DIR", &self.data)
            .env("BUZZ_ROUTER_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stderr = Arc::new(Mutex::new(String::new()));
        let mut pipe = child.stderr.take().unwrap();
        let sink = stderr.clone();
        tokio::spawn(async move {
            let mut buffer = [0_u8; 4096];
            while let Ok(read) = pipe.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                sink.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buffer[..read]));
            }
        });
        Daemon { child, stderr }
    }

    fn api(&self) -> String {
        format!("http://127.0.0.1:{}", self.api_port)
    }

    fn tailnet(&self) -> String {
        format!("http://127.0.0.1:{}", self.tailnet_port)
    }
}

fn write_key(path: &Path, name: &str) {
    std::fs::write(path, keys(name).secret_key().to_secret_hex()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

struct Daemon {
    child: Child,
    stderr: Arc<Mutex<String>>,
}

impl Daemon {
    fn stderr(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    /// Waits for the process to exit and returns its exit code.
    async fn exit_code(&mut self) -> Option<i32> {
        tokio::time::timeout(Duration::from_secs(20), self.child.wait())
            .await
            .unwrap()
            .unwrap()
            .code()
    }
}

/// Polls `check` until it holds, failing after 20 s.
async fn eventually(what: &str, mut check: impl AsyncFnMut() -> bool) {
    for _ in 0..200 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(check().await, "never happened: {what}");
}

async fn listening(base: &str) -> bool {
    let address = base.trim_start_matches("http://").to_owned();
    tokio::net::TcpStream::connect(address).await.is_ok()
}

/// A running daemon over a valid config, with its API up.
async fn started() -> (Sandbox, Daemon, Arc<Mutex<BTreeSet<String>>>) {
    let (relay_url, authed) = mock_relay().await;
    let sandbox = Sandbox::new(&roster_toml(""), &relay_url);
    let daemon = sandbox.spawn();
    let api = sandbox.api();
    eventually("the loopback API listens", async || listening(&api).await).await;
    (sandbox, daemon, authed)
}

#[cfg(unix)]
fn signal(daemon: &Daemon, signal: nix::sys::signal::Signal) {
    let pid = i32::try_from(daemon.child.id().unwrap()).unwrap();
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal).unwrap();
}

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
        signal(&daemon, nix::sys::signal::Signal::SIGTERM);
        assert_eq!(daemon.exit_code().await, Some(0));
    }
    #[cfg(not(unix))]
    daemon.child.kill().await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn ctrl_c_exits_0() {
    let (_sandbox, mut daemon, _) = started().await;

    signal(&daemon, nix::sys::signal::Signal::SIGINT);

    assert_eq!(daemon.exit_code().await, Some(0));
}
