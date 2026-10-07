//! Shared helpers for the buzz-router integration tests (tasks.md, "Shared test support").
//!
//! Each test binary declares `mod support;` and uses what it needs, so not every helper is used
//! by every binary. Every key is derived from a name, so no real key appears anywhere.

#![allow(
    dead_code,
    reason = "every test crate compiles this module on its own and uses only part of it"
)]
#![allow(
    clippy::unwrap_used,
    reason = "helpers in a shared test module fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use buzz_router::adapter::{Adapter, AdapterEvent, WakeContext};
use buzz_router::clock::{Clock, VirtualClock};
use buzz_router::core::{spawn_core, ApiRequest, ApiResponse, CoreDeps, CoreHandle};
use buzz_router::relay::{RelayError, RelayPort};
use buzz_router::store::Store;
use buzz_sdk::ThreadRef;
use chrono::{DateTime, TimeZone, Utc};
use futures_util::future::BoxFuture;
use nostr::{Event, EventBuilder, Filter, Keys, Kind, SecretKey, Tag, Timestamp};
use router_core::config::{parse_roster, parse_router};
use router_core::ids::EventId;
use router_core::ids::{BotName, ChannelId};
use router_core::payload::WakePayload;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Deterministic keys for `name`: the secret is `sha256("buzz-router-fixture:" + name)`.
pub fn keys(name: &str) -> Keys {
    let digest = Sha256::digest(format!("buzz-router-fixture:{name}").as_bytes());
    let secret = SecretKey::from_slice(&digest).unwrap();
    Keys::new(secret)
}

/// The hex pubkey of `keys(name)`.
pub fn pubkey_hex(name: &str) -> String {
    keys(name).public_key().to_hex()
}

/// A fixed channel for tests that need only one.
pub fn channel() -> Uuid {
    Uuid::parse_str("5d1c3a52-8f7e-4b0e-9a63-0c2f1e4d7b90").unwrap()
}

/// Signs a kind-9 message in `channel` at `created_at`. `thread` is `(root, parent)`: a direct
/// reply when they are equal, a nested reply otherwise. `mentions` become `p` tags.
pub fn message(
    author: &Keys,
    channel: Uuid,
    text: &str,
    thread: Option<(&Event, &Event)>,
    mentions: &[&Keys],
    created_at: u64,
) -> Event {
    let thread_ref = thread.map(|(root, parent)| ThreadRef {
        root_event_id: root.id,
        parent_event_id: parent.id,
    });
    let mentions: Vec<String> = mentions.iter().map(|k| k.public_key().to_hex()).collect();
    let mentions: Vec<&str> = mentions.iter().map(String::as_str).collect();
    buzz_sdk::builders::build_message(
        channel,
        text,
        thread_ref.as_ref(),
        &mentions,
        false,
        &[],
        &[],
    )
    .unwrap()
    .custom_created_at(Timestamp::from(created_at))
    .sign_with_keys(author)
    .unwrap()
}

/// Signs a top-level kind-9 message.
pub fn top_level(author: &Keys, channel: Uuid, text: &str, created_at: u64) -> Event {
    message(author, channel, text, None, &[], created_at)
}

/// Signs a kind-9 reply under `root` to `parent`.
pub fn reply(
    author: &Keys,
    channel: Uuid,
    text: &str,
    root: &Event,
    parent: &Event,
    created_at: u64,
) -> Event {
    message(author, channel, text, Some((root, parent)), &[], created_at)
}

/// Signs a kind-40003 edit of `target`, as `buzz_sdk::builders::build_edit` writes it.
pub fn edit(author: &Keys, channel: Uuid, target: &Event, text: &str, created_at: u64) -> Event {
    buzz_sdk::builders::build_edit(channel, target.id, text)
        .unwrap()
        .custom_created_at(Timestamp::from(created_at))
        .sign_with_keys(author)
        .unwrap()
}

/// Signs an event of any kind with the given raw tags.
pub fn raw_event(author: &Keys, kind: u16, text: &str, tags: &[&[&str]], created_at: u64) -> Event {
    let tags: Vec<Tag> = tags
        .iter()
        .map(|t| Tag::parse(t.to_vec()).unwrap())
        .collect();
    EventBuilder::new(Kind::Custom(kind), text)
        .tags(tags)
        .custom_created_at(Timestamp::from(created_at))
        .sign_with_keys(author)
        .unwrap()
}

