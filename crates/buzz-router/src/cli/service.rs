//! `service install`, `service uninstall` and `service status` (design section 13, requirement 56).

use std::io::{self, Write};
use std::path::PathBuf;

use directories::BaseDirs;

use super::{write_error, CliError};
use crate::paths::Dirs;
use crate::service::{
    linux, macos, windows, CommandRunner, ServiceError, ServiceStatus, SystemCommandRunner,
};

/// What to do with the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Install,
    Uninstall,
    Status,
}

/// Runs `action` for this OS's service manager. `status` prints
/// `{"installed":<bool>,"running":<bool>}` on stdout.
pub(super) fn run(action: Action, dirs: &Dirs) -> Result<(), CliError> {
    let mut runner = SystemCommandRunner;
    match action {
        Action::Install => install(&mut runner, dirs).map_err(service_error),
        Action::Uninstall => uninstall(&mut runner, dirs).map_err(service_error),
        Action::Status => {
            let status = status(&mut runner).map_err(service_error)?;
            let line = serde_json::to_string(&status)
                .map_err(|error| CliError::other(error.to_string()))?;
            writeln!(io::stdout(), "{line}").map_err(write_error)
        }
    }
}

fn install(runner: &mut dyn CommandRunner, dirs: &Dirs) -> Result<(), ServiceError> {
    let exe = current_exe()?;
    if cfg!(target_os = "macos") {
        let uid = macos::current_uid(runner)?;
        macos::install(runner, &exe, &home_dir()?, &dirs.data_dir, uid)
    } else if cfg!(target_os = "linux") {
        linux::install(runner, &exe, &home_dir()?)
    } else if cfg!(windows) {
        windows::install(runner, &exe, &dirs.data_dir, &windows_user()?)
    } else {
        Err(ServiceError::Unsupported)
    }
}

fn uninstall(runner: &mut dyn CommandRunner, dirs: &Dirs) -> Result<(), ServiceError> {
    if cfg!(target_os = "macos") {
        let uid = macos::current_uid(runner)?;
        macos::uninstall(runner, &home_dir()?, uid)
    } else if cfg!(target_os = "linux") {
        linux::uninstall(runner, &home_dir()?)
    } else if cfg!(windows) {
        windows::uninstall(runner, &dirs.data_dir)
    } else {
        Err(ServiceError::Unsupported)
    }
}

fn status(runner: &mut dyn CommandRunner) -> Result<ServiceStatus, ServiceError> {
    if cfg!(target_os = "macos") {
        let uid = macos::current_uid(runner)?;
        macos::status(runner, uid)
    } else if cfg!(target_os = "linux") {
        linux::status(runner)
    } else if cfg!(windows) {
        windows::status(runner)
    } else {
        Err(ServiceError::Unsupported)
    }
}

fn current_exe() -> Result<PathBuf, ServiceError> {
    std::env::current_exe().map_err(|error| {
        ServiceError::Environment(format!("cannot locate the buzz-router executable: {error}"))
    })
}

fn home_dir() -> Result<PathBuf, ServiceError> {
    BaseDirs::new()
        .map(|base| base.home_dir().to_path_buf())
        .ok_or_else(|| ServiceError::Environment("cannot determine the home directory".into()))
}

/// `DOMAIN\user` from `USERDOMAIN` and `USERNAME`, or the bare user name without a domain.
fn windows_user() -> Result<String, ServiceError> {
    let user = std::env::var("USERNAME")
        .ok()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ServiceError::Environment("USERNAME is not set".into()))?;
    Ok(match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!(r"{domain}\{user}"),
        _ => user,
    })
}

fn service_error(error: ServiceError) -> CliError {
    CliError::other(error.to_string())
}
