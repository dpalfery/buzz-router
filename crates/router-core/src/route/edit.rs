//! The owner-edit rule of `route` (design section 5.5, `owner_edit`; requirement 16 and
//! assumption A11).
//!
//! An owner's kind-40003 edit can add a `p` tag for a bot that the original message forgot:
//!
//! - each `p` tag that names a roster bot, other than the author's own key, is a newly added
//!   mention: the bot is woken with `Mention` in a `Direct` wake and joins the participants
//!   (requirement 16.1);
//! - the edit's text is not read for mentions or control commands (requirements 16.4 and 16.6);
//! - the targets pass the Halted and Cap gates only, so quiet hours and the budgets do not stop an
//!   owner's wake (requirement 16.3);
//! - an edit starts no round and changes neither the round id nor its mode, so its wakes count as
//!   turns in the current round (requirement 16.2).
//!
//! An edit from anyone but the owner never reaches this rule (requirement 16.5).

use super::gates::gate;
use super::{
    consider, empty_result, Decision, Diagnostic, InEvent, Priority, Reason, RouteResult, Snapshot,
    SuppressWhy, ThreadUpdate,
};
use crate::parse::p_tag_bots;
use crate::thread::RoundMode;

/// The gates an edit's target passes, in the order they are checked (requirement 16.3).
const EDIT_GATES: [SuppressWhy; 2] = [SuppressWhy::Halted, SuppressWhy::Cap];

/// Routes the kind-40003 edit of the owner (design 5.5, `owner_edit`).
///
/// An edit whose edited message or thread the snapshot does not give wakes nobody and carries
/// [`Diagnostic::EditTargetUnknown`].
pub(super) fn owner_edit(ev: &InEvent, snap: &Snapshot<'_>) -> RouteResult {
    if snap.edit_target.is_none() || snap.thread.is_none() {
        return RouteResult {
            diagnostics: vec![Diagnostic::EditTargetUnknown],
            ..empty_result()
        };
    }

    // Only `p` tags count, never the text. A target that is not considered (not local, or not a
    // member of the channel) still joins the participants and gets no decision, as on the owner's
    // kind-9 path.
    let targets = p_tag_bots(ev, snap.roster);
    let decisions = targets
        .iter()
        .filter(|bot| consider(bot, ev.channel, snap))
        .map(|bot| match gate(bot, &EDIT_GATES, snap) {
            Some(why) => Decision::Suppress {
                bot: bot.clone(),
                why,
            },
            None => Decision::Wake {
                bot: bot.clone(),
                reason: Reason::Mention,
                priority: Priority::Owner,
                debounce: false,
            },
        })
        .collect();
    RouteResult {
        control: None,
        decisions,
        thread_update: ThreadUpdate {
            create: None,
            add_participants: targets,
            set_discussion: false,
            new_round: None,
        },
        wake_mode: RoundMode::Direct,
        diagnostics: Vec::new(),
    }
}
