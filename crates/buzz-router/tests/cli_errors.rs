//! Task 1.11 (RED): the JSON error line and the exit-code mapping of the `buzz-router` binary.
//!
//! Covers acceptance criterion 2 and requirement 57 against design 14 and the architect's
//! decisions D1 and D4. On failure the binary writes one line of JSON to stderr, nothing to
//! stdout, and exits with the code of the error's kind:
//!
//! | `ErrorKind` | exit | `error` category | `retryable` |
//! |---|---|---|---|
//! | `BadInput` | 1 | `user_error` | `false` |
//! | `Network` | 2 | `network_error` | `true` |
//! | `Auth` | 3 | `auth_error` | `false` |
//! | `Other` | 4 | `error` | `false` |
//!
//! The line reads `{"error":"<category>","message":"...","retryable":<bool>}`: the three keys in
//! that order, compact, ended by one newline (D1, D4). The tests compare the raw text, because a
//! parsed `serde_json::Value` would hide the key order.
//!
//! **What this file reaches.** It drives the real binary, `env!("CARGO_BIN_EXE_buzz-router")`, in
//! a sandbox of temporary directories, and a command can only fail with the kinds it has a way to
//! fail with in this task:
//!
//! - `BadInput`, exit 1, `user_error`, `retryable` false: a roster that breaks a rule
//!   (`bad_input_exits_one_with_a_user_error_line_of_the_documented_shape`); and, by D4 ("unreadable
//!   or invalid input files give `BadInput`"), a missing roster, an invalid `router.toml`, a missing
//!   or invalid roster or a missing events file given to `route --replay`; and, by D4, every clap
//!   usage error (a missing or unknown subcommand, an unknown flag);
//! - `Other`, exit 4, `error`, `retryable` false: every command that is not built yet returns
//!   `CliError::other("not implemented")`
//!   (`commands_not_yet_built_return_not_implemented_with_exit_four`).
//!
//! **What it does not reach, and why.** `Network` (2) and `Auth` (3) need a command that talks to a
//! daemon or a relay, and none exists before tasks 3.9 and 3.10, whose tests assert them through
//! the binary. Their rows, with every other row, are pinned by the unit tests in
//! `src/cli/mod.rs` (`cargo test -p buzz-router --lib cli::tests`).
//!
//! **Interim rows.** `NOT_YET_BUILT` lists each command that is not built yet, once, with the
//! flags design 12.1 gives it. Each later task that builds a command must delete that command's row
//! when it writes its own tests; the row would otherwise fail, or, for a command with side effects,
//! run it. `run`, `service install` and `service uninstall` have no row for that reason: once
//! built they would start a daemon or change the operating system's services. Their presence is
//! covered by `cli_surface.rs`.
//!
//! **Not tested here.** `capture --since 7x`: the `capture` command validates the duration itself
//! (task 2.8), and `cli_capture.rs` asserts its exit 1.

#![allow(
    clippy::expect_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::TempDir;

/// A valid roster: one owner key, one channel, bot `A`.
const VALID_ROSTER: &str = r#"version = 1

[owner]
name = "Fixture Owner"
pubkeys = ["1111111111111111111111111111111111111111111111111111111111111111"]
timezone = "UTC"

[[channels]]
id = "00000000-0000-0000-0000-000000000001"
name = "fixture-room"

[[bots]]
name = "A"
pubkey = "2222222222222222222222222222222222222222222222222222222222222222"
channels = ["*"]
respond_to = "owner-only"
"#;

/// A roster that breaks a validation rule: `Mars/Olympus` is no IANA time zone (R2.3).
const INVALID_ROSTER: &str = r#"version = 1

[owner]
name = "Fixture Owner"
pubkeys = ["1111111111111111111111111111111111111111111111111111111111111111"]
timezone = "Mars/Olympus"

[[bots]]
name = "A"
pubkey = "2222222222222222222222222222222222222222222222222222222222222222"
channels = ["*"]
respond_to = "owner-only"
"#;

/// A `router.toml` that is not TOML.
const INVALID_ROUTER_TOML: &str = "roster_path = = \"other.toml\"\n[[[\n";

/// The commands that are not built yet, each once, with its flags (design 12.1). See the module
/// documentation: a later task deletes its commands' rows.
const NOT_YET_BUILT: [&[&str]; 2] = [
    &["status", "--json"],
    &["wakes", "--bot", "A", "--state", "queued"],
];

