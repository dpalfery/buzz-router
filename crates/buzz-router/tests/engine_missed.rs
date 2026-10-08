//! Task 4.1: missed owner messages (design 6.2 step 8, DD-14, DA-2, R48.3). A backfilled owner
//! message more than 24 hours older than now wakes nobody and is listed in `missed`, but its
//! thread update and control still apply.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use buzz_router::ingest::Source;
use buzz_router::store::halts::HaltScope;
use router_core::ids::{BotName, ChannelId, EventId};
use support::TestCoreOptions;
use support::{base_secs, channel, keys, spawn_test_core_with, top_level, FakeAdapter, Step};

const HOUR: u64 = 3_600;

async fn backfill_owner(text: &str, age_secs: u64) -> (support::FakeAdapter, Run) {
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, _relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });
    let event = top_level(&keys("owner"), channel(), text, base_secs() - age_secs);
    core.ingest(BotName::new("A").unwrap(), event.clone(), Source::Backfill);
    core.flush().await;
    (adapter, Run { core, store, event })
}

struct Run {
    core: buzz_router::core::CoreHandle,
    store: buzz_router::store::Store,
    event: nostr::Event,
}

#[tokio::test(start_paused = true)]
async fn a_25_hour_old_owner_mention_wakes_nobody_is_missed_and_starts_its_round() {
    let (adapter, run) = backfill_owner("@A are you there?", 25 * HOUR).await;

    assert!(adapter.dispatches().is_empty());
    let missed = run.core.debug_counters().await.missed;
    assert_eq!(missed.len(), 1);
    let id = EventId::from_nostr(&run.event.id);
    assert_eq!(missed[0].event_id, id);
    assert_eq!(missed[0].channel_id, ChannelId::from(channel()));
    assert_eq!(
        missed[0].created_at,
        i64::try_from(run.event.created_at.as_secs()).unwrap()
    );
    let thread = run.store.threads().load(&id).unwrap().unwrap();
    assert_eq!(thread.round_id, id);
}

#[tokio::test(start_paused = true)]
async fn a_25_hour_old_stop_still_writes_its_halt() {
    let (_adapter, run) = backfill_owner("stop", 25 * HOUR).await;

    let scopes: Vec<HaltScope> = run
        .store
        .halts()
        .list()
        .unwrap()
        .into_iter()
        .map(|row| row.scope)
        .collect();
    assert_eq!(scopes, vec![HaltScope::All]);
}

#[tokio::test(start_paused = true)]
async fn a_23_hour_old_owner_mention_wakes() {
    let (adapter, run) = backfill_owner("@A are you there?", 23 * HOUR).await;

    assert_eq!(adapter.dispatches().len(), 1);
    assert!(run.core.debug_counters().await.missed.is_empty());
}
