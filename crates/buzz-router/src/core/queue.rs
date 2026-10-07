//! The wake queue: coalescing, debounce and scheduling (design section 6.5, DD-2, A9).
//!
//! A `Wake` decision appends its [`Trigger`] to the queued wake for (bot, root), or queues a new
//! wake, so there is at most one queued wake per pair (R24). The wake's attributes and
//! `dispatch_after` are recomputed from all its triggers each time. [`Core::schedule`] dispatches
//! due wakes by priority, then FIFO, up to each bot's `max_concurrent`, and never two wakes for
//! the same (bot, root) at once (R25, R26).

use std::cmp::Reverse;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use router_core::config::{AdapterConfig, Limits};
use router_core::ids::{BotName, EventId};
use router_core::payload::{
    format_deadline, turns_left_after_this, ApiRef, ChannelRef, WakePayload,
};
use router_core::route::{Priority, Reason};
use router_core::thread::{RoundMode, ThreadState};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::apply::Core;
use super::CoreMsg;
use crate::adapter::{Adapter, AdapterEvent, WakeContext};
use crate::store::threads::Turns;
use crate::store::wakes::{WakeRow, WakeState, Wakes};
use crate::store::{Store, StoreError};

/// One element of `wakes.triggers` (design section 6.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    /// The message that triggered the wake; for an edit, the edited message.
    pub event_id: String,
    /// The kind-40003 event, for an edit trigger.
    pub edit_id: Option<String>,
    /// The author class: `owner`, `bot`, `foreign_bot` or `human`.
    pub class: String,
    /// The author's display name.
    pub author: String,
    /// Why the bot is woken.
    pub reason: Reason,
    /// How the trigger queues.
    pub priority: Priority,
    /// Whether the trigger waits out the discussion debounce.
    pub debounce: bool,
    /// The round mode: `direct` or `discussion`.
    pub mode: String,
    /// The event's `created_at`, in unix seconds.
    pub created_at: i64,
    /// When ingest received the event, in unix milliseconds.
    pub received_at_ms: i64,
}

/// The debounce inputs for one woken bot (design section 6.5, `dispatch_after`).
#[derive(Debug, Clone, Copy)]
pub(super) struct Debounce {
    /// The bot's `discussion_debounce_secs`, in milliseconds.
    pub wait_ms: i64,
    /// The bot's `discussion_debounce_max_secs`, in milliseconds.
    pub max_ms: i64,
    /// When the core received the thread's latest roster-bot post, in unix milliseconds.
    pub last_bot_post_ms: Option<i64>,
}

impl Debounce {
    pub(super) fn new(limits: &Limits, last_bot_post_ms: Option<i64>) -> Self {
        Self {
            wait_ms: secs_to_ms(limits.discussion_debounce_secs),
            max_ms: secs_to_ms(limits.discussion_debounce_max_secs),
            last_bot_post_ms,
        }
    }
}

/// A dispatched wake the core is waiting on.
pub(super) struct RunningWake {
    pub bot: BotName,
    pub root: EventId,
}

/// The trigger that gives a wake its attributes: the highest priority, the latest among equals
/// (A9).
fn top_trigger(triggers: &[Trigger]) -> Option<&Trigger> {
    triggers.iter().max_by_key(|trigger| trigger.priority)
}

/// The earliest dispatch time of a wake with `triggers` (R23.1–R23.3, DD-2).
fn dispatch_after(triggers: &[Trigger], now_ms: i64, debounce: Debounce) -> i64 {
    if triggers.iter().any(|trigger| !trigger.debounce) {
        return now_ms;
    }
    let first = triggers
        .first()
        .map_or(now_ms, |trigger| trigger.received_at_ms);
    let last_post = debounce.last_bot_post_ms.unwrap_or(first);
    (last_post + debounce.wait_ms)
        .max(first + debounce.wait_ms)
        .min(first + debounce.max_ms)
}

/// Queues `trigger` for `bot` in `thread`: appended to the queued wake for (bot, root) if there
/// is one, else a new queued wake in the thread's current round. Then recomputes the wake's
/// attributes and `dispatch_after` from all its triggers (design 6.5).
pub(super) fn enqueue(
    conn: &Connection,
    bot: &BotName,
    thread: &ThreadState,
    trigger: &Trigger,
    debounce: Debounce,
    now_ms: i64,
) -> Result<(), StoreError> {
    let wakes = Wakes::new(conn);
    let queued = wakes.find_queued(bot, &thread.root_id)?;
    let mut triggers = match &queued {
        Some(queued) => decode(&queued.triggers)?,
        None => Vec::new(),
    };
    triggers.push(trigger.clone());
    let top = top_trigger(&triggers).unwrap_or(trigger);
    let reason = snake(&top.reason);
    let priority = snake(&top.priority);
    let due = dispatch_after(&triggers, now_ms, debounce);
    let encoded = encode(&triggers)?;
    match queued {
        Some(queued) => wakes.update_queued(&queued.id, &reason, &priority, &encoded, due),
        None => wakes.insert(&WakeRow {
            id: Uuid::new_v4(),
            bot: bot.clone(),
            root_id: thread.root_id.clone(),
            round_id: thread.round_id.clone(),
            reason,
            priority,
            triggers: encoded,
            state: WakeState::Queued,
            token_hash: None,
            attempt: 1,
            created_at: now_ms,
            dispatch_after: due,
            started_at: None,
            deadline: None,
            ended_at: None,
            outcome: None,
        }),
    }
}

