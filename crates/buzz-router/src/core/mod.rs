//! The core actor (design section 6.1, DD-1).
//!
//! [`spawn_core`] starts two tasks:
//!
//! - the **ingest task**, which runs every raw event through [`Ingest`] in arrival order and
//!   forwards the results;
//! - the **core actor**, which owns the SQLite write connection and all mutable state, and
//!   serialises every mutation. Its loop waits for a message, the next queued wake falling due,
//!   or a 1-second tick, so wall-clock deadlines are re-checked after an OS sleep. After each
//!   message or timer it fires the running wakes' timers and runs the scheduler ([`queue`]).
//!
//! [`CoreHandle`] is the only way in. Its `flush` and `debug_counters` exist for tests.

mod apply;
pub mod control;
mod dispatch;
pub mod queue;
mod timers;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use router_core::config::{Roster, RouterConfig};
use router_core::ids::{BotName, ChannelId, EventId};
use router_core::route::Decision;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::adapter::{Adapter, AdapterEvent};
use crate::clock::Clock;
use crate::ingest::{EnrichedEvent, Ingest, IngestOutput, Source};
use crate::relay::RelayPort;
use crate::store::Store;
use dispatch::Purpose;

pub use apply::wake_counts;
pub use dispatch::{select_adapters, ApiFailure, ApiRequest, ApiResponse};

/// The longest the core actor sleeps between loop turns.
const TICK: Duration = Duration::from_secs(1);

/// Everything the core needs (design section 6.1, test seam).
pub struct CoreDeps {
    /// The write connection, owned by the core actor.
    pub store: Store,
    /// A read-only connection to the same database, for the ingest task.
    pub ingest_store: Store,
    /// The roster.
    pub roster: Roster,
    /// The router configuration. Its `bots` are the local bots.
    pub config: RouterConfig,
    /// The wall clock.
    pub clock: Arc<dyn Clock>,
    /// Each local bot's relay connection.
    pub relays: BTreeMap<BotName, Arc<dyn RelayPort>>,
    /// Each local bot's signing keys.
    pub keys: BTreeMap<BotName, nostr::Keys>,
    /// Each local bot's channel memberships, as discovery last reported them.
    pub memberships: BTreeMap<BotName, BTreeSet<ChannelId>>,
    /// Each local bot's adapter.
    pub adapters: BTreeMap<BotName, Arc<dyn Adapter>>,
}

/// In-memory counters, for `status` and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebugCounters {
    /// Budget suppressions per bot since start (R20.4, R21.4).
    pub budget_suppressed: BTreeMap<BotName, u64>,
    /// Unmanaged posts detected since start (R51.1, DD-12).
    pub unmanaged_posts: u64,
    /// The decisions `route` returned for the last event the core applied.
    pub last_decisions: Vec<Decision>,
}

/// A message to the core actor.
pub(crate) enum CoreMsg {
    /// Route an event.
    Enriched(Box<EnrichedEvent>),
    /// Rebuild a thread from history before the next event.
    RebuildThread {
        root: EventId,
        events: Vec<nostr::Event>,
    },
    /// A bot's channel memberships changed.
    Memberships {
        bot: BotName,
        channels: BTreeSet<ChannelId>,
    },
    /// Discovery found a channel's name.
    ChannelName { channel: ChannelId, name: String },
    /// A wake's adapter run ended.
    WakeEnded { wake_id: Uuid, event: AdapterEvent },
    /// A wake-token API request.
    Api(ApiRequest, oneshot::Sender<ApiResponse>),
    /// A tracked publish (reply or status note) completed.
    Published {
        wake_id: Uuid,
        event_id: EventId,
        purpose: Purpose,
        result: Result<(), String>,
    },
    /// Reply once everything sent before has been applied (tests only).
    Flush(oneshot::Sender<()>),
    /// Reply with the counters (tests only).
    DebugCounters(oneshot::Sender<DebugCounters>),
    /// Stop the actor.
    Shutdown,
}

/// A message to the ingest task.
enum IngestMsg {
    Event {
        bot: BotName,
        event: Box<nostr::Event>,
        source: Source,
    },
    Flush(oneshot::Sender<()>),
}

/// A handle to a running core. Clones talk to the same core.
#[derive(Clone)]
pub struct CoreHandle {
    ingest_tx: mpsc::UnboundedSender<IngestMsg>,
    core_tx: mpsc::UnboundedSender<CoreMsg>,
}