/// The roster bots every test core serves. All are local and members of [`channel`].
pub const BOTS: [&str; 3] = ["A", "B", "C"];

/// The relay URL in the test `router.toml`. Nothing connects to it.
pub const RELAY_URL: &str = "ws://127.0.0.1:9";

/// The virtual wall-clock start of every test core: 2026-10-05T15:00:00Z, outside default quiet
/// hours.
pub fn base_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 5, 15, 0, 0).unwrap()
}

/// [`base_time`] in unix seconds, for event `created_at` values.
pub fn base_secs() -> u64 {
    base_time().timestamp() as u64
}

/// A roster with owner `owner`, bots A, B and C on every channel responding to anyone, and
/// `limits` (TOML lines) as the `[limits]` table.
pub fn roster_toml(limits: &str) -> String {
    roster_toml_with(limits, "")
}

/// [`roster_toml`] plus `extra` TOML appended, such as `[[channels]]` tables.
pub fn roster_toml_with(limits: &str, extra: &str) -> String {
    let mut roster = format!(
        "version = 1\n\n[owner]\nname = \"Owner\"\npubkeys = [\"{}\"]\ntimezone = \"UTC\"\n\n[limits]\n{limits}\n",
        pubkey_hex("owner")
    );
    for name in BOTS {
        roster.push_str(&format!(
            "\n[[bots]]\nname = \"{name}\"\npubkey = \"{}\"\nchannels = [\"*\"]\nrespond_to = \"anyone\"\n",
            pubkey_hex(name)
        ));
    }
    roster.push_str(extra);
    roster
}

/// The `[bots.adapter]` table every test bot gets unless a test supplies its own.
pub const DEFAULT_ADAPTER_TOML: &str = "type = \"command\"\ncommand = [\"agent\"]\ncwd = \"~/fixture\"\nenv = {}\nprompt_mode = \"stdin\"\nreply_mode = \"stdout\"\nprompt_template = \"\"\n";

/// A `router.toml` serving every bot in [`BOTS`] with `max_concurrent` each.
pub fn router_toml(max_concurrent: u32) -> String {
    router_toml_with(max_concurrent, DEFAULT_ADAPTER_TOML)
}

/// [`router_toml`] with `adapter` as every bot's `[bots.adapter]` table body.
pub fn router_toml_with(max_concurrent: u32, adapter: &str) -> String {
    let mut router = format!(
        "relay_url = \"{RELAY_URL}\"\napi_bind = \"127.0.0.1:47821\"\ntailnet_bind = \"\"\npublic_url = \"\"\nroster_path = \"roster.toml\"\n"
    );
    for name in BOTS {
        router.push_str(&format!(
            "\n[[bots]]\nname = \"{name}\"\nkey = \"keychain\"\nauth_tag = \"\"\nmax_concurrent = {max_concurrent}\n\n[bots.adapter]\n{adapter}"
        ));
    }
    router
}

/// The path of the built `buzz-router-test-agent` binary, building it once per test process
/// (design 16.3).
pub fn test_agent_path() -> std::path::PathBuf {
    static BUILT: OnceLock<std::path::PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
            let status = std::process::Command::new(cargo)
                .args(["build", "-p", "test-agent"])
                .status()
                .unwrap();
            assert!(status.success(), "cannot build the test agent");
            let exe = std::env::current_exe().unwrap();
            let profile_dir = exe.parent().and_then(std::path::Path::parent).unwrap();
            profile_dir.join(format!(
                "buzz-router-test-agent{}",
                std::env::consts::EXE_SUFFIX
            ))
        })
        .clone()
}

/// Writes to the database before a test core starts.
pub type Seed = Box<dyn FnOnce(&Store)>;

/// How to build a test core.
pub struct TestCoreOptions {
    /// The `[limits]` table of the roster.
    pub limits: String,
    /// Each bot's `max_concurrent`.
    pub max_concurrent: u32,
    /// Runs against the database before the core starts.
    pub seed: Option<Seed>,
    /// The adapter every bot uses.
    pub adapter: FakeAdapter,
    /// Extra roster TOML, such as `[[channels]]` tables.
    pub roster_extra: String,
    /// Every bot's `[bots.adapter]` table body.
    pub adapter_toml: String,
    /// An adapter to use instead of `adapter`, such as a real `CommandAdapter`.
    pub real_adapter: Option<Arc<dyn Adapter>>,
    /// The database file, instead of a fresh temporary one.
    pub db_path: Option<std::path::PathBuf>,
}

