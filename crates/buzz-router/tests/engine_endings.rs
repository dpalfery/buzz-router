//! Task 3.4: how wakes end and the reactions they leave (design 6.6 endings, R36, R45).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::time::Duration;

use buzz_router::core::{ApiResponse, CoreHandle};
use buzz_router::ingest::Source;
use buzz_router::store::wakes::{WakeRow, WakeState};
use buzz_router::store::Store;
use nostr::{Event, Kind};
use router_core::ids::BotName;
use support::{
    base_secs, channel, emoji, keys, router_marker, spawn_test_core_with, top_level, FakeAdapter,
    FakeRelay, Step, TestCoreOptions,
};

struct Run {
    _core: CoreHandle,
    relay: FakeRelay,
    store: Store,
    adapter: FakeAdapter,
    event: Event,
}

/// The owner posts `text` at top level; each woken bot runs `script`; then a minute passes.
async fn run(text: &str, script: Vec<Step>) -> Run {
    let adapter = FakeAdapter::new(script);
    let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        max_concurrent: 1,
        ..TestCoreOptions::default()
    });
    let event = top_level(&keys("owner"), channel(), text, base_secs());
    core.ingest(BotName::new("A").unwrap(), event.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    core.flush().await;
    only_replies_and_status_notes(&relay);
    Run {
        _core: core,
        relay,
        store,
        adapter,
        event,
    }
}

impl Run {
    fn wakes(&self) -> Vec<WakeRow> {
        self.adapter
            .dispatches()
            .iter()
            .map(|d| self.store.wakes().get(&d.wake_id).unwrap().unwrap())
            .collect()
    }

    fn only_wake(&self) -> WakeRow {
        let wakes = self.wakes();
        assert_eq!(wakes.len(), 1, "{wakes:?}");
        wakes.into_iter().next().unwrap()
    }
}

/// R45.1: the only kind-9 events the router publishes are agent replies and status notes.
fn only_replies_and_status_notes(relay: &FakeRelay) {
    for event in relay.published() {
        if event.kind == Kind::Custom(9) {
            let marker = router_marker(&event);
            assert!(
                matches!(marker.as_deref(), Some("reply" | "status")),
                "an unexpected kind-9 event: {event:?}"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_post_followed_by_a_non_zero_exit_is_posted() {
    let run = run(
        "@A hi",
        vec![Step::Post("answer".to_owned()), Step::Exit(3)],
    )
    .await;

    let wake = run.only_wake();
    assert_eq!(wake.state, WakeState::Posted);
    let replies = run.relay.messages("reply");
    assert_eq!(replies.len(), 1);
    let outcome: serde_json::Value =
        serde_json::from_str(wake.outcome.as_deref().unwrap()).unwrap();
    assert_eq!(
        outcome["posted"],
        serde_json::json!([replies[0].id.to_hex()])
    );
    assert!(run.relay.reactions(emoji::WARNING).is_empty());
    assert!(run.relay.reactions(emoji::CHECK).is_empty());
    let responses = run.adapter.api_responses();
    assert_eq!(
        responses[0].1,
        ApiResponse::Posted {
            event_id: replies[0].id.to_hex()
        }
    );
    assert!(run
        .store
        .posts()
        .exists(&router_core::ids::EventId::from_nostr(&replies[0].id))
        .unwrap());
}

#[tokio::test(start_paused = true)]
async fn stdout_text_is_posted_as_a_reply_under_the_reaction_target() {
    let run = run("@A hi", vec![Step::Stdout("hello there".to_owned())]).await;

    assert_eq!(run.only_wake().state, WakeState::Posted);
    let replies = run.relay.messages("reply");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].content, "hello there");
    assert_eq!(replies[0].pubkey, keys("A").public_key());
    assert_eq!(replies[0].tags.event_ids().next(), Some(&run.event.id));
}

#[tokio::test(start_paused = true)]
async fn an_api_pass_on_an_owner_direct_wake_reacts_check() {
    let run = run("@A hi", vec![Step::Pass, Step::Exit(0)]).await;

    assert_eq!(run.only_wake().state, WakeState::Passed);
    assert_eq!(run.relay.reactions(emoji::CHECK), vec![run.event.id]);
    assert!(run.relay.messages("reply").is_empty());
}

#[tokio::test(start_paused = true)]
async fn no_reply_or_empty_stdout_is_a_pass() {
    for text in ["[no-reply]", "", "  \n"] {
        let run = run("@A hi", vec![Step::Stdout(text.to_owned())]).await;
        assert_eq!(run.only_wake().state, WakeState::Passed, "{text:?}");
        assert_eq!(run.relay.reactions(emoji::CHECK), vec![run.event.id]);
        assert!(run.relay.messages("reply").is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn a_pass_on_an_owner_discussion_wake_has_no_check() {
    let run = run("@everyone thoughts?", vec![Step::Pass, Step::Exit(0)]).await;

    let wakes = run.wakes();
    assert_eq!(wakes.len(), 3);
    assert!(wakes.iter().all(|wake| wake.state == WakeState::Passed));
    assert!(run.relay.reactions(emoji::CHECK).is_empty());
    assert_eq!(run.relay.reactions(emoji::EYES).len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_non_zero_exit_without_a_post_fails_with_a_warning() {
    let run = run("@A hi", vec![Step::Exit(3)]).await;

    assert_eq!(run.only_wake().state, WakeState::Failed);
    assert_eq!(run.relay.reactions(emoji::WARNING), vec![run.event.id]);
    assert!(run.relay.reactions(emoji::CHECK).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_non_zero_exit_with_stdout_fails_and_posts_nothing() {
    let run = run("@A hi", vec![Step::ExitWith(3, "partial".to_owned())]).await;

    assert_eq!(run.only_wake().state, WakeState::Failed);
    assert!(run.relay.messages("reply").is_empty());
    assert_eq!(run.relay.reactions(emoji::WARNING), vec![run.event.id]);
}

#[tokio::test(start_paused = true)]
async fn timeout_takes_precedence_over_the_cancelled_runs_failure() {
    let adapter = FakeAdapter::new(vec![Step::Hang]);
    let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        limits: "max_wake_minutes = 1".to_owned(),
        ..TestCoreOptions::default()
    });
    let event = top_level(&keys("owner"), channel(), "@A hi", base_secs());
    core.ingest(BotName::new("A").unwrap(), event.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(120)).await;
    core.flush().await;

    let wake = store
        .wakes()
        .get(&adapter.dispatches()[0].wake_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        wake.state,
        WakeState::Timeout,
        "timeout takes precedence over the cancelled run's failure"
    );
    assert!(relay.reactions(emoji::WARNING).is_empty());
    only_replies_and_status_notes(&relay);
}
