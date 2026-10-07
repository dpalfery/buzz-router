//! Task 4.1: recovery of interrupted wakes at startup (design 6.2 steps 5 and 6, R36.7, R49,
//! R65.8). Each test runs a core, shuts it down with a wake still running, and starts a second
//! core on the same database file.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use buzz_router::core::CoreHandle;
use buzz_router::ingest::Source;
use buzz_router::store::halts::HaltScope;
use buzz_router::store::wakes::{WakeRow, WakeState};
use buzz_router::store::Store;
use nostr::Event;
use router_core::ids::BotName;
use support::{
    base_secs, channel, emoji, keys, spawn_test_core_with, top_level, FakeAdapter, FakeRelay, Step,
    TestCoreOptions,
};

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

struct Run {
    core: CoreHandle,
    relay: FakeRelay,
    store: Store,
    adapter: FakeAdapter,
}

/// A core over `db_path` whose bots run `script`.
async fn start(db_path: &Path, script: Vec<Step>) -> Run {
    let adapter = FakeAdapter::new(script);
    let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        db_path: Some(db_path.to_path_buf()),
        ..TestCoreOptions::default()
    });
    core.flush().await;
    Run {
        core,
        relay,
        store,
        adapter,
    }
}

impl Run {
    async fn ingest(&self, author: &str, text: &str) -> Event {
        let event = top_level(&keys(author), channel(), text, base_secs());
        self.core.ingest(bot("A"), event.clone(), Source::Live);
        self.core.flush().await;
        event
    }

    fn wakes(&self) -> Vec<WakeRow> {
        let mut wakes = Vec::new();
        for state in [
            WakeState::Queued,
            WakeState::Running,
            WakeState::Interrupted,
            WakeState::Killed,
        ] {
            wakes.extend(self.store.wakes().with_state(state).unwrap());
        }
        wakes
    }

    async fn settle(&self) {
        self.core.flush().await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        self.core.flush().await;
    }

    fn stop(self) {
        self.core.shutdown();
    }
}

fn db_path() -> PathBuf {
    tempfile::tempdir().unwrap().keep().join("state.sqlite3")
}

#[tokio::test(start_paused = true)]
async fn an_interrupted_owner_wake_with_no_post_is_requeued_once_as_attempt_2() {
    let path = db_path();
    let first = start(&path, vec![Step::Hang]).await;
    first.ingest("owner", "@A hi").await;
    let original = first.adapter.dispatches()[0].wake_id;
    first.stop();

    let second = start(&path, vec![Step::Hang]).await;
    second.settle().await;

    let wake = second.store.wakes().get(&original).unwrap().unwrap();
    assert_eq!(wake.state, WakeState::Interrupted);
    assert!(wake.ended_at.is_some());
    let dispatches = second.adapter.dispatches();
    assert_eq!(dispatches.len(), 1, "dispatched exactly once");
    let retry = second
        .store
        .wakes()
        .get(&dispatches[0].wake_id)
        .unwrap()
        .unwrap();
    assert_ne!(retry.id, original);
    assert_eq!(retry.attempt, 2);
    assert_eq!(retry.state, WakeState::Running);
    assert_eq!(retry.root_id, wake.root_id);
    assert_eq!(retry.triggers, wake.triggers);
    assert!(second.relay.reactions(emoji::WARNING).is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_interrupted_wake_that_posted_gets_a_warning_and_no_retry() {
    let path = db_path();
    let first = start(&path, vec![Step::Post("partial".into()), Step::Hang]).await;
    let trigger = first.ingest("owner", "@A hi").await;
    let original = first.adapter.dispatches()[0].wake_id;
    assert_eq!(first.relay.messages("reply").len(), 1);
    first.stop();

    let second = start(&path, vec![Step::Hang]).await;
    second.settle().await;

    let wake = second.store.wakes().get(&original).unwrap().unwrap();
    assert_eq!(wake.state, WakeState::Interrupted);
    assert!(second.adapter.dispatches().is_empty());
    assert_eq!(second.relay.reactions(emoji::WARNING), vec![trigger.id]);
}

#[tokio::test(start_paused = true)]
async fn an_interrupted_second_attempt_gets_a_warning_and_no_retry() {
    let path = db_path();
    let first = start(&path, vec![Step::Hang]).await;
    let trigger = first.ingest("owner", "@A hi").await;
    first.stop();
    let second = start(&path, vec![Step::Hang]).await;
    second.settle().await;
    let retry = second.adapter.dispatches()[0].wake_id;
    second.stop();

    let third = start(&path, vec![Step::Hang]).await;
    third.settle().await;

    let wake = third.store.wakes().get(&retry).unwrap().unwrap();
    assert_eq!(wake.state, WakeState::Interrupted);
    assert!(third.adapter.dispatches().is_empty());
    assert_eq!(third.relay.reactions(emoji::WARNING), vec![trigger.id]);
    assert!(third
        .wakes()
        .iter()
        .all(|wake| wake.state == WakeState::Interrupted));
}

#[tokio::test(start_paused = true)]
async fn an_interrupted_wake_of_a_halted_bot_gets_a_warning_and_no_retry() {
    let path = db_path();
    let first = start(&path, vec![Step::Hang]).await;
    let trigger = first.ingest("owner", "@A hi").await;
    let original = first.adapter.dispatches()[0].wake_id;
    first
        .store
        .halts()
        .set(&HaltScope::Bot(bot("A")), Some("cli"), 0)
        .unwrap();
    first.stop();

    let second = start(&path, vec![Step::Hang]).await;
    second.settle().await;

    let wake = second.store.wakes().get(&original).unwrap().unwrap();
    assert_eq!(wake.state, WakeState::Interrupted);
    assert!(second.adapter.dispatches().is_empty());
    assert_eq!(second.relay.reactions(emoji::WARNING), vec![trigger.id]);
}

#[tokio::test(start_paused = true)]
async fn the_first_snapshot_after_startup_sees_the_stored_halts() {
    let path = db_path();
    let first = start(&path, vec![Step::Exit(0)]).await;
    first
        .store
        .halts()
        .set(&HaltScope::Bot(bot("A")), Some("cli"), 0)
        .unwrap();
    first.stop();

    let second = start(&path, vec![Step::Exit(0)]).await;
    second.ingest("owner", "@A hi").await;
    second.settle().await;

    assert!(second.adapter.dispatches().is_empty());
}
