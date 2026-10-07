//! E1, the original bug (task 7.2, requirement R66.2, brief §15.3): O posts
//! "@A", A replies, then O replies untagged in the thread panel. A is woken
//! and replies.
//!
//! Skipped unless `BUZZ_E2E=1`. Run with:
//!
//! ```sh
//! BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=ws://127.0.0.1:3000 \
//!   cargo test -p buzz-router --test e2e_thread_reply -- --ignored --test-threads=1
//! ```

mod e2e_support;
mod support;

use e2e_support::{E2e, EYES};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn e1_untagged_owner_reply_in_the_thread_wakes_a() {
    let Some(mut e2e) = E2e::start("e2e-e1", 0).await else {
        return;
    };
    e2e.wait_all_connected().await;

    let root = e2e.owner_post("@A what is the plan?").await;
    let first = e2e.wait_replies("A", &root, 1).await;
    assert_eq!(first.len(), 1, "A replies once to the mention");

    // The thread panel replies to the thread itself, with no tag for A.
    let untagged = e2e
        .owner_reply("and what about tomorrow?", &root, &root)
        .await;
    assert!(
        !untagged.content.contains("@A") && untagged.tags.public_keys().next().is_none(),
        "the follow-up names no bot"
    );

    e2e.wait_reaction("A", &untagged, EYES).await;
    let replies = e2e.wait_replies("A", &root, 2).await;
    assert_eq!(replies.len(), 2, "A replies to the untagged follow-up");

    e2e.stop().await;
}
