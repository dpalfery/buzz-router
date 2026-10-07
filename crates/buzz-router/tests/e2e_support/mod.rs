//! Throwaway harness for the relay integration and end-to-end tests (tasks
//! 2.9 and 7.1, design §16.4).
//!
//! Every identity is generated fresh per test run and every channel UUID is
//! random, so repeated runs against one relay never collide. Nothing here
//! touches a live relay or a real key: [`relay_url`] only accepts a local
//! `ws://127.0.0.1:` (or `ws://localhost:`) URL, as printed by
//! `scripts/e2e-relay.sh up`.
//!
//! [`E2e`] runs `buzz-router run` as a child process serving bots A, B and C
//! in one freshly provisioned channel owned by O.

#![allow(
    dead_code,
    reason = "every e2e test crate compiles this module on its own and uses only part of it"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers in a test support module fail the test by panicking"
)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use buzz_router::relay::rest::RestClient;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use uuid::Uuid;

/// Whether the relay integration tests run. Everything is skipped unless
/// `BUZZ_E2E=1`.
pub fn e2e_enabled() -> bool {
    std::env::var("BUZZ_E2E").as_deref() == Ok("1")
}

/// The relay URL under test, from `BUZZ_E2E_RELAY_URL`. Refuses anything that
/// is not a local relay.
pub fn relay_url() -> String {
    let url = std::env::var("BUZZ_E2E_RELAY_URL")
        .expect("BUZZ_E2E_RELAY_URL must be set (scripts/e2e-relay.sh up prints it)");
    assert!(
        url.starts_with("ws://127.0.0.1:") || url.starts_with("ws://localhost:"),
        "the e2e relay must be local, got {url}"
    );
    url
}

/// How long relay operations may take before a test fails instead of hanging.
pub const TIMEOUT: Duration = Duration::from_secs(20);

/// Four throwaway identities: one owner and three bots, all generated fresh.
pub struct Identities {
    /// The channel owner, which provisions channels and posts owner messages.
    pub owner: nostr::Keys,
    /// The three local bots.
    pub bots: Vec<nostr::Keys>,
}

/// Generates one owner and three bot identities.
pub fn fresh_identities() -> Identities {
    Identities {
        owner: nostr::Keys::generate(),
        bots: vec![
            nostr::Keys::generate(),
            nostr::Keys::generate(),
            nostr::Keys::generate(),
        ],
    }
}

/// A REST client signing as `keys` against the e2e relay.
pub fn rest_for(keys: &nostr::Keys, url: &str) -> RestClient {
    RestClient::new(url, keys.clone(), None)
}

/// Creates a private channel called `name` owned by `owner` and adds every
/// key in `members`, submitting `build_create_channel` and `build_add_member`
/// over REST. Returns the fresh channel UUID.
pub async fn provision_channel(
    owner_rest: &RestClient,
    owner: &nostr::Keys,
    members: &[nostr::Keys],
    name: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    let create = buzz_sdk::builders::build_create_channel(
        id,
        name,
        Some(buzz_sdk::Visibility::Private),
        None,
        None,
        None,
    )
    .expect("valid channel create");
    let create = create.sign_with_keys(owner).expect("channel create signs");
    owner_rest
        .submit_event(&create)
        .await
        .expect("the relay accepts the channel create");
    for member in members {
        let add = buzz_sdk::builders::build_add_member(
            id,
            &member.public_key().to_hex(),
            Some(buzz_sdk::MemberRole::Member),
        )
        .expect("valid add-member");
        let add = add.sign_with_keys(owner).expect("add-member signs");
        owner_rest
            .submit_event(&add)
            .await
            .expect("the relay accepts the add-member");
    }
    id
}

/// Signs a top-level kind-9 channel message.
pub fn channel_message(author: &nostr::Keys, channel: Uuid, text: &str) -> nostr::Event {
    buzz_sdk::builders::build_message(channel, text, None, &[], false, &[], &[])
        .expect("valid channel message")
        .sign_with_keys(author)
        .expect("channel message signs")
}

