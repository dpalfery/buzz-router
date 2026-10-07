//! E6: unmanaged-post detection (task 7.4, requirement R66.7, design
//! §16.4).
//!
//! Publishing directly with bot A's key gets ⚠️ on that post, and `status`
//! shows `unmanaged_posts: 1`. Ignored by default, and fails without `BUZZ_E2E=1`. Run with:
//!
//! ```sh
//! BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=ws://127.0.0.1:3000 \
//!   cargo test -p buzz-router --test e2e_unmanaged -- --ignored --test-threads=1
//! ```

mod e2e_support;
mod support;

use e2e_support::{E2e, WARNING};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn direct_bot_post_gets_a_warning_and_is_counted() {
    let mut e2e = E2e::start("e2e-unmanaged", 0).await;

    e2e.wait_all_connected().await;

    let direct = e2e.bot_post_direct("A", "posting without the router").await;
    e2e.wait_reaction("A", &direct, WARNING).await;

    let status = e2e.status().await;
    assert_eq!(
        status["unmanaged_posts"], 1,
        "status counts the unmanaged post: {status}"
    );

    e2e.stop().await;
}
