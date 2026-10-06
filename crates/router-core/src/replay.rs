//! The offline replay simulator (design section 5.8, requirements 55.2 to 55.4, assumption A17).
//!
//! A [`Replayer`] feeds recorded events through [`route`] one at a time and keeps the state the
//! daemon would keep between them, so the decisions it returns are the ones a router that had seen
//! the same events would have made. It does no I/O and reads no clock: the clock is each event's
//! own `created_at`.
//!
//! For every event, [`Replayer::step`]:
//!
//! 1. derives `now` from `created_at`;
//! 2. builds a [`Snapshot`] from the simulator's own state: its thread map, its index of event id
//!    to author and root (for parent authors and edit targets), its halts, and the wake counts and
//!    quiet set for that `now`;
//! 3. calls [`route`];
//! 4. applies the thread update and the control, and counts every `Wake` as dispatched at once, so
//!    the Cap and Budget gates progress as they would live.
//!
//! Every roster bot is local, and a member of every channel it covers (assumption A17).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use crate::config::{ChannelScope, Roster};
use crate::ids::{BotName, ChannelId, EventId, Pubkey};
use crate::quiet::quiet_set;
use crate::route::{
    route, Control, Decision, EditTarget, Halts, InEvent, RouteResult, Scope, Snapshot,
    ThreadUpdate, WakeCounts, KIND_EDIT, KIND_MESSAGE,
};
use crate::thread::{thread_position, RoundMode, ThreadPos, ThreadState};

/// The length of the hourly budget window, in seconds (assumption A10).
const HOUR_SECS: i64 = 3_600;
/// The length of the daily budget window, in seconds (assumption A10).
const DAY_SECS: i64 = 86_400;

/// Replays recorded events through [`route`] with a simulated clock (design 5.8).
///
/// Feed the events in the order they happened. The replayer holds the threads, halts and wake
/// history that those events build up, and nothing else: it publishes nothing and dispatches
/// nothing.
#[derive(Debug)]
pub struct Replayer {
    /// The roster every event is routed against.
    roster: Roster,
    /// Every roster bot: the replayer serves all of them (assumption A17).
    local_bots: BTreeSet<BotName>,
    /// The halts that the controls replayed so far have set.
    halts: Halts,
    /// The state of every thread the replayer has seen start, by thread root.
    threads: BTreeMap<EventId, ThreadState>,
    /// Every kind-9 event seen so far, for parent authors and edit targets.
    seen: BTreeMap<EventId, Seen>,
    /// For each bot, the simulated time of each wake it has been dispatched.
    dispatched: BTreeMap<BotName, Vec<i64>>,
}

/// What the replayer remembers about a kind-9 event it has routed.
#[derive(Debug)]
struct Seen {
    /// The event's author.
    author: Pubkey,
    /// The root of the event's thread. A top-level event is its own root.
    root: EventId,
}

/// Where an event sits among the things the replayer has seen.
struct Placement {
    /// The root of the event's thread, or of the edited message's thread for an edit. `None` for
    /// an edit whose target the replayer has not seen, and for any kind `route` ignores.
    root: Option<EventId>,
    /// The state of that thread, if the replayer has seen it start.
    thread: Option<ThreadState>,
    /// The author of the reply parent, when the parent is not the root and has been seen.
    parent_author: Option<Pubkey>,
    /// The edited message, for an edit whose target has been seen.
    edit_target: Option<EditTarget>,
}

impl Replayer {
    /// Starts a replay against `roster`, with no threads, no halts and no wakes yet. Every roster
    /// bot is local.
    #[must_use]
    pub fn new(roster: Roster) -> Self {
        let local_bots = roster.bots.keys().cloned().collect();
        Self {
            roster,
            local_bots,
            halts: Halts {
                all: false,
                bots: BTreeSet::new(),
            },
            threads: BTreeMap::new(),
            seen: BTreeMap::new(),
            dispatched: BTreeMap::new(),
        }
    }