/// The error line a failing command must print (decisions D1 and D4).
struct Failure<'a> {
    /// The process exit code of the error's kind.
    exit_code: i32,
    /// The `error` value.
    category: &'a str,
    /// The `retryable` value.
    retryable: bool,
    /// The exact `message`, when the spec fixes it.
    message: Option<&'a str>,
}

/// `ErrorKind::BadInput`: exit 1, `user_error`, not retryable. The message is free text.
const BAD_INPUT: Failure<'static> = Failure {
    exit_code: 1,
    category: "user_error",
    retryable: false,
    message: None,
};

/// `ErrorKind::Other` for a command that is not built yet: exit 4, `error`, not retryable.
const NOT_IMPLEMENTED: Failure<'static> = Failure {
    exit_code: 4,
    category: "error",
    retryable: false,
    message: Some("not implemented"),
};

/// Temporary directories for one test: a config dir, a data dir, a home dir and a directory for
/// input files, all empty.
struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let sandbox = Self {
            root: tempfile::tempdir().expect("a temporary directory"),
        };
        for dir in [
            sandbox.config(),
            sandbox.data(),
            sandbox.home(),
            sandbox.files(),
        ] {
            fs::create_dir(dir).expect("a sandbox directory");
        }
        sandbox
    }

    fn config(&self) -> PathBuf {
        self.root.path().join("config")
    }

    fn data(&self) -> PathBuf {
        self.root.path().join("data")
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    fn files(&self) -> PathBuf {
        self.root.path().join("files")
    }

    /// Writes `contents` to `name` in the config dir.
    fn write_config(&self, name: &str, contents: &str) {
        fs::write(self.config().join(name), contents).expect("a config file");
    }

    /// Runs `buzz-router --config-dir <config> --data-dir <data> <args>` with no router variables
    /// inherited and a home directory inside the sandbox, so that nothing the test does reaches the
    /// user's own directories or a daemon.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_buzz-router"))
            .env("HOME", self.home())
            .env("USERPROFILE", self.home())
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("BUZZ_ROUTER_CONFIG_DIR")
            .env_remove("BUZZ_ROUTER_DATA_DIR")
            .env_remove("BUZZ_ROUTER_LOG")
            .env_remove("BUZZ_ROUTER_URL")
            .env_remove("BUZZ_ROUTER_WAKE_TOKEN")
            .stdin(Stdio::null())
            .arg("--config-dir")
            .arg(self.config())
            .arg("--data-dir")
            .arg(self.data())
            .args(args)
            .output()
            .expect("the binary starts")
    }
}

