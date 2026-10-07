//! Running the `buzz-router` binary as a daemon against an in-process mock relay (tasks 3.10 and
//! 4.3). The mock answers every connection with an AUTH challenge, accepts the auth event and
//! every published event, and records the pubkeys that authenticated. Nothing connects to a real
//! relay.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nostr::{Event, JsonUtil};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::Message;

use super::{keys, roster_toml, DEFAULT_ADAPTER_TOML};

/// A port nothing listens on right now.
pub fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Starts the mock relay and returns its URL and the pubkeys that have authenticated.
pub async fn mock_relay() -> (String, Arc<Mutex<BTreeSet<String>>>) {
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

/// A config and data directory pair for the binary.
pub struct Sandbox {
    _dir: tempfile::TempDir,
    pub config: PathBuf,
    pub data: PathBuf,
    pub api_port: u16,
    pub tailnet_port: u16,
}

impl Sandbox {
    /// Writes `roster` and a `router.toml` serving bots A and B with file keys. Only B's key file
    /// exists.
    pub fn new(roster: &str, relay_url: &str) -> Self {
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

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_buzz-router"));
        command
            .args(args)
            .env("BUZZ_ROUTER_CONFIG_DIR", &self.config)
            .env("BUZZ_ROUTER_DATA_DIR", &self.data)
            .env("BUZZ_ROUTER_LOG", "info")
            .stdin(Stdio::null());
        command
    }

    /// Starts `buzz-router run`.
    pub fn spawn(&self) -> Daemon {
        let mut child = self
            .command(&["run"])
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

    /// Runs another command against the same directories and waits for it.
    pub async fn cli(&self, args: &[&str]) -> Output {
        self.command(args).output().await.unwrap()
    }

    pub fn api(&self) -> String {
        format!("http://127.0.0.1:{}", self.api_port)
    }

    pub fn tailnet(&self) -> String {
        format!("http://127.0.0.1:{}", self.tailnet_port)
    }

    /// The admin token the daemon created.
    pub fn admin_token(&self) -> String {
        std::fs::read_to_string(self.data.join("admin.token"))
            .unwrap()
            .trim()
            .to_owned()
    }
}

/// Writes `name`'s fixture secret to `path`, readable only by the user.
pub fn write_key(path: &Path, name: &str) {
    std::fs::write(path, keys(name).secret_key().to_secret_hex()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

/// A running `buzz-router run`.
pub struct Daemon {
    pub child: Child,
    stderr: Arc<Mutex<String>>,
}

impl Daemon {
    /// Everything the daemon has written to stderr so far.
    pub fn stderr(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    /// Waits for the process to exit and returns its exit code.
    pub async fn exit_code(&mut self) -> Option<i32> {
        tokio::time::timeout(Duration::from_secs(20), self.child.wait())
            .await
            .unwrap()
            .unwrap()
            .code()
    }

    /// Sends `signal` to the daemon.
    #[cfg(unix)]
    pub fn signal(&self, signal: nix::sys::signal::Signal) {
        let pid = i32::try_from(self.child.id().unwrap()).unwrap();
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal).unwrap();
    }
}

/// Polls `check` until it holds, failing after 20 s.
pub async fn eventually(what: &str, mut check: impl AsyncFnMut() -> bool) {
    for _ in 0..200 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(check().await, "never happened: {what}");
}

/// Whether something accepts connections at `base` (`http://host:port`).
pub async fn listening(base: &str) -> bool {
    let address = base.trim_start_matches("http://").to_owned();
    tokio::net::TcpStream::connect(address).await.is_ok()
}

/// A running daemon over a valid config, with its loopback API up.
pub async fn started() -> (Sandbox, Daemon, Arc<Mutex<BTreeSet<String>>>) {
    let (relay_url, authed) = mock_relay().await;
    let sandbox = Sandbox::new(&roster_toml(""), &relay_url);
    let daemon = sandbox.spawn();
    let api = sandbox.api();
    eventually("the loopback API listens", async || listening(&api).await).await;
    (sandbox, daemon, authed)
}