impl Core {
    /// Drops the queued wakes of halted bots and dispatches every due wake that has a free slot,
    /// by priority and then FIFO (design 6.5, `schedule()`).
    pub(super) fn schedule(&mut self) {
        let now = self.clock.now();
        let now_ms = now.timestamp_millis();
        let (halts, queued) = match (self.halts(), self.store.wakes().queued()) {
            (Ok(halts), Ok(queued)) => (halts, queued),
            (Err(error), _) | (_, Err(error)) => {
                tracing::warn!(%error, "cannot read the wake queue");
                return;
            }
        };
        let mut due = Vec::new();
        for wake in queued {
            if halts.all || halts.bots.contains(&wake.bot) {
                self.drop_queued(&wake, now_ms);
            } else if wake.dispatch_after <= now_ms {
                due.push(wake);
            }
        }
        due.sort_by_key(|wake| (Reverse(priority_rank(&wake.priority)), wake.created_at));
        for wake in due {
            let slots = self
                .config
                .bots
                .get(&wake.bot)
                .map_or(0, |bot| bot.max_concurrent);
            let running = self
                .running
                .values()
                .filter(|running| running.bot == wake.bot);
            let (count, same_thread) = running.fold((0_u32, false), |(count, same), running| {
                (count + 1, same || running.root == wake.root_id)
            });
            if count < slots && !same_thread {
                self.dispatch(wake, now);
            }
        }
    }

    /// How long the core may sleep before a queued wake falls due, at most `cap`.
    pub(super) fn next_due_in(&self, cap: std::time::Duration) -> std::time::Duration {
        let now_ms = self.clock.now().timestamp_millis();
        match self.store.wakes().next_dispatch_after(now_ms) {
            Ok(Some(due)) => u64::try_from(due - now_ms)
                .map(std::time::Duration::from_millis)
                .map_or(cap, |wait| wait.min(cap)),
            Ok(None) => cap,
            Err(error) => {
                tracing::warn!(%error, "cannot read the next dispatch time");
                cap
            }
        }
    }

    /// Frees the slot of a wake whose adapter run ended, records the end, and schedules.
    pub(super) fn wake_ended(&mut self, wake_id: Uuid, event: AdapterEvent) {
        let (state, detail) = match &event {
            AdapterEvent::AsyncAccepted => return,
            AdapterEvent::Exited { code: Some(0), .. } => {
                (WakeState::Passed, "exited 0".to_owned())
            }
            AdapterEvent::SyncReply(_) => (WakeState::Passed, "sync reply".to_owned()),
            AdapterEvent::Exited { code, .. } => (WakeState::Failed, format!("exit code {code:?}")),
            AdapterEvent::Failed(message) => (WakeState::Failed, message.clone()),
        };
        if self.running.remove(&wake_id).is_none() {
            return;
        }
        let outcome = serde_json::json!({ "posted": [], "detail": detail }).to_string();
        let ended_at = self.clock.now().timestamp_millis();
        if let Err(error) = self
            .store
            .wakes()
            .finish(&wake_id, state, ended_at, Some(&outcome))
        {
            tracing::warn!(%error, wake_id = %wake_id, "cannot record the end of a wake");
        }
    }

    /// Marks a halted bot's queued wake killed, with no reaction (DD-15).
    fn drop_queued(&self, wake: &WakeRow, now_ms: i64) {
        let outcome = serde_json::json!({ "dropped": true }).to_string();
        if let Err(error) =
            self.store
                .wakes()
                .finish(&wake.id, WakeState::Killed, now_ms, Some(&outcome))
        {
            tracing::warn!(%error, wake_id = %wake.id, "cannot drop a halted bot's wake");
        }
    }

