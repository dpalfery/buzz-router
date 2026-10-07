//! Applying route results and rebuilding threads (design sections 6.3, 6.4 and 9.3).
//!
//! For each [`EnrichedEvent`] the core checks idempotency again, builds the [`Snapshot`] (halts
//! re-read from the `halts` table, members from discovery, wake counts from SQL, the quiet set),
//! calls [`route`], and applies the result in one SQLite transaction: the `events` row, the
//! thread update, the queued wakes and the turn-cap flags. After the commit it reacts ⏸️ for each
//! first Cap suppression in a round, counts Budget suppressions, and advances the cursor.
//!
//! A `RebuildThread` replays history through [`route`] applying only the thread updates, so the
//! router's state matches a thread it never saw: nothing is published and no control is executed
//! (A17).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use buzz_sdk::builders::{build_reaction, extract_channel_id};
use chrono::{DateTime, Utc};
use nostr::nips::nip19::ToBech32;
use nostr::Event;
use router_core::classify::{classify, AuthorClass};
use router_core::config::{Limits, Roster, RouterConfig};
use router_core::ids::{BotName, ChannelId, EventId, Pubkey};
use router_core::quiet::quiet_set;
use router_core::route::{
    route, Decision, Diagnostic, Halts, InEvent, Priority, Reason, RouteResult, Snapshot,
    SuppressWhy, ThreadUpdate, WakeCounts, KIND_EDIT, KIND_MESSAGE,
};
use router_core::thread::{thread_position, RoundMode, ThreadPos, ThreadState};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use uuid::Uuid;

use super::queue::{enqueue, Debounce, RunningWake, Trigger};
use super::{CoreMsg, DebugCounters};
use crate::adapter::Adapter;
use crate::clock::Clock;
use crate::ingest::EnrichedEvent;
use crate::relay::RelayPort;
use crate::store::events::{EventClass, EventRow, Events};
use crate::store::halts::HaltScope;
use crate::store::threads::{Threads, Turns};
use crate::store::wakes::Wakes;
use crate::store::{Store, StoreError};

/// The reaction a bot places the first time the turn cap suppresses it in a round (R19.3).
const PAUSE: &str = "\u{23F8}\u{FE0F}";

/// How many threads the cache holds (design section 6.4).
const THREAD_CACHE_CAPACITY: usize = 2_000;

/// The length of the hourly budget window, in milliseconds (A10).
const HOUR_MS: i64 = 3_600_000;
/// The length of the daily budget window, in milliseconds (A10).
const DAY_MS: i64 = 86_400_000;

/// Each bot's dispatched wakes in the trailing hour and day before `now` (design 9.3, A10).
pub fn wake_counts(
    store: &Store,
    bots: &BTreeSet<BotName>,
    now: DateTime<Utc>,
) -> Result<BTreeMap<BotName, WakeCounts>, StoreError> {
    let now_ms = now.timestamp_millis();
    let wakes = store.wakes();
    bots.iter()
        .map(|bot| {
            let counts = WakeCounts {
                hour: wakes.count_started_since(bot, now_ms - HOUR_MS)?,
                day: wakes.count_started_since(bot, now_ms - DAY_MS)?,
            };
            Ok((bot.clone(), counts))
        })
        .collect()
}

/// The in-memory thread cache: least recently used threads are evicted past the capacity, and
/// reload from the store (design section 6.4).
#[derive(Default)]
pub(super) struct ThreadCache {
    entries: HashMap<EventId, (ThreadState, u64)>,
    tick: u64,
}

impl ThreadCache {
    fn get(&mut self, root: &EventId) -> Option<ThreadState> {
        self.tick += 1;
        let tick = self.tick;
        self.entries.get_mut(root).map(|(state, used)| {
            *used = tick;
            state.clone()
        })
    }

