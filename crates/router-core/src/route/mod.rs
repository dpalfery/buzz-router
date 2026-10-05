//! The routing types and the `route` function (design section 5.2, requirement 5).
//!
//! `route` is pure. It does no I/O and reads no clock except its `now` argument. It is a
//! placeholder for now: it returns no decisions. Tasks 1.6 to 1.9 replace it with the routing
//! rules.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::Roster;
use crate::ids::{BotName, ChannelId, EventId, Pubkey};
use crate::thread::{RoundMode, ThreadState};

/// The kind of a Buzz stream message (9).
pub const KIND_MESSAGE: u16 = buzz_core::kind::KIND_STREAM_MESSAGE as u16;
/// The kind of an edit of a stream message (40003).
pub const KIND_EDIT: u16 = buzz_core::kind::KIND_STREAM_MESSAGE_EDIT as u16;

/// A signature-verified kind-9 or kind-40003 event. The binary builds it from `nostr::Event`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InEvent {
    /// The event id.
    pub id: EventId,
    /// The author's public key.
    pub pubkey: Pubkey,
    /// The event kind.
    pub kind: u16,
    /// Creation time in unix seconds.
    pub created_at: i64,
    /// The channel, from the `h` tag. Events without one are not routed.
    pub channel: ChannelId,
    /// The message text.
    pub content: String,
    /// The raw tag arrays.
    pub tags: Vec<Vec<String>>,
}

/// Halts set by `stop` (requirement 29).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Halts {
    /// Every bot is halted.
    pub all: bool,
    /// These bots are halted.
    pub bots: BTreeSet<BotName>,
}

/// A bot's dispatched wakes over the trailing hour and day (assumption A10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeCounts {
    /// Wakes in the trailing 60 minutes.
    pub hour: u32,
    /// Wakes in the trailing 24 hours.
    pub day: u32,
}

/// The message an edit event edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditTarget {
    /// The edited kind-9 message.
    pub message_id: EventId,
}

/// Everything `route` needs to know besides the event and the time (requirement 5.4).
#[derive(Debug)]
pub struct Snapshot<'a> {
    /// The roster.
    pub roster: &'a Roster,
    /// The bots this router serves.
    pub local_bots: &'a BTreeSet<BotName>,
    /// Local bots that are members of the event's channel (DD-5).
    pub local_members: BTreeSet<BotName>,
    /// The current halts.
    pub halts: &'a Halts,
    /// The event's thread. For an edit, the thread of the edited message.
    pub thread: Option<ThreadState>,
    /// The author of the reply parent, when the parent is not the root.
    pub parent_author: Option<Pubkey>,
    /// The edited message, for an edit event.
    pub edit_target: Option<EditTarget>,
    /// Per-bot trailing wake counts.
    pub wake_counts: &'a BTreeMap<BotName, WakeCounts>,
    /// Bots inside quiet hours at `now`.
    pub quiet: BTreeSet<BotName>,
}

/// What `route` decided about one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteResult {
    /// A control command found in the event.
    pub control: Option<Control>,
    /// At most one decision per local bot, sorted by bot name.
    pub decisions: Vec<Decision>,
    /// How the event changes its thread's state.
    pub thread_update: ThreadUpdate,
    /// The round mode carried by this event's wakes (DD-3).
    pub wake_mode: RoundMode,
    /// Notes for the caller to log, such as roster drift.
    pub diagnostics: Vec<Diagnostic>,
}

/// A control command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// Halt wakes.
    Stop(Scope),
    /// Lift a halt.
    Resume(Scope),
    /// Cancel running wakes.
    Cancel(Scope),
}

/// The bots a control command applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every bot.
    All,
    /// The named bots.
    Bots(BTreeSet<BotName>),
}

/// What to do about one bot for one event (requirement 5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Wake the bot.
    Wake {
        /// The bot to wake.
        bot: BotName,
        /// Why it is woken.
        reason: Reason,
        /// How the wake queues.
        priority: Priority,
        /// Whether the wake waits out the discussion debounce.
        debounce: bool,
    },
    /// Do not wake the bot.
    Suppress {
        /// The bot.
        bot: BotName,
        /// Why not.
        why: SuppressWhy,
    },
}

/// Why a bot is woken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Named in the message.
    Mention,
    /// The owner wrote `@everyone`.
    Everyone,
    /// The message replies to the bot's own message.
    ReplyTarget,
    /// The bot takes part in the thread.
    Participant,
    /// The channel's default bot.
    DefaultBot,
    /// Another bot's post in a discussion.
    Discussion,
    /// Named in a bot's message.
    BotMention,
}

/// How a wake queues. `Owner > Human > Bot`, so the variants run from lowest to highest and the
/// derived order is the queueing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Caused by a bot's message.
    Bot,
    /// Caused by a human's message.
    Human,
    /// Caused by the owner's message.
    Owner,
}

/// Why a bot that would be woken is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressWhy {
    /// The bot or all bots are halted.
    Halted,
    /// The bot used its turns for the round.
    Cap,
    /// The bot is inside quiet hours.
    Quiet,
    /// The bot used its hourly or daily wakes.
    Budget,
    /// The author is not one the bot responds to.
    RespondTo,
}

/// A thread created by a top-level post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThread {
    /// The thread root.
    pub root_id: EventId,
    /// The thread's channel.
    pub channel_id: ChannelId,
}

/// A new round in a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRound {
    /// The event that starts the round.
    pub round_id: EventId,
    /// The round's mode.
    pub mode: RoundMode,
    /// When the round starts, in unix seconds.
    pub started_at: i64,
}

/// How an event changes its thread's state (requirement 5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadUpdate {
    /// The thread to create, for a top-level post.
    pub create: Option<NewThread>,
    /// Bots that join the participants.
    pub add_participants: BTreeSet<BotName>,
    /// Whether the thread becomes a discussion.
    pub set_discussion: bool,
    /// The round to start.
    pub new_round: Option<NewRound>,
}

/// A note `route` leaves for its caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    /// A foreign bot carries an `auth` tag from one of the owner's keys, so it probably belongs in
    /// the roster (requirement 4.4).
    RosterDrift {
        /// The foreign bot's key.
        pubkey: Pubkey,
    },
    /// The event is tagged as a router status note and wakes nobody (requirement 4.5).
    StatusTagIgnored,
}

/// Decides what to do about one event.
///
/// This is a placeholder: it returns no decisions and no thread changes, in `Direct` mode.
pub fn route(_ev: &InEvent, _snap: &Snapshot<'_>, _now: DateTime<Utc>) -> RouteResult {
    RouteResult {
        control: None,
        decisions: Vec::new(),
        thread_update: ThreadUpdate {
            create: None,
            add_participants: BTreeSet::new(),
            set_discussion: false,
            new_round: None,
        },
        wake_mode: RoundMode::Direct,
        diagnostics: Vec::new(),
    }
}
