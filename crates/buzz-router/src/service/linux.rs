//! The Linux `systemd --user` unit (design section 13, requirement 56.3).

use std::path::{Path, PathBuf};

use super::{
    remove_file, run_checked, utf8, write_file, CommandRunner, ServiceError, ServiceStatus,
};

/// The unit name.
pub const UNIT: &str = "buzz-router.service";

/// The unit that runs `<exe> run` and restarts it whenever it exits.
pub fn render_systemd_unit(exe: &str) -> String {
    format!(
        "[Unit]
Description=buzz-router

[Service]
ExecStart={} run
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
",
        systemd_word(exe)
    )
}

/// `exe` as one `ExecStart` word: `%` doubled so it is not a specifier, and the word quoted when
/// it holds whitespace, a quote or a backslash.
fn systemd_word(exe: &str) -> String {
    let word = exe.replace('%', "%%");
    if word
        .chars()
        .any(|ch| ch.is_whitespace() || ch == '"' || ch == '\'' || ch == '\\')
    {
        format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        word
    }
}

/// `~/.config/systemd/user/buzz-router.service`.
pub fn unit_path(home_dir: &Path) -> PathBuf {
    home_dir
        .join(".config")
        .join("systemd")
        .join("user")
        .join(UNIT)
}

/// Writes the unit, then runs `systemctl --user daemon-reload` and
/// `systemctl --user enable --now buzz-router.service`.
pub fn install(
    runner: &mut dyn CommandRunner,
    exe: &Path,
    home_dir: &Path,
) -> Result<(), ServiceError> {
    write_file(
        &unit_path(home_dir),
        render_systemd_unit(utf8(exe)?).as_bytes(),
    )?;
    run_checked(runner, "systemctl", &["--user", "daemon-reload"])?;
    run_checked(runner, "systemctl", &["--user", "enable", "--now", UNIT])?;
    Ok(())
}

/// Runs `systemctl --user disable --now`, deletes the unit and reloads. A unit that is not
/// enabled is not an error.
pub fn uninstall(runner: &mut dyn CommandRunner, home_dir: &Path) -> Result<(), ServiceError> {
    runner.run("systemctl", &["--user", "disable", "--now", UNIT])?;
    remove_file(&unit_path(home_dir))?;
    run_checked(runner, "systemctl", &["--user", "daemon-reload"])?;
    Ok(())
}

/// Runs `systemctl --user is-active` and `is-enabled` for the unit.
pub fn status(runner: &mut dyn CommandRunner) -> Result<ServiceStatus, ServiceError> {
    let active = runner.run("systemctl", &["--user", "is-active", UNIT])?;
    let enabled = runner.run("systemctl", &["--user", "is-enabled", UNIT])?;
    Ok(ServiceStatus {
        installed: enabled.stdout.trim() == "enabled",
        running: active.stdout.trim() == "active",
    })
}