    pub(super) fn put(&mut self, state: ThreadState) {
        self.tick += 1;
        self.entries
            .insert(state.root_id.clone(), (state, self.tick));
        if self.entries.len() > THREAD_CACHE_CAPACITY {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(root, _)| root.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
    }
}

/// The core actor's state.
pub(super) struct Core {
    pub(super) store: Store,
    pub(super) roster: Roster,
    pub(super) config: RouterConfig,
    local_bots: BTreeSet<BotName>,
    pub(super) clock: Arc<dyn Clock>,
    relays: BTreeMap<BotName, Arc<dyn RelayPort>>,
    keys: BTreeMap<BotName, nostr::Keys>,
    memberships: BTreeMap<BotName, BTreeSet<ChannelId>>,
    pub(super) adapters: BTreeMap<BotName, Arc<dyn Adapter>>,
    pub(super) threads: ThreadCache,
    counters: DebugCounters,
    drift_logged: HashSet<Pubkey>,
    publishes: JoinSet<()>,
    pub(super) running: HashMap<Uuid, RunningWake>,
    /// When the core received each thread's latest roster-bot post, in unix milliseconds.
    last_bot_post: HashMap<EventId, i64>,
    pub(super) self_tx: mpsc::UnboundedSender<CoreMsg>,
}

/// What the core actor is built from, besides its own sender.
pub(super) struct CoreParts {
    pub store: Store,
    pub roster: Roster,
    pub config: RouterConfig,
    pub clock: Arc<dyn Clock>,
    pub relays: BTreeMap<BotName, Arc<dyn RelayPort>>,
    pub keys: BTreeMap<BotName, nostr::Keys>,
    pub memberships: BTreeMap<BotName, BTreeSet<ChannelId>>,
    pub adapters: BTreeMap<BotName, Arc<dyn Adapter>>,
}

impl Core {
    pub(super) fn new(parts: CoreParts, self_tx: mpsc::UnboundedSender<CoreMsg>) -> Self {
        let local_bots = parts.config.bots.keys().cloned().collect();
        Self {
            store: parts.store,
            roster: parts.roster,
            config: parts.config,
            local_bots,
            clock: parts.clock,
            relays: parts.relays,
            keys: parts.keys,
            memberships: parts.memberships,
            adapters: parts.adapters,
            threads: ThreadCache::default(),
            counters: DebugCounters::default(),
            drift_logged: HashSet::new(),
            publishes: JoinSet::new(),
            running: HashMap::new(),
            last_bot_post: HashMap::new(),
            self_tx,
        }
    }

    /// Handles one message, then schedules the queue.
    pub(super) async fn handle(&mut self, message: CoreMsg) {
        while self.publishes.try_join_next().is_some() {}
        match message {
            CoreMsg::Enriched(event) => self.apply_event(&event),
            CoreMsg::RebuildThread { root, events } => self.rebuild(&root, events),
            CoreMsg::Memberships { bot, channels } => {
                self.memberships.insert(bot, channels);
            }
            CoreMsg::WakeEnded { wake_id, event } => self.wake_ended(wake_id, event),
            CoreMsg::Flush(reply) => {
                self.schedule();
                while self.publishes.join_next().await.is_some() {}
                let _ = reply.send(());
                return;
            }
            CoreMsg::DebugCounters(reply) => {
                let _ = reply.send(self.counters.clone());
                return;
            }
            CoreMsg::Shutdown => return,
        }
        self.schedule();
    }

