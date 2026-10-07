//! Dispatch, the wake-token API and endings (design section 6.6, DD-10, DD-17, DD-20).
//!
//! [`Core::dispatch`] starts a queued wake in R35.1's order: token and hash, deadline, turn, 👀
//! for an owner trigger, typing, then a WakeRunner that builds the payload context and runs the
//! adapter. Posts, passes and ETAs arrive as [`ApiRequest`]s; replies and status notes get their
//! `posts` row before they are sent (DD-6). A wake ends exactly once, by [`Core::finish`], with
//! the precedence killed, timeout, posted, passed, failed, and the reaction the endings table
//! gives it.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use nostr::{Filter, Kind};
use router_core::classify::classify;
use router_core::config::{AdapterConfig, ReplyMode, Roster, RouterConfig, WebhookMode};
use router_core::ids::{BotName, ChannelId, EventId};
use router_core::payload::{
    format_deadline, turns_left_after_this, ApiRef, ChannelRef, ContextMessage, WakePayload,
};
use router_core::route::{Control, Priority, KIND_MESSAGE};
use router_core::thread::RoundMode;
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::apply::{author_name, event_class, in_event, Core};
use super::queue::{decode, top_trigger, Trigger};
use super::timers::WakeTimers;
use super::CoreMsg;
use crate::adapter::command::CommandAdapter;
use crate::adapter::webhook::WebhookAdapter;
use crate::adapter::{Adapter, AdapterEvent, SyncReply, WakeContext};
use crate::publish::{build_reply, build_status_note, build_typing};
use crate::relay::RelayPort;
use crate::store::posts::PostRow;
use crate::store::threads::Turns;
use crate::store::wakes::{WakeRow, WakeState, Wakes};
use crate::store::{Store, StoreError};

/// The reaction for a dispatched owner-caused wake (R35.4).
const EYES: &str = "\u{1F440}";
/// The reaction for an owner-caused Direct pass (R36.3).
const CHECK: &str = "\u{2705}";
/// The reaction for a timeout (R27.2).
const HOURGLASS: &str = "\u{231B}";
/// The reaction for a failure (R36.5).
const WARNING: &str = "\u{26A0}\u{FE0F}";
/// The reaction for a killed wake (R36.4).
const STOP: &str = super::control::STOP_EMOJI;

/// What a command prints to post nothing (R36.2).
const NO_REPLY: &str = "[no-reply]";
/// How many thread messages the payload context holds (R40.5).
const CONTEXT_LIMIT: usize = 20;
/// How long an api-reply-mode command may run after it passes (DD-20).
const PASS_GRACE: Duration = Duration::from_secs(5);
/// How many trigger events are kept for the context fallback.
const TRIGGER_CACHE_CAPACITY: usize = 2_000;

/// A wake-token request to the core (design section 8). The HTTP layer authenticates nothing
/// itself: the core resolves the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiRequest {
    /// `POST /v1/post`.
    Post {
        /// The bearer wake token.
        token: String,
        /// The reply text.
        text: String,
    },
    /// `POST /v1/pass`.
    Pass {
        /// The bearer wake token.
        token: String,
    },
    /// `POST /v1/eta`.
    Eta {
        /// The bearer wake token.
        token: String,
        /// The estimate, such as `10 minutes`.
        text: String,
    },
    /// `POST /v1/stop`, `/v1/resume` or `/v1/cancel`, already authorised by the admin token.
    Control(Control),
}

/// The core's answer to an [`ApiRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiResponse {
    /// The reply was published.
    Posted {
        /// The reply's event id, in hex.
        event_id: String,
    },
    /// The pass or ETA was recorded.
    Done,
    /// The request was refused or failed.
    Failed(ApiFailure),
}