impl Default for TestCoreOptions {
    fn default() -> Self {
        Self {
            limits: String::new(),
            max_concurrent: 1,
            seed: None,
            adapter: FakeAdapter::new(vec![Step::Exit(0)]),
            roster_extra: String::new(),
            adapter_toml: DEFAULT_ADAPTER_TOML.to_owned(),
            real_adapter: None,
            db_path: None,
        }
    }
}

/// A core with the default options.
pub fn spawn_test_core() -> (CoreHandle, FakeRelay, Store) {
    spawn_test_core_with(TestCoreOptions::default())
}

/// A core over a temporary file database, serving bots A, B and C through one [`FakeRelay`], on a
/// [`VirtualClock`] starting at [`base_time`]. Call it inside `#[tokio::test(start_paused = true)]`.
/// The returned store is a separate connection to the same database, for seeding and assertions.
pub fn spawn_test_core_with(options: TestCoreOptions) -> (CoreHandle, FakeRelay, Store) {
    let path = options
        .db_path
        .unwrap_or_else(|| tempfile::tempdir().unwrap().keep().join("state.sqlite3"));
    let store = Store::open(&path).unwrap();
    if let Some(seed) = options.seed {
        seed(&store);
    }
    let roster = parse_roster(&roster_toml_with(&options.limits, &options.roster_extra)).unwrap();
    let config = parse_router(
        &router_toml_with(options.max_concurrent, &options.adapter_toml),
        &roster,
    )
    .unwrap();
    let clock: Arc<dyn Clock> = Arc::new(VirtualClock::new(base_time()));
    let fake = options.adapter.with_clock(clock.clone());
    let core_slot = fake.core.clone();
    let adapter: Arc<dyn Adapter> = options.real_adapter.unwrap_or_else(|| Arc::new(fake));
    let relay = FakeRelay::new();
    let mut relays: BTreeMap<BotName, Arc<dyn RelayPort>> = BTreeMap::new();
    let mut keys_by_bot = BTreeMap::new();
    let mut memberships = BTreeMap::new();
    let mut adapters = BTreeMap::new();
    for name in BOTS {
        let bot = BotName::new(name).unwrap();
        relays.insert(bot.clone(), Arc::new(relay.clone()));
        keys_by_bot.insert(bot.clone(), keys(name));
        adapters.insert(bot.clone(), adapter.clone());
        memberships.insert(bot, BTreeSet::from([ChannelId::from(channel())]));
    }
    let handle = spawn_core(CoreDeps {
        store: Store::open(&path).unwrap(),
        ingest_store: Store::open_read_only(&path).unwrap(),
        roster,
        config,
        clock,
        relays,
        keys: keys_by_bot,
        memberships,
        adapters,
    });
    let _ = core_slot.set(handle.clone());
    (handle, relay, store)
}

/// One step of a [`FakeAdapter`] script (design 16.2).
#[derive(Debug, Clone)]
pub enum Step {
    /// Sleep for the duration.
    Wait(Duration),
    /// End with exit code `code` and no stdout.
    Exit(i32),
    /// End with exit code 0 and `text` on stdout.
    Stdout(String),
    /// End with exit code `code` and `text` on stdout.
    ExitWith(i32, String),
    /// Run until cancelled.
    Hang,
    /// Post `text` through the core's API with the wake token.
    Post(String),
    /// Pass through the core's API.
    Pass,
    /// Set an ETA through the core's API.
    Eta(String),
}

/// One call of [`FakeAdapter::run`].
#[derive(Debug, Clone)]
pub struct Dispatch {
    /// The wake.
    pub wake_id: Uuid,
    /// The bot woken.
    pub bot: BotName,
    /// The thread root.
    pub root: EventId,
    /// The trigger event ids.
    pub triggers: Vec<EventId>,
    /// The virtual wall-clock time of the call.
    pub at: DateTime<Utc>,
    /// The wake token the adapter received.
    pub token: String,
    /// The payload the adapter received.
    pub payload: WakePayload,
}