    /// The core steps for one event (design 6.3, core steps 1 and 3 to 7).
    fn apply_event(&mut self, ev: &EnrichedEvent) {
        let now = self.clock.now();
        let id = &ev.in_event.id;
        match self.store.events().is_processed(id) {
            Ok(false) => {}
            Ok(true) => {
                self.advance_cursor(ev);
                return;
            }
            Err(error) => {
                tracing::warn!(%error, event_id = %id, "cannot read events; leaving the event unprocessed");
                return;
            }
        }
        let thread = ev.root.as_ref().and_then(|root| self.load_thread(root));
        let (halts, counts) = match (
            self.halts(),
            wake_counts(&self.store, &self.local_bots, now),
        ) {
            (Ok(halts), Ok(counts)) => (halts, counts),
            (Err(error), _) | (_, Err(error)) => {
                tracing::warn!(%error, event_id = %id, "cannot build the snapshot; leaving the event unprocessed");
                return;
            }
        };
        let snapshot = Snapshot {
            roster: &self.roster,
            local_bots: &self.local_bots,
            local_members: self.local_members(ev.in_event.channel),
            halts: &halts,
            thread: thread.clone(),
            parent_author: ev.parent_author.clone(),
            edit_target: ev.edit_target.clone(),
            wake_counts: &counts,
            quiet: quiet_set(&self.roster, now),
        };
        let result = route(&ev.in_event, &snapshot, now);
        self.log_diagnostics(&result);

        let has_wake = result
            .decisions
            .iter()
            .any(|decision| matches!(decision, Decision::Wake { .. }));
        let updated = ev.root.as_ref().and_then(|root| {
            updated_thread(thread, root, &ev.in_event, &result.thread_update, has_wake)
        });
        let class = classify(&ev.in_event, &self.roster);
        let received_ms = ev.received_at.timestamp_millis();
        self.prune_bot_posts(now.timestamp_millis());
        if let (AuthorClass::Bot(_), Some(root), KIND_MESSAGE) =
            (&class, &ev.root, ev.in_event.kind)
        {
            self.last_bot_post.insert(root.clone(), received_ms);
        }
        let last_bot_post = ev
            .root
            .as_ref()
            .and_then(|root| self.last_bot_post.get(root).copied());
        let debounce: BTreeMap<BotName, Debounce> = result
            .decisions
            .iter()
            .filter_map(|decision| match decision {
                Decision::Wake { bot, .. } => {
                    Some((bot.clone(), Debounce::new(&self.limits(bot), last_bot_post)))
                }
                _ => None,
            })
            .collect();
        let trigger_base = TriggerBase {
            event_id: ev
                .edit_target
                .as_ref()
                .map_or_else(|| id.clone(), |target| target.message_id.clone()),
            edit_id: (ev.in_event.kind == KIND_EDIT).then(|| id.clone()),
            class: event_class(&class),
            author: author_name(&ev.in_event.pubkey, &class, &self.roster),
            mode: result.wake_mode,
            created_at: ev.in_event.created_at,
            received_at_ms: received_ms,
        };
        let row = EventRow {
            id: id.clone(),
            channel_id: ev.in_event.channel,
            root_id: ev.root.clone().unwrap_or_else(|| id.clone()),
            author: ev.in_event.pubkey.clone(),
            class: event_class(&class),
            kind: ev.in_event.kind,
            created_at: ev.in_event.created_at,
            processed_at: Some(now.timestamp_millis()),
        };
        let reactions = match commit_event(
            &mut self.store,
            &row,
            updated.as_ref(),
            &result.decisions,
            &trigger_base,
            &debounce,
            now.timestamp_millis(),
        ) {
            Ok(reactions) => reactions,
            Err(error) => {
                tracing::warn!(%error, event_id = %id, "cannot apply the event; it stays unprocessed");
                return;
            }
        };
        if let Some(state) = updated {
            self.threads.put(state);
        }
        for decision in &result.decisions {
            if let Decision::Suppress {
                bot,
                why: SuppressWhy::Budget,
            } = decision
            {
                *self
                    .counters
                    .budget_suppressed
                    .entry(bot.clone())
                    .or_default() += 1;
                tracing::info!(bot = %bot, event_id = %id, "wake suppressed by the budget");
            }
        }
        self.counters.last_decisions = result.decisions;
        for bot in reactions {
            self.react(&bot, &ev.event.id, PAUSE);
        }
        self.advance_cursor(ev);
    }