/// Why the core refused or failed an [`ApiRequest`] (design section 8, DD-22).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiFailure {
    /// No wake has this token.
    #[error("unknown wake token")]
    Unauthorized,
    /// The wake's bot is halted.
    #[error("the bot is halted")]
    Halted,
    /// The wake has ended.
    #[error("the wake has ended")]
    WakeEnded,
    /// The wake has posted `max_posts_per_wake` replies.
    #[error("the wake has posted its maximum")]
    TooManyPosts,
    /// Neither transport delivered the reply.
    #[error("cannot publish: {0}")]
    PublishFailed(String),
    /// The core could not read its state.
    #[error("internal error: {0}")]
    Internal(String),
}

/// What a tracked publish is for, so its result can be applied.
pub(crate) enum Purpose {
    /// An API post, answered when the publish completes.
    Api(oneshot::Sender<ApiResponse>),
    /// A command's stdout or a sync webhook's text: the wake ends with the result.
    Reply,
    /// The status note.
    StatusNote,
}

/// A dispatched wake the core is waiting on (design 6.6, `RunningWake`).
pub(super) struct RunningWake {
    pub bot: BotName,
    pub root: EventId,
    pub channel: ChannelId,
    /// The reaction target, which replies and status notes are threaded under (A8).
    pub target: EventId,
    /// Owner-caused (priority `Owner`) and Direct: ✅ on a pass (A7).
    pub owner_direct: bool,
    /// A command adapter in api reply mode: a pass ends the wake (DD-20).
    pub api_reply_mode: bool,
    /// An async webhook: the first post or a pass ends the wake (A6).
    pub async_webhook: bool,
    pub posts: u32,
    pub posted: Vec<String>,
    pub passed: bool,
    pub eta: Option<String>,
    pub cancel: CancellationToken,
    pub timers: WakeTimers,
}

/// Each configured bot's adapter (R1.5): a [`CommandAdapter`] keeping wake files under
/// `data_dir` and pointing agents at the loopback API, or a [`WebhookAdapter`] sending through
/// the shared rustls client `http`.
pub fn select_adapters(
    config: &RouterConfig,
    data_dir: &Path,
    http: &reqwest::Client,
) -> BTreeMap<BotName, Arc<dyn Adapter>> {
    let webhook: Arc<dyn Adapter> = Arc::new(WebhookAdapter::new(http.clone()));
    let command: Arc<dyn Adapter> = Arc::new(CommandAdapter::new(
        data_dir.to_path_buf(),
        format!("http://{}", config.api_bind),
    ));
    config
        .bots
        .iter()
        .map(|(name, bot)| {
            let adapter = match bot.adapter {
                AdapterConfig::Command { .. } => command.clone(),
                AdapterConfig::Webhook { .. } => webhook.clone(),
            };
            (name.clone(), adapter)
        })
        .collect()
}

/// Recent trigger events, for the payload context when the thread query fails (design 7).
#[derive(Default)]
pub(super) struct TriggerCache {
    events: HashMap<EventId, nostr::Event>,
    order: VecDeque<EventId>,
}

impl TriggerCache {
    pub(super) fn put(&mut self, event: &nostr::Event) {
        let id = EventId::from_nostr(&event.id);
        if self.events.insert(id.clone(), event.clone()).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > TRIGGER_CACHE_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.events.remove(&oldest);
            }
        }
    }

    fn get(&self, id: &EventId) -> Option<&nostr::Event> {
        self.events.get(id)
    }
}

