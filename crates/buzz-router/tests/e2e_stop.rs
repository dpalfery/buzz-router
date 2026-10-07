//! E3: stop during a busy thread (task 7.3, requirement R66.4, design
//! §16.4).
//!
//! While every bot has a running wake, the owner posts `stop`: every agent
//! process is gone within 5 s, each bot reacts 🛑, nothing is published
//! afterwards, the halt survives a router restart, and `resume` brings back
//! ▶️ and normal routing. Skipped unless `BUZZ_E2E=1`. Run with:
//!
//! ```sh
//! BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=ws://127.0.0.1:3000 \
//!   cargo test -p buzz-router --test e2e_stop -- --test-threads=1
//! ```

mod e2e_support;
mod support;

use std::time::Duration;

use e2e_support::{E2e, BOTS, EYES, RESUME, STOP};

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
async fn stop_kills_every_wake_and_the_halt_survives_a_restart() {
    let Some(mut e2e) = E2e::start("e2e-stop", 30).await else {
        return;
    };

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

    // Nothing is published afterwards: the killed agents never post.
    tokio::time::sleep(Duration::from_secs(10)).await;
    for name in BOTS {
        assert!(
            e2e.replies(name, &busy).await.is_empty(),
            "{name} publishes nothing after the stop"
        );
    }

    // The halt survives a router restart.
    e2e.restart().await;
    e2e.wait_all_connected().await;
    let halted = e2e.owner_post("@A are you there").await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(
        e2e.replies("A", &halted).await.is_empty(),
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
    let replies = e2e.wait_replies("A", &ping, 1).await;
    assert_eq!(replies.len(), 1, "A routes normally after resume");

    e2e.stop().await;
}