    /// Replays a thread's history, applying only the thread updates (design 6.3, RebuildThread).
    fn rebuild(&mut self, root: &EventId, mut events: Vec<Event>) {
        events.sort_by_key(|event| (event.created_at.as_secs(), event.id.to_hex()));
        let no_halts = Halts {
            all: false,
            bots: BTreeSet::new(),
        };
        let no_counts = BTreeMap::new();
        let now_ms = self.clock.now().timestamp_millis();
        let mut thread: Option<ThreadState> = None;
        let mut authors: HashMap<EventId, Pubkey> = HashMap::new();
        let mut rows = Vec::new();
        let mut messages = Vec::new();
        for event in &events {
            let Some(channel) = extract_channel_id(event) else {
                continue;
            };
            let in_event = in_event(event, ChannelId::from(channel));
            if in_event.kind != KIND_MESSAGE {
                continue;
            }
            let parent_author = match thread_position(&in_event.tags) {
                ThreadPos::Reply {
                    root: ref reply_root,
                    ref parent,
                } if parent != reply_root => authors.get(parent).cloned(),
                _ => None,
            };
            let at = DateTime::from_timestamp(in_event.created_at, 0)
                .unwrap_or_else(|| self.clock.now());
            let snapshot = Snapshot {
                roster: &self.roster,
                local_bots: &self.local_bots,
                local_members: self.local_members(in_event.channel),
                halts: &no_halts,
                thread: thread.clone(),
                parent_author,
                edit_target: None,
                wake_counts: &no_counts,
                quiet: quiet_set(&self.roster, at),
            };
            let result = route(&in_event, &snapshot, at);
            thread = updated_thread(thread, root, &in_event, &result.thread_update, false);
            authors.insert(in_event.id.clone(), in_event.pubkey.clone());
            let class = classify(&in_event, &self.roster);
            rows.push(EventRow {
                id: in_event.id.clone(),
                channel_id: in_event.channel,
                root_id: root.clone(),
                author: in_event.pubkey.clone(),
                class: event_class(&class),
                kind: in_event.kind,
                created_at: in_event.created_at,
                processed_at: Some(now_ms),
            });
            messages.push((class, in_event.created_at));
        }
        match commit_rebuild(&mut self.store, &rows, thread.as_mut(), &messages) {
            Ok(()) => {
                if let Some(state) = thread {
                    self.threads.put(state);
                }
            }
            Err(error) => {
                tracing::warn!(%error, root = %root, "cannot store the rebuilt thread");
            }
        }
    }

    /// The thread at `root`, from the cache or the store.
    pub(super) fn load_thread(&mut self, root: &EventId) -> Option<ThreadState> {
        if let Some(state) = self.threads.get(root) {
            return Some(state);
        }
        match self.store.threads().load(root) {
            Ok(Some(state)) => {
                self.threads.put(state.clone());
                Some(state)
            }
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, root = %root, "cannot load a thread; routing against a fresh one");
                None
            }
        }
    }

    /// Forgets bot posts older than the longest debounce maximum: a debounced wake is due by its
    /// first trigger plus that maximum, so they can no longer move a `dispatch_after`.
    fn prune_bot_posts(&mut self, now_ms: i64) {
        let horizon_ms = self
            .roster
            .bots
            .values()
            .map(|bot| Debounce::new(&bot.limits, None).max_ms)
            .max()
            .unwrap_or(0);
        self.last_bot_post
            .retain(|_, at| now_ms.saturating_sub(*at) <= horizon_ms);
    }

    /// The current halts, re-read from the `halts` table so CLI-written halts count.
    pub(super) fn halts(&self) -> Result<Halts, StoreError> {
        let mut halts = Halts {
            all: false,
            bots: BTreeSet::new(),
        };
        for row in self.store.halts().list()? {
            match row.scope {
                HaltScope::All => halts.all = true,
                HaltScope::Bot(bot) => {
                    halts.bots.insert(bot);
                }
            }
        }
        Ok(halts)
    }

    /// The local bots that discovery reports as members of `channel`.
    fn local_members(&self, channel: ChannelId) -> BTreeSet<BotName> {
        self.local_bots
            .iter()
            .filter(|bot| {
                self.memberships
                    .get(*bot)
                    .is_some_and(|channels| channels.contains(&channel))
            })
            .cloned()
            .collect()
    }

    /// Logs what `route` left for the caller; roster drift once per pubkey (R4.4).
    fn log_diagnostics(&mut self, result: &RouteResult) {
        for diagnostic in &result.diagnostics {
            match diagnostic {
                Diagnostic::RosterDrift { pubkey } => {
                    if self.drift_logged.insert(pubkey.clone()) {
                        tracing::warn!(pubkey = %pubkey, "a bot outside the roster carries an auth tag from the owner; add it to the roster");
                    }
                }
                Diagnostic::StatusTagIgnored => {
                    tracing::debug!("ignoring a router status note");
                }
                Diagnostic::EditTargetUnknown => {
                    tracing::debug!("ignoring an edit of an unknown message");
                }
            }
        }
    }

    /// Moves the cursor for the receiving bot and relay to the event's `created_at` (R47.4).
    fn advance_cursor(&self, ev: &EnrichedEvent) {
        if let Err(error) =
            self.store
                .cursors()
                .advance(&ev.bot, &self.config.relay_url, ev.in_event.created_at)
        {
            tracing::warn!(%error, bot = %ev.bot, "cannot advance the cursor");
        }
    }

    /// Publishes `bot`'s reaction `emoji` on `target` in the background (R45.2).
    fn react(&mut self, bot: &BotName, target: &nostr::EventId, emoji: &str) {
        let (Some(keys), Some(relay)) = (self.keys.get(bot), self.relays.get(bot)) else {
            tracing::warn!(bot = %bot, "no key or relay for a reaction");
            return;
        };
        let event = match build_reaction(*target, emoji)
            .map_err(|error| error.to_string())
            .and_then(|builder| {
                builder
                    .sign_with_keys(keys)
                    .map_err(|error| error.to_string())
            }) {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(%error, bot = %bot, "cannot build a reaction");
                return;
            }
        };
        let relay = Arc::clone(relay);
        let bot = bot.clone();
        self.publishes.spawn(async move {
            if let Err(error) = relay.publish(event).await {
                tracing::warn!(%error, bot = %bot, "cannot publish a reaction");
            }
        });
    }
}

