//! `status [--json]` (design 12.1, 12.2, R52.2, R52.3).
//!
//! Calls `GET /v1/status` with the token in `<data-dir>/admin.token`. With `--json` it prints the
//! body exactly as the daemon sent it; otherwise a summary and one table row per bot. A daemon
//! that can't be reached, or a missing admin token, is a `network_error` (exit 2).

use std::io::{self, Write};
use std::time::Duration;

use super::agent::{request, CallError};
use super::control::load_config;
use super::{write_error, CliError};
use crate::api::admin_token;
use crate::core::Status;
use crate::paths::Dirs;

/// How long the daemon has to answer.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// Prints the running router's status.
pub(super) fn run(dirs: &Dirs, json: bool) -> Result<(), CliError> {
    let (_, config) = load_config(dirs)?;
    let base_url = format!("http://{}", config.api_bind);
    let body = admin_token::read(&dirs.data_dir)
        .map_err(|error| CallError::Unreachable(format!("{error}; is the router running?")))
        .and_then(|token| {
            request(
                reqwest::Method::GET,
                &base_url,
                "/v1/status",
                &token,
                None,
                STATUS_TIMEOUT,
            )
        })
        .map_err(CallError::into_cli)?;
    let mut stdout = io::stdout().lock();
    if json {
        stdout.write_all(&body).map_err(write_error)?;
        return writeln!(stdout).map_err(write_error);
    }
    let status: Status = serde_json::from_slice(&body)
        .map_err(|error| CliError::other(format!("cannot read the status: {error}")))?;
    write!(stdout, "{}", table(&status)).map_err(write_error)
}

/// The status as text: a summary, then a row per bot.
fn table(status: &Status) -> String {
    let yes_no = |flag: bool| if flag { "yes" } else { "no" };
    let halts = if status.halts.is_empty() {
        "none".to_owned()
    } else {
        status.halts.join(", ")
    };
    let mut text = format!(
        "version          {}\nroster_hash      {}\nhalts            {halts}\nmissed           {}\nunmanaged_posts  {}\n\n",
        status.version,
        status.roster_hash,
        status.missed.len(),
        status.unmanaged_posts,
    );
    let width = status
        .bots
        .iter()
        .map(|bot| bot.name.len())
        .chain([3])
        .max()
        .unwrap_or(3);
    text.push_str(&format!(
        "{:<width$}  AVAILABLE  CONNECTED  HALTED  RUNNING  QUEUED  HOUR     DAY      SUPPRESSED\n",
        "BOT"
    ));
    for bot in &status.bots {
        let budget = &bot.budget;
        text.push_str(&format!(
            "{:<width$}  {:<9}  {:<9}  {:<6}  {:<7}  {:<6}  {:<7}  {:<7}  {}\n",
            bot.name,
            yes_no(bot.available),
            yes_no(bot.connected),
            yes_no(bot.halted),
            bot.running.len(),
            bot.queued,
            format!("{}/{}", budget.hour.used, budget.hour.limit),
            format!("{}/{}", budget.day.used, budget.day.limit),
            budget.suppressed_since_start,
        ));
    }
    text
}