impl Core {
    /// Starts `wake` and hands it to a WakeRunner (design 6.6, dispatch steps 1 to 7).
    pub(super) fn dispatch(&mut self, wake: WakeRow, now: DateTime<Utc>) {
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
        let limits = self.limits(&bot);
        let deadline = now + duration_secs(limits.max_wake_minutes.saturating_mul(60));
        let previous_ms = self
            .store
            .wakes()
            .previous_start(&bot, &wake.root_id, &wake.id)
            .unwrap_or_else(|error| {
                tracing::warn!(%error, wake_id = %wake.id, "cannot read the previous wake; marking all context new");
                None
            });
        let used = match start_wake(
            &mut self.store,
            &wake,
            &thread.round_id,
            &token_hash(&token),
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
        let target = reaction_target(&triggers).unwrap_or_else(|| wake.root_id.clone());
        let mode = round_mode(&top.mode);
        if triggers.iter().any(|trigger| trigger.class == "owner") {
            self.react_on(&bot, &target, EYES);
        }
        let turns_left = turns_left_after_this(limits.turns_per_round, used);
        let channel_name = self.channel_name(thread.channel_id);
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
            reply_parent_id: target.to_string(),
            reason: top.reason,
            round_mode: mode,
            turns_left_after_this: turns_left,
            turns_per_round: limits.turns_per_round,
            deadline: format_deadline(deadline),
            triggers: trigger_ids.iter().map(ToString::to_string).collect(),
            context: Vec::new(),
            api: ApiRef::new(api_url),
        };
        let owner_direct = top.priority == Priority::Owner && mode == RoundMode::Direct;
        let api_reply_mode = matches!(
            adapter_config,
            AdapterConfig::Command {
                reply_mode: ReplyMode::Api,
                ..
            }
        );
        let async_webhook = matches!(
            adapter_config,
            AdapterConfig::Webhook {
                mode: WebhookMode::Async,
                ..
            }
        );
        let context = ContextSource {
            relay: self.relays.get(&bot).cloned(),
            roster: self.roster.clone(),
            channel: thread.channel_id,
            root: wake.root_id.clone(),
            previous_ms,
            fallback: trigger_ids
                .iter()
                .filter_map(|id| self.trigger_events.get(id).cloned())
                .collect(),
        };
        let ctx = WakeContext {
            wake_id: wake.id,
            token,
            bot: bot.clone(),
            adapter: adapter_config,
            channel: thread.channel_id,
            root: wake.root_id.clone(),
            reply_parent: target.clone(),
            reason: top.reason,
            reason_author: top.author,
            mode,
            turns_left,
            limits,
            deadline,
            trigger_ids,
        };
        let cancel = CancellationToken::new();
        let status_note_at =
            owner_direct.then(|| now + duration_secs(limits.status_note_after_secs));
        self.running.insert(
            wake.id,
            RunningWake {
                bot,
                root: wake.root_id,
                channel: thread.channel_id,
                target,
                owner_direct,
                api_reply_mode,
                async_webhook,
                posts: 0,
                posted: Vec::new(),
                passed: false,
                eta: None,
                cancel: cancel.clone(),
                timers: WakeTimers::new(now, deadline, status_note_at),
            },
        );
        self.send_typing(&wake.id);
        spawn_runner(
            &adapter,
            ctx,
            payload,
            context,
            cancel,
            self.self_tx.clone(),
        );
    }