/// Signs a kind-9 reply to `parent` under `root`.
pub fn reply_to(
    author: &nostr::Keys,
    channel: Uuid,
    text: &str,
    root: &nostr::Event,
    parent: &nostr::Event,
) -> nostr::Event {
    let thread = buzz_sdk::ThreadRef {
        root_event_id: root.id,
        parent_event_id: parent.id,
    };
    buzz_sdk::builders::build_message(channel, text, Some(&thread), &[], false, &[], &[])
        .expect("valid reply")
        .sign_with_keys(author)
        .expect("reply signs")
}

/// The roster names of the three bots, in [`Identities::bots`] order.
pub const BOTS: [&str; 3] = ["A", "B", "C"];
/// The reaction a bot gives when its wake starts.
pub const EYES: &str = "\u{1F440}";
/// The reaction a bot gives to an acknowledged stop.
pub const STOP: &str = "\u{1F6D1}";
/// The reaction a bot gives to an acknowledged resume.
pub const RESUME: &str = "\u{25B6}\u{FE0F}";
/// The reaction for a wake that ran past its deadline.
pub const HOURGLASS: &str = "\u{231B}";
/// The reaction on an unmanaged post.
pub const WARNING: &str = "\u{26A0}\u{FE0F}";

/// How long the polling helpers wait by default.
pub const WAIT: Duration = Duration::from_secs(30);

/// The marker tag value of a router reply (design §6.8).
const REPLY_MARKER: &str = "reply";
/// The marker tag value of a router status note.
const STATUS_MARKER: &str = "status";

/// Fails the test instead of hanging when `fut` takes longer than `limit`.
pub async fn within<T>(what: &str, limit: Duration, fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(limit, fut)
        .await
        .unwrap_or_else(|_| panic!("timed out after {limit:?} waiting for {what}"))
}

/// The router child process, its sandbox and the provisioned channel.
pub struct E2e {
    _dir: tempfile::TempDir,
    /// The router's config directory (`roster.toml`, `router.toml`, `keys/`).
    pub config: PathBuf,
    /// The router's data directory (`state.sqlite3`, `admin.token`).
    pub data: PathBuf,
    /// The relay URL under test.
    pub url: String,
    /// O and the bots A, B and C.
    pub ids: Identities,
    /// The channel O provisioned with every bot as a member.
    pub channel: Uuid,
    /// A REST client signing as O.
    pub owner_rest: RestClient,
    api_port: u16,
    router: Option<RouterChild>,
}

/// A running `buzz-router run` and everything it has written to stderr.
struct RouterChild {
    child: Child,
    stderr: Arc<Mutex<String>>,
}

impl E2e {
    /// Provisions a channel called `name` and starts the router, with every
    /// bot running `buzz-router-test-agent echo --delay <delay_secs>`.
    /// Returns `None` when the e2e tests are skipped.
    pub async fn start(name: &str, delay_secs: u64) -> Option<Self> {
        Self::start_with(name, delay_secs, "").await
    }

