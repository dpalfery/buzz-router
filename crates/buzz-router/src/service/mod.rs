//! Per-user service installation (design section 13, requirement 56, DD-8).
//!
//! The renderers are pure functions. Every OS command goes through [`CommandRunner`], so tests
//! record the commands instead of installing a real service. Each platform module compiles on
//! every OS; the CLI picks the one for the OS it runs on.

pub mod linux;
pub mod macos;
pub mod windows;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

pub use linux::render_systemd_unit;
pub use macos::render_launchd_plist;
pub use windows::render_task_xml;

/// Why a service command failed.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// A file could not be written, read or removed.
    #[error("cannot access {path}: {source}")]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// An OS command could not be started.
    #[error("cannot run {program}: {source}")]
    Spawn {
        /// The program that failed to start.
        program: String,
        /// The underlying error.
        source: io::Error,
    },
    /// An OS command ran and reported failure.
    #[error("`{command}` failed: {stderr}")]
    CommandFailed {
        /// The command line.
        command: String,
        /// What the command wrote to stderr, trimmed.
        stderr: String,
    },
    /// A path that a service definition must hold is not valid UTF-8.
    #[error("the path {0:?} is not valid UTF-8")]
    NonUtf8Path(PathBuf),
    /// Something the definition needs could not be found, such as the user id.
    #[error("{0}")]
    Environment(String),
    /// The OS has no supported service manager.
    #[error("service installation is not supported on this OS")]
    Unsupported,
}

/// What an OS command reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutput {
    /// Whether it exited with status 0.
    pub success: bool,
    /// Its standard output.
    pub stdout: String,
    /// Its standard error.
    pub stderr: String,
}

/// Runs OS commands. Production uses [`SystemCommandRunner`]; tests record the calls.
pub trait CommandRunner {
    /// Runs `program` with `args` to completion and returns what it reported.
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, ServiceError>;
}

/// Runs commands with `std::process::Command`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, ServiceError> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|source| ServiceError::Spawn {
                program: program.to_owned(),
                source,
            })?;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Whether the service is installed and running (requirement 56.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ServiceStatus {
    /// The OS knows the service.
    pub installed: bool,
    /// The router process is running under it.
    pub running: bool,
}

/// Runs a command and fails unless it succeeds.
fn run_checked(
    runner: &mut dyn CommandRunner,
    program: &str,
    args: &[&str],
) -> Result<CommandOutput, ServiceError> {
    let output = runner.run(program, args)?;
    if output.success {
        Ok(output)
    } else {
        Err(ServiceError::CommandFailed {
            command: std::iter::once(program)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" "),
            stderr: output.stderr.trim().to_owned(),
        })
    }
}

/// The path as UTF-8 text, which every service definition needs.
fn utf8(path: &Path) -> Result<&str, ServiceError> {
    path.to_str()
        .ok_or_else(|| ServiceError::NonUtf8Path(path.to_path_buf()))
}

/// Writes `contents` to `path`, creating its parent directories.
fn write_file(path: &Path, contents: &[u8]) -> Result<(), ServiceError> {
    let io_error = |source| ServiceError::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    fs::write(path, contents).map_err(io_error)
}

/// Creates `dir` and its parents.
fn create_dir(dir: &Path) -> Result<(), ServiceError> {
    fs::create_dir_all(dir).map_err(|source| ServiceError::Io {
        path: dir.to_path_buf(),
        source,
    })
}

/// Removes `path`; a file that is already gone is not an error.
fn remove_file(path: &Path) -> Result<(), ServiceError> {
    match fs::remove_file(path) {
        Err(source) if source.kind() != io::ErrorKind::NotFound => Err(ServiceError::Io {
            path: path.to_path_buf(),
            source,
        }),
        _ => Ok(()),
    }
}

/// Escapes text for an XML element body.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}
