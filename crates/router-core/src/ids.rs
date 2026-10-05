//! Identifier newtypes shared by every router-core module (design section 5.1).
//!
//! Each constructor validates its input, so a value of one of these types is always well formed.
//! Deserialising validates too (`try_from`), so a malformed identifier cannot enter through a
//! JSON or TOML file. All four serialise as plain strings.

use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Why an identifier could not be built from text.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdError {
    /// A bot name was empty or held only whitespace.
    #[error("a name must contain at least one non-whitespace character")]
    EmptyName,
    /// The text was not exactly 64 hexadecimal characters.
    #[error("expected 64 hexadecimal characters, got {0:?}")]
    NotHex64(String),
    /// The text was not a UUID.
    #[error("expected a UUID, got {0:?}")]
    NotUuid(String),
    /// The 64 hex characters do not describe a valid Nostr public key.
    #[error("{0:?} is not a valid Nostr public key")]
    BadPubkey(String),
}

/// Checks for 64 hexadecimal characters of either case and returns them lowercased.
fn lowercase_hex64(text: &str) -> Result<String, IdError> {
    if text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(text.to_ascii_lowercase())
    } else {
        Err(IdError::NotHex64(text.to_owned()))
    }
}

/// Implements `Display`, `FromStr`, `TryFrom<String>` and `From<$id> for String` for a newtype
/// over `String`, all through its validating constructor `$build(&str)`.
macro_rules! string_id {
    ($id:ident, $build:path) => {
        impl fmt::Display for $id {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $id {
            type Err = IdError;

            fn from_str(text: &str) -> Result<Self, IdError> {
                $build(text)
            }
        }

        impl TryFrom<String> for $id {
            type Error = IdError;

            fn try_from(text: String) -> Result<Self, IdError> {
                $build(text.as_str())
            }
        }

        impl From<$id> for String {
            fn from(id: $id) -> String {
                id.0
            }
        }
    };
}

/// The exact Buzz display name of a bot, such as `dp-grok-bot`. Case is kept as written.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BotName(String);

impl BotName {
    /// Builds a name, rejecting an empty or whitespace-only one.
    pub fn new(name: impl Into<String>) -> Result<Self, IdError> {
        let name = name.into();
        if name.trim().is_empty() {
            Err(IdError::EmptyName)
        } else {
            Ok(Self(name))
        }
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for BotName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

string_id!(BotName, BotName::new);

/// A Nostr public key as 64 lowercase hex characters.
///
/// Construction checks the shape only (64 hex characters, any case in, lowercase stored). It does
/// not check that the bytes are a point on the curve; [`Pubkey::to_nostr`] does.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Pubkey(String);

impl Pubkey {
    /// Builds a key from 64 hex characters of either case.
    pub fn from_hex(text: &str) -> Result<Self, IdError> {
        lowercase_hex64(text).map(Self)
    }

    /// Wraps a key that `nostr` has already validated.
    pub fn from_nostr(key: &nostr::PublicKey) -> Self {
        Self(key.to_hex())
    }

    /// Converts to a `nostr` key, which checks that the bytes are a valid public key.
    pub fn to_nostr(&self) -> Result<nostr::PublicKey, IdError> {
        nostr::PublicKey::from_hex(&self.0).map_err(|_| IdError::BadPubkey(self.0.clone()))
    }

    /// The 64 lowercase hex characters.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

string_id!(Pubkey, Pubkey::from_hex);

/// A Nostr event id as 64 lowercase hex characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EventId(String);

impl EventId {
    /// Builds an id from 64 hex characters of either case.
    pub fn from_hex(text: &str) -> Result<Self, IdError> {
        lowercase_hex64(text).map(Self)
    }

    /// Wraps an id that `nostr` has already validated.
    pub fn from_nostr(id: &nostr::EventId) -> Self {
        Self(id.to_hex())
    }

    /// The 64 lowercase hex characters.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

string_id!(EventId, EventId::from_hex);

/// A Buzz channel id, which is a UUID. Displays in the hyphenated lowercase form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChannelId(Uuid);

impl ChannelId {
    /// Parses a UUID in any form the `uuid` crate accepts.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        Uuid::parse_str(text)
            .map(Self)
            .map_err(|_| IdError::NotUuid(text.to_owned()))
    }

    /// The underlying UUID.
    pub fn uuid(&self) -> Uuid {
        self.0
    }
}

impl From<Uuid> for ChannelId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for ChannelId {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, IdError> {
        Self::parse(text)
    }
}
