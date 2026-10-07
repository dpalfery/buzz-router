//! Task 2.6: the ingest pipeline (design 6.3 ingest steps 1-7, DD-13).

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use buzz_router::clock::{Clock, VirtualClock};
use buzz_router::ingest::{EnrichedEvent, Ingest, IngestOutput, Source};
use buzz_router::relay::RelayPort;
use buzz_router::store::events::{EventClass, EventRow};
use buzz_router::store::Store;
use chrono::{DateTime, TimeZone, Utc};
use nostr::Event;
use router_core::ids::{BotName, ChannelId, EventId, Pubkey};
use router_core::route::EditTarget;
use router_core::thread::{RoundMode, ThreadPos, ThreadState};
use support::{channel, edit, keys, raw_event, reply, top_level, FakeRelay};

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

fn id(event: &Event) -> EventId {
    EventId::from_nostr(&event.id)
}

fn base() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 5, 3, 0, 0).unwrap()
}

struct Harness {
    ingest: Ingest,
    relay_a: FakeRelay,
    relay_b: FakeRelay,
}

/// An ingest over `store` with two bots, A and B, each with its own fake relay.
fn harness(store: Store) -> Harness {
    let relay_a = FakeRelay::new();
    let relay_b = FakeRelay::new();
    let mut relays: BTreeMap<BotName, Arc<dyn RelayPort>> = BTreeMap::new();
    relays.insert(bot("A"), Arc::new(relay_a.clone()));
    relays.insert(bot("B"), Arc::new(relay_b.clone()));
    let clock: Arc<dyn Clock> = Arc::new(VirtualClock::new(base()));
    Harness {
        ingest: Ingest::new(store, relays, clock),
        relay_a,
        relay_b,
    }
}

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn seed_thread(store: &Store, root: &Event) {
    store
        .threads()
        .upsert(&ThreadState {
            root_id: id(root),
            channel_id: ChannelId::from(channel()),
            participants: BTreeSet::new(),
            discussion: false,
            round_id: id(root),
            round_mode: RoundMode::Direct,
            round_started_at: root.created_at.as_secs() as i64,
            turns_used: BTreeMap::new(),
        })
        .unwrap();
}

fn seed_event(store: &Store, event: &Event, root: &Event, processed: bool) {
    store
        .events()
        .insert_or_ignore(&EventRow {
            id: id(event),
            channel_id: ChannelId::from(channel()),
            root_id: id(root),
            author: Pubkey::from_nostr(&event.pubkey),
            class: EventClass::Human,
            kind: event.kind.as_u16(),
            created_at: event.created_at.as_secs() as i64,
            processed_at: processed.then_some(1),
        })
        .unwrap();
}

fn enriched(outputs: &[IngestOutput]) -> Vec<&EnrichedEvent> {
    outputs
        .iter()
        .filter_map(|output| match output {
            IngestOutput::Enriched(event) => Some(event.as_ref()),
            IngestOutput::RebuildThread { .. } => None,
        })
        .collect()
}

fn only_enriched(outputs: &[IngestOutput]) -> &EnrichedEvent {
    assert_eq!(
        outputs.len(),
        1,
        "expected exactly one output, got {outputs:?}"
    );
    match &outputs[0] {
        IngestOutput::Enriched(event) => event,
        other => panic!("expected an enriched event, got {other:?}"),
    }
}

