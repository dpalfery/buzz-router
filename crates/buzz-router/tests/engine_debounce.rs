//! Task 3.3: discussion debounce (design 6.5, R23, R65.2).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use buzz_router::core::queue::Trigger;
use buzz_router::ingest::Source;
use buzz_router::store::Store;
use chrono::TimeDelta;
use nostr::Event;
use router_core::ids::{BotName, ChannelId, EventId};
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

/// Each bot's dispatch times, in seconds after the base time.
fn dispatch_offsets(adapter: &FakeAdapter) -> BTreeMap<BotName, Vec<i64>> {
    let mut offsets: BTreeMap<BotName, Vec<i64>> = BTreeMap::new();
    for dispatch in adapter.dispatches() {
        offsets
            .entry(dispatch.bot)
            .or_default()
            .push((dispatch.at - base_time()).num_seconds());
    }
    offsets
}

#[tokio::test(start_paused = true)]
async fn three_bot_posts_5s_apart_wake_each_other_participant_once_20s_after_the_last() {
    let root = top_level(&keys("owner"), channel(), "@everyone discuss", base_secs());
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, _relay, _store) = spawn_test_core_with(TestCoreOptions {
        seed: Some(seed_discussion(&root)),
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });

    for i in 0..3_u64 {
        if i > 0 {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        let post = reply(
            &keys("A"),
            channel(),
            &format!("post {i}"),
            &root,
            &root,
            base_secs() + 5 * i,
        );
        core.ingest(bot("A"), post, Source::Live);
        core.flush().await;
    }
    tokio::time::sleep(Duration::from_secs(60)).await;
    core.flush().await;

    let offsets = dispatch_offsets(&adapter);
    assert_eq!(
        offsets,
        BTreeMap::from([(bot("B"), vec![30]), (bot("C"), vec![30])]),
        "one wake per other participant at the last post + 20 s"
    );
}

#[tokio::test(start_paused = true)]
async fn a_steady_stream_dispatches_at_the_first_trigger_plus_90s() {
    let root = top_level(&keys("owner"), channel(), "@everyone discuss", base_secs());
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, _relay, _store) = spawn_test_core_with(TestCoreOptions {
        seed: Some(seed_discussion(&root)),
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });

    for i in 0..13_u64 {
        if i > 0 {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        let post = reply(
            &keys("A"),
            channel(),
            &format!("post {i}"),
            &root,
            &root,
            base_secs() + 10 * i,
        );
        core.ingest(bot("A"), post, Source::Live);
        core.flush().await;
    }
    core.flush().await;

    let offsets = dispatch_offsets(&adapter);
    for name in ["B", "C"] {
        assert_eq!(
            offsets.get(&bot(name)).and_then(|times| times.first()),
            Some(&90),
            "{name}: {offsets:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn owner_and_human_wakes_dispatch_immediately() {
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, _relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });

    tokio::time::sleep(Duration::from_secs(3)).await;
    let owner = top_level(&keys("owner"), channel(), "@A status?", base_secs() + 3);
    core.ingest(bot("A"), owner.clone(), Source::Live);
    let human = top_level(&keys("human"), channel(), "@B hello", base_secs() + 3);
    core.ingest(bot("B"), human.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    let dispatches = adapter.dispatches();
    let bots: BTreeSet<_> = dispatches.iter().map(|d| d.bot.clone()).collect();
    assert_eq!(bots, BTreeSet::from([bot("A"), bot("B")]));
    for dispatch in &dispatches {
        assert_eq!(
            dispatch.at - base_time(),
            TimeDelta::seconds(3),
            "{dispatch:?}"
        );
        let wake = store.wakes().get(&dispatch.wake_id).unwrap().unwrap();
        let triggers: Vec<Trigger> = serde_json::from_str(&wake.triggers).unwrap();
        assert!(triggers.iter().all(|trigger| !trigger.debounce));
    }
}
