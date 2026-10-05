//! The human and foreign-bot kind-9 rules of `route` (design section 5.5, `human_message` and
//! `foreign_message`; requirements 14 and 15).
//!
//! Both pick their targets the same way: the roster bots the message mentions, by text, `nostr:`
//! URI or `p` tag, and only when it mentions none, the roster bot whose message it replies to
//! (requirement 14.1, assumption A12). Neither counts `@everyone`, reads a control command,
//! starts a round or touches the thread (requirement 14.4), and neither affects the owner's or a
//! bot's wakes (requirement 14.5).
//!
//! - A human's target that answers only its owner gets `Suppress(RespondTo)`, before any other
//!   gate. One that answers anyone goes through the Halted, Quiet, Cap and Budget gates, in that
//!   order, and a pass is a `Human` wake that does not wait out the debounce (requirement 14).
//! - A foreign bot never wakes anyone: every target gets `Suppress(RespondTo)` (requirement 15).
//!   When its `auth` tag names one of the owner's keys, the result also carries a roster-drift
//!   diagnostic (requirement 4.4).

use std::collections::BTreeSet;

use super::gates::gate;
use super::{
    consider, empty_result, reply_target, Decision, Diagnostic, InEvent, Priority, Reason,
    RouteResult, Snapshot, SuppressWhy,
};
use crate::classify::AuthorClass;
use crate::config::RespondTo;
use crate::ids::BotName;
use crate::parse::mentioned_bots;
use crate::thread::thread_position;

/// The gates a human-caused target passes, in the order they are checked (requirement 14.3).
const HUMAN_GATES: [SuppressWhy; 4] = [
    SuppressWhy::Halted,
    SuppressWhy::Quiet,
    SuppressWhy::Cap,
    SuppressWhy::Budget,
];

/// The bots a message addresses, and why.
struct Targets {
    /// The addressed bots. Not every one is considered: a bot that is not local, or not a member
    /// of the channel, gets no decision.
    bots: BTreeSet<BotName>,
    /// `Mention` for the bots the message names, `ReplyTarget` for the bot it replies to.
    reason: Reason,
}

/// Routes the kind-9 message of a human (design 5.5, `human_message`).
pub(super) fn human_message(ev: &InEvent, snap: &Snapshot<'_>) -> RouteResult {
    let Targets { bots, reason } = targets(ev, snap);
    let decisions = bots
        .into_iter()
        .filter(|bot| consider(bot, ev.channel, snap))
        .map(|bot| match human_gate(&bot, snap) {
            Some(why) => Decision::Suppress { bot, why },
            None => Decision::Wake {
                bot,
                reason,
                priority: Priority::Human,
                debounce: false,
            },
        })
        .collect();
    RouteResult {
        decisions,
        ..empty_result()
    }
}

/// Routes the kind-9 message of a foreign bot (design 5.5, `foreign_message`).
///
/// `owner_is_ours` says whether the author's verified `auth` tag names one of `owner.pubkeys`
/// (design 5.3). The roster-drift diagnostic goes with every kind-9 message of such an author,
/// whether or not it addresses a bot; the caller logs it once per key (requirement 4.4).
pub(super) fn foreign_message(
    ev: &InEvent,
    snap: &Snapshot<'_>,
    owner_is_ours: bool,
) -> RouteResult {
    let Targets { bots, .. } = targets(ev, snap);
    let decisions = bots
        .into_iter()
        .filter(|bot| consider(bot, ev.channel, snap))
        .map(|bot| Decision::Suppress {
            bot,
            why: SuppressWhy::RespondTo,
        })
        .collect();
    let diagnostics = if owner_is_ours {
        vec![Diagnostic::RosterDrift {
            pubkey: ev.pubkey.clone(),
        }]
    } else {
        Vec::new()
    };
    RouteResult {
        decisions,
        diagnostics,
        ..empty_result()
    }
}

/// The targets of a human's or foreign bot's message: the bots it mentions, else the bot it
/// replies to. `@everyone` names nobody here, and a reply to the thread root is not a reply to a
/// bot (requirement 14.1).
///
/// A foreign bot's message is read as a human's, so its `p` tags count too (design 5.5: the same
/// target selection as a human; requirement 15.2: "in any form").
fn targets(ev: &InEvent, snap: &Snapshot<'_>) -> Targets {
    let mentioned = mentioned_bots(ev, &AuthorClass::Human, snap.roster, None);
    if !mentioned.is_empty() {
        return Targets {
            bots: mentioned,
            reason: Reason::Mention,
        };
    }
    let replied_to = reply_target(&thread_position(&ev.tags), snap)
        .map(|bot| bot.name.clone())
        .into_iter()
        .collect();
    Targets {
        bots: replied_to,
        reason: Reason::ReplyTarget,
    }
}

/// Why a human's message does not wake `bot`, if it does not: `RespondTo` first, for a bot that
/// answers only its owner (requirement 14.2), then the ordered limit gates (requirement 14.3).
fn human_gate(bot: &BotName, snap: &Snapshot<'_>) -> Option<SuppressWhy> {
    let answers_anyone = snap
        .roster
        .bots
        .get(bot)
        .is_some_and(|entry| entry.respond_to == RespondTo::Anyone);
    if answers_anyone {
        gate(bot, &HUMAN_GATES, snap)
    } else {
        Some(SuppressWhy::RespondTo)
    }
}