    /// Starts `wake` and hands it to the bot's adapter (design 6.6, dispatch steps 1 to 3 and 7).
    fn dispatch(&mut self, wake: WakeRow, now: DateTime<Utc>) {
        let bot = wake.bot.clone();
        let (Some(adapter), Some(bot_config)) =
            (self.adapters.get(&bot).cloned(), self.config.bots.get(&bot))
        else {
            tracing::warn!(bot = %bot, wake_id = %wake.id, "no adapter for a queued wake");
            return;
        };
        let adapter_config = bot_config.adapter.clone();
        let Some(thread) = self.load_thread(&wake.root_id) else {
            tracing::warn!(bot = %bot, wake_id = %wake.id, "no thread for a queued wake");
            return;
        };
        let triggers = match decode(&wake.triggers) {
            Ok(triggers) => triggers,
            Err(error) => {
                tracing::warn!(%error, wake_id = %wake.id, "cannot read a wake's triggers");
                return;
            }
        };
        let Some(top) = top_trigger(&triggers).cloned() else {
            tracing::warn!(wake_id = %wake.id, "a queued wake without triggers");
            return;
        };
        let token = match new_token() {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!(%error, wake_id = %wake.id, "cannot generate a wake token");
                return;
            }
        };
        let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
        let limits = self.limits(&bot);
        let deadline = now
            + i64::try_from(limits.max_wake_minutes)
                .ok()
                .and_then(TimeDelta::try_minutes)
                .unwrap_or(TimeDelta::MAX);
        let used = match start_wake(
            &mut self.store,
            &wake,
            &thread.round_id,
            &token_hash,
            now.timestamp_millis(),
            deadline.timestamp_millis(),
        ) {
            Ok(used) => used,
            Err(error) => {
                tracing::warn!(%error, wake_id = %wake.id, "cannot start a wake");
                return;
            }
        };
        let mut updated = thread.clone();
        updated.turns_used.insert(bot.clone(), used);
        self.threads.put(updated);

        let trigger_ids: Vec<EventId> = triggers
            .iter()
            .filter_map(|trigger| EventId::from_hex(&trigger.event_id).ok())
            .collect();
        let reply_parent = reaction_target(&triggers).unwrap_or_else(|| wake.root_id.clone());
        let mode = round_mode(&top.mode);
        let turns_left = turns_left_after_this(limits.turns_per_round, used);
        let channel_name = self.roster.channels.get(&thread.channel_id).map_or_else(
            || thread.channel_id.to_string(),
            |channel| channel.name.clone(),
        );
        let api_url = match &adapter_config {
            AdapterConfig::Webhook { .. } => self.config.public_url.clone(),
            AdapterConfig::Command { .. } => None,
        }
        .unwrap_or_else(|| format!("http://{}", self.config.api_bind));
        let payload = WakePayload {
            wake_id: wake.id,
            token: token.clone(),
            bot: bot.to_string(),
            channel: ChannelRef {
                id: thread.channel_id.to_string(),
                name: channel_name,
            },
            thread_root_id: wake.root_id.to_string(),
            reply_parent_id: reply_parent.to_string(),
            reason: top.reason,
            round_mode: mode,
            turns_left_after_this: turns_left,
            turns_per_round: limits.turns_per_round,
            deadline: format_deadline(deadline),
            triggers: trigger_ids.iter().map(ToString::to_string).collect(),
            context: Vec::new(),
            api: ApiRef::new(api_url),
        };
        let ctx = WakeContext {
            wake_id: wake.id,
            token,
            bot: bot.clone(),
            adapter: adapter_config,
            channel: thread.channel_id,
            root: wake.root_id.clone(),
            reply_parent,
            reason: top.reason,
            reason_author: top.author,
            mode,
            turns_left,
            limits,
            deadline,
            trigger_ids,
        };
        self.running.insert(
            wake.id,
            RunningWake {
                bot,
                root: wake.root_id,
            },
        );
        spawn_runner(&adapter, ctx, payload, self.self_tx.clone());
    }

    /// The effective limits of `bot`.
    pub(super) fn limits(&self, bot: &BotName) -> Limits {
        self.roster
            .bots
            .get(bot)
            .map(|bot| bot.limits)
            .unwrap_or_default()
    }
}

/// Runs the adapter in its own task and reports the result to the core.
fn spawn_runner(
    adapter: &Arc<dyn Adapter>,
    ctx: WakeContext,
    payload: WakePayload,
    core_tx: tokio::sync::mpsc::UnboundedSender<CoreMsg>,
) {
    let wake_id = ctx.wake_id;
    let run = adapter.run(ctx, payload, CancellationToken::new());
    tokio::spawn(async move {
        let event = run.await;
        let _ = core_tx.send(CoreMsg::WakeEnded { wake_id, event });
    });
}