/// The parts of a trigger shared by every bot an event wakes.
struct TriggerBase {
    event_id: EventId,
    edit_id: Option<EventId>,
    class: EventClass,
    author: String,
    mode: RoundMode,
    created_at: i64,
    received_at_ms: i64,
}

impl TriggerBase {
    fn trigger(&self, reason: Reason, priority: Priority, debounce: bool) -> Trigger {
        Trigger {
            event_id: self.event_id.to_string(),
            edit_id: self.edit_id.as_ref().map(ToString::to_string),
            class: self.class.as_str().to_owned(),
            author: self.author.clone(),
            reason,
            priority,
            debounce,
            mode: match self.mode {
                RoundMode::Direct => "direct",
                RoundMode::Discussion => "discussion",
            }
            .to_owned(),
            created_at: self.created_at,
            received_at_ms: self.received_at_ms,
        }
    }
}

/// Applies one event in a single transaction (design 6.3, core step 5). Returns the bots that
/// must react ⏸️ on the event.
fn commit_event(
    store: &mut Store,
    row: &EventRow,
    thread: Option<&ThreadState>,
    decisions: &[Decision],
    trigger: &TriggerBase,
    debounces: &BTreeMap<BotName, Debounce>,
    now_ms: i64,
) -> Result<Vec<BotName>, StoreError> {
    let debounce_of = |bot: &BotName| {
        debounces
            .get(bot)
            .copied()
            .unwrap_or_else(|| Debounce::new(&Limits::default(), None))
    };
    let tx = store.connection_mut().transaction()?;
    let events = Events::new(&tx);
    events.insert_or_ignore(row)?;
    if let Some(processed_at) = row.processed_at {
        events.mark_processed(&row.id, processed_at)?;
    }
    if let Some(thread) = thread {
        Threads::new(&tx).upsert(thread)?;
    }
    let mut reactions = Vec::new();
    for decision in decisions {
        match (decision, thread) {
            (
                Decision::Wake {
                    bot,
                    reason,
                    priority,
                    debounce,
                },
                Some(thread),
            ) => enqueue(
                &tx,
                bot,
                thread,
                &trigger.trigger(*reason, *priority, *debounce),
                debounce_of(bot),
                now_ms,
            )?,
            (
                Decision::Suppress {
                    bot,
                    why: SuppressWhy::Cap,
                },
                Some(thread),
            ) => {
                let turns = Turns::new(&tx);
                if !turns.cap_reacted(&thread.root_id, &thread.round_id, bot)? {
                    turns.set_cap_reacted(&thread.root_id, &thread.round_id, bot)?;
                    reactions.push(bot.clone());
                }
            }
            (Decision::Wake { bot, .. }, None) => {
                tracing::warn!(bot = %bot, "a wake without a thread; dropping it");
            }
            _ => {}
        }
    }
    tx.commit()?;
    Ok(reactions)
}

