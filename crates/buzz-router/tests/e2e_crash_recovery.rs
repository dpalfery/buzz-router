//! E5: crash recovery (task 7.4, requirement R66.6, design §16.4).
//!
//! The router child is killed mid-wake, the owner posts while it is down,
//! then it restarts: the interrupted wake is re-run exactly once, the message
//! sent during the downtime is answered, and there are no duplicate replies.
//! Skipped unless `BUZZ_E2E=1`. Run with:
//!
//! ```sh
//! BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=ws://127.0.0.1:3000 \
//!   cargo test -p buzz-router --test e2e_crash_recovery -- --ignored --test-threads=1
//! ```

mod e2e_support;
mod support;

use std::time::Duration;

use e2e_support::{E2e, EYES};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn killed_mid_wake_reruns_once_and_answers_the_downtime_message() {
    let Some(mut e2e) = E2e::start("e2e-crash", 10).await else {
        return;
    };

    e2e.wait_all_connected().await;

    // A wake is running (10 s echo delay) when the router dies.
    let first = e2e.owner_post("@A work item one").await;
    e2e.wait_reaction("A", &first, EYES).await;
    e2e.kill().await;

    // Posted while the router is down.
    let second = e2e.owner_post("@A work item two").await;

    e2e.restart().await;
    e2e.wait_all_connected().await;

    // The interrupted wake is re-run, and the downtime message is answered.
    let first_replies = e2e.wait_replies("A", &first, 1).await;
    assert_eq!(
        first_replies.len(),
        1,
        "the interrupted wake is re-run once"
    );
    let second_replies = e2e.wait_replies("A", &second, 1).await;
    assert_eq!(second_replies.len(), 1, "the downtime message is answered");

    // No duplicate replies: one quiet window past the 20 s debounce.
    tokio::time::sleep(Duration::from_secs(25)).await;
    assert_eq!(
        e2e.replies("A", &first).await.len(),
        1,
        "no duplicate reply in the first thread"
    );
    assert_eq!(
        e2e.replies("A", &second).await.len(),
        1,
        "no duplicate reply in the second thread"
    );

    e2e.stop().await;
}
