//! Smoke test for task 1.1: prove the `keyring` ecosystem is pinned and usable
//! without touching a real OS credential store.
//!
//! `keyring` 4.x has no mock of its own. Its `Entry` is a wrapper that installs the
//! native store (macOS Keychain, Windows Credential Manager, Linux Secret Service)
//! on first use, so it cannot be redirected to a mock and would reach the real
//! keychain on a developer machine. The mock credential store lives in `keyring-core`,
//! and the entry type used here is `keyring_core::Entry`, resolved through the
//! process-wide default store.
//!
//! The test name carries the `smoke` substring because the task's run command,
//! `cargo test --workspace --locked smoke`, filters on test names, not file names.

use keyring_core::{mock, set_default_store, Entry};

const SERVICE: &str = "buzz-router-smoke";
const ACCOUNT: &str = "smoke-account";
const SECRET: &str = "smoke-test-secret";

#[test]
fn smoke_keyring_mock_store_round_trips_a_password() -> keyring_core::Result<()> {
    set_default_store(mock::Store::new()?);
    let entry = Entry::new(SERVICE, ACCOUNT)?;

    entry.set_password(SECRET)?;
    let stored = entry.get_password()?;

    assert_eq!(stored, SECRET);
    Ok(())
}
