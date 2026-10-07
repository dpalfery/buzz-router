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

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use buzz_router::relay::{RelayError, RelayPort};
use buzz_sdk::ThreadRef;
use nostr::{Event, EventBuilder, Filter, Keys, Kind, SecretKey, Tag, Timestamp};
use sha2::{Digest, Sha256};
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
