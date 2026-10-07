//! Task 1.11 (RED): `buzz-router roster check`.
//!
//! Covers requirements 2.13 to 2.15 against design 12.1 and 4.1. Each test runs the real binary,
//! `env!("CARGO_BIN_EXE_buzz-router")`, in a sandbox of temporary directories and never touches
//! the user's own configuration:
//!
//! - a valid roster prints exactly the lowercase hex SHA-256 of the file's bytes and exits 0
//!   (`a_valid_roster_prints_the_sha256_of_its_bytes_and_exits_zero`). The digests below are
//!   known answers, computed outside the code under test with `shasum -a 256` over the exact
//!   bytes of the constants, so the test does not lean on the router's own hash function. The hash
//!   covers the raw bytes, not the parsed roster (`the_hash_covers_the_raw_bytes_of_the_file`);
//! - an invalid roster gives one JSON error line on stderr, category `user_error`, and exit 1
//!   (`an_invalid_roster_gives_one_user_error_json_line_and_exit_one`);
//! - without `router.toml` the roster is `<config-dir>/roster.toml`
//!   (`without_router_toml_the_roster_is_read_from_the_config_dir`);
//! - with `roster_path = "other.toml"` in `router.toml` that file, relative to the config dir, is
//!   read instead (`router_toml_roster_path_selects_the_roster_to_check`);
//! - a `router.toml` that omits `roster_path` falls back to `roster.toml` (R2.15, assumption A2:
//!   `a_router_toml_without_roster_path_falls_back_to_roster_toml`);
//! - the hidden global flags `--config-dir` and `--data-dir` and the variables
//!   `BUZZ_ROUTER_CONFIG_DIR` and `BUZZ_ROUTER_DATA_DIR` are accepted, before or after the
//!   subcommand, and the config directory they name is the one read (`the_config_dir_flag_...`,
//!   `the_config_dir_variable_...`, `the_data_dir_flag_and_variable_are_accepted`). Nothing in
//!   `roster check` reads the data directory, so acceptance is all that can be observed of it.
//!
//! Of the router's JSON error shape this file asserts only the category and the exit code. The
//! rest of the shape is `cli_errors.rs`.
//!
//! The fixture rosters hold placeholder keys: `1111...` for the owner and one digit repeated for
//! each bot. They are valid (64 hex characters each, no curve check, assumption A3) and are no
//! real key.

#![allow(
    clippy::expect_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::TempDir;

/// A valid roster: one owner key, one channel, bot `A`.
const ROSTER: &str = r#"version = 1

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

/// `shasum -a 256` of the bytes of [`ROSTER`].
const ROSTER_SHA256: &str = "314376e639335bfe00e186f7f7334ea34cea455bb3bf775e1a83a9d89fe2819a";

/// `shasum -a 256` of the bytes of [`ROSTER`] with every `\n` replaced by `\r\n`.
const ROSTER_CRLF_SHA256: &str = "fa7bef4afcbf06e3916f96215eecda77714b8247cbc350fb5332607710e2d5fe";

/// A second valid roster, with other bytes, so that reading the wrong file shows in the hash.
const OTHER_ROSTER: &str = r#"version = 1

[owner]
name = "Other Owner"
pubkeys = ["3333333333333333333333333333333333333333333333333333333333333333"]
timezone = "UTC"

[[channels]]
id = "00000000-0000-0000-0000-000000000002"
name = "other-room"

[[bots]]
name = "B"
pubkey = "4444444444444444444444444444444444444444444444444444444444444444"
channels = ["*"]
respond_to = "anyone"
"#;

/// `shasum -a 256` of the bytes of [`OTHER_ROSTER`].
const OTHER_ROSTER_SHA256: &str =
    "8471255486cfa99cc7f8ccf832236f8d24c0f157d80b60a744b12f0c8841b3ee";

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

/// Temporary directories for one test: a config dir, a data dir and a home dir, all empty.
struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let sandbox = Self {
            root: tempfile::tempdir().expect("a temporary directory"),
        };
        for dir in [sandbox.config(), sandbox.data(), sandbox.home()] {
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

    /// Writes `contents` to `name` in the config dir.
    fn write_config(&self, name: &str, contents: impl AsRef<[u8]>) {
        fs::write(self.config().join(name), contents).expect("a config file");
    }

    /// The binary, with no router variables inherited and a home directory inside the sandbox, so
    /// that nothing the test does reaches the user's own directories.
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_buzz-router"));
        command
            .env("HOME", self.home())
            .env("USERPROFILE", self.home())
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("BUZZ_ROUTER_CONFIG_DIR")
            .env_remove("BUZZ_ROUTER_DATA_DIR")
            .env_remove("BUZZ_ROUTER_LOG")
            .stdin(Stdio::null());
        command
    }

    /// Runs `buzz-router --config-dir <config> --data-dir <data> <args>`.
    fn run(&self, args: &[&str]) -> Output {
        self.command()
            .arg("--config-dir")
            .arg(self.config())
            .arg("--data-dir")
            .arg(self.data())
            .args(args)
            .output()
            .expect("the binary starts")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Asserts that the command printed exactly `hash` (one trailing newline allowed) and exited 0.
fn assert_prints_hash(output: &Output, hash: &str) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "exit code; stdout: {:?}, stderr: {:?}",
        stdout(output),
        stderr(output)
    );
    let printed = stdout(output);
    assert_eq!(printed.strip_suffix('\n').unwrap_or(&printed), hash);
}

