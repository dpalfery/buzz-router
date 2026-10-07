//! Task 3.2: the core actor applying route results (design 6.1, 6.3 core and rebuild, 6.4, 9.3).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};

use buzz_router::core::wake_counts;
use buzz_router::ingest::Source;
use buzz_router::store::halts::HaltScope;
use buzz_router::store::wakes::{WakeRow, WakeState};
use buzz_router::store::Store;
use nostr::Event;
use router_core::ids::{BotName, ChannelId, EventId};
use router_core::route::{Decision, SuppressWhy, WakeCounts};
use router_core::thread::{RoundMode, ThreadState};
use serde_json::{json, Value};
use support::{
    base_secs, base_time, channel, keys, reply, spawn_test_core, spawn_test_core_with, top_level,
    TestCoreOptions, RELAY_URL,
};
use uuid::Uuid;

const PAUSE: &str = "\u{23F8}\u{FE0F}";

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

fn id(event: &Event) -> EventId {
    EventId::from_nostr(&event.id)
}

fn base_ms() -> i64 {
    base_time().timestamp_millis()
}

/// A discussion thread at `root` with participants A, B and C, its round started by `root`.
fn seed_discussion(store: &Store, root: &Event) {
    store
        .threads()
        .upsert(&ThreadState {
            root_id: id(root),
            channel_id: ChannelId::from(channel()),
            participants: ["A", "B", "C"].into_iter().map(bot).collect(),
            discussion: true,
            round_id: id(root),
            round_mode: RoundMode::Discussion,
            round_started_at: root.created_at.as_secs() as i64,
            turns_used: BTreeMap::new(),
        })
        .unwrap();
}

/// A finished wake of `bot` in the thread `root`, round `round`, started at `started_at_ms`.
fn seed_started_wake(
    store: &Store,
    bot_name: &str,
    root: &EventId,
    round: &EventId,
    started_at_ms: i64,
) {
    store
        .wakes()
        .insert(&WakeRow {
            id: Uuid::new_v4(),
            bot: bot(bot_name),
            root_id: root.clone(),
            round_id: round.clone(),
            reason: "discussion".into(),
            priority: "bot".into(),
            triggers: "[]".into(),
            state: WakeState::Passed,
            token_hash: None,
            attempt: 1,
            created_at: started_at_ms,
            dispatch_after: started_at_ms,
            started_at: Some(started_at_ms),
            deadline: None,
            ended_at: Some(started_at_ms + 1_000),
            outcome: None,
        })
        .unwrap();
}

fn reactions_by(published: &[Event], name: &str) -> Vec<Event> {
    let author = keys(name).public_key();
    published
        .iter()
        .filter(|event| event.kind.as_u16() == 7 && event.pubkey == author)
        .cloned()
        .collect()
}

