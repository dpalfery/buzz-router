//! Task 3.6: posts per wake (R28, R65.7).

mod support;

use buzz_router::core::{ApiFailure, ApiResponse};
use buzz_router::ingest::Source;
use buzz_router::store::wakes::WakeState;
use router_core::ids::BotName;
use support::{
    base_secs, channel, keys, spawn_test_core_with, top_level, FakeAdapter, Step, TestCoreOptions,
};

#[tokio::test(start_paused = true)]
async fn with_the_default_limit_the_fourth_post_gets_429_and_the_wake_goes_on() {
    let adapter = FakeAdapter::new(vec![
        Step::Post("1".to_owned()),
        Step::Post("2".to_owned()),
        Step::Post("3".to_owned()),
        Step::Post("4".to_owned()),
        Step::Pass,
        Step::Exit(0),
    ]);
    let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });

    let event = top_level(&keys("owner"), channel(), "@A hi", base_secs());
    core.ingest(BotName::new("A").unwrap(), event, Source::Live);
    core.flush().await;
    core.flush().await;

    let responses: Vec<ApiResponse> = adapter
        .api_responses()
        .into_iter()
        .map(|(_, response)| response)
        .collect();
    assert_eq!(responses.len(), 5, "{responses:?}");
    for response in &responses[..3] {
        assert!(
            matches!(response, ApiResponse::Posted { .. }),
            "{response:?}"
        );
    }
    assert_eq!(
        responses[3],
        ApiResponse::Failed(ApiFailure::TooManyPosts),
        "the 4th post"
    );
    assert_eq!(responses[4], ApiResponse::Done, "a pass after the 429");
    let replies = relay.messages("reply");
    assert_eq!(replies.len(), 3);
    assert!(replies.iter().all(|reply| reply.content != "4"));
    let wake = store
        .wakes()
        .get(&adapter.dispatches()[0].wake_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        wake.state,
        WakeState::Posted,
        "the 429 did not end the wake"
    );
}