impl CoreHandle {
    /// Feeds an event received by `bot` through the ingest pipeline.
    pub fn ingest(&self, bot: BotName, event: nostr::Event, source: Source) {
        let message = IngestMsg::Event {
            bot,
            event: Box::new(event),
            source,
        };
        if self.ingest_tx.send(message).is_err() {
            tracing::warn!("the ingest task has stopped; dropping an event");
        }
    }

    /// Reports a bot's channel memberships from discovery.
    pub fn memberships(&self, bot: BotName, channels: BTreeSet<ChannelId>) {
        let _ = self.core_tx.send(CoreMsg::Memberships { bot, channels });
    }

    /// Reports a channel's name from discovery (kind 39000).
    pub fn channel_name(&self, channel: ChannelId, name: String) {
        let _ = self.core_tx.send(CoreMsg::ChannelName { channel, name });
    }

    /// Sends a wake-token request to the core and waits for its answer.
    pub async fn api(&self, request: ApiRequest) -> ApiResponse {
        let (tx, rx) = oneshot::channel();
        if self.core_tx.send(CoreMsg::Api(request, tx)).is_err() {
            return ApiResponse::Failed(ApiFailure::Internal("the core has stopped".to_owned()));
        }
        rx.await.unwrap_or_else(|_| {
            ApiResponse::Failed(ApiFailure::Internal("the core has stopped".to_owned()))
        })
    }

    /// Resolves once every event ingested before the call has been applied, and the publishes it
    /// caused have completed (tests only).
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        if self.ingest_tx.send(IngestMsg::Flush(tx)).is_ok() {
            let _ = rx.await;
        }
    }

    /// The in-memory counters (tests only).
    pub async fn debug_counters(&self) -> DebugCounters {
        let (tx, rx) = oneshot::channel();
        if self.core_tx.send(CoreMsg::DebugCounters(tx)).is_err() {
            return DebugCounters::default();
        }
        rx.await.unwrap_or_default()
    }

    /// Stops the core actor.
    pub fn shutdown(&self) {
        let _ = self.core_tx.send(CoreMsg::Shutdown);
    }
}

/// Starts the ingest task and the core actor on the current tokio runtime.
pub fn spawn_core(deps: CoreDeps) -> CoreHandle {
    let (ingest_tx, ingest_rx) = mpsc::unbounded_channel();
    let (core_tx, core_rx) = mpsc::unbounded_channel();
    let ingest = Ingest::new(deps.ingest_store, deps.relays.clone(), deps.clock.clone());
    tokio::spawn(run_ingest(ingest, ingest_rx, core_tx.clone()));
    let core = apply::Core::new(
        apply::CoreParts {
            store: deps.store,
            roster: deps.roster,
            config: deps.config,
            clock: deps.clock,
            relays: deps.relays,
            keys: deps.keys,
            memberships: deps.memberships,
            adapters: deps.adapters,
        },
        core_tx.clone(),
    );
    tokio::spawn(run_core(core, core_rx));
    CoreHandle { ingest_tx, core_tx }
}

/// The ingest task: one event at a time, in arrival order.
async fn run_ingest(
    mut ingest: Ingest,
    mut rx: mpsc::UnboundedReceiver<IngestMsg>,
    core_tx: mpsc::UnboundedSender<CoreMsg>,
) {
    while let Some(message) = rx.recv().await {
        let sent = match message {
            IngestMsg::Event { bot, event, source } => ingest
                .handle(&bot, *event, source)
                .await
                .into_iter()
                .all(|output| {
                    let message = match output {
                        IngestOutput::Enriched(event) => CoreMsg::Enriched(event),
                        IngestOutput::RebuildThread { root, events } => {
                            CoreMsg::RebuildThread { root, events }
                        }
                    };
                    core_tx.send(message).is_ok()
                }),
            IngestMsg::Flush(reply) => core_tx.send(CoreMsg::Flush(reply)).is_ok(),
        };
        if !sent {
            break;
        }
    }
}

/// The core actor loop (design section 6.1).
async fn run_core(mut core: apply::Core, mut rx: mpsc::UnboundedReceiver<CoreMsg>) {
    loop {
        let wait = core.next_due_in(TICK);
        tokio::select! {
            message = rx.recv() => match message {
                None | Some(CoreMsg::Shutdown) => break,
                Some(message) => core.handle(message).await,
            },
            () = tokio::time::sleep(wait) => {}
        }
        core.fire_timers();
        core.schedule();
    }
}