    /// [`E2e::start`] with `limits` added to the roster's `[limits]` table.
    pub async fn start_with(name: &str, delay_secs: u64, limits: &str) -> Option<Self> {
        if !e2e_enabled() {
            return None;
        }
        let url = relay_url();
        let ids = fresh_identities();
        let owner_rest = rest_for(&ids.owner, &url);
        let channel = within(
            "channel provisioning",
            TIMEOUT,
            provision_channel(&owner_rest, &ids.owner, &ids.bots, name),
        )
        .await;
        let dir = tempfile::tempdir().expect("temp dir");
        let config = dir.path().join("config");
        let data = dir.path().join("data");
        let work = dir.path().join("work");
        std::fs::create_dir_all(config.join("keys")).expect("keys dir");
        std::fs::create_dir_all(&work).expect("work dir");
        std::fs::write(config.join("roster.toml"), roster_toml(&ids, limits)).expect("roster");
        let api_port = free_port();
        let agent = super::support::test_agent_path();
        std::fs::write(
            config.join("router.toml"),
            router_toml(&url, api_port, &agent, &work, delay_secs),
        )
        .expect("router.toml");
        for (name, keys) in BOTS.iter().zip(&ids.bots) {
            write_key_file(&config.join("keys").join(format!("{name}.key")), keys);
        }
        let mut e2e = Self {
            _dir: dir,
            config,
            data,
            url,
            ids,
            channel,
            owner_rest,
            api_port,
            router: None,
        };
        e2e.spawn();
        Some(e2e)
    }

