//! Task 3.4: the status note (design 6.6 timers, R46, R65.6).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::time::Duration;

use buzz_router::core::{ApiResponse, CoreHandle};
use buzz_router::ingest::Source;
use router_core::ids::BotName;
use support::{
    base_secs, channel, keys, spawn_test_core_with, top_level, FakeAdapter, FakeRelay, Step,
    TestCoreOptions,
};

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

struct Asked {
    _core: CoreHandle,
    relay: FakeRelay,
    event: nostr::Event,
    adapter: FakeAdapter,
}

/// The owner posts `text` at top level, and every woken bot runs `script`.
async fn ask(text: &str, script: Vec<Step>) -> Asked {
    let adapter = FakeAdapter::new(script);
    let (core, relay, _store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });
    let event = top_level(&keys("owner"), channel(), text, base_secs());
    core.ingest(bot("A"), event.clone(), Source::Live);
    core.flush().await;
    Asked {
        _core: core,
        relay,
        event,
        adapter,
    }
}

fn e_tags(event: &nostr::Event) -> Vec<Vec<String>> {
    event
        .tags
        .iter()
        .map(|tag| tag.as_slice().to_vec())
        .filter(|tag| tag.first().map(String::as_str) == Some("e"))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn an_owner_direct_wake_gets_one_status_note_at_twenty_seconds() {
    let Asked { relay, event, .. } = ask(
        "@A hi",
        vec![Step::Wait(Duration::from_secs(60)), Step::Exit(0)],
    )
    .await;

    tokio::time::sleep(Duration::from_millis(19_900)).await;
    assert!(relay.messages("status").is_empty(), "not before 20 s");

    tokio::time::sleep(Duration::from_millis(200)).await;
    let notes = relay.messages("status");
    assert_eq!(notes.len(), 1, "at 20 s");
    assert_eq!(notes[0].content, "On it, this will take a bit.");
    assert_eq!(notes[0].pubkey, keys("A").public_key());
    assert_eq!(
        e_tags(&notes[0]),
        vec![vec![
            "e".to_owned(),
            event.id.to_hex(),
            String::new(),
            "reply".to_owned()
        ]],
        "threaded under the reaction target"
    );

    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(relay.messages("status").len(), 1, "exactly once");
}

#[tokio::test(start_paused = true)]
async fn a_discussion_wake_never_gets_a_status_note() {
    let Asked { relay, adapter, .. } = ask(
        "@everyone thoughts?",
        vec![Step::Wait(Duration::from_secs(60)), Step::Exit(0)],
    )
    .await;

    tokio::time::sleep(Duration::from_secs(120)).await;
    assert!(!adapter.dispatches().is_empty());
    assert!(relay.messages("status").is_empty());
}

#[tokio::test(start_paused = true)]
async fn no_status_note_when_the_agent_posts_at_nineteen_seconds() {
    let Asked { relay, adapter, .. } = ask(
        "@A hi",
        vec![
            Step::Wait(Duration::from_secs(19)),
            Step::Post("done".to_owned()),
            Step::Wait(Duration::from_secs(30)),
            Step::Exit(0),
        ],
    )
    .await;

    tokio::time::sleep(Duration::from_secs(60)).await;
    let responses = adapter.api_responses();
    assert!(
        matches!(responses[0].1, ApiResponse::Posted { .. }),
        "{responses:?}"
    );
    assert_eq!(relay.messages("reply").len(), 1);
    assert!(relay.messages("status").is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_status_note_uses_the_eta() {
    let Asked { relay, .. } = ask(
        "@A hi",
        vec![
            Step::Wait(Duration::from_secs(5)),
            Step::Eta("10 minutes".to_owned()),
            Step::Wait(Duration::from_secs(60)),
            Step::Exit(0),
        ],
    )
    .await;

    tokio::time::sleep(Duration::from_secs(21)).await;
    let notes = relay.messages("status");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].content, "On it, about 10 minutes.");
}