fn e_tags(event: &Event) -> Vec<String> {
    event
        .tags
        .iter()
        .filter_map(|tag| match tag.as_slice() {
            [name, value, ..] if name == "e" => Some(value.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn an_owner_mention_queues_a_wake_and_starts_a_round() {
    let (core, _relay, store) = spawn_test_core();
    let owner = keys("owner");
    let o1 = top_level(&owner, channel(), "@A status?", base_secs());
    core.ingest(bot("A"), o1.clone(), Source::Live);
    core.flush().await;

    let wake = store
        .wakes()
        .find_queued(&bot("A"), &id(&o1))
        .unwrap()
        .unwrap();
    assert_eq!(wake.state, WakeState::Queued);
    assert_eq!(wake.reason, "mention");
    assert_eq!(wake.priority, "owner");
    assert_eq!(wake.round_id, id(&o1));
    assert_eq!(wake.attempt, 1);
    let triggers: Value = serde_json::from_str(&wake.triggers).unwrap();
    let triggers = triggers.as_array().unwrap();
    assert_eq!(triggers.len(), 1);
    let trigger = triggers[0].as_object().unwrap();
    let keys_in_order: Vec<&str> = trigger.keys().map(String::as_str).collect();
    let mut sorted = keys_in_order.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        [
            "author",
            "class",
            "created_at",
            "debounce",
            "edit_id",
            "event_id",
            "mode",
            "priority",
            "reason",
            "received_at_ms"
        ]
    );
    assert_eq!(trigger["event_id"], json!(o1.id.to_hex()));
    assert_eq!(trigger["edit_id"], Value::Null);
    assert_eq!(trigger["class"], "owner");
    assert_eq!(trigger["author"], "Owner");
    assert_eq!(trigger["reason"], "mention");
    assert_eq!(trigger["priority"], "owner");
    assert_eq!(trigger["debounce"], false);
    assert_eq!(trigger["mode"], "direct");
    assert_eq!(trigger["created_at"], json!(base_secs()));
    let received = trigger["received_at_ms"].as_i64().unwrap();
    assert!(
        (base_ms()..base_ms() + 2_000).contains(&received),
        "{received}"
    );

    let thread = store.threads().load(&id(&o1)).unwrap().unwrap();
    assert_eq!(thread.round_id, id(&o1));
    assert_eq!(thread.round_mode, RoundMode::Direct);
    assert_eq!(thread.participants, BTreeSet::from([bot("A")]));

    // The wake runs and uses a turn; the owner's next message starts a new round.
    store
        .wakes()
        .finish(&wake.id, WakeState::Passed, base_ms() + 1_000, None)
        .unwrap();
    store
        .turns()
        .increment(&id(&o1), &id(&o1), &bot("A"))
        .unwrap();

    let o2 = reply(&owner, channel(), "@A more", &o1, &o1, base_secs() + 60);
    core.ingest(bot("A"), o2.clone(), Source::Live);
    core.flush().await;

    let thread = store.threads().load(&id(&o1)).unwrap().unwrap();
    assert_eq!(thread.round_id, id(&o2));
    assert_eq!(thread.round_started_at, base_secs() as i64 + 60);
    assert!(
        thread.turns_used.is_empty(),
        "turns reset: {:?}",
        thread.turns_used
    );
    let wake = store
        .wakes()
        .find_queued(&bot("A"), &id(&o1))
        .unwrap()
        .unwrap();
    assert_eq!(wake.round_id, id(&o2));
}

#[tokio::test(start_paused = true)]
async fn the_first_cap_suppression_in_a_round_reacts_once() {
    let root = top_level(&keys("owner"), channel(), "@everyone discuss", base_secs());
    let seeded = root.clone();
    let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
        limits: "turns_per_round = 1".into(),
        seed: Some(Box::new(move |store: &Store| {
            seed_discussion(store, &seeded);
            store
                .turns()
                .increment(&id(&seeded), &id(&seeded), &bot("B"))
                .unwrap();
        })),
        ..TestCoreOptions::default()
    });

    let a1 = reply(
        &keys("A"),
        channel(),
        "my take",
        &root,
        &root,
        base_secs() + 10,
    );
    core.ingest(bot("A"), a1.clone(), Source::Live);
    core.flush().await;

    let reactions = reactions_by(&relay.published(), "B");
    assert_eq!(reactions.len(), 1, "{reactions:?}");
    assert_eq!(reactions[0].content, PAUSE);
    assert_eq!(e_tags(&reactions[0]), vec![a1.id.to_hex()]);
    reactions[0].verify().unwrap();
    assert!(store
        .turns()
        .cap_reacted(&id(&root), &id(&root), &bot("B"))
        .unwrap());

    let a2 = reply(
        &keys("A"),
        channel(),
        "another take",
        &root,
        &root,
        base_secs() + 20,
    );
    core.ingest(bot("A"), a2, Source::Live);
    core.flush().await;
    assert_eq!(
        reactions_by(&relay.published(), "B").len(),
        1,
        "case 11: ⏸️ once per round"
    );
}

#[tokio::test(start_paused = true)]
async fn a_budget_suppression_increments_the_counter() {
    let root = top_level(&keys("owner"), channel(), "@everyone discuss", base_secs());
    let seeded = root.clone();
    let (core, relay, _store) = spawn_test_core_with(TestCoreOptions {
        limits: "wakes_per_hour = 1".into(),
        seed: Some(Box::new(move |store: &Store| {
            seed_discussion(store, &seeded);
            seed_started_wake(store, "C", &id(&seeded), &id(&seeded), base_ms() - 600_000);
        })),
        ..TestCoreOptions::default()
    });

    let a1 = reply(
        &keys("A"),
        channel(),
        "my take",
        &root,
        &root,
        base_secs() + 10,
    );
    core.ingest(bot("A"), a1, Source::Live);
    core.flush().await;

    let counters = core.debug_counters().await;
    assert_eq!(counters.budget_suppressed.get(&bot("C")), Some(&1));
    assert_eq!(counters.budget_suppressed.get(&bot("B")), None);
    assert!(counters.last_decisions.contains(&Decision::Suppress {
        bot: bot("C"),
        why: SuppressWhy::Budget
    }));
    assert!(
        relay.published().is_empty(),
        "a budget suppression reacts nothing"
    );
}

/// The relay holds a discussion: the owner's `@everyone` root, A's reply, a historical
/// `@everyone stop`, and B's reply. A posts again, and the core has never seen the thread.
fn seed_history(relay: &support::FakeRelay) -> (Event, Event) {
    let owner = keys("owner");
    let root = top_level(&owner, channel(), "@everyone discuss", base_secs());
    let a1 = reply(
        &keys("A"),
        channel(),
        "first",
        &root,
        &root,
        base_secs() + 10,
    );
    let stop = reply(
        &owner,
        channel(),
        "@everyone stop",
        &root,
        &root,
        base_secs() + 20,
    );
    let b1 = reply(
        &keys("B"),
        channel(),
        "second",
        &root,
        &root,
        base_secs() + 30,
    );
    for event in [&root, &a1, &stop, &b1] {
        relay.seed(event.clone());
    }
    let current = reply(
        &keys("A"),
        channel(),
        "again",
        &root,
        &root,
        base_secs() + 40,
    );
    relay.seed(current.clone());
    (root, current)
}

#[tokio::test(start_paused = true)]
async fn a_rebuild_restores_the_thread_without_side_effects() {
    let (core, relay, store) = spawn_test_core();
    let (root, current) = seed_history(&relay);
    core.ingest(bot("A"), current, Source::Live);
    core.flush().await;

    let thread = store.threads().load(&id(&root)).unwrap().unwrap();
    assert_eq!(
        thread.participants,
        ["A", "B", "C"]
            .into_iter()
            .map(bot)
            .collect::<BTreeSet<_>>()
    );
    assert!(thread.discussion);
    assert_eq!(thread.round_id, id(&root));
    assert_eq!(thread.round_mode, RoundMode::Discussion);
    assert_eq!(
        thread.turns_used,
        BTreeMap::from([(bot("A"), 1), (bot("B"), 1)]),
        "turns come from bot posts since the round started"
    );
    assert!(
        store.halts().list().unwrap().is_empty(),
        "a historical stop sets no halt"
    );
    assert!(relay.published().is_empty(), "a rebuild publishes nothing");
    assert!(store.events().is_processed(&id(&root)).unwrap());
}

#[tokio::test(start_paused = true)]
async fn a_rebuild_takes_turns_from_wakes_when_there_are_any() {
    let owner = keys("owner");
    let root = top_level(&owner, channel(), "@everyone discuss", base_secs());
    let root_id = id(&root);
    let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
        seed: Some(Box::new(move |store: &Store| {
            seed_started_wake(store, "A", &root_id, &root_id, base_ms() + 1_000);
            seed_started_wake(store, "A", &root_id, &root_id, base_ms() + 2_000);
        })),
        ..TestCoreOptions::default()
    });
    let (root, current) = seed_history(&relay);
    core.ingest(bot("A"), current, Source::Live);
    core.flush().await;

    let thread = store.threads().load(&id(&root)).unwrap().unwrap();
    assert_eq!(thread.turns_used, BTreeMap::from([(bot("A"), 2)]));
}