    /// Answers a wake-token request (design section 8, R42.4 precedence).
    pub(super) fn api(&mut self, request: ApiRequest, reply: oneshot::Sender<ApiResponse>) {
        let token = match &request {
            ApiRequest::Post { token, .. }
            | ApiRequest::Pass { token }
            | ApiRequest::Eta { token, .. } => token,
            ApiRequest::Control(control) => return self.admin_control(control, reply),
        };
        let wake = match self.store.wakes().find_by_token_hash(&token_hash(token)) {
            Ok(Some(wake)) => wake,
            Ok(None) => return answer(reply, Err(ApiFailure::Unauthorized)),
            Err(error) => return answer(reply, Err(ApiFailure::Internal(error.to_string()))),
        };
        match request {
            ApiRequest::Post { text, .. } => self.api_post(&wake, &text, reply),
            ApiRequest::Pass { .. } => {
                let Some(running) = self.live(&wake) else {
                    return answer(reply, Err(ApiFailure::WakeEnded));
                };
                running.passed = true;
                if running.api_reply_mode {
                    let cancel = running.cancel.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(PASS_GRACE).await;
                        cancel.cancel();
                    });
                    self.finish(&wake.id, WakeState::Passed, "passed");
                } else if running.async_webhook {
                    self.finish(&wake.id, WakeState::Passed, "passed");
                }
                answer(reply, Ok(None));
            }
            ApiRequest::Eta { text, .. } => {
                let Some(running) = self.live(&wake) else {
                    return answer(reply, Err(ApiFailure::WakeEnded));
                };
                running.eta = Some(text);
                answer(reply, Ok(None));
            }
            ApiRequest::Control(_) => {}
        }
    }

    /// `POST /v1/post`: 423 if halted, then 410 if ended, then 429 at the limit, else publish.
    fn api_post(&mut self, wake: &WakeRow, text: &str, reply: oneshot::Sender<ApiResponse>) {
        match self.is_halted(&wake.bot) {
            Ok(false) => {}
            Ok(true) => return answer(reply, Err(ApiFailure::Halted)),
            Err(error) => return answer(reply, Err(ApiFailure::Internal(error.to_string()))),
        }
        let max_posts = self.limits(&wake.bot).max_posts_per_wake;
        let Some(running) = self.live(wake) else {
            return answer(reply, Err(ApiFailure::WakeEnded));
        };
        // An async webhook's first post ends the wake once it is published (A6).
        if running.async_webhook && running.posts > 0 {
            return answer(reply, Err(ApiFailure::WakeEnded));
        }
        if running.posts >= max_posts {
            return answer(reply, Err(ApiFailure::TooManyPosts));
        }
        match self.build_wake_reply(&wake.id, text) {
            Ok(event) => self.publish_tracked(&wake.id, event, Purpose::Api(reply)),
            Err(error) => answer(reply, Err(ApiFailure::PublishFailed(error))),
        }
    }

    /// The running wake behind `wake`, unless its row has ended.
    fn live(&mut self, wake: &WakeRow) -> Option<&mut RunningWake> {
        if wake.state != WakeState::Running {
            return None;
        }
        self.running.get_mut(&wake.id)
    }

    /// Applies a tracked publish's result (design 6.8, reply step 5).
    pub(super) fn published(
        &mut self,
        wake_id: Uuid,
        event_id: EventId,
        purpose: Purpose,
        result: Result<(), String>,
    ) {
        if let Err(error) = &result {
            if let Err(store_error) = self.store.posts().delete(&event_id) {
                tracing::warn!(error = %store_error, "cannot delete the posts row of a failed publish");
            }
            tracing::warn!(%error, wake_id = %wake_id, "cannot publish");
        }
        let running = self.running.get_mut(&wake_id);
        match (purpose, result) {
            (Purpose::Api(reply), Ok(())) => {
                let ends = running.is_some_and(|running| {
                    running.posted.push(event_id.to_string());
                    running.async_webhook
                });
                answer(reply, Ok(Some(event_id.to_string())));
                if ends {
                    self.finish(&wake_id, WakeState::Posted, "posted");
                }
            }
            (Purpose::Api(reply), Err(error)) => {
                if let Some(running) = running {
                    running.posts = running.posts.saturating_sub(1);
                }
                answer(reply, Err(ApiFailure::PublishFailed(error)));
            }
            (Purpose::Reply, Ok(())) => {
                if let Some(running) = running {
                    running.posted.push(event_id.to_string());
                }
                self.finish(&wake_id, WakeState::Posted, "posted");
            }
            (Purpose::Reply, Err(error)) => {
                if let Some(running) = running {
                    running.posts = running.posts.saturating_sub(1);
                }
                self.finish(&wake_id, WakeState::Failed, &error);
            }
            (Purpose::StatusNote, _) => {}
        }
    }

    /// Ends a wake whose adapter run returned (design 6.6, endings).
    pub(super) fn wake_ended(&mut self, wake_id: Uuid, event: AdapterEvent) {
        let Some(running) = self.running.get(&wake_id) else {
            return;
        };
        if running.posts > 0 {
            return self.finish(&wake_id, WakeState::Posted, "posted");
        }
        if running.passed {
            return self.finish(&wake_id, WakeState::Passed, "passed");
        }
        match event {
            AdapterEvent::AsyncAccepted => {}
            AdapterEvent::Exited {
                code: Some(0),
                stdout,
            } => match stdout.as_deref().and_then(reply_text) {
                Some(text) => self.reply_and_end(&wake_id, text),
                None => self.finish(&wake_id, WakeState::Passed, "passed"),
            },
            AdapterEvent::SyncReply(SyncReply::Text(text)) => match reply_text(&text) {
                Some(text) => self.reply_and_end(&wake_id, text),
                None => self.finish(&wake_id, WakeState::Passed, "passed"),
            },
            AdapterEvent::SyncReply(SyncReply::Pass) => {
                self.finish(&wake_id, WakeState::Passed, "passed");
            }
            AdapterEvent::Exited { code, .. } => {
                self.finish(&wake_id, WakeState::Failed, &format!("exit code {code:?}"));
            }
            AdapterEvent::Failed(message) => self.finish(&wake_id, WakeState::Failed, &message),
        }
    }

    /// Publishes a stdout or sync-webhook reply; the wake ends with the publish result. A halted
    /// bot's reply is discarded (R30.5).
    fn reply_and_end(&mut self, wake_id: &Uuid, text: &str) {
        let bot = match self.running.get(wake_id) {
            Some(running) => running.bot.clone(),
            None => return,
        };
        if !matches!(self.is_halted(&bot), Ok(false)) {
            return self.finish(wake_id, WakeState::Killed, "halted; reply discarded");
        }
        match self.build_wake_reply(wake_id, text) {
            Ok(event) => self.publish_tracked(wake_id, event, Purpose::Reply),
            Err(error) => self.finish(wake_id, WakeState::Failed, &error),
        }
    }

    /// Builds a reply for a running wake, threaded under its reaction target.
    fn build_wake_reply(&self, wake_id: &Uuid, text: &str) -> Result<nostr::Event, String> {
        let running = self.running.get(wake_id).ok_or("the wake has ended")?;
        let keys = self.keys.get(&running.bot).ok_or("no key for the bot")?;
        build_reply(
            &self.roster,
            keys,
            self.auth_tag(&running.bot),
            &running.channel,
            &running.root,
            &running.target,
            text,
        )
        .map_err(|error| error.to_string())
    }

    /// Publishes the status note, unless the bot is halted (R46, R30.5).
    pub(super) fn send_status_note(&mut self, wake_id: &Uuid) {
        let Some(running) = self.running.get(wake_id) else {
            return;
        };
        if !matches!(self.is_halted(&running.bot), Ok(false)) {
            return;
        }
        let Some(keys) = self.keys.get(&running.bot) else {
            return;
        };
        match build_status_note(
            &self.roster,
            keys,
            self.auth_tag(&running.bot),
            &running.channel,
            &running.root,
            &running.target,
            running.eta.as_deref(),
        ) {
            Ok(event) => self.publish_tracked(wake_id, event, Purpose::StatusNote),
            Err(error) => tracing::warn!(%error, wake_id = %wake_id, "cannot build a status note"),
        }
    }

    /// Publishes a typing indicator for a running wake over the WebSocket; failures are ignored
    /// (R35.5).
    pub(super) fn send_typing(&mut self, wake_id: &Uuid) {
        let Some(running) = self.running.get(wake_id) else {
            return;
        };
        let (Some(keys), Some(relay)) = (
            self.keys.get(&running.bot),
            self.relays.get(&running.bot).cloned(),
        ) else {
            return;
        };
        let event = match build_typing(keys, &running.channel, &running.root, &running.target) {
            Ok(event) => event,
            Err(error) => {
                tracing::debug!(%error, "cannot build a typing indicator");
                return;
            }
        };
        self.publishes.spawn(async move {
            if let Err(error) = relay.publish(event).await {
                tracing::debug!(%error, "cannot publish a typing indicator");
            }
        });
    }

    /// Writes the `posts` row, counts a reply against the wake, and publishes in the background
    /// (DD-6). The result comes back as [`CoreMsg::Published`].
    fn publish_tracked(&mut self, wake_id: &Uuid, event: nostr::Event, purpose: Purpose) {
        let Some(running) = self.running.get_mut(wake_id) else {
            return;
        };
        let event_id = EventId::from_nostr(&event.id);
        let row = PostRow {
            event_id: event_id.clone(),
            bot: running.bot.clone(),
            wake_id: Some(*wake_id),
            created_at: i64::try_from(event.created_at.as_secs()).unwrap_or(i64::MAX),
        };
        if let Err(error) = self.store.posts().insert(&row) {
            tracing::warn!(%error, wake_id = %wake_id, "cannot record a post; not sending it");
            if let Purpose::Api(reply) = purpose {
                answer(reply, Err(ApiFailure::Internal(error.to_string())));
            }
            return;
        }
        if !matches!(purpose, Purpose::StatusNote) {
            running.posts += 1;
        }
        let Some(relay) = self.relays.get(&running.bot).cloned() else {
            return self.published(*wake_id, event_id, purpose, Err("no relay".to_owned()));
        };
        let core_tx = self.self_tx.clone();
        let wake_id = *wake_id;
        self.publishes.spawn(async move {
            let result = relay
                .publish(event)
                .await
                .map_err(|error| error.to_string());
            let _ = core_tx.send(CoreMsg::Published {
                wake_id,
                event_id,
                purpose,
                result,
            });
        });
    }

    /// Ends a running wake: records the state and outcome (revoking the token), reacts as the
    /// endings table says, and frees the slot. The caller cancels the runner when it must.
    pub(super) fn finish(&mut self, wake_id: &Uuid, state: WakeState, detail: &str) {
        let Some(running) = self.running.remove(wake_id) else {
            return;
        };
        let outcome = serde_json::json!({ "posted": running.posted, "detail": detail }).to_string();
        let ended_at = self.clock.now().timestamp_millis();
        if let Err(error) = self
            .store
            .wakes()
            .finish(wake_id, state, ended_at, Some(&outcome))
        {
            tracing::warn!(%error, wake_id = %wake_id, "cannot record the end of a wake");
        }
        let reaction = match state {
            WakeState::Passed if running.owner_direct => Some(CHECK),
            WakeState::Timeout => Some(HOURGLASS),
            WakeState::Killed => Some(STOP),
            WakeState::Failed => Some(WARNING),
            _ => None,
        };
        if let Some(emoji) = reaction {
            self.react_on(&running.bot, &running.target, emoji);
        }
    }

    /// The channel's name: the roster name, else the discovered name, else the UUID (design 7).
    fn channel_name(&self, channel: ChannelId) -> String {
        self.roster
            .channels
            .get(&channel)
            .map(|channel| channel.name.clone())
            .or_else(|| self.channel_names.get(&channel).cloned())
            .unwrap_or_else(|| channel.to_string())
    }

    fn auth_tag(&self, bot: &BotName) -> Option<&nostr::Tag> {
        self.config
            .bots
            .get(bot)
            .and_then(|bot| bot.auth_tag.as_ref())
    }

    fn is_halted(&self, bot: &BotName) -> Result<bool, StoreError> {
        let halts = self.halts()?;
        Ok(halts.all || halts.bots.contains(bot))
    }

    /// Publishes `bot`'s reaction `emoji` on `target`.
    fn react_on(&mut self, bot: &BotName, target: &EventId, emoji: &str) {
        match nostr::EventId::from_hex(target.as_str()) {
            Ok(target) => self.react(bot, &target, emoji),
            Err(error) => tracing::warn!(%error, "a reaction target that is not an event id"),
        }
    }
}

