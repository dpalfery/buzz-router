//! Task 3.3: coalescing triggers into one wake (design 6.5, R24, A9, R65.3).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use buzz_router::core::queue::Trigger;
use buzz_router::ingest::Source;
use buzz_router::store::Store;
use nostr::Event;
use router_core::ids::{BotName, ChannelId, EventId};
use router_core::route::Priority;
use router_core::thread::{RoundMode, ThreadState};
use support::{
    base_secs, base_time, channel, keys, reply, spawn_test_core_with, top_level, FakeAdapter, Step,
    TestCoreOptions,
};

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

fn id(event: &Event) -> EventId {
    EventId::from_nostr(&event.id)
}

fn seed_discussion(root: &Event) -> support::Seed {
    let root = root.clone();
    Box::new(move |store: &Store| {
        store
            .threads()
            .upsert(&ThreadState {
                root_id: id(&root),
                channel_id: ChannelId::from(channel()),
                participants: ["A", "B", "C"].into_iter().map(bot).collect(),
                discussion: true,
                round_id: id(&root),
                round_mode: RoundMode::Discussion,
                round_started_at: root.created_at.as_secs() as i64,
                turns_used: BTreeMap::new(),
            })
            .unwrap();
    })
}

fn wakes_for(store: &Store, name: &str, root: &Event) -> i64 {
    store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM wakes WHERE bot = ?1 AND root_id = ?2",
            [name, &root.id.to_hex()],
            |row| row.get(0),
        )
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn a_trigger_during_a_running_wake_makes_exactly_one_follow_up() {
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    adapter.push_script(vec![Step::Wait(Duration::from_secs(10)), Step::Exit(0)]);
    let (core, _relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });
    let owner = keys("owner");

    let one = top_level(&owner, channel(), "@A one", base_secs());
    core.ingest(bot("A"), one.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let two = reply(&owner, channel(), "@A two", &one, &one, base_secs() + 2);
    core.ingest(bot("A"), two.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let three = reply(&owner, channel(), "@A three", &one, &one, base_secs() + 3);
    core.ingest(bot("A"), three.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(30)).await;
    core.flush().await;

    let dispatches = adapter.dispatches();
    assert_eq!(dispatches.len(), 2, "{dispatches:?}");
    assert_eq!((dispatches[0].at - base_time()).num_seconds(), 0);
    assert_eq!(
        (dispatches[1].at - base_time()).num_seconds(),
        10,
        "the follow-up starts when the running wake ends"
    );
    assert_eq!(dispatches[1].triggers, vec![id(&two), id(&three)]);
    assert_eq!(
        wakes_for(&store, "A", &one),
        2,
        "exactly one follow-up wake"
    );
}

#[tokio::test(start_paused = true)]
async fn appended_triggers_leave_one_wake_that_costs_one_turn() {
    let root = top_level(&keys("owner"), channel(), "@everyone discuss", base_secs());
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, _relay, store) = spawn_test_core_with(TestCoreOptions {
        seed: Some(seed_discussion(&root)),
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });

    let b1 = reply(&keys("B"), channel(), "one", &root, &root, base_secs());
    core.ingest(bot("B"), b1.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let b2 = reply(&keys("B"), channel(), "two", &root, &root, base_secs() + 5);
    core.ingest(bot("B"), b2.clone(), Source::Live);
    core.flush().await;
    let queued = store
        .wakes()
        .find_queued(&bot("C"), &id(&root))
        .unwrap()
        .unwrap();
    let triggers: Vec<Trigger> = serde_json::from_str(&queued.triggers).unwrap();
    assert_eq!(triggers.len(), 2);
    assert_eq!(wakes_for(&store, "C", &root), 1);

    tokio::time::sleep(Duration::from_secs(60)).await;
    core.flush().await;
    let c_dispatches: Vec<_> = adapter
        .dispatches()
        .into_iter()
        .filter(|d| d.bot == bot("C"))
        .collect();
    assert_eq!(c_dispatches.len(), 1);
    assert_eq!(c_dispatches[0].triggers, vec![id(&b1), id(&b2)]);
    let used = store.turns().used(&id(&root), &id(&root)).unwrap();
    assert_eq!(
        used.get(&bot("C")),
        Some(&1),
        "a coalesced wake costs one turn"
    );
}

#[tokio::test(start_paused = true)]
async fn an_owner_trigger_makes_a_debounced_wake_owner_priority_and_due_now() {
    let root = top_level(&keys("owner"), channel(), "@everyone discuss", base_secs());
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, _relay, store) = spawn_test_core_with(TestCoreOptions {
        seed: Some(seed_discussion(&root)),
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });

    let b1 = reply(&keys("B"), channel(), "thoughts", &root, &root, base_secs());
    core.ingest(bot("B"), b1.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let owner = reply(
        &keys("owner"),
        channel(),
        "@C now please",
        &root,
        &root,
        base_secs() + 5,
    );
    core.ingest(bot("C"), owner.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    let c_dispatch = adapter
        .dispatches()
        .into_iter()
        .find(|d| d.bot == bot("C"))
        .unwrap();
    assert_eq!((c_dispatch.at - base_time()).num_seconds(), 5);
    assert_eq!(c_dispatch.triggers, vec![id(&b1), id(&owner)]);
    let wake = store.wakes().get(&c_dispatch.wake_id).unwrap().unwrap();
    assert_eq!(wake.priority, "owner");
    assert_eq!(wake.reason, "mention");
    let triggers: Vec<Trigger> = serde_json::from_str(&wake.triggers).unwrap();
    assert_eq!(triggers[1].priority, Priority::Owner);
}
