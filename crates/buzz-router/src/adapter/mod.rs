//! Adapters: how a woken bot's agent runs (design section 7).
//!
//! The core dispatches a wake by calling [`Adapter::run`] with the [`WakeContext`] and the
//! [`WakePayload`]; the returned future resolves to the [`AdapterEvent`] that ends the run. The
//! core cancels a run through its [`CancellationToken`].

pub mod command;
pub mod webhook;

use chrono::{DateTime, Utc};
use futures_util::future::BoxFuture;
use router_core::config::{AdapterConfig, Limits};
use router_core::ids::{BotName, ChannelId, EventId};
use router_core::payload::WakePayload;
use router_core::route::Reason;
use router_core::thread::RoundMode;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// What a WakeRunner knows about the wake it runs (design section 6.6, dispatch step 7).
#[derive(Debug, Clone)]
pub struct WakeContext {
    /// The wake.
    pub wake_id: Uuid,
    /// The wake token, in hex.
    pub token: String,
    /// The bot woken.
    pub bot: BotName,
    /// The bot's adapter configuration.
    pub adapter: AdapterConfig,
    /// The channel of the thread.
    pub channel: ChannelId,
    /// The thread root.
    pub root: EventId,
    /// The parent a reply is threaded under: the reaction target.
    pub reply_parent: EventId,
    /// Why the bot is woken.
    pub reason: Reason,
    /// The display name of the author of the trigger that gave the reason.
    pub reason_author: String,
    /// The round mode.
    pub mode: RoundMode,
    /// Turns the bot has left in the round after this wake.
    pub turns_left: u32,
    /// The bot's effective limits.
    pub limits: Limits,
    /// When the wake times out.
    pub deadline: DateTime<Utc>,
    /// The trigger event ids, in trigger order.
    pub trigger_ids: Vec<EventId>,
}

/// How an adapter run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterEvent {
    /// A command exited.
    Exited {
        /// The exit code, or `None` when killed by a signal.
        code: Option<i32>,
        /// The captured standard output.
        stdout: Option<String>,
    },
    /// An async webhook answered 2xx; the agent calls back through the API.
    AsyncAccepted,
    /// A sync webhook answered.
    SyncReply(SyncReply),
    /// The adapter could not run the wake.
    Failed(String),
}

/// A sync webhook's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncReply {
    /// Post this text.
    Text(String),
    /// Post nothing.
    Pass,
}

/// Runs a bot's agent for one wake.
pub trait Adapter: Send + Sync {
    /// Runs the wake until it ends or `cancel` fires.
    fn run(
        &self,
        ctx: WakeContext,
        payload: WakePayload,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, AdapterEvent>;

    /// Tells the agent, best-effort, that a stop or cancel killed wake `wake_id`, which ran with
    /// `config`. Called after the run is cancelled; a deadline does not call it.
    fn killed(&self, config: &AdapterConfig, wake_id: Uuid) -> BoxFuture<'static, ()> {
        let _ = (config, wake_id);
        Box::pin(async {})
    }
}
