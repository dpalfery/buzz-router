//! Logging (design section 14).
//!
//! A `tracing-subscriber` with an [`EnvFilter`] from [`FILTER_VAR`], default
//! `info`: human-readable to stderr, plus JSON lines to
//! `<data-dir>/logs/buzz-router.log` through a daily-rotating file appender
//! that keeps [`MAX_LOG_FILES`] files. Spans carry `bot`, `wake_id`,
//! `event_id` and `root_id` at their call sites.
//!
//! [`LogLimiter`] provides the once-per-key and once-per-key-per-hour
//! warnings for roster drift and unmanaged posts (R4.4, R51.2). [`redact`]
//! replaces secrets (wake tokens, nsecs, admin tokens, webhook secrets) before
//! they can reach a log line (R59.4).

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::EnvFilter;

/// The environment variable that holds the log filter, in `EnvFilter` syntax.
pub const FILTER_VAR: &str = "BUZZ_ROUTER_LOG";

/// The filter used when [`FILTER_VAR`] is unset or does not parse.
const DEFAULT_FILTER: &str = "info";

/// The file prefix of the rotating JSON log inside `<data-dir>/logs/`.
const LOG_FILE_PREFIX: &str = "buzz-router.log";

/// How many rotated log files are kept (design 14).
pub const MAX_LOG_FILES: usize = 14;

/// The replacement text [`redact`] substitutes for a secret.
pub const REDACTED: &str = "[redacted]";

/// The [`EnvFilter`] from [`FILTER_VAR`], or [`DEFAULT_FILTER`] when the
/// variable is unset or does not parse.
pub fn filter_from_env() -> EnvFilter {
    EnvFilter::try_from_env(FILTER_VAR).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
}

/// The log directory inside a data directory: `<data-dir>/logs`.
pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// Deletes the oldest rotated logs in `dir` until at most `keep` files named
/// [`LOG_FILE_PREFIX`]`* remain. Returns how many files were removed.
pub fn prune_old_logs(dir: &Path, keep: usize) -> io::Result<usize> {
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(LOG_FILE_PREFIX) {
            names.push(name);
        }
    }
    names.sort();
    let mut removed = 0;
    while names.len() > keep {
        let oldest = names.remove(0);
        match std::fs::remove_file(dir.join(&oldest)) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(removed)
}

/// A non-blocking writer for the daily-rotating JSON log under
/// `<data-dir>/logs/`. The returned guard flushes on drop and must be kept
/// alive as long as logging continues. Older files are pruned to
/// [`MAX_LOG_FILES`] on every rotation, so a long-running daemon stays
/// within the limit too; pre-existing excess files are pruned here as well.
pub fn file_writer(data_dir: &Path) -> io::Result<(NonBlocking, WorkerGuard)> {
    let dir = log_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let _ = prune_old_logs(&dir, MAX_LOG_FILES);
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(&dir)
        .map_err(io::Error::other)?;
    Ok(tracing_appender::non_blocking(appender))
}

/// Installs the stderr subscriber. A subscriber that is already installed stays in place.
pub fn init() {
    // The only way this fails is that a global subscriber exists already, which is fine.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter_from_env())
        .with_writer(std::io::stderr)
        .try_init();
}

/// Installs the stderr subscriber plus the JSON file layer for `data_dir`.
/// A subscriber that is already installed stays in place. The returned guard
/// flushes the file writer on drop and must be kept alive by the daemon.
pub fn init_with_data_dir(data_dir: &Path) -> io::Result<WorkerGuard> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let (writer, guard) = file_writer(data_dir)?;
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let file = tracing_subscriber::fmt::layer().json().with_writer(writer);
    // The only way this fails is that a global subscriber exists already, which is fine.
    let _ = tracing_subscriber::registry()
        .with(filter_from_env())
        .with(stderr)
        .with(file)
        .try_init();
    Ok(guard)
}

/// Rate-limits repeated warnings: once per key ever ([`allow_once`]), or once
/// per key per hour ([`allow_hourly`]) on the caller's clock, so tests can use
/// virtual time.
#[derive(Debug, Default)]
pub struct LogLimiter {
    seen_once: HashSet<String>,
    last_hourly: HashMap<String, DateTime<Utc>>,
}

impl LogLimiter {
    /// A limiter that has allowed nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true the first time each key is seen, false afterwards.
    /// Roster drift uses this: one warning per foreign-bot pubkey (R4.4).
    pub fn allow_once(&mut self, key: &str) -> bool {
        self.seen_once.insert(key.to_owned())
    }

    /// Returns true when `key` never warned, or when `now` is at least an
    /// hour after its last allowed warning. Unmanaged posts use this: one
    /// warning per bot per hour (R51.2).
    pub fn allow_hourly(&mut self, key: &str, now: DateTime<Utc>) -> bool {
        let due = match self.last_hourly.get(key) {
            Some(last) => now.signed_duration_since(*last) >= chrono::Duration::hours(1),
            None => true,
        };
        if due {
            self.last_hourly.insert(key.to_owned(), now);
        }
        due
    }
}

/// Replaces every non-empty secret in `message` with [`REDACTED`]. Callers
/// pass the wake token, key material, admin token and webhook secret they
/// hold so that none of them ever reaches a log line (R59.4).
pub fn redact(message: &str, secrets: &[&str]) -> String {
    let mut redacted = message.to_owned();
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        redacted = redacted.replace(secret, REDACTED);
    }
    redacted
}
