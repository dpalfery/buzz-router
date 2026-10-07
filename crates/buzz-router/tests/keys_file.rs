//! RED tests for T2.2: signing keys from files (design section 11).
//!
//! The implementation (`buzz_router::keys::{KeySource, load_key}`) does not
//! exist yet. These tests define its contract:
//!
//! - `load_key(&KeySource, &BotName) -> Result<nostr::Keys, KeyError>`,
//!   where `BotName` is `router_core::ids::BotName`.
//! - `KeySource::File(path)` reads, trims and parses the file with
//!   `nostr::Keys::parse` (hex or nsec-bech32).
//! - `KeySource::Keychain` returns `KeyError::Unsupported` (keychain loading
//!   comes in task 6.1).
//! - Expected `KeyError` shapes: `Io { .. }` (missing/unreadable file),
//!   `Permissions { .. }` (Unix open permissions, Display names the path),
//!   `Unsupported` (keychain, unit variant).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use buzz_router::keys::{load_key, KeyError, KeySource};
use nostr::nips::nip19::ToBech32;
use router_core::ids::BotName;

fn bot_name() -> BotName {
    BotName::new("test-bot").unwrap()
}

/// Writes `contents` to `path`, restricting it to 0600 on Unix so the
/// permission check passes (`std::fs::write` creates 0644 under a 022 umask).
fn write_key_file(path: &std::path::Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn nsec_bech32_file_loads() {
    let keys = nostr::Keys::generate();
    let bech32 = keys.secret_key().to_bech32().unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bot.nsec");
    write_key_file(&path, &bech32);

    let loaded = load_key(&KeySource::File(path), &bot_name()).unwrap();
    assert_eq!(loaded.public_key(), keys.public_key());
}

#[test]
fn hex_secret_file_loads() {
    let keys = nostr::Keys::generate();
    let hex = keys.secret_key().to_secret_hex();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bot.hex");
    write_key_file(&path, &hex);

    let loaded = load_key(&KeySource::File(path), &bot_name()).unwrap();
    assert_eq!(loaded.public_key(), keys.public_key());
}

#[test]
fn surrounding_whitespace_is_trimmed() {
    let keys = nostr::Keys::generate();
    let hex = keys.secret_key().to_secret_hex();
    let padded = format!("  \n\t{hex}\n  ");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bot.padded");
    write_key_file(&path, &padded);

    let loaded = load_key(&KeySource::File(path), &bot_name()).unwrap();
    assert_eq!(loaded.public_key(), keys.public_key());
}

#[cfg(unix)]
#[test]
fn open_permissions_are_rejected_on_unix() {
    use std::os::unix::fs::PermissionsExt;

    let keys = nostr::Keys::generate();
    let hex = keys.secret_key().to_secret_hex();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bot.key");
    std::fs::write(&path, &hex).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let err = load_key(&KeySource::File(path.clone()), &bot_name()).unwrap_err();
    assert!(
        matches!(err, KeyError::Permissions { .. }),
        "expected KeyError::Permissions, got {err:?}"
    );
    let message = err.to_string();
    assert!(
        message.contains(&path.display().to_string()),
        "Permissions error should name the path {path:?}, got {message:?}"
    );

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let loaded = load_key(&KeySource::File(path), &bot_name()).unwrap();
    assert_eq!(loaded.public_key(), keys.public_key());
}

#[test]
fn missing_file_gives_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.key");

    let err = load_key(&KeySource::File(path), &bot_name()).unwrap_err();
    assert!(
        matches!(err, KeyError::Io { .. }),
        "expected KeyError::Io, got {err:?}"
    );
}

#[test]
fn keychain_source_is_unsupported() {
    let err = load_key(&KeySource::Keychain, &bot_name()).unwrap_err();
    assert!(
        matches!(err, KeyError::Unsupported),
        "expected KeyError::Unsupported, got {err:?}"
    );
}
