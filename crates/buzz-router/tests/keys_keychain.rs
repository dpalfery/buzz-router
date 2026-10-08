//! Task 6.1: `KeySource::Keychain` reads the OS keychain entry with service `buzz-router` and the
//! bot name as account (design section 11, DD-20, requirement 59.1).
//!
//! Runs against `keyring_core`'s in-memory mock store, installed once for this test binary. Each
//! test uses its own bot names, because the default store is shared by every test thread.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::sync::OnceLock;

use buzz_router::keys::{load_key, KeyError, KeySource, KEYCHAIN_SERVICE};
use nostr::nips::nip19::ToBech32;
use router_core::ids::BotName;

fn mock_store() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        keyring_core::set_default_store(keyring_core::mock::Store::new().unwrap());
    });
}

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

#[test]
fn the_service_is_buzz_router() {
    assert_eq!(KEYCHAIN_SERVICE, "buzz-router");
}

#[test]
fn a_keychain_source_reads_the_entry_for_the_bot() {
    mock_store();
    let keys = nostr::Keys::generate();
    let nsec = keys.secret_key().to_bech32().unwrap();
    keyring_core::Entry::new("buzz-router", "A")
        .unwrap()
        .set_password(&nsec)
        .unwrap();

    let loaded = load_key(&KeySource::Keychain, &bot("A")).unwrap();
    assert_eq!(loaded.public_key(), keys.public_key());
}

#[test]
fn surrounding_whitespace_in_the_entry_is_trimmed() {
    mock_store();
    let keys = nostr::Keys::generate();
    let nsec = keys.secret_key().to_bech32().unwrap();
    keyring_core::Entry::new("buzz-router", "trimmed")
        .unwrap()
        .set_password(&format!("{nsec}\n"))
        .unwrap();

    let loaded = load_key(&KeySource::Keychain, &bot("trimmed")).unwrap();
    assert_eq!(loaded.public_key(), keys.public_key());
}

#[test]
fn a_missing_entry_is_a_keychain_error_naming_the_bot() {
    mock_store();
    let err = load_key(&KeySource::Keychain, &bot("absent")).unwrap_err();
    assert!(
        matches!(err, KeyError::Keychain { .. }),
        "expected KeyError::Keychain, got {err:?}"
    );
    assert!(err.to_string().contains("absent"), "{err}");
}

#[test]
fn an_entry_that_is_not_a_key_is_a_parse_error() {
    mock_store();
    keyring_core::Entry::new("buzz-router", "garbage")
        .unwrap()
        .set_password("not a key")
        .unwrap();
    let err = load_key(&KeySource::Keychain, &bot("garbage")).unwrap_err();
    assert!(
        matches!(err, KeyError::Parse(_)),
        "expected KeyError::Parse, got {err:?}"
    );
}
