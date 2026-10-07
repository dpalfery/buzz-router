//! The Windows Task Scheduler task (design section 13, requirement 56.4).

use std::path::{Path, PathBuf};

use super::{
    remove_file, run_checked, utf8, write_file, xml_escape, CommandRunner, ServiceError,
    ServiceStatus,
};

/// The task name.
pub const TASK: &str = "buzz-router";

/// The task that runs `<exe> run` at `user`'s logon, as that user, and restarts it on failure.
/// The text declares UTF-16, the encoding [`install`] writes it in.
pub fn render_task_xml(exe: &str, user: &str) -> String {
    let exe = xml_escape(exe);
    let user = xml_escape(user);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>buzz-router</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>999</Count>
    </RestartOnFailure>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>run</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// `data_dir\buzz-router-task.xml`.
pub fn xml_path(data_dir: &Path) -> PathBuf {
    data_dir.join("buzz-router-task.xml")
}

/// Writes the task XML as UTF-16 LE with a byte-order mark, then runs
/// `schtasks /Create /TN buzz-router /XML <file> /F` and `schtasks /Run /TN buzz-router`.
pub fn install(
    runner: &mut dyn CommandRunner,
    exe: &Path,
    data_dir: &Path,
    user: &str,
) -> Result<(), ServiceError> {
    let xml = xml_path(data_dir);
    write_file(&xml, &utf16le_with_bom(&render_task_xml(utf8(exe)?, user)))?;
    run_checked(
        runner,
        "schtasks",
        &["/Create", "/TN", TASK, "/XML", utf8(&xml)?, "/F"],
    )?;
    run_checked(runner, "schtasks", &["/Run", "/TN", TASK])?;
    Ok(())
}

/// Runs `schtasks /End` (a task that is not running is not an error) and
/// `schtasks /Delete /TN buzz-router /F`, then deletes the XML file.
pub fn uninstall(runner: &mut dyn CommandRunner, data_dir: &Path) -> Result<(), ServiceError> {
    runner.run("schtasks", &["/End", "/TN", TASK])?;
    run_checked(runner, "schtasks", &["/Delete", "/TN", TASK, "/F"])?;
    remove_file(&xml_path(data_dir))
}

/// Runs `schtasks /Query /TN buzz-router /FO LIST /V`: it succeeds when the task exists, and its
/// `Status:` line reads `Running` while the router runs.
pub fn status(runner: &mut dyn CommandRunner) -> Result<ServiceStatus, ServiceError> {
    let output = runner.run("schtasks", &["/Query", "/TN", TASK, "/FO", "LIST", "/V"])?;
    let running = output.stdout.lines().any(|line| {
        line.split_once(':')
            .is_some_and(|(key, value)| key.trim() == "Status" && value.trim() == "Running")
    });
    Ok(ServiceStatus {
        installed: output.success,
        running: output.success && running,
    })
}

fn utf16le_with_bom(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}
