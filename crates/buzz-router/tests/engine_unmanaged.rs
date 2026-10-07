//! Task 4.2: unmanaged-post detection (design 6.3 step 2, R44.7, R51, R65.9).
//!
//! A kind-9 event signed by a local bot's key whose id isn't in `posts` was published by
//! something other than the router. The bot reacts ⚠️ on it and the in-memory counter grows;
//! the warning (reaction plus log line) fires at most once per bot per hour.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::time::Duration;

use buzz_router::ingest::Source;
use buzz_router::store::posts::PostRow;
use nostr::Event;
use router_core::ids::{BotName, EventId};
use support::{base_secs, channel, keys, spawn_test_core, top_level};

/// The warning reaction (R51.1).
const WARNING: &str = "\u{26A0}\u{FE0F}";

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

fn id(event: &Event) -> EventId {
    EventId::from_nostr(&event.id)
}

/// The ⚠️ reactions `name` has published so far.
fn warnings_by(published: &[Event], name: &str) -> Vec<Event> {
    let author = keys(name).public_key();
    published
        .iter()
        .filter(|event| {
            event.kind.as_u16() == 7 && event.pubkey == author && event.content == WARNING
        })
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
async fn a_bot_signed_post_not_in_posts_gets_a_warning() {
    let (core, relay, _store) = spawn_test_core();
    let stray = top_level(&keys("A"), channel(), "a stray post", base_secs());
    core.ingest(bot("A"), stray.clone(), Source::Live);
    core.flush().await;

    let warnings = warnings_by(&relay.published(), "A");
    assert_eq!(warnings.len(), 1, "{:?}", relay.published());
    assert_eq!(e_tags(&warnings[0]), vec![stray.id.to_hex()]);
    warnings[0].verify().unwrap();
    assert_eq!(core.debug_counters().await.unmanaged_posts, 1);
}

#[tokio::test(start_paused = true)]
async fn a_router_published_echo_is_not_flagged() {
    let (core, relay, store) = spawn_test_core();
    // The router writes the `posts` row before sending (DD-6), so when the relay echoes the
    // reply back the id is already known.
    let reply = top_level(&keys("A"), channel(), "the router sent this", base_secs());
    store
        .posts()
        .insert(&PostRow {
            event_id: id(&reply),
            bot: bot("A"),
            wake_id: None,
            created_at: reply.created_at.as_secs() as i64,
        })
        .unwrap();
    core.ingest(bot("A"), reply, Source::Live);
    core.flush().await;

    assert!(warnings_by(&relay.published(), "A").is_empty());
    assert_eq!(core.debug_counters().await.unmanaged_posts, 0);
}

#[tokio::test(start_paused = true)]
async fn warnings_are_rate_limited_to_one_per_bot_per_hour() {
    let (core, relay, _store) = spawn_test_core();
    let first = top_level(&keys("A"), channel(), "stray one", base_secs());
    core.ingest(bot("A"), first, Source::Live);
    core.flush().await;
    assert_eq!(warnings_by(&relay.published(), "A").len(), 1);

    // Ten minutes later: counted, but no second warning.
    tokio::time::sleep(Duration::from_secs(600)).await;
    let second = top_level(&keys("A"), channel(), "stray two", base_secs() + 600);
    core.ingest(bot("A"), second, Source::Live);
    core.flush().await;
    assert_eq!(warnings_by(&relay.published(), "A").len(), 1);
    assert_eq!(core.debug_counters().await.unmanaged_posts, 2);

    // Sixty-one minutes after that: the warning fires again.
    tokio::time::sleep(Duration::from_secs(3_660)).await;
    let third = top_level(&keys("A"), channel(), "stray three", base_secs() + 4_260);
    core.ingest(bot("A"), third.clone(), Source::Live);
    core.flush().await;
    let warnings = warnings_by(&relay.published(), "A");
    assert_eq!(warnings.len(), 2);
    assert_eq!(e_tags(&warnings[1]), vec![third.id.to_hex()]);
    assert_eq!(core.debug_counters().await.unmanaged_posts, 3);
}

#[tokio::test(start_paused = true)]
async fn a_bot_edit_is_not_flagged() {
    let (core, relay, _store) = spawn_test_core();
    let owner = keys("owner");
    let target = top_level(&owner, channel(), "just a note", base_secs());
    relay.seed(target.clone());
    let edit = support::edit(&keys("A"), channel(), &target, "bot edit", base_secs() + 1);
    core.ingest(bot("A"), edit, Source::Live);
    core.flush().await;

    assert!(warnings_by(&relay.published(), "A").is_empty());
    assert_eq!(core.debug_counters().await.unmanaged_posts, 0);
}