fn query_ids(filters: &[nostr::Filter]) -> Vec<String> {
    let json = serde_json::to_value(filters).unwrap();
    json.as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f.get("ids"))
        .flat_map(|ids| {
            ids.as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_bad_signature_is_dropped() {
    let mut h = harness(store());
    let mut event = top_level(&keys("owner"), channel(), "hello", 100);
    event.content = "tampered".into();
    let out = h.ingest.handle(&bot("A"), event, Source::Live).await;
    assert!(out.is_empty(), "{out:?}");
}

#[tokio::test(start_paused = true)]
async fn kind_7_is_ignored() {
    let mut h = harness(store());
    let target = top_level(&keys("owner"), channel(), "hello", 100);
    let channel = channel().to_string();
    let target_hex = target.id.to_hex();
    let reaction = raw_event(
        &keys("owner"),
        7,
        "👍",
        &[&["h", &channel], &["e", &target_hex]],
        101,
    );
    let out = h.ingest.handle(&bot("A"), reaction, Source::Live).await;
    assert!(out.is_empty(), "{out:?}");
}

#[tokio::test(start_paused = true)]
async fn an_event_without_an_h_tag_is_ignored() {
    let mut h = harness(store());
    let event = raw_event(&keys("owner"), 9, "hello", &[], 100);
    let out = h.ingest.handle(&bot("A"), event, Source::Live).await;
    assert!(out.is_empty(), "{out:?}");
}

#[tokio::test(start_paused = true)]
async fn the_same_id_from_two_bots_is_forwarded_once() {
    let mut h = harness(store());
    let event = top_level(&keys("owner"), channel(), "hello", 100);
    let first = h
        .ingest
        .handle(&bot("A"), event.clone(), Source::Live)
        .await;
    let second = h.ingest.handle(&bot("B"), event, Source::Live).await;
    assert_eq!(only_enriched(&first).bot, bot("A"));
    assert!(second.is_empty(), "{second:?}");
}

#[tokio::test(start_paused = true)]
async fn an_already_processed_id_is_skipped() {
    let store = store();
    let event = top_level(&keys("owner"), channel(), "hello", 100);
    seed_event(&store, &event, &event, true);
    let mut h = harness(store);
    let out = h.ingest.handle(&bot("A"), event, Source::Backfill).await;
    assert!(out.is_empty(), "{out:?}");
}

#[tokio::test(start_paused = true)]
async fn a_top_level_message_is_enriched() {
    let mut h = harness(store());
    let event = top_level(&keys("owner"), channel(), "hello", 100);
    let out = h
        .ingest
        .handle(&bot("A"), event.clone(), Source::Backfill)
        .await;
    let enriched = only_enriched(&out);
    assert_eq!(enriched.bot, bot("A"));
    assert_eq!(enriched.source, Source::Backfill);
    assert_eq!(enriched.event, event);
    assert_eq!(enriched.in_event.id, id(&event));
    assert_eq!(enriched.in_event.channel, ChannelId::from(channel()));
    assert_eq!(enriched.pos, ThreadPos::TopLevel);
    assert_eq!(enriched.root, Some(id(&event)));
    assert_eq!(enriched.parent_author, None);
    assert_eq!(enriched.edit_target, None);
    assert_eq!(enriched.received_at, base());
}

#[tokio::test(start_paused = true)]
async fn a_kind_9_replys_thread_is_resolved() {
    let store = store();
    let owner = keys("owner");
    let root = top_level(&owner, channel(), "root", 100);
    seed_thread(&store, &root);
    let mut h = harness(store);
    let event = reply(&owner, channel(), "direct", &root, &root, 110);
    let out = h.ingest.handle(&bot("A"), event, Source::Live).await;
    let enriched = only_enriched(&out);
    assert_eq!(
        enriched.pos,
        ThreadPos::Reply {
            root: id(&root),
            parent: id(&root)
        }
    );
    assert_eq!(enriched.root, Some(id(&root)));
    assert_eq!(
        enriched.parent_author, None,
        "a direct reply has no parent author"
    );
    assert!(
        h.relay_a.queries().is_empty(),
        "a known root needs no fetch"
    );
}

#[tokio::test(start_paused = true)]
async fn an_edit_target_is_resolved_from_events() {
    let store = store();
    let owner = keys("owner");
    let root = top_level(&owner, channel(), "root", 100);
    let target = reply(&owner, channel(), "typo", &root, &root, 110);
    seed_thread(&store, &root);
    seed_event(&store, &target, &root, true);
    let mut h = harness(store);
    let edit = edit(&owner, channel(), &target, "fixed", 120);
    let out = h.ingest.handle(&bot("A"), edit, Source::Live).await;
    let enriched = only_enriched(&out);
    assert_eq!(
        enriched.edit_target,
        Some(EditTarget {
            message_id: id(&target)
        })
    );
    assert_eq!(enriched.root, Some(id(&root)));
    assert!(h.relay_a.queries().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_unknown_edit_target_is_fetched_from_the_relay() {
    let store = store();
    let owner = keys("owner");
    let root = top_level(&owner, channel(), "root", 100);
    let target = reply(&owner, channel(), "typo", &root, &root, 110);
    seed_thread(&store, &root);
    let mut h = harness(store);
    h.relay_a.seed(root.clone());
    h.relay_a.seed(target.clone());
    let edit = edit(&owner, channel(), &target, "fixed", 120);
    let out = h.ingest.handle(&bot("A"), edit, Source::Live).await;
    let enriched = only_enriched(&out);
    assert_eq!(
        enriched.edit_target,
        Some(EditTarget {
            message_id: id(&target)
        })
    );
    assert_eq!(enriched.root, Some(id(&root)));
    let queries = h.relay_a.queries();
    assert_eq!(queries.len(), 1, "{queries:?}");
    assert_eq!(query_ids(&queries[0]), vec![target.id.to_hex()]);
    assert!(
        h.relay_b.queries().is_empty(),
        "only the receiving bot's relay is used"
    );
}

#[tokio::test(start_paused = true)]
async fn an_unknown_root_rebuilds_the_thread_with_only_earlier_events() {
    let owner = keys("owner");
    let alice = keys("alice");
    let root = top_level(&owner, channel(), "root", 100);
    let earlier = reply(&alice, channel(), "earlier", &root, &root, 110);
    let current = reply(&owner, channel(), "current", &root, &earlier, 120);
    let later = reply(&alice, channel(), "later", &root, &root, 130);
    let mut h = harness(store());
    for event in [&root, &earlier, &current, &later] {
        h.relay_a.seed(event.clone());
    }
    let out = h
        .ingest
        .handle(&bot("A"), current.clone(), Source::Live)
        .await;
    assert_eq!(out.len(), 2, "{out:?}");
    match &out[0] {
        IngestOutput::RebuildThread { root: r, events } => {
            assert_eq!(r, &id(&root));
            let ids: Vec<_> = events.iter().map(|e| e.id).collect();
            assert_eq!(ids, vec![root.id, earlier.id]);
        }
        other => panic!("expected RebuildThread first, got {other:?}"),
    }
    let enriched = enriched(&out);
    assert_eq!(enriched.len(), 1);
    assert_eq!(enriched[0].event, current);
    assert_eq!(enriched[0].root, Some(id(&root)));
    assert_eq!(
        enriched[0].parent_author,
        Some(Pubkey::from_nostr(&alice.public_key()))
    );
}

#[tokio::test(start_paused = true)]
async fn a_rebuilt_root_is_not_fetched_again() {
    let owner = keys("owner");
    let root = top_level(&owner, channel(), "root", 100);
    let first = reply(&owner, channel(), "one", &root, &root, 110);
    let second = reply(&owner, channel(), "two", &root, &root, 120);
    let mut h = harness(store());
    h.relay_a.seed(root.clone());
    let out = h.ingest.handle(&bot("A"), first, Source::Live).await;
    assert_eq!(out.len(), 2, "{out:?}");
    let fetches = h.relay_a.queries().len();
    let out = h.ingest.handle(&bot("A"), second, Source::Live).await;
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(h.relay_a.queries().len(), fetches);
}

#[tokio::test(start_paused = true)]
async fn a_failed_thread_fetch_still_forwards_the_event() {
    let owner = keys("owner");
    let root = top_level(&owner, channel(), "root", 100);
    let current = reply(&owner, channel(), "current", &root, &root, 120);
    let mut h = harness(store());
    h.relay_a.fail_queries(true);
    let out = h
        .ingest
        .handle(&bot("A"), current.clone(), Source::Live)
        .await;
    let enriched = only_enriched(&out);
    assert_eq!(enriched.event, current);
    assert_eq!(enriched.root, Some(id(&root)));
}

#[tokio::test(start_paused = true)]
async fn the_parent_author_comes_from_the_store() {
    let store = store();
    let owner = keys("owner");
    let alice = keys("alice");
    let root = top_level(&owner, channel(), "root", 100);
    let parent = reply(&alice, channel(), "parent", &root, &root, 110);
    seed_thread(&store, &root);
    seed_event(&store, &parent, &root, true);
    let mut h = harness(store);
    let current = reply(&owner, channel(), "current", &root, &parent, 120);
    let out = h.ingest.handle(&bot("A"), current, Source::Live).await;
    let enriched = only_enriched(&out);
    assert_eq!(
        enriched.parent_author,
        Some(Pubkey::from_nostr(&alice.public_key()))
    );
    assert!(h.relay_a.queries().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_parent_author_comes_from_the_relay_when_unknown() {
    let store = store();
    let owner = keys("owner");
    let alice = keys("alice");
    let root = top_level(&owner, channel(), "root", 100);
    let parent = reply(&alice, channel(), "parent", &root, &root, 110);
    seed_thread(&store, &root);
    let mut h = harness(store);
    h.relay_a.seed(parent.clone());
    let current = reply(&owner, channel(), "current", &root, &parent, 120);
    let out = h.ingest.handle(&bot("A"), current, Source::Live).await;
    let enriched = only_enriched(&out);
    assert_eq!(
        enriched.parent_author,
        Some(Pubkey::from_nostr(&alice.public_key()))
    );
    let queries = h.relay_a.queries();
    assert_eq!(queries.len(), 1, "{queries:?}");
    assert_eq!(query_ids(&queries[0]), vec![parent.id.to_hex()]);
}

#[tokio::test(start_paused = true)]
async fn the_parent_author_comes_from_an_earlier_forwarded_event() {
    let store = store();
    let owner = keys("owner");
    let alice = keys("alice");
    let root = top_level(&owner, channel(), "root", 100);
    let parent = reply(&alice, channel(), "parent", &root, &root, 110);
    seed_thread(&store, &root);
    let mut h = harness(store);
    h.ingest
        .handle(&bot("A"), parent.clone(), Source::Live)
        .await;
    let current = reply(&owner, channel(), "current", &root, &parent, 120);
    let out = h.ingest.handle(&bot("A"), current, Source::Live).await;
    let enriched = only_enriched(&out);
    assert_eq!(
        enriched.parent_author,
        Some(Pubkey::from_nostr(&alice.public_key()))
    );
    assert!(h.relay_a.queries().is_empty());
}
