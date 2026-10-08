//! The status document behind `GET /v1/status` and `status --json` (design 12.2, DD-12).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use router_core::ids::BotName;
use serde::{Deserialize, Serialize};

use super::apply::{wake_counts, Core};
use crate::store::wakes::WakeState;
use crate::store::StoreError;

/// Whether a bot's relay connection is authenticated right now.
pub type ConnectedProbe = Arc<dyn Fn() -> bool + Send + Sync>;

/// What the status document needs from outside the core.
#[derive(Clone, Default)]
pub struct StatusSources {
    /// The hex SHA-256 of the roster file, as `roster check` prints it.
    pub roster_hash: String,
    /// The bots in `router.toml` whose key did not load, which the core does not serve (DD-23).
    pub unavailable: BTreeSet<BotName>,
    /// Each served bot's connection probe. A bot without one shows as disconnected.
    pub connected: BTreeMap<BotName, ConnectedProbe>,
}

/// The status document (design 12.2). Field order is the JSON key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// The router's version.
    pub version: String,
    /// The hex SHA-256 of the roster file.
    pub roster_hash: String,
    /// Every bot in `router.toml`, by name.
    pub bots: Vec<BotStatus>,
    /// `all` first, then the halted bots by name.
    pub halts: Vec<String>,
    /// Backfilled owner messages too old to wake anyone, since start (R48.3).
    pub missed: Vec<MissedStatus>,
    /// Unmanaged posts since start (R51.1).
    pub unmanaged_posts: u64,
}

/// One bot in the status document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotStatus {
    /// The bot's name.
    pub name: String,
    /// Whether its key loaded, so the router serves it (DD-23).
    pub available: bool,
    /// Whether its relay connection is authenticated right now.
    pub connected: bool,
    /// Whether a halt covers it.
    pub halted: bool,
    /// Its running wakes' ids.
    pub running: Vec<String>,
    /// How many of its wakes are queued.
    pub queued: u64,
    /// Its wake budget.
    pub budget: BudgetStatus,
}

/// A bot's wake budget (R20.4, R21.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetStatus {
    /// Wakes in the trailing hour.
    pub hour: BudgetUse,
    /// Wakes in the trailing day.
    pub day: BudgetUse,
    /// Wakes the budget suppressed since start (DD-12).
    pub suppressed_since_start: u64,
}

/// Wakes used against a limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetUse {
    /// Wakes dispatched in the window.
    pub used: u32,
    /// The most the window allows.
    pub limit: u32,
}

/// A missed message in the status document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissedStatus {
    /// The message's event id, in hex.
    pub event_id: String,
    /// Its channel.
    pub channel_id: String,
    /// Its `created_at`, in unix seconds.
    pub created_at: i64,
}

impl Core {
    /// Builds the status document from the store, the counters and the status sources. The bots
    /// are the served ones plus the unavailable ones.
    pub(super) fn status(&self) -> Result<Status, StoreError> {
        let sources = &self.status_sources;
        let bots: BTreeSet<BotName> = self
            .config
            .bots
            .keys()
            .chain(&sources.unavailable)
            .cloned()
            .collect();
        let halts = self.halts()?;
        let counts = wake_counts(&self.store, &bots, self.clock.now())?;
        let wakes = self.store.wakes();
        let running = wakes.with_state(WakeState::Running)?;
        let queued = wakes.with_state(WakeState::Queued)?;
        let counters = &self.counters;
        let bots = bots
            .iter()
            .map(|bot| {
                let limits = self.limits(bot);
                let (hour, day) = counts
                    .get(bot)
                    .map_or((0, 0), |count| (count.hour, count.day));
                BotStatus {
                    name: bot.as_str().to_owned(),
                    available: !sources.unavailable.contains(bot),
                    connected: sources.connected.get(bot).is_some_and(|probe| probe()),
                    halted: halts.all || halts.bots.contains(bot),
                    running: running
                        .iter()
                        .filter(|wake| &wake.bot == bot)
                        .map(|wake| wake.id.to_string())
                        .collect(),
                    queued: queued.iter().filter(|wake| &wake.bot == bot).count() as u64,
                    budget: BudgetStatus {
                        hour: BudgetUse {
                            used: hour,
                            limit: limits.wakes_per_hour,
                        },
                        day: BudgetUse {
                            used: day,
                            limit: limits.wakes_per_day,
                        },
                        suppressed_since_start: counters
                            .budget_suppressed
                            .get(bot)
                            .copied()
                            .unwrap_or(0),
                    },
                }
            })
            .collect();
        let halt_names = halts
            .all
            .then(|| "all".to_owned())
            .into_iter()
            .chain(halts.bots.iter().map(|bot| bot.as_str().to_owned()))
            .collect();
        Ok(Status {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            roster_hash: sources.roster_hash.clone(),
            bots,
            halts: halt_names,
            missed: counters
                .missed
                .iter()
                .map(|missed| MissedStatus {
                    event_id: missed.event_id.as_str().to_owned(),
                    channel_id: missed.channel_id.to_string(),
                    created_at: missed.created_at,
                })
                .collect(),
            unmanaged_posts: counters.unmanaged_posts,
        })
    }
}
