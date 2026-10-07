//! Bot signing keys (design section 11, requirements 54 and 59).
//!
//! [`load_key`] loads a bot's [`nostr::Keys`] from a [`KeySource`]. File keys
//! are read, trimmed and parsed with [`nostr::Keys::parse`], which accepts
//! either bech32 (`nsec1…`) or hex. On Unix a key file whose permissions grant
//! anything to group or others is refused (assumption A3, requirement R59.2).
//! Keychain loading arrives in task 6.1; until then [`KeySource::Keychain`]
//! returns [`KeyError::Unsupported`].

use std::path::PathBuf;

use router_core::ids::BotName;

/// Where a bot's signing key comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// A file holding the secret (bech32 or hex), read with the Unix
    /// permission check.
    File(PathBuf),
    /// The OS keychain (service `buzz-router`, account `<bot name>`).
    /// Not implemented until task 6.1.
    Keychain,
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
    /// The keychain source, which arrives in task 6.1.
    #[error("keychain key loading is not implemented yet")]
    Unsupported,
    /// The file contents are not a valid secret.
    #[error("cannot parse the key: {0}")]
    Parse(String),
}

/// Loads the signing key for `bot` from `source`.
///
/// File contents are trimmed before parsing, so a trailing newline is fine.
pub fn load_key(source: &KeySource, _bot: &BotName) -> Result<nostr::Keys, KeyError> {
    match source {
        KeySource::Keychain => Err(KeyError::Unsupported),
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