#[derive(Default)]
struct FakeAdapterState {
    scripts: VecDeque<Vec<Step>>,
    dispatches: Vec<Dispatch>,
    cancelled: Vec<Uuid>,
    api_responses: Vec<(Uuid, ApiResponse)>,
}

/// An [`Adapter`] that runs a script per wake and records every dispatch. Clones share state.
#[derive(Clone)]
pub struct FakeAdapter {
    default_script: Vec<Step>,
    clock: Arc<dyn Clock>,
    state: Arc<Mutex<FakeAdapterState>>,
    core: Arc<OnceLock<CoreHandle>>,
}

impl FakeAdapter {
    /// Runs `script` for every wake.
    pub fn new(script: Vec<Step>) -> Self {
        Self {
            default_script: script,
            clock: Arc::new(VirtualClock::new(base_time())),
            state: Arc::default(),
            core: Arc::default(),
        }
    }

    /// The wakes whose run was cancelled, in order.
    pub fn cancelled(&self) -> Vec<Uuid> {
        self.state.lock().unwrap().cancelled.clone()
    }

    /// Every API answer a script step received, with its wake.
    pub fn api_responses(&self) -> Vec<(Uuid, ApiResponse)> {
        self.state.lock().unwrap().api_responses.clone()
    }

    /// Runs the next of `scripts` for each wake, then the default script.
    pub fn push_script(&self, script: Vec<Step>) {
        self.state.lock().unwrap().scripts.push_back(script);
    }

    /// Every dispatch so far, in order.
    pub fn dispatches(&self) -> Vec<Dispatch> {
        self.state.lock().unwrap().dispatches.clone()
    }

    fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }
}

impl Adapter for FakeAdapter {
    fn run(
        &self,
        ctx: WakeContext,
        payload: WakePayload,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, AdapterEvent> {
        let script = {
            let mut state = self.state.lock().unwrap();
            state.dispatches.push(Dispatch {
                wake_id: ctx.wake_id,
                bot: ctx.bot.clone(),
                root: ctx.root.clone(),
                triggers: ctx.trigger_ids.clone(),
                at: self.clock.now(),
                token: ctx.token.clone(),
                payload,
            });
            state
                .scripts
                .pop_front()
                .unwrap_or_else(|| self.default_script.clone())
        };
        let state = Arc::clone(&self.state);
        let core = Arc::clone(&self.core);
        let wake_id = ctx.wake_id;
        let token = ctx.token;
        Box::pin(async move {
            let cancelled = || {
                state.lock().unwrap().cancelled.push(wake_id);
                AdapterEvent::Failed("cancelled".into())
            };
            for step in script {
                let request = match step {
                    Step::Wait(duration) => {
                        tokio::select! {
                            () = tokio::time::sleep(duration) => {}
                            () = cancel.cancelled() => return cancelled(),
                        }
                        continue;
                    }
                    Step::Exit(code) => {
                        return AdapterEvent::Exited {
                            code: Some(code),
                            stdout: None,
                        }
                    }
                    Step::Stdout(text) => {
                        return AdapterEvent::Exited {
                            code: Some(0),
                            stdout: Some(text),
                        }
                    }
                    Step::ExitWith(code, text) => {
                        return AdapterEvent::Exited {
                            code: Some(code),
                            stdout: Some(text),
                        }
                    }
                    Step::Hang => {
                        cancel.cancelled().await;
                        return cancelled();
                    }
                    Step::Post(text) => ApiRequest::Post {
                        token: token.clone(),
                        text,
                    },
                    Step::Pass => ApiRequest::Pass {
                        token: token.clone(),
                    },
                    Step::Eta(text) => ApiRequest::Eta {
                        token: token.clone(),
                        text,
                    },
                };
                let handle = core.get().cloned().unwrap();
                let response = handle.api(request).await;
                state
                    .lock()
                    .unwrap()
                    .api_responses
                    .push((wake_id, response));
            }
            AdapterEvent::Exited {
                code: Some(0),
                stdout: None,
            }
        })
    }
}

/// A hook called with each event as `FakeRelay` receives a publish, before it is recorded.
pub type PublishHook = Box<dyn Fn(&Event) + Send + Sync>;

#[derive(Default)]
struct FakeRelayState {
    seeded: Vec<Event>,
    published: Vec<Event>,
    queries: Vec<Vec<Filter>>,
    fail_publish: bool,
    fail_query: bool,
    hook: Option<PublishHook>,
}

/// An in-memory [`RelayPort`]: records publishes and queries, and serves queries from seeded
/// events. Clones share state.
#[derive(Clone, Default)]
pub struct FakeRelay {
    state: Arc<Mutex<FakeRelayState>>,
}

impl FakeRelay {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, FakeRelayState> {
        self.state.lock().unwrap()
    }