/// The text to post from a command's stdout or a sync reply, or `None` for a pass (R36.2).
fn reply_text(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty() && text != NO_REPLY).then_some(text)
}

fn answer(reply: oneshot::Sender<ApiResponse>, result: Result<Option<String>, ApiFailure>) {
    let response = match result {
        Ok(Some(event_id)) => ApiResponse::Posted { event_id },
        Ok(None) => ApiResponse::Done,
        Err(failure) => ApiResponse::Failed(failure),
    };
    let _ = reply.send(response);
}

/// What a WakeRunner needs to build the payload context (design 7).
struct ContextSource {
    relay: Option<Arc<dyn RelayPort>>,
    roster: Roster,
    channel: ChannelId,
    root: EventId,
    /// When this bot's previous wake in the thread started, in unix milliseconds (DD-17).
    previous_ms: Option<i64>,
    /// The trigger events, used when the thread query fails.
    fallback: Vec<nostr::Event>,
}

impl ContextSource {
    /// The root and its replies, oldest first, the last [`CONTEXT_LIMIT`] kept (R40.5).
    async fn build(self) -> Vec<ContextMessage> {
        let queried = match (&self.relay, nostr::EventId::from_hex(self.root.as_str())) {
            (Some(relay), Ok(root)) => relay
                .query(vec![
                    Filter::new().id(root),
                    Filter::new()
                        .kind(Kind::Custom(KIND_MESSAGE))
                        .event(root)
                        .limit(CONTEXT_LIMIT),
                ])
                .await
                .map_err(|error| error.to_string()),
            (None, _) => Err("no relay".to_owned()),
            (_, Err(error)) => Err(error.to_string()),
        };
        let mut events = queried.unwrap_or_else(|error| {
            tracing::warn!(%error, root = %self.root, "cannot fetch the thread; the context holds only the triggers");
            self.fallback.clone()
        });
        events.sort_by_key(|event| (event.created_at.as_secs(), event.id.to_hex()));
        let mut seen = HashSet::new();
        events.retain(|event| seen.insert(event.id));
        let skip = events.len().saturating_sub(CONTEXT_LIMIT);
        events
            .iter()
            .skip(skip)
            .map(|event| self.message(event))
            .collect()
    }