    /// Routes `ev` at the simulated time `ev.created_at` and returns what [`route`] decided.
    ///
    /// The thread update and the control in the result are applied before the next event, and
    /// each `Wake` counts as a turn in its thread's current round and toward the bot's hourly and
    /// daily budgets, as if it had been dispatched at once.
    pub fn step(&mut self, ev: &InEvent) -> RouteResult {
        let now = simulated_now(ev.created_at);
        let placement = self.place(ev);
        let wake_counts = self.wake_counts(ev.created_at);
        let snapshot = Snapshot {
            roster: &self.roster,
            local_bots: &self.local_bots,
            local_members: self.members_of(ev.channel),
            halts: &self.halts,
            thread: placement.thread,
            parent_author: placement.parent_author,
            edit_target: placement.edit_target,
            wake_counts: &wake_counts,
            quiet: quiet_set(&self.roster, now),
        };
        let result = route(ev, &snapshot, now);
        self.apply(ev, placement.root.as_ref(), &result);
        result
    }

    /// Finds the thread, parent author and edit target of `ev` from what the replayer has seen.
    fn place(&self, ev: &InEvent) -> Placement {
        let (root, parent_author, edit_target) = match ev.kind {
            KIND_MESSAGE => match thread_position(&ev.tags) {
                ThreadPos::TopLevel => (Some(ev.id.clone()), None, None),
                ThreadPos::Reply { root, parent } => {
                    let parent_author = (parent != root)
                        .then(|| self.seen.get(&parent))
                        .flatten()
                        .map(|seen| seen.author.clone());
                    (Some(root), parent_author, None)
                }
            },
            KIND_EDIT => {
                let target = edit_target_id(&ev.tags)
                    .and_then(|id| self.seen.get(&id).map(|seen| (id, seen.root.clone())));
                match target {
                    Some((message_id, root)) => (Some(root), None, Some(EditTarget { message_id })),
                    None => (None, None, None),
                }
            }
            _ => (None, None, None),
        };
        let thread = root
            .as_ref()
            .and_then(|root| self.threads.get(root))
            .cloned();
        Placement {
            root,
            thread,
            parent_author,
            edit_target,
        }
    }

    /// The roster bots that are members of `channel`: every bot that covers it, since the
    /// replayer treats each as a member of every channel it covers.
    fn members_of(&self, channel: ChannelId) -> BTreeSet<BotName> {
        self.roster
            .bots
            .values()
            .filter(|bot| match &bot.channels {
                ChannelScope::All => true,
                ChannelScope::Only(channels) => channels.contains(&channel),
            })
            .map(|bot| bot.name.clone())
            .collect()
    }

    /// Each bot's dispatched wakes in the trailing hour and day before the simulated time `at`
    /// (assumption A10). The windows are closed at their start, like the daemon's query
    /// (design 9.3).
    fn wake_counts(&self, at: i64) -> BTreeMap<BotName, WakeCounts> {
        let within = |times: &[i64], window: i64| {
            let since = at.saturating_sub(window);
            let count = times.iter().filter(|&&time| time >= since).count();
            u32::try_from(count).unwrap_or(u32::MAX)
        };
        self.dispatched
            .iter()
            .map(|(bot, times)| {
                let counts = WakeCounts {
                    hour: within(times, HOUR_SECS),
                    day: within(times, DAY_SECS),
                };
                (bot.clone(), counts)
            })
            .collect()
    }

    /// Records `ev`, then applies what `route` returned for it: the thread update first, so that
    /// a new round resets the turns before this event's own wakes count into it, then the wakes,
    /// then the control.
    fn apply(&mut self, ev: &InEvent, root: Option<&EventId>, result: &RouteResult) {
        if let (KIND_MESSAGE, Some(root)) = (ev.kind, root) {
            self.seen.insert(
                ev.id.clone(),
                Seen {
                    author: ev.pubkey.clone(),
                    root: root.clone(),
                },
            );
        }
        self.apply_thread_update(ev, root, &result.thread_update);
        self.count_wakes(ev.created_at, root, &result.decisions);
        if let Some(control) = &result.control {
            self.apply_control(control);
        }
    }

