//! Task 3.4: the wake deadline (design 6.6, R27.1, R27.2, R65.5).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::time::Duration;

use buzz_router::core::{ApiFailure, ApiRequest, ApiResponse};
use buzz_router::ingest::Source;
use buzz_router::store::wakes::WakeState;
use router_core::ids::BotName;
use support::{
    base_secs, channel, emoji, keys, spawn_test_core_with, top_level, FakeAdapter, Step,
    TestCoreOptions,
};

#[tokio::test(start_paused = true)]
async fn at_the_deadline_the_adapter_is_cancelled_the_wake_times_out_and_gets_an_hourglass() {
    let adapter = FakeAdapter::new(vec![Step::Hang]);
    let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        limits: "max_wake_minutes = 1".to_owned(),
        ..TestCoreOptions::default()
    });

    let ask = top_level(&keys("owner"), channel(), "@A hi", base_secs());
    core.ingest(BotName::new("A").unwrap(), ask.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(59)).await;
    core.flush().await;

    let dispatch = adapter.dispatches().pop().unwrap();
    let wake = store.wakes().get(&dispatch.wake_id).unwrap().unwrap();
    assert_eq!(wake.state, WakeState::Running);
    assert!(relay.reactions(emoji::HOURGLASS).is_empty());

    tokio::time::sleep(Duration::from_secs(2)).await;
    core.flush().await;

    let wake = store.wakes().get(&dispatch.wake_id).unwrap().unwrap();
    assert_eq!(wake.state, WakeState::Timeout);
    assert!(wake.ended_at.is_some());
    assert_eq!(adapter.cancelled(), vec![dispatch.wake_id]);
    assert_eq!(relay.reactions(emoji::HOURGLASS), vec![ask.id]);

    let typing = relay.typing().len();
    tokio::time::sleep(Duration::from_secs(10)).await;
    core.flush().await;
    assert_eq!(relay.typing().len(), typing, "typing stops at the deadline");

    let late = core
        .api(ApiRequest::Post {
            token: dispatch.token.clone(),
            text: "too late".to_owned(),
        })
        .await;
    assert_eq!(late, ApiResponse::Failed(ApiFailure::WakeEnded));
    assert!(relay.messages("reply").is_empty(), "nothing is published");
}