    fn message(&self, event: &nostr::Event) -> ContextMessage {
        let in_event = in_event(event, self.channel);
        let class = classify(&in_event, &self.roster);
        let created_ms = in_event.created_at.saturating_mul(1_000);
        ContextMessage {
            id: event.id.to_hex(),
            author: author_name(&in_event.pubkey, &class, &self.roster),
            class: event_class(&class).as_str().to_owned(),
            created_at: DateTime::from_timestamp(in_event.created_at, 0)
                .map(format_deadline)
                .unwrap_or_default(),
            text: event.content.clone(),
            new: self
                .previous_ms
                .is_none_or(|previous| created_ms > previous),
        }
    }
}

/// Builds the payload context, then runs the adapter in its own task and reports the result.
fn spawn_runner(
    adapter: &Arc<dyn Adapter>,
    ctx: WakeContext,
    mut payload: WakePayload,
    context: ContextSource,
    cancel: CancellationToken,
    core_tx: mpsc::UnboundedSender<CoreMsg>,
) {
    let adapter = Arc::clone(adapter);
    let wake_id = ctx.wake_id;
    tokio::spawn(async move {
        payload.context = context.build().await;
        let event = adapter.run(ctx, payload, cancel).await;
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

/// The stored form of a wake token: hex SHA-256 (DD-10).
fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn duration_secs(secs: u64) -> TimeDelta {
    i64::try_from(secs)
        .ok()
        .and_then(TimeDelta::try_seconds)
        .unwrap_or(TimeDelta::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::route::Reason;

    fn trigger(class: &str, created_at: i64) -> Trigger {
        Trigger {
            event_id: format!("{created_at:064x}"),
            edit_id: None,
            class: class.to_owned(),
            author: "X".to_owned(),
            reason: Reason::Mention,
            priority: Priority::Bot,
            debounce: false,
            mode: "direct".to_owned(),
            created_at,
            received_at_ms: created_at * 1_000,
        }
    }

    #[test]
    fn the_reaction_target_prefers_the_latest_owner_trigger() {
        let owner = trigger("owner", 2);
        let triggers = [owner.clone(), trigger("bot", 9)];
        assert_eq!(
            reaction_target(&triggers).map(|id| id.to_string()),
            Some(owner.event_id)
        );
    }

    #[test]
    fn empty_or_no_reply_output_is_a_pass() {
        assert_eq!(reply_text(""), None);
        assert_eq!(reply_text(" [no-reply]\n"), None);
        assert_eq!(reply_text(" hi \n"), Some("hi"));
    }
}