#[test]
fn a_valid_roster_prints_the_sha256_of_its_bytes_and_exits_zero() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", ROSTER);

    let output = sandbox.run(&["roster", "check"]);

    assert_prints_hash(&output, ROSTER_SHA256);
    assert_eq!(
        ROSTER_SHA256.len(),
        64,
        "the known answer is a SHA-256 digest"
    );
    assert!(
        ROSTER_SHA256
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "the known answer is lowercase hex"
    );
}

#[test]
fn the_hash_covers_the_raw_bytes_of_the_file() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", ROSTER.replace('\n', "\r\n"));

    let output = sandbox.run(&["roster", "check"]);

    assert_prints_hash(&output, ROSTER_CRLF_SHA256);
    assert_ne!(
        ROSTER_CRLF_SHA256, ROSTER_SHA256,
        "the two known answers must differ for this test to mean anything"
    );
}

#[test]
fn an_invalid_roster_gives_one_user_error_json_line_and_exit_one() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", INVALID_ROSTER);

    let output = sandbox.run(&["roster", "check"]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr: {:?}",
        stderr(&output)
    );
    assert_eq!(
        stdout(&output),
        "",
        "no hash is printed for an invalid roster"
    );
    let error = stderr(&output);
    assert_eq!(
        error.lines().count(),
        1,
        "stderr should be one line: {error:?}"
    );
    let json: Value = serde_json::from_str(error.trim_end()).expect("the error line is JSON");
    assert_eq!(json["error"], "user_error");
}

#[test]
fn without_router_toml_the_roster_is_read_from_the_config_dir() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", ROSTER);
    sandbox.write_config("other.toml", OTHER_ROSTER);
    assert!(!sandbox.config().join("router.toml").exists());

    let output = sandbox.run(&["roster", "check"]);

    assert_prints_hash(&output, ROSTER_SHA256);
}

#[test]
fn router_toml_roster_path_selects_the_roster_to_check() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", ROSTER);
    sandbox.write_config("other.toml", OTHER_ROSTER);
    sandbox.write_config("router.toml", "roster_path = \"other.toml\"\n");

    let output = sandbox.run(&["roster", "check"]);

    assert_prints_hash(&output, OTHER_ROSTER_SHA256);
}

#[test]
fn a_router_toml_without_roster_path_falls_back_to_roster_toml() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", ROSTER);
    sandbox.write_config("other.toml", OTHER_ROSTER);
    sandbox.write_config(
        "router.toml",
        "# no roster_path: the default is roster.toml\n",
    );

    let output = sandbox.run(&["roster", "check"]);

    assert_prints_hash(&output, ROSTER_SHA256);
}

#[test]
fn the_config_dir_flag_is_global_and_names_the_directory_that_is_read() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", ROSTER);

    let after_the_subcommand = sandbox
        .command()
        .env("BUZZ_ROUTER_DATA_DIR", sandbox.data())
        .args(["roster", "check", "--config-dir"])
        .arg(sandbox.config())
        .output()
        .expect("the binary starts");

    assert_prints_hash(&after_the_subcommand, ROSTER_SHA256);
}

#[test]
fn the_config_dir_variable_names_the_directory_that_is_read() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", ROSTER);

    let output = sandbox
        .command()
        .env("BUZZ_ROUTER_CONFIG_DIR", sandbox.config())
        .env("BUZZ_ROUTER_DATA_DIR", sandbox.data())
        .args(["roster", "check"])
        .output()
        .expect("the binary starts");

    assert_prints_hash(&output, ROSTER_SHA256);
}

#[test]
fn the_data_dir_flag_and_variable_are_accepted() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", ROSTER);

    let flag = sandbox
        .command()
        .args(["roster", "check", "--config-dir"])
        .arg(sandbox.config())
        .arg("--data-dir")
        .arg(sandbox.data())
        .output()
        .expect("the binary starts");
    let variable = sandbox
        .command()
        .env("BUZZ_ROUTER_CONFIG_DIR", sandbox.config())
        .env("BUZZ_ROUTER_DATA_DIR", sandbox.data())
        .args(["roster", "check"])
        .output()
        .expect("the binary starts");

    assert_prints_hash(&flag, ROSTER_SHA256);
    assert_prints_hash(&variable, ROSTER_SHA256);
}
