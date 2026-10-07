//! Service definitions and the OS commands that load them (task 6.2; requirement 56, design
//! section 13, DD-8).
//!
//! The renderers are pure and run on every OS. The install, uninstall and status commands of all
//! three platforms are recorded by a fake `CommandRunner`, so no test installs a real service.

#![allow(
    clippy::unwrap_used,
    reason = "test helpers fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::collections::VecDeque;
use std::path::Path;

use buzz_router::service::{
    linux, macos, render_launchd_plist, render_systemd_unit, render_task_xml, windows,
    CommandOutput, CommandRunner, ServiceError, ServiceStatus,
};

const EXE: &str = "/opt/buzz/bin/buzz-router";

/// Records every command and answers from a queue of scripted outputs, then with success.
#[derive(Default)]
struct FakeRunner {
    calls: Vec<Vec<String>>,
    replies: VecDeque<CommandOutput>,
}

impl FakeRunner {
    fn replying(replies: impl IntoIterator<Item = CommandOutput>) -> Self {
        Self {
            calls: Vec::new(),
            replies: replies.into_iter().collect(),
        }
    }
}

impl CommandRunner for FakeRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, ServiceError> {
        let mut call = vec![program.to_owned()];
        call.extend(args.iter().map(|arg| (*arg).to_owned()));
        self.calls.push(call);
        Ok(self.replies.pop_front().unwrap_or_else(|| ok("")))
    }
}

fn ok(stdout: &str) -> CommandOutput {
    CommandOutput {
        success: true,
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}

fn failed(stderr: &str) -> CommandOutput {
    CommandOutput {
        success: false,
        stdout: String::new(),
        stderr: stderr.to_owned(),
    }
}

fn calls(runner: &FakeRunner) -> Vec<Vec<&str>> {
    runner
        .calls
        .iter()
        .map(|call| call.iter().map(String::as_str).collect())
        .collect()
}

fn text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn utf16_text(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xFE], "a UTF-16 LE byte-order mark");
    let (pairs, _) = bytes[2..].as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    String::from_utf16(&units).unwrap()
}

// ---- renderers ----

#[test]
fn launchd_plist_has_label_program_keepalive_and_log_paths() {
    let plist = render_launchd_plist(EXE, "/data/logs");

    assert!(plist.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(plist.contains("<key>Label</key>\n  <string>com.buzz-router</string>"));
    assert!(plist.contains(&format!(
        "<key>ProgramArguments</key>\n  <array>\n    <string>{EXE}</string>\n    <string>run</string>\n  </array>"
    )));
    assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
    assert!(plist.contains("<key>KeepAlive</key>\n  <true/>"));
    assert!(plist.contains("<key>StandardOutPath</key>\n  <string>/data/logs/"));
    assert!(plist.contains("<key>StandardErrorPath</key>\n  <string>/data/logs/"));
}

#[test]
fn launchd_plist_escapes_xml_in_paths() {
    let plist = render_launchd_plist("/Users/a&b/<bin>/buzz-router", "/data/logs");
    assert!(plist.contains("<string>/Users/a&amp;b/&lt;bin&gt;/buzz-router</string>"));
}

#[test]
fn systemd_unit_has_execstart_restart_and_wantedby() {
    let unit = render_systemd_unit(EXE);
    let lines: Vec<&str> = unit.lines().collect();

    assert!(lines.contains(&"[Service]"));
    assert!(lines.contains(&format!("ExecStart={EXE} run").as_str()));
    assert!(lines.contains(&"Restart=always"));
    assert!(lines.contains(&"RestartSec=5"));
    assert!(lines.contains(&"[Install]"));
    assert!(lines.contains(&"WantedBy=default.target"));
}

#[test]
fn systemd_unit_quotes_an_exe_with_spaces_and_escapes_percent() {
    let unit = render_systemd_unit("/home/me/my bin/100%/buzz-router");
    assert!(unit
        .lines()
        .any(|line| line == "ExecStart=\"/home/me/my bin/100%%/buzz-router\" run"));
}

#[test]
fn task_xml_has_logon_trigger_principal_restart_and_exec() {
    let exe = r"C:\Users\me\bin\buzz-router.exe";
    let xml = render_task_xml(exe, r"HOST\me");

    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-16\"?>"));
    assert!(xml.contains("<LogonTrigger>"));
    assert!(xml.contains(r"<UserId>HOST\me</UserId>"));
    assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
    assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
    assert!(xml.contains(
        "<RestartOnFailure>\n      <Interval>PT1M</Interval>\n      <Count>999</Count>\n    </RestartOnFailure>"
    ));
    assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
    assert!(xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
    assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
    assert!(xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
    assert!(xml.contains(&format!(
        "<Exec>\n      <Command>{exe}</Command>\n      <Arguments>run</Arguments>\n    </Exec>"
    )));
}

#[test]
fn task_xml_restarts_a_dead_router_every_minute() {
    // R56.4: RestartOnFailure may not fire when the process exits non-zero, so a
    // repeating trigger restarts it within about a minute instead.
    let xml = render_task_xml(r"C:\Users\me\bin\buzz-router.exe", r"HOST\me");

    assert!(xml.contains("<TimeTrigger>"));
    assert!(xml.contains(
        "<Repetition>\n        <Interval>PT1M</Interval>\n        <StopAtDurationEnd>false</StopAtDurationEnd>\n      </Repetition>"
    ));
}

// ---- macOS commands ----

#[test]
fn macos_install_writes_the_plist_then_bootstraps_it() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut runner = FakeRunner::default();

    macos::install(&mut runner, Path::new(EXE), home.path(), data.path(), 501).unwrap();

    let plist = home
        .path()
        .join("Library/LaunchAgents/com.buzz-router.plist");
    let logs = data.path().join("logs");
    assert_eq!(
        text(&plist),
        render_launchd_plist(EXE, logs.to_str().unwrap())
    );
    assert!(logs.is_dir(), "the log directory is created");
    assert_eq!(
        calls(&runner),
        [vec![
            "launchctl",
            "bootstrap",
            "gui/501",
            plist.to_str().unwrap()
        ]]
    );
}

