//! `roster check` and the roster file rule (design 12.1, requirements 2.13 to 2.15).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use router_core::config::{parse_roster, roster_hash, router_roster_path, Roster};

use super::{write_error, CliError};
use crate::paths::Dirs;

/// The router configuration file, in the config directory.
const ROUTER_TOML: &str = "router.toml";

/// The roster file when `router.toml` is absent or omits `roster_path`.
const DEFAULT_ROSTER: &str = "roster.toml";

/// Validates the roster and prints the lowercase hex SHA-256 of its file's bytes.
pub(super) fn check(dirs: &Dirs) -> Result<(), CliError> {
    let path = roster_path(&dirs.config_dir)?;
    let (_, text) = load(&path)?;
    writeln!(io::stdout().lock(), "{}", roster_hash(text.as_bytes())).map_err(write_error)
}

/// The roster file the configuration names: `roster_path` from `router.toml`, relative to the
/// config directory, or `roster.toml` there when `router.toml` does not exist.
///
/// Only `roster_path` is read from `router.toml`, so a roster can be checked before the rest of
/// `router.toml` is valid (requirement 2.15).
pub(super) fn roster_path(config_dir: &Path) -> Result<PathBuf, CliError> {
    let router_toml = config_dir.join(ROUTER_TOML);
    match fs::read_to_string(&router_toml) {
        Ok(text) => router_roster_path(&text)
            .map(|roster| config_dir.join(roster))
            .map_err(|errors| {
                CliError::bad_input(format!("invalid {}: {errors}", router_toml.display()))
            }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(config_dir.join(DEFAULT_ROSTER))
        }
        Err(error) => Err(CliError::bad_input(format!(
            "cannot read {}: {error}",
            router_toml.display()
        ))),
    }
}

/// Reads and validates the roster at `path`. Returns it with the text of the file, whose bytes are
/// what the roster hash covers.
pub(super) fn load(path: &Path) -> Result<(Roster, String), CliError> {
    let text = fs::read_to_string(path).map_err(|error| {
        CliError::bad_input(format!(
            "cannot read the roster {}: {error}",
            path.display()
        ))
    })?;
    let roster = parse_roster(&text).map_err(|errors| {
        CliError::bad_input(format!("invalid roster {}: {errors}", path.display()))
    })?;
    Ok((roster, text))
}
