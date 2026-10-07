//! E4 (task 7.2, requirement R66.5, brief §15.3): O posts "@A" with a 60 s
//! task. 👀 appears immediately, one status note appears at about 20 s, and
//! the final reply is threaded under O's message.
//!
//! Ignored by default, and fails without `BUZZ_E2E=1`. Run with:
//!
//! ```sh
//! BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=ws://127.0.0.1:3000 \
//!   cargo test -p buzz-router --test e2e_status_note -- --ignored --test-threads=1
//! ```

mod e2e_support;
mod support;

use std::time::{Duration, Instant};

use e2e_support::{E2e, EYES};

/// How long the test agent works on the task.
const TASK_SECS: u64 = 60;

/// The `e` tag of `event` with `marker` (`root` or `reply`), as a hex id.
fn e_tag(event: &nostr::Event, marker: &str) -> Option<String> {
    event.tags.iter().find_map(|tag| {
        let parts = tag.as_slice();
        (parts.first().map(String::as_str) == Some("e")
            && parts.get(3).map(String::as_str) == Some(marker))
        .then(|| parts.get(1).cloned())
        .flatten()
    })
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn e4_long_task_gets_eyes_one_status_note_and_a_threaded_reply() {
    let mut e2e = E2e::start("e2e-e4", TASK_SECS).await;
    e2e.wait_all_connected().await;

    let task = e2e.owner_post("@A run the 60 second task").await;
    let posted = Instant::now();

    e2e.wait_for(
        "👀 from A immediately",
        Duration::from_secs(5),
        async || {
            let author = e2e.bot("A").public_key();
            e2e.reactions(&task)
                .await
                .iter()
                .any(|reaction| reaction.pubkey == author && reaction.content == EYES)
                .then_some(())
        },
    )
    .await;

    let notes = e2e
        .wait_for("A's status note", Duration::from_secs(35), async || {
            let notes = e2e.status_notes("A", &task).await;
            (!notes.is_empty()).then_some(notes)
        })
        .await;
    let note_after = notes[0]
        .created_at
        .as_secs()
        .saturating_sub(task.created_at.as_secs());
    assert!(
        (17..=26).contains(&note_after),
        "the status note comes at about 20 s, got {note_after} s"
    );
    assert!(
        posted.elapsed() < Duration::from_secs(TASK_SECS),
        "the status note comes before the task finishes"
    );

    let replies = e2e
        .wait_replies_within("A", &task, 1, Duration::from_secs(TASK_SECS + 30))
        .await;
    assert_eq!(replies.len(), 1, "one final reply");
    let reply = &replies[0];
    assert_eq!(
        e_tag(reply, "reply"),
        Some(task.id.to_hex()),
        "the reply is threaded directly under O's message"
    );
    assert!(
        reply.created_at.as_secs() >= task.created_at.as_secs() + TASK_SECS,
        "the final reply comes after the task"
    );

    assert_eq!(
        e2e.status_notes("A", &task).await.len(),
        1,
        "exactly one status note"
    );

    e2e.stop().await;
}
