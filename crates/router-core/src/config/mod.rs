//! Parsing and validation of `roster.toml` and `router.toml` (design section 4).
//!
//! [`parse_roster`] and [`parse_router`] read the TOML text, check every rule that needs no I/O
//! (requirements 1, 2 and 22.3, assumptions A2 and A3) and return the resolved [`Roster`] and
//! [`RouterConfig`]. A rejected file reports *every* problem at once, each as a [`ConfigIssue`]
//! with the TOML path of the key at fault. Checks that need the file system, such as the
//! permissions of a `file:` key, belong to the daemon.
//!
//! # Issue paths
//!
//! A path is the dotted keys with zero-based `[i]` array positions and no quoting, such as
//! `owner.pubkeys[1]`, `bots[0].limits.quiet_hours` or `bots[0].adapter.prompt_mode`. A missing,
//! unknown, ill-typed or out-of-set key is reported at the key itself. A file that is not valid
//! TOML gets one issue whose path is `""`.

mod limits;
mod roster;
mod router;
mod table;
mod validate;

use std::fmt;
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use toml::Table;

pub use limits::{Limits, QuietHours, QuietHoursError};
pub use roster::{Bot, Channel, ChannelScope, Owner, RespondTo, Roster};
pub use router::{
    AdapterConfig, KeySource, PromptMode, ReplyMode, RouterBot, RouterConfig, WebhookMode,
};

use roster::RosterFile;
use router::{read_roster_path, RouterFile};
use table::Issues;
use validate::{validate_roster, validate_router};

/// One problem found in a config file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConfigIssue {
    /// The TOML path of the key at fault, or `""` for a file that is not valid TOML.
    pub path: String,
    /// What is wrong, for the operator. Tests and callers match on `path`, never on this text.
    pub message: String,
}

impl fmt::Display for ConfigIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// Every problem found in a config file. It is never empty, and it is sorted by path and then
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigErrors(pub Vec<ConfigIssue>);

impl ConfigErrors {
    /// Builds the error from the collected issues, sorted and without exact repeats. An empty list
    /// would mean validation failed without saying why, so it becomes one internal issue rather
    /// than an error with nothing in it.
    fn from_issues(mut issues: Vec<ConfigIssue>) -> Self {
        if issues.is_empty() {
            issues.push(ConfigIssue {
                path: String::new(),
                message: "internal error: validation failed without reporting an issue".to_owned(),
            });
        }
        issues.sort();
        issues.dedup();
        Self(issues)
    }

    /// The issues, in order.
    pub fn iter(&self) -> std::slice::Iter<'_, ConfigIssue> {
        self.0.iter()
    }

    /// The first issue reported at exactly `path`.
    pub fn find(&self, path: &str) -> Option<&ConfigIssue> {
        self.0.iter().find(|issue| issue.path == path)
    }
}

impl<'a> IntoIterator for &'a ConfigErrors {
    type Item = &'a ConfigIssue;
    type IntoIter = std::slice::Iter<'a, ConfigIssue>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl fmt::Display for ConfigErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (position, issue) in self.0.iter().enumerate() {
            if position > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigErrors {}

/// Parses and validates `roster.toml`.
///
/// Omitted `[limits]` keys take the defaults of R2.5, and each bot's effective [`Limits`] are its
/// own values over `[limits]` over those defaults (R2.6).
pub fn parse_roster(source: &str) -> Result<Roster, ConfigErrors> {
    let root = parse_table(source)?;
    let mut issues = Issues::default();
    let file = RosterFile::read(&root, &mut issues);
    validate_roster(&file, issues)
}

/// Parses and validates `router.toml` against the roster it will run with.
///
/// The bots of the result are the ones this machine serves, which may be a subset of the roster.
pub fn parse_router(source: &str, roster: &Roster) -> Result<RouterConfig, ConfigErrors> {
    let root = parse_table(source)?;
    let mut issues = Issues::default();
    let file = RouterFile::read(&root, &mut issues);
    validate_router(&file, roster, issues)
}

/// Reads only `roster_path` from `router.toml` text, so the caller knows which roster to load
/// before it can call [`parse_router`]. Every other key is ignored, which lets `roster check` run
/// on a `router.toml` that is not valid yet (R2.15). The path is returned as written, and is
/// `roster.toml` when the key is omitted or empty (R1.2).
pub fn router_roster_path(router_toml: &str) -> Result<PathBuf, ConfigErrors> {
    let root = parse_table(router_toml)?;
    let mut issues = Issues::default();
    let path = read_roster_path(&root, &mut issues);
    if issues.is_empty() {
        Ok(path)
    } else {
        Err(ConfigErrors::from_issues(issues.into_vec()))
    }
}

/// The lowercase hex SHA-256 of a roster file's bytes (R2.13, R2.16).
pub fn roster_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Parses TOML text into a table, turning a syntax error into one issue at the empty path.
fn parse_table(source: &str) -> Result<Table, ConfigErrors> {
    source.parse::<Table>().map_err(|error| {
        ConfigErrors::from_issues(vec![ConfigIssue {
            path: String::new(),
            message: syntax_message(source, &error),
        }])
    })
}

/// Describes a syntax error by line and column. It does not repeat the offending line, which
/// could be part of a credential.
fn syntax_message(source: &str, error: &toml::de::Error) -> String {
    match error.span().and_then(|span| source.get(..span.start)) {
        Some(before) => {
            let line = before.matches('\n').count() + 1;
            let column = before
                .rsplit('\n')
                .next()
                .map_or(0, |line_so_far| line_so_far.chars().count())
                + 1;
            format!(
                "not valid TOML at line {line}, column {column}: {}",
                error.message()
            )
        }
        None => format!("not valid TOML: {}", error.message()),
    }
}
