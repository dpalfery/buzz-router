//! The macOS user LaunchAgent (design section 13, requirement 56.2).

use std::path::{Path, PathBuf};

use super::{
    create_dir, remove_file, run_checked, utf8, write_file, xml_escape, CommandRunner,
    ServiceError, ServiceStatus,
};

/// The launchd label.
pub const LABEL: &str = "com.buzz-router";

/// The plist that runs `<exe> run` at load and keeps it alive, logging under `log_dir`.
pub fn render_launchd_plist(exe: &str, log_dir: &str) -> String {
    let exe = xml_escape(exe);
    let log_dir = xml_escape(log_dir.trim_end_matches(['/', '\\']));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>run</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{log_dir}/launchd.stdout.log</string>
  <key>StandardErrorPath</key>
  <string>{log_dir}/launchd.stderr.log</string>
</dict>
</plist>
"#
    )
}

/// `~/Library/LaunchAgents/com.buzz-router.plist`.
pub fn plist_path(home_dir: &Path) -> PathBuf {
    home_dir
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// Writes the plist and runs `launchctl bootstrap gui/<uid> <plist>`.
pub fn install(
    runner: &mut dyn CommandRunner,
    exe: &Path,
    home_dir: &Path,
    data_dir: &Path,
    uid: u32,
) -> Result<(), ServiceError> {
    let log_dir = data_dir.join("logs");
    create_dir(&log_dir)?;
    let plist = plist_path(home_dir);
    write_file(
        &plist,
        render_launchd_plist(utf8(exe)?, utf8(&log_dir)?).as_bytes(),
    )?;
    run_checked(
        runner,
        "launchctl",
        &["bootstrap", &format!("gui/{uid}"), utf8(&plist)?],
    )?;
    Ok(())
}

/// Runs `launchctl bootout gui/<uid>/com.buzz-router`, then deletes the plist. A service that
/// is not loaded is not an error.
pub fn uninstall(
    runner: &mut dyn CommandRunner,
    home_dir: &Path,
    uid: u32,
) -> Result<(), ServiceError> {
    runner.run("launchctl", &["bootout", &format!("gui/{uid}/{LABEL}")])?;
    remove_file(&plist_path(home_dir))
}

/// Runs `launchctl print gui/<uid>/com.buzz-router`: it succeeds when the agent is loaded, and
/// reports `state = running` while the router runs.
pub fn status(runner: &mut dyn CommandRunner, uid: u32) -> Result<ServiceStatus, ServiceError> {
    let output = runner.run("launchctl", &["print", &format!("gui/{uid}/{LABEL}")])?;
    Ok(ServiceStatus {
        installed: output.success,
        running: output.success
            && output
                .stdout
                .lines()
                .any(|line| line.trim() == "state = running"),
    })
}

/// The current user's id, from `id -u`.
pub fn current_uid(runner: &mut dyn CommandRunner) -> Result<u32, ServiceError> {
    let output = run_checked(runner, "id", &["-u"])?;
    output.stdout.trim().parse().map_err(|_| {
        ServiceError::Environment(format!(
            "`id -u` printed {:?}, not a user id",
            output.stdout.trim()
        ))
    })
}