/// What is wrong with the way `output` reports `failure`; empty when nothing is. Every line of the
/// answer names one broken rule.
fn error_report_problems(output: &Output, failure: &Failure<'_>) -> Vec<String> {
    let mut problems = Vec::new();
    if output.status.code() != Some(failure.exit_code) {
        problems.push(format!(
            "exit code {:?}, wanted {}",
            output.status.code(),
            failure.exit_code
        ));
    }
    if !output.stdout.is_empty() {
        problems.push(format!(
            "stdout should be empty, was {:?}",
            String::from_utf8_lossy(&output.stdout)
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if stderr.lines().count() != 1 {
        problems.push(format!("stderr should be one line, was {stderr:?}"));
        return problems;
    }
    if !stderr.ends_with('\n') {
        problems.push(format!("stderr should end with a newline, was {stderr:?}"));
    }
    let line = stderr.trim_end_matches('\n');
    let prefix = format!(r#"{{"error":"{}","message":"#, failure.category);
    let suffix = format!(r#","retryable":{}}}"#, failure.retryable);
    if !(line.starts_with(&prefix) && line.ends_with(&suffix)) {
        problems.push(format!(
            "the line should read {prefix}<message>{suffix} (keys in that order, compact), was {line:?}"
        ));
    }
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(line) else {
        problems.push(format!("stderr should be a JSON object, was {stderr:?}"));
        return problems;
    };
    let keys: BTreeSet<&str> = fields.keys().map(String::as_str).collect();
    if keys != BTreeSet::from(["error", "message", "retryable"]) {
        problems.push(format!(
            "the keys should be error, message and retryable, were {keys:?}"
        ));
    }
    if fields.get("error").and_then(Value::as_str) != Some(failure.category) {
        problems.push(format!(
            "error should be {:?}, was {:?}",
            failure.category,
            fields.get("error")
        ));
    }
    match fields.get("message").and_then(Value::as_str) {
        None | Some("") => problems.push(format!(
            "message should be a non-empty string, was {:?}",
            fields.get("message")
        )),
        Some(text) => {
            if failure.message.is_some_and(|wanted| wanted != text) {
                problems.push(format!(
                    "message should be {:?}, was {text:?}",
                    failure.message
                ));
            }
        }
    }
    if fields.get("retryable") != Some(&Value::Bool(failure.retryable)) {
        problems.push(format!(
            "retryable should be {}, was {:?}",
            failure.retryable,
            fields.get("retryable")
        ));
    }
    problems
}

/// Asserts that `output` reports `failure`, and says how it does not if it does not.
fn assert_reports(output: &Output, failure: &Failure<'_>) {
    let problems = error_report_problems(output, failure);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn bad_input_exits_one_with_a_user_error_line_of_the_documented_shape() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", INVALID_ROSTER);

    let output = sandbox.run(&["roster", "check"]);

    assert_reports(&output, &BAD_INPUT);
}

#[test]
fn commands_not_yet_built_return_not_implemented_with_exit_four() {
    let sandbox = Sandbox::new();

    let problems: Vec<String> = NOT_YET_BUILT
        .iter()
        .flat_map(|args| {
            let output = sandbox.run(args);
            error_report_problems(&output, &NOT_IMPLEMENTED)
                .into_iter()
                .map(move |problem| format!("`buzz-router {}`: {problem}", args.join(" ")))
        })
        .collect();

    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_missing_roster_file_is_bad_input() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["roster", "check"]);

    assert_reports(&output, &BAD_INPUT);
}

#[test]
fn an_invalid_router_toml_is_bad_input() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", VALID_ROSTER);
    sandbox.write_config("router.toml", INVALID_ROUTER_TOML);

    let output = sandbox.run(&["roster", "check"]);

    assert_reports(&output, &BAD_INPUT);
}

#[test]
fn replay_with_a_missing_events_file_is_bad_input() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", VALID_ROSTER);
    let missing = sandbox.files().join("no-such-events.jsonl");

    let output = sandbox.run(&["route", "--replay", &missing.to_string_lossy()]);

    assert_reports(&output, &BAD_INPUT);
}

#[test]
fn replay_with_an_invalid_roster_is_bad_input() {
    let sandbox = Sandbox::new();
    let roster = sandbox.files().join("r.toml");
    fs::write(&roster, INVALID_ROSTER).expect("a roster file");
    let events = sandbox.files().join("events.jsonl");
    fs::write(&events, "").expect("an events file");

    let output = sandbox.run(&[
        "route",
        "--replay",
        &events.to_string_lossy(),
        "--roster",
        &roster.to_string_lossy(),
    ]);

    assert_reports(&output, &BAD_INPUT);
}

#[test]
fn replay_with_a_missing_roster_is_bad_input() {
    let sandbox = Sandbox::new();
    let events = sandbox.files().join("events.jsonl");
    fs::write(&events, "").expect("an events file");
    let missing = sandbox.files().join("no-such-roster.toml");

    let named = sandbox.run(&[
        "route",
        "--replay",
        &events.to_string_lossy(),
        "--roster",
        &missing.to_string_lossy(),
    ]);
    let configured = sandbox.run(&["route", "--replay", &events.to_string_lossy()]);

    assert_reports(&named, &BAD_INPUT);
    assert_reports(&configured, &BAD_INPUT);
}

#[test]
fn a_usage_error_is_bad_input() {
    let sandbox = Sandbox::new();
    let cases: [&[&str]; 3] = [
        // No subcommand at all.
        &[],
        &["frobnicate"],
        &["roster", "check", "--no-such-flag"],
    ];

    let problems: Vec<String> = cases
        .iter()
        .flat_map(|args| {
            let output = sandbox.run(args);
            error_report_problems(&output, &BAD_INPUT)
                .into_iter()
                .map(move |problem| format!("`buzz-router {}`: {problem}", args.join(" ")))
        })
        .collect();

    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