    /// Makes `event` available to `query`.
    pub fn seed(&self, event: Event) {
        self.lock().seeded.push(event);
    }

    /// Every event published so far, in order.
    pub fn published(&self) -> Vec<Event> {
        self.lock().published.clone()
    }

    /// Every query's filters so far, in order.
    pub fn queries(&self) -> Vec<Vec<Filter>> {
        self.lock().queries.clone()
    }

    /// Makes every later publish fail.
    pub fn fail_publishes(&self, fail: bool) {
        self.lock().fail_publish = fail;
    }

    /// Makes every later query fail.
    pub fn fail_queries(&self, fail: bool) {
        self.lock().fail_query = fail;
    }

    /// Calls `hook` with each published event before it is recorded.
    pub fn on_publish(&self, hook: PublishHook) {
        self.lock().hook = Some(hook);
    }

    /// The targets of every published kind-7 reaction with content `emoji`, in order.
    pub fn reactions(&self, emoji: &str) -> Vec<nostr::EventId> {
        self.published()
            .iter()
            .filter(|event| event.kind == Kind::Reaction && event.content == emoji)
            .filter_map(|event| event.tags.event_ids().next().copied())
            .collect()
    }

    /// Every published kind-20002 typing indicator.
    pub fn typing(&self) -> Vec<Event> {
        self.published()
            .into_iter()
            .filter(|event| event.kind == Kind::Custom(20002))
            .collect()
    }

    /// Every published kind-9 event whose `buzz-router` tag has `marker` (`reply` or `status`).
    pub fn messages(&self, marker: &str) -> Vec<Event> {
        self.published()
            .into_iter()
            .filter(|event| {
                event.kind == Kind::Custom(9) && router_marker(event) == Some(marker.to_owned())
            })
            .collect()
    }
}

/// The third element of an event's `buzz-router` tag, if it has one.
pub fn router_marker(event: &Event) -> Option<String> {
    event.tags.iter().find_map(|tag| {
        let parts = tag.as_slice();
        (parts.first().map(String::as_str) == Some("buzz-router"))
            .then(|| parts.get(2).cloned())
            .flatten()
    })
}

/// The reaction emojis the router uses.
pub mod emoji {
    pub const EYES: &str = "\u{1F440}";
    pub const CHECK: &str = "\u{2705}";
    pub const HOURGLASS: &str = "\u{231B}";
    pub const WARNING: &str = "\u{26A0}\u{FE0F}";
    pub const STOP: &str = "\u{1F6D1}";
}

impl RelayPort for FakeRelay {
    fn publish(
        &self,
        event: Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), RelayError>> + Send + '_>> {
        Box::pin(async move {
            let mut state = self.lock();
            if let Some(hook) = &state.hook {
                hook(&event);
            }
            if state.fail_publish {
                return Err(RelayError::Transport("fake publish failure".into()));
            }
            state.published.push(event);
            Ok(())
        })
    }

    fn query(
        &self,
        filters: Vec<Filter>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Event>, RelayError>> + Send + '_>> {
        Box::pin(async move {
            let mut state = self.lock();
            state.queries.push(filters.clone());
            if state.fail_query {
                return Err(RelayError::Status(503));
            }
            let mut found: Vec<Event> = state
                .seeded
                .iter()
                .filter(|event| {
                    filters
                        .iter()
                        .any(|f| f.match_event(event, nostr::filter::MatchEventOptions::new()))
                })
                .cloned()
                .collect();
            found.sort_by_key(|e| (e.created_at.as_secs(), e.id.to_hex()));
            found.dedup_by_key(|e| e.id);
            Ok(found)
        })
    }
}