/// Stores a rebuilt thread with its events and turn counts (design 6.3, RebuildThread steps 2
/// to 4). `messages` holds each replayed message's author class and `created_at`.
fn commit_rebuild(
    store: &mut Store,
    rows: &[EventRow],
    thread: Option<&mut ThreadState>,
    messages: &[(AuthorClass, i64)],
) -> Result<(), StoreError> {
    let tx = store.connection_mut().transaction()?;
    let events = Events::new(&tx);
    for row in rows {
        events.insert_or_ignore(row)?;
        if let Some(processed_at) = row.processed_at {
            events.mark_processed(&row.id, processed_at)?;
        }
    }
    if let Some(thread) = thread {
        let mut used = Wakes::new(&tx).started_in_round(&thread.root_id, &thread.round_id)?;
        if used.is_empty() {
            for (class, created_at) in messages {
                if let (AuthorClass::Bot(bot), true) =
                    (class, *created_at >= thread.round_started_at)
                {
                    *used.entry(bot.clone()).or_default() += 1;
                }
            }
        }
        thread.turns_used = used;
        Threads::new(&tx).upsert(thread)?;
        let turns = Turns::new(&tx);
        for (bot, count) in &thread.turns_used {
            turns.set_used(&thread.root_id, &thread.round_id, bot, *count)?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Applies a thread update to the thread at `root` (design 6.3, core step 5).
///
/// A thread the router has no state for starts fresh, with its root as its first round, when the
/// update changes anything or the event wakes someone (design 6.4). Otherwise there is nothing to
/// store.
fn updated_thread(
    thread: Option<ThreadState>,
    root: &EventId,
    ev: &InEvent,
    update: &ThreadUpdate,
    has_wake: bool,
) -> Option<ThreadState> {
    let changes = update.create.is_some()
        || !update.add_participants.is_empty()
        || update.set_discussion
        || update.new_round.is_some();
    let mut state = match thread {
        Some(state) => state,
        None if changes || has_wake => ThreadState {
            root_id: root.clone(),
            channel_id: update
                .create
                .as_ref()
                .map_or(ev.channel, |create| create.channel_id),
            participants: BTreeSet::new(),
            discussion: false,
            round_id: root.clone(),
            round_mode: RoundMode::Direct,
            round_started_at: ev.created_at,
            turns_used: BTreeMap::new(),
        },
        None => return None,
    };
    state
        .participants
        .extend(update.add_participants.iter().cloned());
    state.discussion |= update.set_discussion;
    if let Some(round) = &update.new_round {
        state.round_id = round.round_id.clone();
        state.round_mode = round.mode;
        state.round_started_at = round.started_at;
        state.turns_used.clear();
    }
    Some(state)
}

/// The stored class of an author.
fn event_class(class: &AuthorClass) -> EventClass {
    match class {
        AuthorClass::Owner => EventClass::Owner,
        AuthorClass::Bot(_) => EventClass::Bot,
        AuthorClass::ForeignBot { .. } => EventClass::ForeignBot,
        AuthorClass::Human => EventClass::Human,
    }
}

/// The author's display name: the roster bot name, `owner.name`, or the first 12 characters of
/// the npub (design section 7).
fn author_name(pubkey: &Pubkey, class: &AuthorClass, roster: &Roster) -> String {
    match class {
        AuthorClass::Owner => roster.owner.name.clone(),
        AuthorClass::Bot(name) => name.to_string(),
        AuthorClass::ForeignBot { .. } | AuthorClass::Human => pubkey
            .to_nostr()
            .ok()
            .and_then(|key| key.to_bech32().ok())
            .map_or_else(
                || pubkey.as_str().chars().take(12).collect(),
                |npub| npub.chars().take(12).collect(),
            ),
    }
}

/// The `router_core` view of a stored-history event in `channel`.
fn in_event(event: &Event, channel: ChannelId) -> InEvent {
    InEvent {
        id: EventId::from_nostr(&event.id),
        pubkey: Pubkey::from_nostr(&event.pubkey),
        kind: event.kind.as_u16(),
        created_at: i64::try_from(event.created_at.as_secs()).unwrap_or(i64::MAX),
        channel,
        content: event.content.clone(),
        tags: event
            .tags
            .iter()
            .map(|tag| tag.as_slice().to_vec())
            .collect(),
    }
}
