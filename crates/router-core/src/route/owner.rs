//! The owner kind-9 rules of `route` (design section 5.5, `owner_message`; requirements 6 to 10).
//!
//! The first matching rule picks the targets, the reason and the round mode:
//!
//! - (a) `@everyone`: every roster bot that covers the channel, as a discussion (requirement 7).
//! - (b) explicit mentions: exactly the mentioned bots (requirement 6.2).
//! - (c) a reply to a roster bot's message, not the root's: that bot (requirement 8).
//! - (d) a reply in a thread with participants: the participants (requirement 9).
//! - (e) the channel's `default_bot`, for a top-level message or a thread with no participants
//!   (requirement 10.1).
//! - (f) otherwise nobody (requirement 10.2).
//!
//! Targets start a new round. Halted is the only gate on an owner's wake (requirements 6.7 and
//! 6.8).

use std::collections::BTreeSet;

use super::gates::gate;
use super::{
    consider, covers, empty_result, reply_target, Decision, InEvent, NewRound, NewThread, Priority,
    Reason, RouteResult, Snapshot, SuppressWhy, ThreadUpdate,
};
use crate::classify::AuthorClass;
use crate::ids::BotName;
use crate::parse::{contains_everyone, mention_text, mentioned_bots};
use crate::thread::{thread_position, RoundMode, ThreadPos};

/// What the first matching rule decided.
struct Targets {
    /// The bots the owner addressed. Not every one is considered: a target that is not local, or
    /// not a member of the channel, still joins the participants but gets no decision.
    bots: BTreeSet<BotName>,
    /// Why each of them is woken.
    reason: Reason,
    /// The mode of the new round.
    mode: RoundMode,
    /// Whether the thread becomes a discussion. Only `@everyone` sets it.
    set_discussion: bool,
}

impl Targets {
    /// A direct round for `bots` that does not start a discussion.
    fn direct(bots: BTreeSet<BotName>, reason: Reason) -> Self {
        Self {
            bots,
            reason,
            mode: RoundMode::Direct,
            set_discussion: false,
        }
    }
}

/// Routes an owner's kind-9 message (design 5.5, `owner_message`, steps 2 to 5).
///
/// Step 1, the control commands, is not part of this function yet.
pub(super) fn owner_message(ev: &InEvent, snap: &Snapshot<'_>) -> RouteResult {
    let mentioned = mentioned_bots(ev, &AuthorClass::Owner, snap.roster, None);
    let pos = thread_position(&ev.tags);

    let Some(Targets {
        bots,
        reason,
        mode,
        set_discussion,
    }) = pick_targets(ev, snap, &mentioned, &pos)
    else {
        return empty_result();
    };
    if bots.is_empty() {
        return empty_result();
    }

    let decisions = bots
        .iter()
        .filter(|bot| consider(bot, ev.channel, snap))
        .map(|bot| match gate(bot, &[SuppressWhy::Halted], snap) {
            Some(why) => Decision::Suppress {
                bot: bot.clone(),
                why,
            },
            None => Decision::Wake {
                bot: bot.clone(),
                reason,
                priority: Priority::Owner,
                debounce: false,
            },
        })
        .collect();

    let create = matches!(pos, ThreadPos::TopLevel).then(|| NewThread {
        root_id: ev.id.clone(),
        channel_id: ev.channel,
    });
    RouteResult {
        control: None,
        decisions,
        thread_update: ThreadUpdate {
            create,
            add_participants: bots,
            set_discussion,
            new_round: Some(NewRound {
                round_id: ev.id.clone(),
                mode,
                started_at: ev.created_at,
            }),
        },
        wake_mode: mode,
        diagnostics: Vec::new(),
    }
}

/// Finds the first rule, (a) to (e), that matches. `None` is rule (f): nobody.
fn pick_targets(
    ev: &InEvent,
    snap: &Snapshot<'_>,
    mentioned: &BTreeSet<BotName>,
    pos: &ThreadPos,
) -> Option<Targets> {
    // (a) `@everyone`, read on the text without code and quoted lines (requirements 7.1, 7.3).
    if contains_everyone(&mention_text(&ev.content)) {
        let bots = snap
            .roster
            .bots
            .keys()
            .filter(|bot| covers(bot, ev.channel, snap))
            .cloned()
            .collect();
        return Some(Targets {
            bots,
            reason: Reason::Everyone,
            mode: RoundMode::Discussion,
            set_discussion: true,
        });
    }

    // (b) explicit mentions beat the reply target (requirements 6.2, 6.3).
    if !mentioned.is_empty() {
        return Some(Targets::direct(mentioned.clone(), Reason::Mention));
    }

    // (c) a reply to a roster bot's message. A reply to the root is not one (requirement 8.2).
    if let Some(bot) = reply_target(pos, snap) {
        return Some(Targets::direct(
            BTreeSet::from([bot.name.clone()]),
            Reason::ReplyTarget,
        ));
    }

    // (d) a reply in a thread with participants. A thread the router has no state for has none
    // (requirement 9.1).
    if matches!(pos, ThreadPos::Reply { .. }) {
        if let Some(thread) = snap
            .thread
            .as_ref()
            .filter(|thread| !thread.participants.is_empty())
        {
            return Some(Targets {
                bots: thread.participants.clone(),
                reason: Reason::Participant,
                mode: if thread.discussion {
                    RoundMode::Discussion
                } else {
                    RoundMode::Direct
                },
                set_discussion: false,
            });
        }
    }

    // (e) the channel's default bot. Rule (d) took every reply in a thread with participants, so
    // what reaches this point is a top-level message or a reply in a thread with none
    // (requirement 10.1).
    let default_bot = snap
        .roster
        .channels
        .get(&ev.channel)?
        .default_bot
        .as_ref()?;
    Some(Targets::direct(
        BTreeSet::from([default_bot.clone()]),
        Reason::DefaultBot,
    ))
}
