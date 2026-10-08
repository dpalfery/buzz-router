//! A graceful shutdown ends each running wake and kills its agent (design 6.9, review #7).
//!
//! Real processes and real time: the command adapter runs `buzz-router-test-agent
//! spawn-grandchild <pidfile>`, which writes its own pid and its child's.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use buzz_router::adapter::command::CommandAdapter;
use buzz_router::ingest::Source;
use buzz_router::store::wakes::WakeState;
use router_core::ids::BotName;
use support::{
    base_secs, channel, keys, pid_alive, spawn_test_core_with, test_agent_path, top_level,
    TestCoreOptions,
};

fn adapter_toml(pidfile: &Path, cwd: &Path) -> String {
    format!(
        "type = \"command\"\ncommand = ['{}', 'spawn-grandchild', '{}']\ncwd = '{}'\nenv = {{}}\nprompt_mode = \"stdin\"\nreply_mode = \"api\"\nprompt_template = \"\"\n",
        test_agent_path().display(),
        pidfile.display(),
        cwd.display()
    )
}

/// Polls `probe` every 20 ms until it returns a value, for at most `limit`.
async fn within<T>(limit: Duration, what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(started.elapsed() < limit, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_kills_running_agents_and_ends_their_wakes() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let pidfile = dir.path().join("pids");
    let adapter = CommandAdapter::new(data_dir.clone(), "http://127.0.0.1:9".to_owned());
    let (core, _relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter_toml: adapter_toml(&pidfile, dir.path()),
        real_adapter: Some(Arc::new(adapter)),
        ..TestCoreOptions::default()
    });

    core.ingest(
        BotName::new("A").unwrap(),
        top_level(&keys("owner"), channel(), "@A work", base_secs()),
        Source::Live,
    );
    let pids: Vec<u32> = within(Duration::from_secs(30), "the pid file", || {
        let text = std::fs::read_to_string(&pidfile).ok()?;
        let pids: Vec<u32> = text.lines().filter_map(|line| line.parse().ok()).collect();
        (pids.len() == 2).then_some(pids)
    })
    .await;
    assert!(pids.iter().all(|pid| pid_alive(*pid)), "{pids:?}");
    let wake_id: String = store
        .connection()
        .query_row("SELECT id FROM wakes WHERE state = 'running'", [], |row| {
            row.get(0)
        })
        .unwrap();

    let started = Instant::now();
    core.shutdown().await;
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "shutdown took {:?}",
        started.elapsed()
    );

    within(Duration::from_secs(5), "both processes to die", || {
        pids.iter().all(|pid| !pid_alive(*pid)).then_some(())
    })
    .await;
    let wake = store
        .wakes()
        .get(&uuid::Uuid::parse_str(&wake_id).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(wake.state, WakeState::Interrupted);
    assert!(store
        .wakes()
        .with_state(WakeState::Running)
        .unwrap()
        .is_empty());
    let queued = store.wakes().queued().unwrap();
    assert_eq!(
        queued.len(),
        1,
        "an owner wake is re-queued as at a restart"
    );
    assert_eq!(queued[0].attempt, 2);
}
