//! The wake payload given to adapters (design section 5.7, requirement 40, brief section 9.3).

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Serialize, Serializer};

use crate::route::Reason;
use crate::thread::RoundMode;

/// The JSON payload describing one wake (requirement 40.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WakePayload {
    /// The wake's identifier.
    pub wake_id: uuid::Uuid,
    /// The wake token, in hex.
    pub token: String,
    /// The bot's name.
    pub bot: String,
    /// The channel the wake is in.
    pub channel: ChannelRef,
    /// The wake's thread root (requirement 40.7).
    pub thread_root_id: String,
    /// The parent the reply will be threaded under (requirement 40.7).
    pub reply_parent_id: String,
    /// Why the bot was woken, in snake case (requirement 40.2).
    pub reason: Reason,
    /// `"direct"` or `"discussion"` (requirement 40.2).
    #[serde(serialize_with = "serialize_round_mode")]
    pub round_mode: RoundMode,
    /// From [`turns_left_after_this`] (requirement 40.3).
    pub turns_left_after_this: u32,
    /// The bot's effective limit (requirement 40.3).
    pub turns_per_round: u32,
    /// From [`format_deadline`] (requirement 40.2).
    pub deadline: String,
    /// The event ids of every trigger coalesced into the wake (requirement 40.4).
    pub triggers: Vec<String>,
    /// The last messages of the thread, oldest first (requirement 40.5).
    pub context: Vec<ContextMessage>,
    /// Where the agent answers (requirement 40.6).
    pub api: ApiRef,
}

/// A channel's id and display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelRef {
    /// The channel UUID.
    pub id: String,
    /// The channel name.
    pub name: String,
}

/// One thread message in the payload context (requirement 40.5, DD-17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextMessage {
    /// The event id, in hex.
    pub id: String,
    /// The roster bot name, `owner.name`, or a short npub.
    pub author: String,
    /// The author's class, such as `"owner"`.
    pub class: String,
    /// When the message was created.
    pub created_at: String,
    /// The message text.
    pub text: String,
    /// Whether the message is new since this bot's last turn in the thread.
    pub new: bool,
}

/// The API base URL and endpoint paths (requirement 40.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiRef {
    /// The loopback API address for command bots, `public_url` for webhook bots.
    pub url: String,
    /// Always `/v1/post`.
    pub post: String,
    /// Always `/v1/pass`.
    pub pass: String,
    /// Always `/v1/eta`.
    pub eta: String,
}

impl ApiRef {
    /// The API reference for base `url` with the fixed endpoint paths.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            post: "/v1/post".to_owned(),
            pass: "/v1/pass".to_owned(),
            eta: "/v1/eta".to_owned(),
        }
    }
}

/// `turns_per_round` minus the turns used including this wake, never below zero
/// (requirement 40.3).
pub fn turns_left_after_this(turns_per_round: u32, turns_used_after: u32) -> u32 {
    turns_per_round.saturating_sub(turns_used_after)
}

/// A UTC timestamp as RFC 3339 with whole seconds and a `Z` suffix, such as
/// `2026-10-05T03:20:00Z`.
pub fn format_deadline(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn serialize_round_mode<S: Serializer>(mode: &RoundMode, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(match mode {
        RoundMode::Direct => "direct",
        RoundMode::Discussion => "discussion",
    })
}
