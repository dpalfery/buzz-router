//! `wakes [--bot N] [--state S]` (design 12.1, R53.4).
//!
//! Reads `state.sqlite3` read-only, so it works while the daemon is down, and prints one JSON
//! line per wake, oldest first. The token hash is left out. No database yet means no wakes.

use std::io::{self, Write};

use router_core::ids::BotName;
use serde::Serialize;
use serde_json::Value;

use super::{write_error, CliError};
use crate::paths::Dirs;
use crate::store::wakes::{WakeRow, WakeState};
use crate::store::{Store, FILE_NAME};

/// One printed wake. Field order is the JSON key order.
#[derive(Serialize)]
struct WakeLine<'a> {
    id: String,
    bot: &'a str,
    state: &'static str,
    reason: &'a str,
    priority: &'a str,
    root_id: &'a str,
    round_id: &'a str,
    attempt: u32,
    created_at: i64,
    dispatch_after: i64,
    started_at: Option<i64>,
    deadline: Option<i64>,
    ended_at: Option<i64>,
    triggers: Value,
    outcome: Option<Value>,
}

impl<'a> WakeLine<'a> {
    fn new(row: &'a WakeRow) -> Self {
        let json = |text: &str| {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
        };
        Self {
            id: row.id.to_string(),
            bot: row.bot.as_str(),
            state: row.state.as_str(),
            reason: &row.reason,
            priority: &row.priority,
            root_id: row.root_id.as_str(),
            round_id: row.round_id.as_str(),
            attempt: row.attempt,
            created_at: row.created_at,
            dispatch_after: row.dispatch_after,
            started_at: row.started_at,
            deadline: row.deadline,
            ended_at: row.ended_at,
            triggers: json(&row.triggers),
            outcome: row.outcome.as_deref().map(json),
        }
    }
}

/// Prints the wakes, filtered by `bot` and `state`.
pub(super) fn run(dirs: &Dirs, bot: Option<&str>, state: Option<&str>) -> Result<(), CliError> {
    let bot = bot
        .map(|name| {
            BotName::new(name).map_err(|error| CliError::bad_input(format!("bad --bot: {error}")))
        })
        .transpose()?;
    let state = state
        .map(|name| {
            WakeState::from_name(name)
                .ok_or_else(|| CliError::bad_input(format!("unknown wake state {name:?}")))
        })
        .transpose()?;
    let path = dirs.data_dir.join(FILE_NAME);
    if !path.is_file() {
        return Ok(());
    }
    let read_error = |error| CliError::other(format!("cannot read {}: {error}", path.display()));
    let store = Store::open_read_only(&path).map_err(read_error)?;
    let rows = store
        .wakes()
        .list(bot.as_ref(), state)
        .map_err(read_error)?;
    let mut stdout = io::stdout().lock();
    for row in &rows {
        let line = serde_json::to_string(&WakeLine::new(row))
            .map_err(|error| CliError::other(format!("cannot encode a wake: {error}")))?;
        writeln!(stdout, "{line}").map_err(write_error)?;
    }
    Ok(())
}
