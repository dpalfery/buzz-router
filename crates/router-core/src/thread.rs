//! Thread state and an event's position in its thread (design section 5.2).
//!
//! [`thread_position`] reads an event's NIP-10 `e` tags through Buzz's own parser, so the router
//! and the relay agree on every thread (requirements 8.2 and 62.4).

use std::collections::{BTreeMap, BTreeSet};

use buzz_core::nip10::parse_thread_markers_from_parts;

use crate::ids::{BotName, ChannelId, EventId};

/// Where an event sits in its thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadPos {
    /// The event starts a thread and is its own root.
    TopLevel,
    /// The event replies within the thread rooted at `root`. `parent == root` is a direct reply
    /// to the root.
    Reply {
        /// The thread root.
        root: EventId,
        /// The message this event replies to.
        parent: EventId,
    },
}

/// Whether a round's wakes are direct or part of a discussion (requirement 18.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundMode {
    /// The owner addressed specific bots.
    Direct,
    /// The owner started an `@everyone` discussion.
    Discussion,
}

/// What the router keeps for one thread (requirement 18.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadState {
    /// The thread root.
    pub root_id: EventId,
    /// The channel the thread lives in.
    pub channel_id: ChannelId,
    /// Roster bots taking part in the thread.
    pub participants: BTreeSet<BotName>,
    /// Whether the thread is a discussion. Once true it stays true (requirement 7.6).
    pub discussion: bool,
    /// The event that started the current round.
    pub round_id: EventId,
    /// The mode of the current round.
    pub round_mode: RoundMode,
    /// When the current round started, in unix seconds.
    pub round_started_at: i64,
    /// Wakes dispatched per bot in the current round (requirements 5.4 and 18.1).
    pub turns_used: BTreeMap<BotName, u32>,
}

/// Resolves an event's raw tag arrays to its thread position.
///
/// A `root` and a `reply` marker give `Reply { root, parent }`. A `reply` marker alone is a direct
/// reply, so `parent == root`. Anything else, including a lone `root` marker, is `TopLevel`.
pub fn thread_position(tags: &[Vec<String>]) -> ThreadPos {
    let markers = parse_thread_markers_from_parts(tags.iter().map(Vec::as_slice));
    match markers.resolve() {
        Some((root, parent)) => match (EventId::from_hex(&root), EventId::from_hex(&parent)) {
            (Ok(root), Ok(parent)) => ThreadPos::Reply { root, parent },
            // Buzz only reports ids it has checked as 64 hex characters, which `EventId` accepts
            // too, so this arm is not reached. It keeps the function total without a panic.
            _ => ThreadPos::TopLevel,
        },
        None => ThreadPos::TopLevel,
    }
}
