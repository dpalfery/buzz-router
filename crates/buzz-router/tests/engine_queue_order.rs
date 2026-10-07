//! Task 3.3: queue order and concurrency (design 6.5 `schedule()`, R25, R26).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::time::Duration;

use buzz_router::core::queue::Trigger;
use buzz_router::ingest::Source;
use nostr::Event;
use router_core::ids::{BotName, EventId};
use support::{
    base_secs, base_time, channel, keys, spawn_test_core_with, top_level, FakeAdapter, Step,
    TestCoreOptions,
};

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

fn id(event: &Event) -> EventId {
    EventId::from_nostr(&event.id)
}

/// A's dispatches as (thread root, seconds after the base time).
fn a_dispatches(adapter: &FakeAdapter) -> Vec<(EventId, i64)> {
    adapter
        .dispatches()
        .into_iter()
        .filter(|d| d.bot == bot("A"))
        .map(|d| (d.root, (d.at - base_time()).num_seconds()))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn one_slot_dispatches_owner_then_human_then_bot() {
    let adapter = FakeAdapter::new(vec![Step::Wait(Duration::from_secs(10)), Step::Exit(0)]);
    adapter.push_script(vec![Step::Wait(Duration::from_secs(30)), Step::Exit(0)]);
    let (core, _relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });

    let busy = top_level(&keys("owner"), channel(), "@A busy work", base_secs());
    core.ingest(bot("A"), busy.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let by_bot = top_level(&keys("B"), channel(), "@A have a look", base_secs() + 1);
    core.ingest(bot("A"), by_bot.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let by_human = top_level(&keys("human"), channel(), "@A question", base_secs() + 2);
    core.ingest(bot("A"), by_human.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let by_owner = top_level(&keys("owner"), channel(), "@A urgent", base_secs() + 3);
    core.ingest(bot("A"), by_owner.clone(), Source::Live);
    core.flush().await;

    let queued = store
        .wakes()
        .find_queued(&bot("A"), &id(&by_bot))
        .unwrap()
        .unwrap();
    let triggers: Vec<Trigger> = serde_json::from_str(&queued.triggers).unwrap();
    assert!(triggers[0].debounce, "a bot mention is debounced");

    tokio::time::sleep(Duration::from_secs(120)).await;
    core.flush().await;
    assert_eq!(
        a_dispatches(&adapter),
        vec![
            (id(&busy), 0),
            (id(&by_owner), 30),
            (id(&by_human), 40),
            (id(&by_bot), 50),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn one_slot_is_first_in_first_out_within_a_priority() {
    let adapter = FakeAdapter::new(vec![Step::Wait(Duration::from_secs(10)), Step::Exit(0)]);
    adapter.push_script(vec![Step::Wait(Duration::from_secs(30)), Step::Exit(0)]);
    let (core, _relay, _store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });
    let owner = keys("owner");

    let busy = top_level(&owner, channel(), "@A busy work", base_secs());
    core.ingest(bot("A"), busy.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let first = top_level(&owner, channel(), "@A first", base_secs() + 1);
    core.ingest(bot("A"), first.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let second = top_level(&owner, channel(), "@A second", base_secs() + 2);
    core.ingest(bot("A"), second.clone(), Source::Live);
    core.flush().await;

    tokio::time::sleep(Duration::from_secs(120)).await;
    core.flush().await;
    assert_eq!(
        a_dispatches(&adapter),
        vec![(id(&busy), 0), (id(&first), 30), (id(&second), 40)]
    );
}

#[tokio::test(start_paused = true)]
async fn two_slots_run_two_wakes_at_once() {
    let adapter = FakeAdapter::new(vec![Step::Wait(Duration::from_secs(10)), Step::Exit(0)]);
    let (core, _relay, _store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        max_concurrent: 2,
        ..TestCoreOptions::default()
    });
    let owner = keys("owner");

    let one = top_level(&owner, channel(), "@A one", base_secs());
    let two = top_level(&owner, channel(), "@A two", base_secs());
    let three = top_level(&owner, channel(), "@A three", base_secs());
    for event in [&one, &two, &three] {
        core.ingest(bot("A"), event.clone(), Source::Live);
    }
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    core.flush().await;

    let times: Vec<i64> = a_dispatches(&adapter)
        .into_iter()
        .map(|(_, at)| at)
        .collect();
    assert_eq!(
        times,
        vec![0, 0, 10],
        "two run at once, the third waits for a slot"
    );
}
