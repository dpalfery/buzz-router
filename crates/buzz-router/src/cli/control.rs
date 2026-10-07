//! `stop`, `resume` and `cancel` (design 6.7 and 12.1, R33).
//!
//! Each calls the daemon's admin API with the token in `<data-dir>/admin.token`. When `stop`
//! can't reach the daemon (connection refused, a 2 s timeout, or no admin token), it falls back
//! to doing the work itself: it writes the halt rows to SQLite with `set_by_event = "cli"` and
//! kills the process tree of every in-scope running wake from its `wakes/<id>/pid` file. A
//! running daemon honours those rows, because it re-reads `halts` for every decision.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use router_core::config::{parse_router, Roster, RouterConfig};
use router_core::ids::BotName;
use router_core::route::{Control, Scope};
use serde_json::{json, Value};

use super::agent::{call, print_json, CallError};
use super::roster::{load as load_roster, roster_path};
use super::CliError;
use crate::api::admin_token;
use crate::core::control::{write_halts, SET_BY_CLI};
use crate::paths::Dirs;
use crate::store::wakes::{WakeRow, WakeState};
use crate::store::{Store, StoreError, FILE_NAME as STORE_FILE};

/// How long the admin API has to answer before `stop` falls back (design 6.7).
const ADMIN_TIMEOUT: Duration = Duration::from_secs(2);
/// The router configuration file, in the config directory.
const ROUTER_TOML: &str = "router.toml";

/// Which control command to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Stop,
    Resume,
    Cancel,
}

impl Action {
    fn path(self) -> &'static str {
        match self {
            Self::Stop => "/v1/stop",
            Self::Resume => "/v1/resume",
            Self::Cancel => "/v1/cancel",
        }
    }
}

/// Runs `action` for the named bots, or every local bot when `bots` is empty.
pub(super) fn run(dirs: &Dirs, action: Action, bots: &[String]) -> Result<(), CliError> {
    let (roster, config) = load_config(dirs)?;
    let scope = scope(&roster, bots)?;
    let body = match &scope {
        Scope::All => json!({}),
        Scope::Bots(bots) => json!({ "bots": names(bots) }),
    };
    let base_url = format!("http://{}", config.api_bind);
    let answer = admin_token::read(&dirs.data_dir)
        .map_err(|error| CallError::Unreachable(format!("{error}; is the router running?")))
        .and_then(|token| call(&base_url, action.path(), &token, Some(body), ADMIN_TIMEOUT));
    match answer {
        Ok(answer) => print_json(&answer),
        Err(CallError::Unreachable(_)) if action == Action::Stop => {
            fallback_stop(&dirs.data_dir, &roster, &scope)
        }
        Err(error) => Err(error.into_cli()),
    }
}

/// The roster and `router.toml`.
pub(super) fn load_config(dirs: &Dirs) -> Result<(Roster, RouterConfig), CliError> {
    let (roster, _) = load_roster(&roster_path(&dirs.config_dir)?)?;
    let path = dirs.config_dir.join(ROUTER_TOML);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| CliError::bad_input(format!("cannot read {}: {error}", path.display())))?;
    let config = parse_router(&text, &roster)
        .map_err(|errors| CliError::bad_input(format!("invalid {}: {errors}", path.display())))?;
    Ok((roster, config))
}

/// Every bot when none is named, else the named roster bots; an unknown name is bad input.
fn scope(roster: &Roster, bots: &[String]) -> Result<Scope, CliError> {
    if bots.is_empty() {
        return Ok(Scope::All);
    }
    bots.iter()
        .map(|name| {
            BotName::new(name.clone())
                .ok()
                .filter(|bot| roster.bots.contains_key(bot))
                .ok_or_else(|| CliError::bad_input(format!("unknown bot {name:?}")))
        })
        .collect::<Result<BTreeSet<_>, _>>()
        .map(Scope::Bots)
}

fn names(bots: &BTreeSet<BotName>) -> Vec<&str> {
    bots.iter().map(BotName::as_str).collect()
}

/// The CLI fallback for `stop` (design 6.7): halts first, then kills. Exits 0 once the halts
/// are written and every in-scope running wake is killed or reported; a running-wakes read
/// that fails is an error, so a silent `killed: []` never hides live agents.
fn fallback_stop(data_dir: &Path, roster: &Roster, scope: &Scope) -> Result<(), CliError> {
    let store_error =
        |error: StoreError| CliError::other(format!("cannot write the halt: {error}"));
    std::fs::create_dir_all(data_dir).map_err(|error| {
        CliError::other(format!("cannot create {}: {error}", data_dir.display()))
    })?;
    let mut store = Store::open(&data_dir.join(STORE_FILE)).map_err(store_error)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let tx = store
        .connection_mut()
        .transaction()
        .map_err(|error| store_error(error.into()))?;
    write_halts(
        &tx,
        &Control::Stop(scope.clone()),
        SET_BY_CLI,
        now_ms,
        roster,
    )
    .map_err(store_error)?;
    tx.commit().map_err(|error| store_error(error.into()))?;

    let running = store
        .wakes()
        .with_state(WakeState::Running)
        .map_err(|error| CliError::other(format!("cannot list running wakes: {error}")))?;
    let mut killed = Vec::new();
    let mut failed = Vec::new();
    for wake in running.iter().filter(|wake| in_scope(scope, &wake.bot)) {
        match kill_wake(data_dir, wake) {
            Ok(()) => killed.push(wake.id.to_string()),
            Err(error) => failed.push(json!({ "wake_id": wake.id, "error": error })),
        }
    }
    let shown: Value = match scope {
        Scope::All => json!("all"),
        Scope::Bots(bots) => json!(names(bots)),
    };
    print_json(&json!({
        "scope": shown,
        "fallback": true,
        "killed": killed,
        "kill_failed": failed,
    }))
}

fn in_scope(scope: &Scope, bot: &BotName) -> bool {
    match scope {
        Scope::All => true,
        Scope::Bots(bots) => bots.contains(bot),
    }
}

/// Kills the process tree whose leader's pid is in `wakes/<id>/pid`.
fn kill_wake(data_dir: &Path, wake: &WakeRow) -> Result<(), String> {
    let path = data_dir.join("wakes").join(wake.id.to_string()).join("pid");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let pid: u32 = text
        .trim()
        .parse()
        .map_err(|error| format!("bad pid in {}: {error}", path.display()))?;
    kill_tree(pid)
}

/// Unix: `killpg(pid, SIGKILL)`; the command adapter makes the child its group's leader.
#[cfg(unix)]
fn kill_tree(pid: u32) -> Result<(), String> {
    use nix::errno::Errno;
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    let pid = i32::try_from(pid).map_err(|error| error.to_string())?;
    match killpg(Pid::from_raw(pid), Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Windows: `taskkill /PID <pid> /T /F`.
#[cfg(windows)]
fn kill_tree(pid: u32) -> Result<(), String> {
    let output = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output()
        .map_err(|error| format!("cannot run taskkill: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}