#[test]
fn macos_install_reports_a_failed_bootstrap() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut runner = FakeRunner::replying([failed("Bootstrap failed: 5")]);

    let error =
        macos::install(&mut runner, Path::new(EXE), home.path(), data.path(), 501).unwrap_err();

    assert!(
        matches!(error, ServiceError::CommandFailed { .. }),
        "{error:?}"
    );
}

#[test]
fn macos_uninstall_boots_out_then_deletes_the_plist() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut runner = FakeRunner::default();
    macos::install(&mut runner, Path::new(EXE), home.path(), data.path(), 501).unwrap();
    let plist = home
        .path()
        .join("Library/LaunchAgents/com.buzz-router.plist");

    let mut runner = FakeRunner::default();
    macos::uninstall(&mut runner, home.path(), 501).unwrap();

    assert_eq!(
        calls(&runner),
        [vec!["launchctl", "bootout", "gui/501/com.buzz-router"]]
    );
    assert!(!plist.exists());
}

#[test]
fn macos_uninstall_tolerates_a_service_that_is_not_loaded() {
    let home = tempfile::tempdir().unwrap();
    let mut runner = FakeRunner::replying([failed("No such process")]);

    macos::uninstall(&mut runner, home.path(), 501).unwrap();
}

#[test]
fn macos_status_prints_the_service_and_reads_its_state() {
    let mut runner =
        FakeRunner::replying([ok("gui/501/com.buzz-router = {\n\tstate = running\n}")]);

    let status = macos::status(&mut runner, 501).unwrap();

    assert_eq!(
        calls(&runner),
        [vec!["launchctl", "print", "gui/501/com.buzz-router"]]
    );
    assert_eq!(
        status,
        ServiceStatus {
            installed: true,
            running: true
        }
    );

    let mut runner = FakeRunner::replying([failed("Could not find service")]);
    assert_eq!(
        macos::status(&mut runner, 501).unwrap(),
        ServiceStatus {
            installed: false,
            running: false
        }
    );
}

#[test]
fn macos_current_uid_runs_id_u() {
    let mut runner = FakeRunner::replying([ok("501\n")]);
    assert_eq!(macos::current_uid(&mut runner).unwrap(), 501);
    assert_eq!(calls(&runner), [vec!["id", "-u"]]);
}

// ---- Linux commands ----

