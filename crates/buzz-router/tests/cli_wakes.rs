//! Task 4.3: `wakes [--bot N] [--state S]` reads SQLite directly, so it works while the daemon is
//! down (design 12.1, R53.4).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use buzz_router::store::wakes::{WakeRow, WakeState};
use buzz_router::store::{Store, FILE_NAME};
use router_core::ids::{BotName, EventId};
use serde_json::Value;
use support::daemon::Sandbox;
use support::roster_toml;
use uuid::Uuid;

fn wake(bot: &str, state: WakeState, created_at: i64) -> WakeRow {
    let root = EventId::from_hex(&"ab".repeat(32)).unwrap();
    WakeRow {
        id: Uuid::new_v4(),
        bot: BotName::new(bot).unwrap(),
        root_id: root.clone(),
        round_id: root,
        reason: "mention".to_owned(),
        priority: "owner".to_owned(),
        triggers: "[]".to_owned(),
        state,
        token_hash: None,
        attempt: 1,
        created_at,
        dispatch_after: created_at,
        started_at: None,
        deadline: None,
        ended_at: None,
        outcome: None,
    }
}

/// A sandbox whose database holds A queued (twice), A running and B queued, with no daemon.
fn seeded() -> (Sandbox, Vec<WakeRow>) {
    let sandbox = Sandbox::new(&roster_toml(""), "ws://127.0.0.1:9");
    std::fs::create_dir_all(&sandbox.data).unwrap();
    let store = Store::open(&sandbox.data.join(FILE_NAME)).unwrap();
    let rows = vec![
        wake("A", WakeState::Queued, 1_000),
        wake("A", WakeState::Running, 2_000),
        wake("B", WakeState::Queued, 3_000),
        wake("A", WakeState::Queued, 4_000),
    ];
    for row in &rows {
        store.wakes().insert(row).unwrap();
    }
    (sandbox, rows)
}

fn lines(output: &std::process::Output) -> Vec<Value> {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn wakes_lists_a_bots_rows_in_a_state_while_the_daemon_is_down() {
    let (sandbox, rows) = seeded();

    let listed = lines(
        &sandbox
            .cli(&["wakes", "--bot", "A", "--state", "queued"])
            .await,
    );

    let ids: Vec<&str> = listed
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [rows[0].id.to_string(), rows[3].id.to_string()]);
    assert_eq!(listed[0]["bot"], "A");
    assert_eq!(listed[0]["state"], "queued");
    assert_eq!(listed[0]["reason"], "mention");
    assert_eq!(listed[0]["created_at"], 1_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn wakes_without_filters_lists_every_row_oldest_first() {
    let (sandbox, rows) = seeded();

    let listed = lines(&sandbox.cli(&["wakes"]).await);

    let ids: Vec<String> = listed
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    let expected: Vec<String> = rows.iter().map(|row| row.id.to_string()).collect();
    assert_eq!(ids, expected);
    assert!(listed.iter().all(|row| row.get("token_hash").is_none()));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_state_is_bad_input() {
    let (sandbox, _) = seeded();

    let output = sandbox.cli(&["wakes", "--state", "sleeping"]).await;

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    let line: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(line["error"], "user_error");
}
