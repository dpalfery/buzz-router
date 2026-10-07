//! End-to-end harness smoke test (task 7.1, requirement R66.1, design §16.4).
//!
//! The router runs as a child process against a local Buzz relay with three
//! throwaway bots. Skipped unless `BUZZ_E2E=1`. Run with:
//!
//! ```sh
//! BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=ws://127.0.0.1:3000 \
//!   cargo test -p buzz-router --test e2e_harness_smoke -- --ignored --test-threads=1
//! ```

mod e2e_support;
mod support;

use e2e_support::{E2e, EYES};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn router_connects_all_bots_and_answers_a_mention() {
    let Some(mut e2e) = E2e::start("e2e-smoke", 0).await else {
        return;
    };

    e2e.wait_all_connected().await;
    let status = e2e.status().await;
    let bots = status["bots"].as_array().expect("status lists the bots");
    assert_eq!(bots.len(), 3, "{status}");
    assert!(
        bots.iter().all(|bot| bot["connected"] == true),
        "every bot is connected: {status}"
    );

    let ping = e2e.owner_post("@A ping").await;
    e2e.wait_reaction("A", &ping, EYES).await;
    let replies = e2e.wait_replies("A", &ping, 1).await;
    assert_eq!(replies.len(), 1, "A replies once");

    e2e.stop().await;
}