    /// The keys of bot `name` (`"A"`, `"B"` or `"C"`).
    pub fn bot(&self, name: &str) -> &nostr::Keys {
        let at = BOTS
            .iter()
            .position(|bot| *bot == name)
            .unwrap_or_else(|| panic!("no bot {name}"));
        &self.ids.bots[at]
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

    /// Starts `buzz-router run`. Panics if it is already running.
    pub fn spawn(&mut self) {
        assert!(self.router.is_none(), "the router is already running");
        let mut child = self
            .command(&["run"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("buzz-router run starts");
        let stderr = Arc::new(Mutex::new(String::new()));
        let mut pipe = child.stderr.take().expect("piped stderr");
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
        self.router = Some(RouterChild { child, stderr });
    }

    /// Stops the router gracefully (SIGTERM on Unix, a kill elsewhere) and
    /// waits for it to exit.
    pub async fn stop(&mut self) {
        let Some(mut router) = self.router.take() else {
            return;
        };
        #[cfg(unix)]
        {
            if let Some(pid) = router.child.id() {
                let pid = nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits"));
                let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        let _ = router.child.start_kill();
        within("the router to exit", TIMEOUT, router.child.wait())
            .await
            .expect("the router exits");
    }

    /// Kills the router outright (`Child::kill`, SIGKILL on Unix), as a crash.
    pub async fn kill(&mut self) {
        let Some(mut router) = self.router.take() else {
            return;
        };
        router.child.kill().await.expect("the router is killed");
    }

    /// Restarts a stopped or killed router over the same directories.
    pub async fn restart(&mut self) {
        self.stop().await;
        self.spawn();
    }

    /// Everything the current router process has written to stderr.
    pub fn stderr(&self) -> String {
        self.router
            .as_ref()
            .map(|router| router.stderr.lock().unwrap().clone())
            .unwrap_or_default()
    }

    /// Runs another `buzz-router` command against the same directories.
    pub async fn cli(&self, args: &[&str]) -> std::process::Output {
        within(
            "a buzz-router command",
            TIMEOUT,
            self.command(args).output(),
        )
        .await
        .expect("buzz-router runs")
    }

    /// `buzz-router status --json`, or `None` while the daemon is unreachable.
    pub async fn try_status(&self) -> Option<serde_json::Value> {
        let output = self.cli(&["status", "--json"]).await;
        if !output.status.success() {
            return None;
        }
        serde_json::from_slice(&output.stdout).ok()
    }

    /// `buzz-router status --json`, waiting for the daemon to answer.
    pub async fn status(&self) -> serde_json::Value {
        self.wait_for("the status document", WAIT, async || {
            self.try_status().await
        })
        .await
    }

    /// Waits until `status` shows every bot connected.
    pub async fn wait_all_connected(&self) {
        self.wait_for("every bot to connect", WAIT, async || {
            let status = self.try_status().await?;
            let bots = status["bots"].as_array()?;
            (bots.len() == BOTS.len() && bots.iter().all(|bot| bot["connected"] == true))
                .then_some(())
        })
        .await;
    }

    /// Polls `check` every 250 ms until it returns `Some`, failing the test
    /// with the router's stderr after `limit`.
    pub async fn wait_for<T>(
        &self,
        what: &str,
        limit: Duration,
        mut check: impl AsyncFnMut() -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(found) = check().await {
                return found;
            }
            if Instant::now() >= deadline {
                panic!(
                    "timed out after {limit:?} waiting for {what}\n--- router stderr ---\n{}",
                    self.stderr()
                );
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Signs `text` as O, publishes it at the top level and returns it.
    pub async fn owner_post(&self, text: &str) -> nostr::Event {
        let event = channel_message(&self.ids.owner, self.channel, text);
        self.submit(&event).await;
        event
    }

    /// Signs `text` as O, publishes it as a reply to `parent` under `root`
    /// and returns it.
    pub async fn owner_reply(
        &self,
        text: &str,
        root: &nostr::Event,
        parent: &nostr::Event,
    ) -> nostr::Event {
        let event = reply_to(&self.ids.owner, self.channel, text, root, parent);
        self.submit(&event).await;
        event
    }

    /// Signs `text` with bot `name`'s own key and publishes it directly,
    /// bypassing the router.
    pub async fn bot_post_direct(&self, name: &str, text: &str) -> nostr::Event {
        let event = channel_message(self.bot(name), self.channel, text);
        within(
            "the direct bot publish",
            TIMEOUT,
            rest_for(self.bot(name), &self.url).submit_event(&event),
        )
        .await
        .expect("the relay accepts the bot's direct post");
        event
    }

    async fn submit(&self, event: &nostr::Event) {
        within(
            "the owner publish",
            TIMEOUT,
            self.owner_rest.submit_event(event),
        )
        .await
        .expect("the relay accepts the owner message");
    }

    /// Every kind-7 reaction on `target`.
    pub async fn reactions(&self, target: &nostr::Event) -> Vec<nostr::Event> {
        self.query(
            nostr::Filter::new()
                .kind(nostr::Kind::Reaction)
                .event(target.id),
        )
        .await
    }

    /// Waits until bot `name` has reacted to `target` with `emoji`.
    pub async fn wait_reaction(&self, name: &str, target: &nostr::Event, emoji: &str) {
        let author = self.bot(name).public_key();
        self.wait_for(&format!("{emoji} from {name}"), WAIT, async || {
            self.reactions(target)
                .await
                .iter()
                .any(|reaction| reaction.pubkey == author && reaction.content == emoji)
                .then_some(())
        })
        .await;
    }

    /// Every kind-9 message in the thread rooted at `root`, oldest first.
    pub async fn thread(&self, root: &nostr::Event) -> Vec<nostr::Event> {
        self.query(
            nostr::Filter::new()
                .kind(nostr::Kind::Custom(9))
                .event(root.id),
        )
        .await
    }

    /// Bot `name`'s router replies (not status notes) in `root`'s thread.
    pub async fn replies(&self, name: &str, root: &nostr::Event) -> Vec<nostr::Event> {
        self.marked(name, root, REPLY_MARKER).await
    }

    /// Bot `name`'s status notes in `root`'s thread.
    pub async fn status_notes(&self, name: &str, root: &nostr::Event) -> Vec<nostr::Event> {
        self.marked(name, root, STATUS_MARKER).await
    }

    async fn marked(&self, name: &str, root: &nostr::Event, marker: &str) -> Vec<nostr::Event> {
        let author = self.bot(name).public_key();
        self.thread(root)
            .await
            .into_iter()
            .filter(|event| event.pubkey == author && router_marker(event) == Some(marker))
            .collect()
    }

    /// Waits until bot `name` has at least `count` replies in `root`'s
    /// thread, and returns them.
    pub async fn wait_replies(
        &self,
        name: &str,
        root: &nostr::Event,
        count: usize,
    ) -> Vec<nostr::Event> {
        self.wait_replies_within(name, root, count, WAIT).await
    }

    /// [`E2e::wait_replies`] with an explicit time limit.
    pub async fn wait_replies_within(
        &self,
        name: &str,
        root: &nostr::Event,
        count: usize,
        limit: Duration,
    ) -> Vec<nostr::Event> {
        self.wait_for(&format!("{count} replies from {name}"), limit, async || {
            let replies = self.replies(name, root).await;
            (replies.len() >= count).then_some(replies)
        })
        .await
    }

    /// Every event matching `filter` that O can read.
    pub async fn query(&self, filter: nostr::Filter) -> Vec<nostr::Event> {
        within("a REST query", TIMEOUT, self.owner_rest.query(vec![filter]))
            .await
            .expect("the REST query succeeds")
    }

    /// The router's loopback API base URL.
    pub fn api(&self) -> String {
        format!("http://127.0.0.1:{}", self.api_port)
    }
}

impl Drop for E2e {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("--- router stderr ---\n{}", self.stderr());
        }
    }
}

/// The `buzz-router` marker value (`reply` or `status`) of a router post.
pub fn router_marker(event: &nostr::Event) -> Option<&str> {
    event.tags.iter().find_map(|tag| {
        let parts = tag.as_slice();
        (parts.first().map(String::as_str) == Some("buzz-router"))
            .then(|| parts.get(2).map(String::as_str))
            .flatten()
    })
}

/// A port nothing listens on right now.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
    listener.local_addr().expect("local addr").port()
}

/// A TOML literal string: paths with backslashes stay verbatim on Windows.
fn literal(path: &Path) -> String {
    let text = path.display().to_string();
    assert!(!text.contains('\''), "path {text} cannot be a TOML literal");
    format!("'{text}'")
}

/// `roster.toml` for owner O and bots A, B and C in every channel, with
/// quiet hours off so the tests behave the same at any time of day.
fn roster_toml(ids: &Identities, limits: &str) -> String {
    let mut roster = format!(
        "version = 1\n\n[owner]\nname = \"Owner\"\npubkeys = [\"{}\"]\ntimezone = \"UTC\"\n\n[limits]\nquiet_hours = \"\"\n{limits}\n",
        ids.owner.public_key().to_hex()
    );
    for (name, keys) in BOTS.iter().zip(&ids.bots) {
        roster.push_str(&format!(
            "\n[[bots]]\nname = \"{name}\"\npubkey = \"{}\"\nchannels = [\"*\"]\nrespond_to = \"anyone\"\n",
            keys.public_key().to_hex()
        ));
    }
    roster
}

/// `router.toml` serving A, B and C with `file:` keys and command adapters
/// running `buzz-router-test-agent echo --delay <delay_secs>`.
fn router_toml(url: &str, api_port: u16, agent: &Path, work: &Path, delay_secs: u64) -> String {
    let mut router = format!(
        "relay_url = \"{url}\"\napi_bind = \"127.0.0.1:{api_port}\"\ntailnet_bind = \"\"\npublic_url = \"\"\nroster_path = \"roster.toml\"\n"
    );
    for name in BOTS {
        router.push_str(&format!(
            "\n[[bots]]\nname = \"{name}\"\nkey = \"file:keys/{name}.key\"\nauth_tag = \"\"\nmax_concurrent = 1\n\n[bots.adapter]\ntype = \"command\"\ncommand = [{}, \"echo\", \"--delay\", \"{delay_secs}\"]\ncwd = {}\nprompt_mode = \"stdin\"\nreply_mode = \"stdout\"\n",
            literal(agent),
            literal(work),
        ));
    }
    router
}

/// Writes `keys`' secret to `path`, readable only by the user.
fn write_key_file(path: &Path, keys: &nostr::Keys) {
    std::fs::write(path, keys.secret_key().to_secret_hex()).expect("write key file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("key file mode 0600");
    }
}
