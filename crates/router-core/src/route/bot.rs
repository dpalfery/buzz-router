//! The bot kind-9 rules of `route` (design section 5.5, `bot_message`; requirements 11 to 13).
//!
//! A roster bot's message never wakes its author and never starts an owner round:
//!
//! - the author joins the thread's participants (requirement 11.2);
//! - a top-level post creates its thread with a bot round in `Direct` mode (requirement 13.1);
//! - bots named in the text or by `nostr:` URI are woken with `BotMention` and join the
//!   participants. `p` tags and `@everyone` count for nothing here (requirements 11.3, 11.4 and
//!   11.8);
//! - in a `Discussion` round every other participant is woken with `Discussion`, unless the post
//!   mentions it, which keeps `BotMention` (requirements 12.1 and 12.2);
//! - every considered target goes through the Halted, Quiet, Cap and Budget gates, in that order
//!   (requirement 12.4), and a pass is a `Bot` wake that waits out the discussion debounce
//!   (requirement 12.5).
//!
//! A bot's message never starts a new round after the first one and never changes the round's
//! mode, so a bot round's turns are not reset until the owner posts in the thread (requirements
//! 11.5 and 13.2).

use std::collections::{BTreeMap, BTreeSet};

use super::gates::gate;
use super::{
    consider, Decision, InEvent, NewRound, NewThread, Priority, Reason, RouteResult, Snapshot,
    SuppressWhy, ThreadUpdate,
};
use crate::classify::AuthorClass;
use crate::ids::BotName;
use crate::parse::mentioned_bots;
use crate::thread::{thread_position, RoundMode, ThreadPos};

/// The gates a bot-caused target passes, in the order they are checked (requirement 12.4).
const BOT_CAUSED_GATES: [SuppressWhy; 4] = [
    SuppressWhy::Halted,
    SuppressWhy::Quiet,
    SuppressWhy::Cap,
    SuppressWhy::Budget,
];

/// Routes the kind-9 message of the roster bot `author` (design 5.5, `bot_message`).
pub(super) fn bot_message(ev: &InEvent, snap: &Snapshot<'_>, author: &BotName) -> RouteResult {
    // Step 1: the author takes part, and a top-level post starts the bot round.
    let mut add_participants = BTreeSet::from([author.clone()]);
    let (create, new_round) = match thread_position(&ev.tags) {
        ThreadPos::TopLevel => (
            Some(NewThread {
                root_id: ev.id.clone(),
                channel_id: ev.channel,
            }),
            Some(NewRound {
                round_id: ev.id.clone(),
                mode: RoundMode::Direct,
                started_at: ev.created_at,
            }),
        ),
        ThreadPos::Reply { .. } => (None, None),
    };

    // Step 2: text and URI mentions, never `p` tags, never the author. A mentioned bot joins the
    // participants whether or not it is considered or passes its gates.
    let mentioned = mentioned_bots(
        ev,
        &AuthorClass::Bot(author.clone()),
        snap.roster,
        Some(author),
    );
    add_participants.extend(mentioned.iter().cloned());
    let mut targets: BTreeMap<BotName, Reason> = mentioned
        .into_iter()
        .map(|bot| (bot, Reason::BotMention))
        .collect();

    // Step 3: in a `Discussion` round (the round's mode, not the thread's `discussion` flag) every
    // other participant is a target. A mention keeps its `BotMention` reason.
    let discussion_participants = snap
        .thread
        .iter()
        .filter(|thread| thread.round_mode == RoundMode::Discussion)
        .flat_map(|thread| thread.participants.iter())
        .filter(|bot| *bot != author);
    for bot in discussion_participants {
        targets.entry(bot.clone()).or_insert(Reason::Discussion);
    }

    // Step 4: only considered bots get a decision, in bot-name order.
    let decisions = targets
        .into_iter()
        .filter(|(bot, _)| consider(bot, ev.channel, snap))
        .map(|(bot, reason)| match gate(&bot, &BOT_CAUSED_GATES, snap) {
            Some(why) => Decision::Suppress { bot, why },
            None => Decision::Wake {
                bot,
                reason,
                priority: Priority::Bot,
                debounce: true,
            },
        })
        .collect();

    // Step 5: the wakes carry the round's mode. A new thread has no state yet, and its bot round
    // is `Direct`.
    let wake_mode = snap
        .thread
        .as_ref()
        .map_or(RoundMode::Direct, |thread| thread.round_mode);

    RouteResult {
        control: None,
        decisions,
        thread_update: ThreadUpdate {
            create,
            add_participants,
            set_discussion: false,
            new_round,
        },
        wake_mode,
        diagnostics: Vec::new(),
    }
}