    /// Applies a thread update: creates the thread, adds participants, sets the discussion flag
    /// and starts a new round, which resets every bot's turns (requirement 19.5).
    ///
    /// An update for a thread the replayer has not seen start, a reply under an unseen root, has
    /// nothing to apply to and is dropped.
    fn apply_thread_update(&mut self, ev: &InEvent, root: Option<&EventId>, update: &ThreadUpdate) {
        if let Some(new_thread) = &update.create {
            // A thread begins with its root as its first round, as the daemon's `threads` row
            // does: the round's `created_at` is the root's. `new_round` then replaces it.
            self.threads
                .entry(new_thread.root_id.clone())
                .or_insert_with(|| ThreadState {
                    root_id: new_thread.root_id.clone(),
                    channel_id: new_thread.channel_id,
                    participants: BTreeSet::new(),
                    discussion: false,
                    round_id: new_thread.root_id.clone(),
                    round_mode: RoundMode::Direct,
                    round_started_at: ev.created_at,
                    turns_used: BTreeMap::new(),
                });
        }
        let Some(thread) = root.and_then(|root| self.threads.get_mut(root)) else {
            return;
        };
        thread
            .participants
            .extend(update.add_participants.iter().cloned());
        thread.discussion |= update.set_discussion;
        if let Some(round) = &update.new_round {
            thread.round_id = round.round_id.clone();
            thread.round_mode = round.mode;
            thread.round_started_at = round.started_at;
            thread.turns_used.clear();
        }
    }

    /// Counts every `Wake` in `decisions` as dispatched at the simulated time `at`: one turn for
    /// the bot in its thread's current round (requirement 19.1) and one entry in its wake history
    /// for the budgets. A `Suppress` consumes nothing (requirement 19.4).
    fn count_wakes(&mut self, at: i64, root: Option<&EventId>, decisions: &[Decision]) {
        for decision in decisions {
            let Decision::Wake { bot, .. } = decision else {
                continue;
            };
            self.dispatched.entry(bot.clone()).or_default().push(at);
            if let Some(thread) = root.and_then(|root| self.threads.get_mut(root)) {
                let used = thread.turns_used.entry(bot.clone()).or_insert(0);
                *used = used.saturating_add(1);
            }
        }
    }

    /// Applies a control command to the halts (design 6.7, assumption A4). `Cancel` kills running
    /// wakes, and the replayer has none, so it changes nothing.
    fn apply_control(&mut self, control: &Control) {
        match control {
            Control::Stop(Scope::All) => self.halts.all = true,
            Control::Stop(Scope::Bots(bots)) => self.halts.bots.extend(bots.iter().cloned()),
            Control::Resume(Scope::All) => {
                self.halts.all = false;
                self.halts.bots.clear();
            }
            Control::Resume(Scope::Bots(bots)) => {
                if self.halts.all {
                    // Only the named bots resume: every other roster bot stays halted.
                    self.halts.all = false;
                    self.halts.bots = self.roster.bots.keys().cloned().collect();
                }
                for bot in bots {
                    self.halts.bots.remove(bot);
                }
            }
            Control::Cancel(_) => {}
        }
    }
}

/// The simulated clock: `created_at` as a UTC instant. A time chrono cannot represent clamps to
/// the earliest or latest instant it can, so a corrupt capture cannot stop a replay.
fn simulated_now(created_at: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(created_at, 0).unwrap_or(if created_at < 0 {
        DateTime::<Utc>::MIN_UTC
    } else {
        DateTime::<Utc>::MAX_UTC
    })
}

/// The message an edit edits: the unmarked `["e", <id>]` tag that `buzz_sdk::builders::build_edit`
/// writes (design 6.3, step 4).
fn edit_target_id(tags: &[Vec<String>]) -> Option<EventId> {
    tags.iter().find_map(|tag| match tag.as_slice() {
        [name, id] if name == "e" => EventId::from_hex(id).ok(),
        _ => None,
    })
}