/// Marks the wake running in the thread's current round and counts its turn, in one transaction
/// (design 6.6, dispatch step 3). Returns the bot's turns used in the round.
fn start_wake(
    store: &mut Store,
    wake: &WakeRow,
    round_id: &EventId,
    token_hash: &str,
    started_ms: i64,
    deadline_ms: i64,
) -> Result<u32, StoreError> {
    let tx = store.connection_mut().transaction()?;
    let wakes = Wakes::new(&tx);
    wakes.start(&wake.id, token_hash, started_ms, deadline_ms)?;
    wakes.set_round(&wake.id, round_id)?;
    let used = Turns::new(&tx).increment(&wake.root_id, round_id, &wake.bot)?;
    tx.commit()?;
    Ok(used)
}

/// The latest owner trigger's event by (`created_at`, `event_id`), else the latest trigger's
/// (A8).
fn reaction_target(triggers: &[Trigger]) -> Option<EventId> {
    let latest = |owner_only: bool| {
        triggers
            .iter()
            .filter(|trigger| !owner_only || trigger.class == "owner")
            .max_by(|a, b| (a.created_at, &a.event_id).cmp(&(b.created_at, &b.event_id)))
    };
    latest(true)
        .or_else(|| latest(false))
        .and_then(|trigger| EventId::from_hex(&trigger.event_id).ok())
}

/// Queue order: owner first, then human, then bot (R25.2).
fn priority_rank(priority: &str) -> u8 {
    match priority {
        "owner" => 2,
        "human" => 1,
        _ => 0,
    }
}

fn round_mode(mode: &str) -> RoundMode {
    if mode == "discussion" {
        RoundMode::Discussion
    } else {
        RoundMode::Direct
    }
}

/// A wake token: 32 random bytes in hex (DD-10).
fn new_token() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)?;
    Ok(hex::encode(bytes))
}

fn secs_to_ms(secs: u64) -> i64 {
    i64::try_from(secs.saturating_mul(1_000)).unwrap_or(i64::MAX)
}

fn decode(triggers: &str) -> Result<Vec<Trigger>, StoreError> {
    serde_json::from_str(triggers).map_err(|error| StoreError::Corrupt {
        column: "wakes.triggers",
        message: error.to_string(),
    })
}

fn encode(triggers: &[Trigger]) -> Result<String, StoreError> {
    serde_json::to_string(triggers).map_err(|error| StoreError::Corrupt {
        column: "wakes.triggers",
        message: error.to_string(),
    })
}

/// The snake_case JSON string of a serialisable enum value, such as `mention` or `owner`.
fn snake<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger(priority: Priority, debounce: bool, received_at_ms: i64) -> Trigger {
        Trigger {
            event_id: format!("{received_at_ms:064x}"),
            edit_id: None,
            class: "bot".to_owned(),
            author: "B".to_owned(),
            reason: Reason::Discussion,
            priority,
            debounce,
            mode: "discussion".to_owned(),
            created_at: received_at_ms / 1_000,
            received_at_ms,
        }
    }

    const DEBOUNCE: Debounce = Debounce {
        wait_ms: 20_000,
        max_ms: 90_000,
        last_bot_post_ms: None,
    };

    #[test]
    fn a_debounced_wake_waits_for_the_last_bot_post_capped_at_the_maximum() {
        let triggers = [trigger(Priority::Bot, true, 1_000)];
        assert_eq!(dispatch_after(&triggers, 5_000, DEBOUNCE), 21_000);
        let later_post = Debounce {
            last_bot_post_ms: Some(40_000),
            ..DEBOUNCE
        };
        assert_eq!(dispatch_after(&triggers, 40_000, later_post), 60_000);
        let much_later = Debounce {
            last_bot_post_ms: Some(85_000),
            ..DEBOUNCE
        };
        assert_eq!(dispatch_after(&triggers, 85_000, much_later), 91_000);
    }

    #[test]
    fn any_non_debounced_trigger_makes_the_wake_due_now() {
        let triggers = [
            trigger(Priority::Bot, true, 1_000),
            trigger(Priority::Owner, false, 2_000),
        ];
        assert_eq!(dispatch_after(&triggers, 2_000, DEBOUNCE), 2_000);
    }

    #[test]
    fn the_top_trigger_is_the_latest_of_the_highest_priority() {
        let triggers = [
            trigger(Priority::Human, false, 1),
            trigger(Priority::Owner, false, 2),
            trigger(Priority::Owner, false, 3),
            trigger(Priority::Bot, true, 4),
        ];
        assert_eq!(top_trigger(&triggers).map(|t| t.received_at_ms), Some(3));
    }

    #[test]
    fn the_reaction_target_prefers_the_latest_owner_trigger() {
        let mut owner = trigger(Priority::Owner, false, 2_000);
        owner.class = "owner".to_owned();
        let triggers = [owner.clone(), trigger(Priority::Bot, true, 9_000)];
        assert_eq!(
            reaction_target(&triggers).map(|id| id.to_string()),
            Some(owner.event_id)
        );
    }
}
