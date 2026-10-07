//! E2: `@everyone` wakes every bot (task 7.3, requirement R66.3, design
//! §16.4).
//!
//! The owner posts `@everyone`; every bot reacts 👀 within 5 s, no bot is
//! woken more than 4 times, and the thread goes quiet. Skipped unless
//! `BUZZ_E2E=1`. Run with:
//!
//! ```sh
//! BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=ws://127.0.0.1:3000 \
//!   cargo test -p buzz-router --test e2e_everyone -- --test-threads=1
//! ```

#![allow(
    clippy::expect_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod e2e_support;
mod support;

use std::time::Duration;

use e2e_support::{E2e, BOTS, EYES};

/// Every bot reacts 👀 within 5 s of the `@everyone` mention.
async fn wait_eyes_within_5s(e2e: &E2e, name: &str, target: &nostr::Event) {
    let author = e2e.bot(name).public_key();
    e2e.wait_for(
        &format!("{EYES} from {name} within 5 s"),
        Duration::from_secs(5),
        async || {
            e2e.reactions(target)
                .await
                .iter()
                .any(|reaction| reaction.pubkey == author && reaction.content == EYES)
                .then_some(())
        },
    )
    .await;
}

/// Whether any bot still has a running or queued wake in `status`.
fn backlog(status: &serde_json::Value) -> bool {
    status["bots"]
        .as_array()
        .expect("status lists the bots")
        .iter()
        .any(|bot| {
            !bot["running"]
                .as_array()
                .expect("running is a list")
                .is_empty()
                || bot["queued"].as_u64().expect("queued is a count") > 0
        })
}

/// How many wake rows bot `name` has in SQLite.
async fn wake_rows(e2e: &E2e, name: &str) -> usize {
    let output = e2e.cli(&["wakes", "--bot", name]).await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    String::from_utf8(output.stdout)
        .expect("wakes prints UTF-8")
        .lines()
        .filter(|line| !line.is_empty())
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn everyone_wakes_every_bot_and_the_thread_goes_quiet() {
    let Some(mut e2e) = E2e::start("e2e-everyone", 0).await else {
        return;
    };

    e2e.wait_all_connected().await;

    let root = e2e.owner_post("@everyone status please").await;
    for name in BOTS {
        wait_eyes_within_5s(&e2e, name, &root).await;
    }

    // Every bot answers at least once.
    for name in BOTS {
        let replies = e2e.wait_replies(name, &root, 1).await;
        assert!(!replies.is_empty(), "{name} replies to @everyone");
    }

    // Wait until no wake is running or queued anywhere.
    e2e.wait_for(
        "every wake to finish",
        Duration::from_secs(300),
        async || (!backlog(&e2e.try_status().await?)).then_some(()),
    )
    .await;

    // One quiet window past the 20 s discussion debounce: nothing new.
    let before: Vec<usize> = {
        let mut counts = Vec::new();
        for name in BOTS {
            counts.push(e2e.replies(name, &root).await.len());
        }
        counts
    };
    tokio::time::sleep(Duration::from_secs(25)).await;
    for (name, count) in BOTS.iter().zip(&before) {
        assert_eq!(
            e2e.replies(name, &root).await.len(),
            *count,
            "{name} stays quiet"
        );
    }

    // No bot was woken more than 4 times.
    for name in BOTS {
        let rows = wake_rows(&e2e, name).await;
        assert!(
            (1..=4).contains(&rows),
            "{name} has {rows} wake rows, expected 1-4"
        );
    }

    e2e.stop().await;
}
