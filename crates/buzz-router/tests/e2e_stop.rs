//! E3: stop during a busy thread (task 7.3, requirement R66.4, design
//! §16.4).
//!
//! While every bot has a running wake, the owner posts `stop`: every agent
//! process is gone within 5 s, each bot reacts 🛑, nothing is published
//! afterwards, the halt survives a router restart, and `resume` brings back
//! ▶️ and normal routing. Ignored by default, and fails without `BUZZ_E2E=1`. Run with:
//!
//! ```sh
//! BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=ws://127.0.0.1:3000 \
//!   cargo test -p buzz-router --test e2e_stop -- --ignored --test-threads=1
//! ```

#![allow(
    clippy::expect_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod e2e_support;
mod support;

use std::time::Duration;

use e2e_support::{E2e, BOTS, EYES, RESUME, STOP};

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

/// Whether bot `name` has reacted to `target` with `emoji`.
async fn has_reaction(e2e: &E2e, name: &str, target: &nostr::Event, emoji: &str) -> bool {
    let author = e2e.bot(name).public_key();
    e2e.reactions(target)
        .await
        .iter()
        .any(|reaction| reaction.pubkey == author && reaction.content == emoji)
}

/// The agent pids the router currently tracks under `<data>/wakes/*/pid`.
fn agent_pids(e2e: &E2e) -> Vec<u32> {
    let mut pids = Vec::new();
    let wakes = e2e.data.join("wakes");
    let Ok(entries) = std::fs::read_dir(&wakes) else {
        return pids;
    };
    for entry in entries.flatten() {
        let pid_file = entry.path().join("pid");
        if let Ok(text) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = text.trim().parse() {
                pids.push(pid);
            }
        }
    }
    pids
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn stop_kills_every_wake_and_the_halt_survives_a_restart() {
    let mut e2e = E2e::start("e2e-stop", 30).await;

    e2e.wait_all_connected().await;

    // Busy thread: every bot has a running wake (30 s echo delay).
    let busy = e2e.owner_post("@everyone take your time").await;
    for name in BOTS {
        e2e.wait_reaction(name, &busy, EYES).await;
    }
    let pids = agent_pids(&e2e);
    assert_eq!(pids.len(), 3, "three agent processes are running: {pids:?}");
    assert!(
        pids.iter().all(|pid| support::pid_alive(*pid)),
        "the agents are alive before the stop"
    );

    let stop = e2e.owner_post("stop").await;

    // No agent process within 5 s.
    e2e.wait_for(
        "every agent process to die",
        Duration::from_secs(5),
        async || {
            pids.iter()
                .all(|pid| !support::pid_alive(*pid))
                .then_some(())
        },
    )
    .await;

    // 🛑 from each bot on the stop message.
    for name in BOTS {
        e2e.wait_reaction(name, &stop, STOP).await;
    }

    // Nothing is dispatched afterwards: no new wake row appears for any
    // bot. (The killed agents never post, so "no reply" would pass even if
    // the halt were broken.)
    let rows_before: Vec<usize> = {
        let mut counts = Vec::new();
        for name in BOTS {
            counts.push(wake_rows(&e2e, name).await);
        }
        counts
    };
    tokio::time::sleep(Duration::from_secs(10)).await;
    for (name, before) in BOTS.iter().zip(&rows_before) {
        assert_eq!(
            wake_rows(&e2e, name).await,
            *before,
            "{name} dispatches no new wake after the stop"
        );
    }

    // The halt survives a router restart.
    e2e.restart().await;
    e2e.wait_all_connected().await;
    let halted = e2e.owner_post("@A are you there").await;
    let halted_rows_before = wake_rows(&e2e, "A").await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    // A would react 👀 within 5 s of a wake start and gain a wake row, long
    // before its 30 s agent could reply: either proves the halt held.
    assert!(
        !has_reaction(&e2e, "A", &halted, EYES).await,
        "A starts no wake while halted"
    );
    assert_eq!(
        wake_rows(&e2e, "A").await,
        halted_rows_before,
        "A stays halted after the restart"
    );
    let status = e2e.status().await;
    assert!(
        status["halts"]
            .as_array()
            .expect("halts is a list")
            .contains(&serde_json::json!("all")),
        "the halt row survives the restart: {status}"
    );

    // Resume brings back ▶️ and normal routing.
    let resume = e2e.owner_post("resume").await;
    for name in BOTS {
        e2e.wait_reaction(name, &resume, RESUME).await;
    }
    let ping = e2e.owner_post("@A ping after resume").await;
    // The agent sleeps 30 s before replying, so allow 60 s.
    let replies = e2e
        .wait_replies_within("A", &ping, 1, Duration::from_secs(60))
        .await;
    assert_eq!(replies.len(), 1, "A routes normally after resume");

    e2e.stop().await;
}
