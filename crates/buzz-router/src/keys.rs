//! Bot signing keys (design section 11, requirements 54 and 59).
//!
//! [`load_key`] loads a bot's [`nostr::Keys`] from a [`KeySource`]. File keys
//! are read, trimmed and parsed with [`nostr::Keys::parse`], which accepts
//! either bech32 (`nsec1…`) or hex. On Unix a key file whose permissions grant
//! anything to group or others is refused (assumption A3, requirement R59.2).
//! Keychain keys are the entry with service [`KEYCHAIN_SERVICE`] and the bot
//! name as account (DD-20, requirement R59.1).
//!
//! Entries are `keyring_core::Entry` values in the process's default store.
//! Production code calls [`use_native_store`] once first; tests install
//! `keyring_core::mock::Store` instead and never call it.

use std::path::{Path, PathBuf};

use router_core::ids::BotName;

/// The keychain service name every bot entry uses.
pub const KEYCHAIN_SERVICE: &str = "buzz-router";

/// Where a bot's signing key comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// A file holding the secret (bech32 or hex), read with the Unix
    /// permission check.
    File(PathBuf),
    /// The OS keychain (service `buzz-router`, account `<bot name>`).
    Keychain,
}

impl KeySource {
    /// The source a configured bot names. A relative file path is taken
    /// relative to `config_dir`.
    pub fn from_config(source: &router_core::config::KeySource, config_dir: &Path) -> Self {
        match source {
            router_core::config::KeySource::Keychain => Self::Keychain,
            router_core::config::KeySource::File(path) => Self::File(config_dir.join(path)),
        }
    }
}

/// A failure to load a bot's signing key.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// The file could not be read.
    #[error("cannot read the key file at {path}: {source}")]
    Io {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        source: std::io::Error,
    },
    /// The file's permissions are too open (Unix only).
    #[error("refusing to read the key file at {path}: group or other permissions are set")]
    Permissions {
        /// The file that was refused.
        path: PathBuf,
    },
    /// The keychain entry could not be read or written.
    #[error("keychain entry {KEYCHAIN_SERVICE}/{bot}: {message}")]
    Keychain {
        /// The bot whose entry it is.
        bot: String,
        /// What the keychain reported.
        message: String,
    },
    /// The file contents are not a valid secret.
    #[error("cannot parse the key: {0}")]
    Parse(String),
}

/// Installs the platform credential store as the process default. Call it
/// once before the first keychain access; never in tests, where it would
/// replace the mock store.
pub fn use_native_store() -> Result<(), KeyError> {
    keyring::Entry::store_status()
        .as_ref()
        .map_err(|error| KeyError::Keychain {
            bot: String::from("*"),
            message: format!("no usable OS keychain: {error}"),
        })?;
    Ok(())
}

/// The keychain entry for `bot`.
fn keychain_entry(bot: &BotName) -> Result<keyring_core::Entry, KeyError> {
    keyring_core::Entry::new(KEYCHAIN_SERVICE, bot.as_str())
        .map_err(|error| keychain_error(bot, &error))
}

fn keychain_error(bot: &BotName, error: &keyring_core::Error) -> KeyError {
    KeyError::Keychain {
        bot: bot.to_string(),
        message: error.to_string(),
    }
}

/// Stores `secret` as the keychain entry for `bot`.
pub fn store_keychain_secret(bot: &BotName, secret: &str) -> Result<(), KeyError> {
    keychain_entry(bot)?
        .set_password(secret)
        .map_err(|error| keychain_error(bot, &error))
}

/// Loads the signing key for `bot` from `source`.
///
/// The secret is trimmed before parsing, so a trailing newline is fine.
pub fn load_key(source: &KeySource, bot: &BotName) -> Result<nostr::Keys, KeyError> {
    match source {
        KeySource::Keychain => {
            let secret = keychain_entry(bot)?
                .get_password()
                .map_err(|error| keychain_error(bot, &error))?;
            nostr::Keys::parse(secret.trim()).map_err(|error| KeyError::Parse(error.to_string()))
        }
        KeySource::File(path) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;

                let mode = std::fs::metadata(path)
                    .map(|metadata| metadata.permissions().mode())
                    .map_err(|source| KeyError::Io {
                        path: path.clone(),
                        source,
                    })?;
                if mode & 0o077 != 0 {
                    return Err(KeyError::Permissions { path: path.clone() });
                }
            }
            let text = std::fs::read_to_string(path).map_err(|source| KeyError::Io {
                path: path.clone(),
                source,
            })?;
            nostr::Keys::parse(text.trim()).map_err(|error| KeyError::Parse(error.to_string()))
        }
    }
}
