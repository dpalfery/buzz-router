//! Task 6.1: `keys set` and `keys check` (design section 11, requirements 54.1 to 54.4, A16).
//!
//! The in-process tests call `buzz_router::cli::keys::{set, check}` against `keyring_core`'s mock
//! store, installed once for this test binary. Each test uses its own bot names, because the
//! default store is shared by every test thread. The process test runs the real binary on an
//! invalid nsec, which is rejected before any keychain is touched.
//!
//! Bot keys are derived from public seeds, `sha256("buzz-router-fixture:" + name)`, so they are no
//! real key.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::fs;
use std::io::{Cursor, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use buzz_router::cli::keys::{check, set};
use buzz_router::cli::ErrorKind;
use buzz_router::paths::Dirs;
use nostr::nips::nip19::ToBech32;
use router_core::ids::BotName;
use sha2::{Digest, Sha256};

fn mock_store() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        keyring_core::set_default_store(keyring_core::mock::Store::new().unwrap());
    });
}

fn fixture_keys(name: &str) -> nostr::Keys {
    let secret = Sha256::digest(format!("buzz-router-fixture:{name}").as_bytes());
    nostr::Keys::parse(&hex::encode(secret)).unwrap()
}

fn nsec(name: &str) -> String {
    fixture_keys(name).secret_key().to_bech32().unwrap()
}

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

fn stored(name: &str) -> Option<String> {
    keyring_core::Entry::new("buzz-router", name)
        .unwrap()
        .get_password()
        .ok()
}

fn store(name: &str, secret: &str) {
    keyring_core::Entry::new("buzz-router", name)
        .unwrap()
        .set_password(secret)
        .unwrap();
}

/// A roster with every bot in `bots`, each with its fixture public key, and a router file that
/// serves every one of them from the keychain.
fn write_config(dir: &Path, bots: &[&str]) {
    let mut roster = String::from(
        r#"version = 1

[owner]
name = "Fixture Owner"
pubkeys = ["1111111111111111111111111111111111111111111111111111111111111111"]
timezone = "UTC"

[[channels]]
id = "00000000-0000-0000-0000-000000000001"
name = "fixture-room"
"#,
    );
    let mut router = String::from(
        r#"relay_url = "wss://relay.invalid"
api_bind = "127.0.0.1:47821"
tailnet_bind = ""
public_url = ""
roster_path = "roster.toml"
"#,
    );
    for name in bots {
        roster.push_str(&format!(
            "\n[[bots]]\nname = \"{name}\"\npubkey = \"{}\"\nchannels = [\"*\"]\nrespond_to = \"owner-only\"\n",
            fixture_keys(name).public_key().to_hex()
        ));
        router.push_str(&format!(
            r#"
[[bots]]
name = "{name}"
key = "keychain"
auth_tag = ""
max_concurrent = 1

[bots.adapter]
type = "command"
command = ["agent"]
cwd = "~/fixture"
env = {{}}
prompt_mode = "stdin"
reply_mode = "stdout"
prompt_template = ""
"#
        ));
    }
    fs::write(dir.join("roster.toml"), roster).unwrap();
    fs::write(dir.join("router.toml"), router).unwrap();
}

fn dirs(config: &Path, data: &Path) -> Dirs {
    Dirs::resolve(Some(config.to_path_buf()), Some(data.to_path_buf())).unwrap()
}

#[test]
fn set_stores_a_valid_nsec_read_from_a_reader() {
    mock_store();
    let secret = nsec("set-ok");
    set(&bot("set-ok"), &mut Cursor::new(format!("{secret}\n"))).unwrap();
    assert_eq!(stored("set-ok").as_deref(), Some(secret.as_str()));
}

#[test]
fn set_rejects_an_invalid_nsec_as_bad_input_and_stores_nothing() {
    mock_store();
    let err = set(&bot("set-bad"), &mut Cursor::new("nsec1notavalidkey\n")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadInput);
    assert_eq!(stored("set-bad"), None);
}

#[test]
fn set_rejects_a_hex_secret_because_it_requires_bech32() {
    mock_store();
    let hex = fixture_keys("set-hex").secret_key().to_secret_hex();
    let err = set(&bot("set-hex"), &mut Cursor::new(hex)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadInput);
    assert_eq!(stored("set-hex"), None);
}

#[test]
fn check_passes_when_every_key_matches_its_roster_pubkey() {
    mock_store();
    let (config, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_config(config.path(), &["check-ok-1", "check-ok-2"]);
    store("check-ok-1", &nsec("check-ok-1"));
    store("check-ok-2", &nsec("check-ok-2"));

    let mut out = Vec::new();
    check(&dirs(config.path(), data.path()), &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(out.lines().count(), 2, "{out}");
    assert!(
        out.contains("check-ok-1") && out.contains("check-ok-2"),
        "{out}"
    );
}

#[test]
fn check_fails_with_exit_three_when_a_key_does_not_match() {
    mock_store();
    let (config, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_config(config.path(), &["check-mm-1", "check-mm-2"]);
    store("check-mm-1", &nsec("check-mm-1"));
    store("check-mm-2", &nsec("someone-else"));

    let mut out = Vec::new();
    let err = check(&dirs(config.path(), data.path()), &mut out).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Key);
    assert_eq!(err.kind.exit_code(), 3);
    assert_eq!(err.kind.category(), "key_error");
    let out = String::from_utf8(out).unwrap();
    assert_eq!(out.lines().count(), 2, "one line per bot: {out}");
}

#[test]
fn check_fails_with_exit_three_when_an_entry_is_missing() {
    mock_store();
    let (config, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_config(config.path(), &["check-missing"]);

    let err = check(&dirs(config.path(), data.path()), &mut Vec::new()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Key);
}

#[test]
fn check_fails_with_exit_three_without_a_configuration() {
    mock_store();
    let (config, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());

    let err = check(&dirs(config.path(), data.path()), &mut Vec::new()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Key);
}

#[test]
fn keys_set_with_an_invalid_nsec_exits_one_without_any_configuration() {
    let (config, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut child = Command::new(env!("CARGO_BIN_EXE_buzz-router"))
        .args(["keys", "set", "--bot", "A", "--config-dir"])
        .arg(config.path())
        .arg("--data-dir")
        .arg(data.path())
        .env_remove("BUZZ_ROUTER_CONFIG_DIR")
        .env_remove("BUZZ_ROUTER_DATA_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"nsec1notavalidkey\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let line: serde_json::Value =
        serde_json::from_str(stderr.trim_end()).expect("one JSON error line");
    assert_eq!(line["error"], "user_error");
    assert_eq!(line["retryable"], false);
}