#[test]
fn linux_install_writes_the_unit_then_reloads_and_enables_it() {
    let home = tempfile::tempdir().unwrap();
    let mut runner = FakeRunner::default();

    linux::install(&mut runner, Path::new(EXE), home.path()).unwrap();

    let unit = home.path().join(".config/systemd/user/buzz-router.service");
    assert_eq!(text(&unit), render_systemd_unit(EXE));
    assert_eq!(
        calls(&runner),
        [
            vec!["systemctl", "--user", "daemon-reload"],
            vec![
                "systemctl",
                "--user",
                "enable",
                "--now",
                "buzz-router.service"
            ],
        ]
    );
}

#[test]
fn linux_uninstall_disables_deletes_and_reloads() {
    let home = tempfile::tempdir().unwrap();
    let mut runner = FakeRunner::default();
    linux::install(&mut runner, Path::new(EXE), home.path()).unwrap();
    let unit = home.path().join(".config/systemd/user/buzz-router.service");

    let mut runner = FakeRunner::default();
    linux::uninstall(&mut runner, home.path()).unwrap();

    assert_eq!(
        calls(&runner),
        [
            vec![
                "systemctl",
                "--user",
                "disable",
                "--now",
                "buzz-router.service"
            ],
            vec!["systemctl", "--user", "daemon-reload"],
        ]
    );
    assert!(!unit.exists());
}

#[test]
fn linux_status_asks_is_active_and_is_enabled() {
    let mut runner = FakeRunner::replying([ok("active\n"), ok("enabled\n")]);

    let status = linux::status(&mut runner).unwrap();

    assert_eq!(
        calls(&runner),
        [
            vec!["systemctl", "--user", "is-active", "buzz-router.service"],
            vec!["systemctl", "--user", "is-enabled", "buzz-router.service"],
        ]
    );
    assert_eq!(
        status,
        ServiceStatus {
            installed: true,
            running: true
        }
    );

    let mut runner = FakeRunner::replying([failed(""), failed("")]);
    assert_eq!(
        linux::status(&mut runner).unwrap(),
        ServiceStatus {
            installed: false,
            running: false
        }
    );
}

// ---- Windows commands ----

#[test]
fn windows_install_writes_the_task_xml_then_creates_and_runs_the_task() {
    let data = tempfile::tempdir().unwrap();
    let exe = r"C:\Users\me\bin\buzz-router.exe";
    let mut runner = FakeRunner::default();

    windows::install(&mut runner, Path::new(exe), data.path(), r"HOST\me").unwrap();

    let xml = data.path().join("buzz-router-task.xml");
    assert_eq!(utf16_text(&xml), render_task_xml(exe, r"HOST\me"));
    assert_eq!(
        calls(&runner),
        [
            vec![
                "schtasks",
                "/Create",
                "/TN",
                "buzz-router",
                "/XML",
                xml.to_str().unwrap(),
                "/F"
            ],
            vec!["schtasks", "/Run", "/TN", "buzz-router"],
        ]
    );
}

#[test]
fn windows_uninstall_ends_and_deletes_the_task() {
    let data = tempfile::tempdir().unwrap();
    let mut runner = FakeRunner::default();
    windows::install(&mut runner, Path::new(EXE), data.path(), "me").unwrap();
    let xml = data.path().join("buzz-router-task.xml");

    let mut runner = FakeRunner::replying([failed("not running")]);
    windows::uninstall(&mut runner, data.path()).unwrap();

    assert_eq!(
        calls(&runner),
        [
            vec!["schtasks", "/End", "/TN", "buzz-router"],
            vec!["schtasks", "/Delete", "/TN", "buzz-router", "/F"],
        ]
    );
    assert!(!xml.exists());
}

#[test]
fn windows_status_queries_the_task() {
    let mut runner = FakeRunner::replying([ok(
        "TaskName:      \\buzz-router\nStatus:        Running\nLogon Mode:    Interactive only\n",
    )]);

    let status = windows::status(&mut runner).unwrap();

    assert_eq!(
        calls(&runner),
        [vec![
            "schtasks",
            "/Query",
            "/TN",
            "buzz-router",
            "/FO",
            "LIST",
            "/V"
        ]]
    );
    assert_eq!(
        status,
        ServiceStatus {
            installed: true,
            running: true
        }
    );

    let mut runner = FakeRunner::replying([ok("TaskName: \\buzz-router\nStatus: Ready\n")]);
    assert_eq!(
        windows::status(&mut runner).unwrap(),
        ServiceStatus {
            installed: true,
            running: false
        }
    );
}
