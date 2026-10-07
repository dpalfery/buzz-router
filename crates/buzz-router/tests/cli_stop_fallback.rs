//! Task 3.8: `stop` with no daemon writes the halts itself and kills running wakes' process trees
//! (design 6.7 CLI fallback, R33.3, R33.4).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]
#![allow(
    clippy::zombie_processes,
    reason = "spawn_tree hands its child to the test, which waits on it after the CLI kills it"
)]

mod support;

use std::path::Path;
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

use buzz_router::store::halts::HaltScope;
use buzz_router::store::wakes::{WakeRow, WakeState};
use buzz_router::store::Store;
use router_core::ids::{BotName, EventId};
use support::{pid_alive, roster_toml, router_toml, test_agent_path};
use uuid::Uuid;

/// A port nothing listens on.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Starts `spawn-grandchild` as the leader of its own process group, as the command adapter
/// does, and waits for both pids.
fn spawn_tree(pidfile: &Path) -> (Child, Vec<u32>) {
    let mut command = Command::new(test_agent_path());
    command.arg("spawn-grandchild").arg(pidfile);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn().unwrap();
    let started = Instant::now();
    loop {
        if let Ok(text) = std::fs::read_to_string(pidfile) {
            let pids: Vec<u32> = text.lines().filter_map(|line| line.parse().ok()).collect();
            if pids.len() == 2 {
                return (child, pids);
            }
        }
        assert!(started.elapsed() < Duration::from_secs(30), "no pid file");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn running_wake(bot: &str) -> WakeRow {
    let id = Uuid::new_v4();
    WakeRow {
        id,
        bot: BotName::new(bot).unwrap(),
        root_id: EventId::from_hex(&"ab".repeat(32)).unwrap(),
        round_id: EventId::from_hex(&"ab".repeat(32)).unwrap(),
        reason: "mention".to_owned(),
        priority: "owner".to_owned(),
        triggers: "[]".to_owned(),
        state: WakeState::Running,
        token_hash: Some(id.simple().to_string()),
        attempt: 1,
        created_at: 1,
        dispatch_after: 1,
        started_at: Some(1),
        deadline: Some(i64::MAX),
        ended_at: None,
        outcome: None,
    }
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_buzz-router"))
        .args(args)
        .env("BUZZ_ROUTER_CONFIG_DIR", dir.join("config"))
        .env("BUZZ_ROUTER_DATA_DIR", dir.join("data"))
        .output()
        .unwrap()
}

#[test]
fn stop_without_a_daemon_writes_the_halt_and_kills_the_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    let data = dir.path().join("data");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("roster.toml"), roster_toml("")).unwrap();
    std::fs::write(
        config.join("router.toml"),
        router_toml(1).replace("127.0.0.1:47821", &format!("127.0.0.1:{}", closed_port())),
    )
    .unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let store = Store::open(&data.join("state.sqlite3")).unwrap();
    let wake_a = running_wake("A");
    let wake_b = running_wake("B");
    store.wakes().insert(&wake_a).unwrap();
    store.wakes().insert(&wake_b).unwrap();

    let (mut child_a, pids_a) = spawn_tree(&dir.path().join("pids-a"));
    let (mut child_b, pids_b) = spawn_tree(&dir.path().join("pids-b"));
    for (wake, child) in [(&wake_a, &child_a), (&wake_b, &child_b)] {
        let wake_dir = data.join("wakes").join(wake.id.to_string());
        std::fs::create_dir_all(&wake_dir).unwrap();
        std::fs::write(wake_dir.join("pid"), format!("{}\n", child.id())).unwrap();
    }

    let output = run(dir.path(), &["stop", "--bot", "A"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let halts = store.halts().list().unwrap();
    assert_eq!(halts.len(), 1);
    assert_eq!(halts[0].scope, HaltScope::Bot(BotName::new("A").unwrap()));
    assert_eq!(halts[0].set_by_event.as_deref(), Some("cli"));

    let started = Instant::now();
    let _ = child_a.wait();
    while pids_a.iter().any(|pid| pid_alive(*pid)) {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "A's tree survived: {pids_a:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        pids_b.iter().all(|pid| pid_alive(*pid)),
        "B is out of scope: {pids_b:?}"
    );
    let _ = child_b.kill();
    let _ = child_b.wait();
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(pids_b[1]) {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pids_b[1].to_string(), "/T", "/F"])
            .output();
    }
}

#[test]
fn resume_without_a_daemon_is_a_network_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(config.join("roster.toml"), roster_toml("")).unwrap();
    std::fs::write(
        config.join("router.toml"),
        router_toml(1).replace("127.0.0.1:47821", &format!("127.0.0.1:{}", closed_port())),
    )
    .unwrap();

    let output = run(dir.path(), &["resume"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
}
