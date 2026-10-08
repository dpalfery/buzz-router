//! Control execution: stop, resume and cancel (design section 6.7, DD-15, R30–R33, A4).
//!
//! [`write_halts`] is step 1, the halt rows. The core runs it inside the event's apply
//! transaction (or its own, for the admin API), and the CLI fallback runs it against its own
//! connection. [`Core::control_effects`] runs after the commit: in-scope local bots have their
//! running wakes killed and their queued wakes dropped (stop and cancel), and react on the
//! control message when there is one.

use std::collections::BTreeSet;

use router_core::config::Roster;
use router_core::ids::BotName;
use router_core::route::{Control, Scope};
use rusqlite::Connection;
use tokio::sync::oneshot;

use super::apply::Core;
use super::dispatch::{ApiFailure, ApiResponse};
use crate::store::halts::{HaltScope, Halts};
use crate::store::wakes::WakeState;
use crate::store::StoreError;

/// The reaction on a stop or cancel message, and on a killed wake's reaction target (R30.1).
pub const STOP_EMOJI: &str = "\u{1F6D1}";
/// The reaction on a resume message (R31.1).
pub const RESUME_EMOJI: &str = "\u{25B6}\u{FE0F}";
/// `set_by_event` for a halt set through the admin API.
pub const SET_BY_ADMIN_API: &str = "admin-api";
/// `set_by_event` for a halt set by the CLI fallback.
pub const SET_BY_CLI: &str = "cli";

/// Writes or deletes the halt rows for `control` (design 6.7). `set_by` is the control message
/// id, [`SET_BY_ADMIN_API`] or [`SET_BY_CLI`]. Cancel writes nothing.
///
/// Resuming named bots while an `'all'` row exists replaces it with one row per other roster
/// bot, carrying the `'all'` row's origin, then deletes the named bots' rows (A4).
pub fn write_halts(
    conn: &Connection,
    control: &Control,
    set_by: &str,
    now_ms: i64,
    roster: &Roster,
) -> Result<(), StoreError> {
    let halts = Halts::new(conn);
    match control {
        Control::Stop(Scope::All) => halts.set(&HaltScope::All, Some(set_by), now_ms)?,
        Control::Stop(Scope::Bots(bots)) => {
            for bot in bots {
                halts.set(&HaltScope::Bot(bot.clone()), Some(set_by), now_ms)?;
            }
        }
        Control::Resume(Scope::All) => {
            for row in halts.list()? {
                halts.clear(&row.scope)?;
            }
        }
        Control::Resume(Scope::Bots(bots)) => {
            let rows = halts.list()?;
            if let Some(all) = rows.iter().find(|row| row.scope == HaltScope::All) {
                halts.clear(&HaltScope::All)?;
                for other in roster.bots.keys().filter(|name| !bots.contains(*name)) {
                    let scope = HaltScope::Bot(other.clone());
                    if !rows.iter().any(|row| row.scope == scope) {
                        halts.set(&scope, all.set_by_event.as_deref(), all.set_at)?;
                    }
                }
            }
            for bot in bots {
                halts.clear(&HaltScope::Bot(bot.clone()))?;
            }
        }
        Control::Cancel(_) => {}
    }
    Ok(())
}

impl Core {
    /// Runs the after-commit effects of `control` (design 6.7 steps 2 and 3). `message` is the
    /// control message the in-scope bots react on; the admin API has none.
    pub(super) fn control_effects(&mut self, control: &Control, message: Option<&nostr::EventId>) {
        let (scope, kills, emoji) = match control {
            Control::Stop(scope) => (scope, true, STOP_EMOJI),
            Control::Cancel(scope) => (scope, true, STOP_EMOJI),
            Control::Resume(scope) => (scope, false, RESUME_EMOJI),
        };
        let bots = self.in_scope(scope);
        if kills {
            let now_ms = self.clock.now().timestamp_millis();
            for bot in &bots {
                self.kill_running(bot);
                self.drop_queued_of(bot, now_ms);
            }
        }
        if let Some(message) = message {
            for bot in &bots {
                self.react(bot, message, emoji);
            }
        }
    }

    /// Executes an admin-API control: the halt rows in one transaction, then the effects with
    /// no reactions (R33.2).
    pub(super) fn admin_control(&mut self, control: &Control, reply: oneshot::Sender<ApiResponse>) {
        let now_ms = self.clock.now().timestamp_millis();
        let written = (|| {
            let tx = self.store.connection_mut().transaction()?;
            write_halts(&tx, control, SET_BY_ADMIN_API, now_ms, &self.roster)?;
            tx.commit()?;
            Ok::<(), StoreError>(())
        })();
        let response = match written {
            Ok(()) => {
                self.control_effects(control, None);
                ApiResponse::Done
            }
            Err(error) => ApiResponse::Failed(ApiFailure::Internal(error.to_string())),
        };
        let _ = reply.send(response);
    }

    /// The local bots `scope` covers.
    fn in_scope(&self, scope: &Scope) -> BTreeSet<BotName> {
        let local = self.config.bots.keys();
        match scope {
            Scope::All => local.cloned().collect(),
            Scope::Bots(named) => local.filter(|bot| named.contains(*bot)).cloned().collect(),
        }
    }

    /// Cancels `bot`'s running wakes, ending them as `killed` with 🛑 on their reaction targets,
    /// and tells the adapter in the background (a webhook's `cancel_url`, DD-16).
    fn kill_running(&mut self, bot: &BotName) {
        let notify = self
            .adapters
            .get(bot)
            .cloned()
            .zip(self.config.bots.get(bot).map(|bot| bot.adapter.clone()));
        let wake_ids: Vec<_> = self
            .running
            .iter()
            .filter(|(_, running)| running.bot == *bot)
            .map(|(id, _)| *id)
            .collect();
        for wake_id in wake_ids {
            if let Some(running) = self.running.get(&wake_id) {
                running.cancel.cancel();
            }
            self.finish(&wake_id, WakeState::Killed, "killed");
            if let Some((adapter, config)) = &notify {
                self.publishes.spawn(adapter.killed(config, wake_id));
            }
        }
    }

    /// Marks `bot`'s queued wakes `killed` with `{"dropped":true}` and no reaction (DD-15).
    fn drop_queued_of(&self, bot: &BotName, now_ms: i64) {
        match self.store.wakes().queued() {
            Ok(queued) => {
                for wake in queued.iter().filter(|wake| wake.bot == *bot) {
                    self.drop_queued(wake, now_ms);
                }
            }
            Err(error) => tracing::warn!(%error, bot = %bot, "cannot read the queue to drop it"),
        }
    }
}