#[tokio::test(start_paused = true)]
async fn a_halt_row_written_directly_suppresses_the_next_owner_mention() {
    let (core, _relay, store) = spawn_test_core();
    store
        .halts()
        .set(&HaltScope::All, Some("cli"), base_ms())
        .unwrap();

    let o1 = top_level(&keys("owner"), channel(), "@A status?", base_secs());
    core.ingest(bot("A"), o1.clone(), Source::Live);
    core.flush().await;

    let counters = core.debug_counters().await;
    assert_eq!(
        counters.last_decisions,
        vec![Decision::Suppress {
            bot: bot("A"),
            why: SuppressWhy::Halted
        }]
    );
    assert!(store
        .wakes()
        .find_queued(&bot("A"), &id(&o1))
        .unwrap()
        .is_none());
}

#[tokio::test(start_paused = true)]
async fn the_cursor_advances_after_the_apply() {
    let (core, _relay, store) = spawn_test_core();
    let o1 = top_level(&keys("owner"), channel(), "hello", base_secs() + 5);
    core.ingest(bot("A"), o1, Source::Backfill);
    core.flush().await;
    assert_eq!(
        store.cursors().get(&bot("A"), RELAY_URL).unwrap(),
        Some(base_secs() as i64 + 5)
    );
}

#[tokio::test(start_paused = true)]
async fn wake_counts_cover_the_trailing_hour_and_day() {
    let store = Store::open_in_memory().unwrap();
    let root = EventId::from_hex(&"ab".repeat(32)).unwrap();
    seed_started_wake(&store, "A", &root, &root, base_ms() - 59 * 60_000);
    seed_started_wake(&store, "A", &root, &root, base_ms() - 61 * 60_000);
    let bots = BTreeSet::from([bot("A"), bot("B")]);

    let counts = wake_counts(&store, &bots, base_time()).unwrap();
    assert_eq!(counts.get(&bot("A")), Some(&WakeCounts { hour: 1, day: 2 }));
    assert_eq!(counts.get(&bot("B")), Some(&WakeCounts { hour: 0, day: 0 }));
}
