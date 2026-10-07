//! The ingest pipeline (design section 6.3, steps 1 to 7, and DD-13).
//!
//! One [`Ingest`] runs as a single sequential task, so events keep their arrival order. For each
//! raw event a relay connection hands it, [`Ingest::handle`]:
//!
//! 1. drops it unless its signature verifies (R4.1);
//! 2. ignores it unless it is kind 9 or 40003 with an `h` tag holding a channel UUID (R61.3);
//! 3. skips it when it was already forwarded or is already processed in `events` (R47.3, R61.4);
//! 4. resolves its thread: a message's from its NIP-10 tags, an edit's from the edited message,
//!    looked up in `events` or the forwarded map, else fetched from the relay (R16.7);
//! 5. for a reply under a root it has not seen, fetches the thread and emits
//!    [`IngestOutput::RebuildThread`] with only the events ordered before this one (R18.2, DD-13);
//! 6. resolves the reply parent's author when the parent is not the root;
//! 7. emits the [`EnrichedEvent`].
//!
//! Relay lookups use the receiving bot's [`RelayPort`]. A failed lookup is logged and the event is
//! still forwarded with what is known.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use buzz_sdk::builders::extract_channel_id;
use chrono::{DateTime, Utc};
use nostr::{Event, Filter, Kind};
use router_core::ids::{BotName, ChannelId, EventId, Pubkey};
use router_core::route::{EditTarget, InEvent, KIND_EDIT, KIND_MESSAGE};
use router_core::thread::{thread_position, ThreadPos};

use crate::clock::Clock;
use crate::relay::RelayPort;
use crate::store::Store;

/// How many forwarded event ids the dedupe map remembers (design 6.3, step 3).
const FORWARDED_CAPACITY: usize = 100_000;

/// Where an event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The live subscription.
    Live,
    /// Backfill after a (re)connect.
    Backfill,
}

/// An event that passed ingest, with everything the core needs to route it.
#[derive(Debug, Clone)]
pub struct EnrichedEvent {
    /// The local bot whose connection received the event.
    pub bot: BotName,
    /// Live or backfill.
    pub source: Source,
    /// The signed event.
    pub event: Event,
    /// The event as `router_core` sees it.
    pub in_event: InEvent,
    /// The event's own position from its NIP-10 tags. An edit is always `TopLevel`; its thread is
    /// the edited message's, in `root`.
    pub pos: ThreadPos,
    /// The thread root: the event itself when top-level, the edited message's root for an edit,
    /// and `None` for an edit whose target could not be found.
    pub root: Option<EventId>,
    /// The reply parent's author, when the parent is not the root and could be found.
    pub parent_author: Option<Pubkey>,
    /// The edited message, for an edit whose target was found.
    pub edit_target: Option<EditTarget>,
    /// When ingest forwarded the event.
    pub received_at: DateTime<Utc>,
}

/// What ingest hands the core.
#[derive(Debug, Clone)]
pub enum IngestOutput {
    /// Route this event.
    Enriched(Box<EnrichedEvent>),
    /// Rebuild the thread at `root` from `events`, in `(created_at, id)` order, before routing
    /// the next enriched event (design 6.3, RebuildThread handling).
    RebuildThread {
        /// The thread root.
        root: EventId,
        /// The thread's kind-9 events ordered before the current one.
        events: Vec<Event>,
    },
}

/// What ingest remembers about an event it forwarded.
#[derive(Debug, Clone)]
struct Known {
    root: EventId,
    author: Pubkey,
}

/// The forwarded map: event ids ingest has forwarded, with each one's root and author. It holds
/// at most [`FORWARDED_CAPACITY`] entries and evicts the oldest first.
#[derive(Debug, Default)]
struct Forwarded {
    entries: HashMap<EventId, Known>,
    order: VecDeque<EventId>,
}

impl Forwarded {
    fn get(&self, id: &EventId) -> Option<&Known> {
        self.entries.get(id)
    }

    fn contains(&self, id: &EventId) -> bool {
        self.entries.contains_key(id)
    }

    fn insert(&mut self, id: EventId, known: Known) {
        if self.entries.insert(id.clone(), known).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > FORWARDED_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

/// The ingest stage: verifies, filters, dedupes and enriches raw events.
pub struct Ingest {
    store: Store,
    relays: BTreeMap<BotName, Arc<dyn RelayPort>>,
    clock: Arc<dyn Clock>,
    forwarded: Forwarded,
}

impl Ingest {
    /// An ingest stage reading `store` (a read-only connection in the daemon) and querying each
    /// bot's relay through `relays`.
    pub fn new(
        store: Store,
        relays: BTreeMap<BotName, Arc<dyn RelayPort>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            store,
            relays,
            clock,
            forwarded: Forwarded::default(),
        }
    }

    /// Runs one raw event received by `bot` through ingest steps 1 to 7 and returns what to send
    /// the core, in order. Dropped, ignored and duplicate events give nothing.
    pub async fn handle(
        &mut self,
        bot: &BotName,
        event: Event,
        source: Source,
    ) -> Vec<IngestOutput> {
        if event.verify().is_err() {
            tracing::debug!(event_id = %event.id, "dropping an event with a bad signature");
            return Vec::new();
        }
        let kind = event.kind.as_u16();
        if kind != KIND_MESSAGE && kind != KIND_EDIT {
            return Vec::new();
        }
        let Some(channel) = extract_channel_id(&event) else {
            return Vec::new();
        };
        let id = EventId::from_nostr(&event.id);
        if self.forwarded.contains(&id) || self.is_processed(&id) {
            return Vec::new();
        }
        let in_event = in_event(&event, ChannelId::from(channel));

        let mut out = Vec::new();
        let mut parent_author = None;
        let mut edit_target = None;
        let (pos, root) = if kind == KIND_MESSAGE {
            let pos = thread_position(&in_event.tags);
            let root = match &pos {
                ThreadPos::TopLevel => id.clone(),
                ThreadPos::Reply { root, parent } => {
                    if !self.root_known(root) {
                        if let Some(rebuild) = self.fetch_thread(bot, root, &event).await {
                            out.push(rebuild);
                        }
                    }
                    if parent != root {
                        parent_author = self.author_of(bot, parent).await;
                    }
                    root.clone()
                }
            };
            (pos, Some(root))
        } else {
            let mut root = None;
            if let Some(target) = edit_target_id(&in_event.tags) {
                root = self.root_of(bot, &target).await;
                if root.is_some() {
                    edit_target = Some(EditTarget { message_id: target });
                }
            }
            (ThreadPos::TopLevel, root)
        };

        self.forwarded.insert(
            id.clone(),
            Known {
                root: root.clone().unwrap_or(id),
                author: in_event.pubkey.clone(),
            },
        );
        out.push(IngestOutput::Enriched(Box::new(EnrichedEvent {
            bot: bot.clone(),
            source,
            event,
            in_event,
            pos,
            root,
            parent_author,
            edit_target,
            received_at: self.clock.now(),
        })));
        out
    }

    /// Whether `events` records `id` as processed. A store error counts as not processed; the
    /// core's own idempotency check catches a duplicate.
    fn is_processed(&self, id: &EventId) -> bool {
        self.store.events().is_processed(id).unwrap_or_else(|error| {
            tracing::warn!(%error, event_id = %id, "cannot read events; treating as unprocessed");
            false
        })
    }

    /// Whether the thread at `root` is known: forwarded, or in the `threads` table.
    fn root_known(&self, root: &EventId) -> bool {
        if self.forwarded.contains(root) {
            return true;
        }
        self.store.threads().exists(root).unwrap_or_else(|error| {
            tracing::warn!(%error, root = %root, "cannot read threads; treating the root as unknown");
            false
        })
    }

    /// The thread root of the message `target`: from the forwarded map, then `events`, then the
    /// relay (design 6.3, step 4).
    async fn root_of(&mut self, bot: &BotName, target: &EventId) -> Option<EventId> {
        if let Some(known) = self.forwarded.get(target) {
            return Some(known.root.clone());
        }
        match self.store.events().get(target) {
            Ok(Some(row)) => return Some(row.root_id),
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, event_id = %target, "cannot read events"),
        }
        let event = self.fetch_one(bot, target).await?;
        Some(match thread_position(&tags_of(&event)) {
            ThreadPos::TopLevel => target.clone(),
            ThreadPos::Reply { root, .. } => root,
        })
    }

    /// The author of the message `id`: from the forwarded map, then `events`, then the relay
    /// (design 6.3, step 6).
    async fn author_of(&mut self, bot: &BotName, id: &EventId) -> Option<Pubkey> {
        if let Some(known) = self.forwarded.get(id) {
            return Some(known.author.clone());
        }
        match self.store.events().get(id) {
            Ok(Some(row)) => return Some(row.author),
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, event_id = %id, "cannot read events"),
        }
        let event = self.fetch_one(bot, id).await?;
        Some(Pubkey::from_nostr(&event.pubkey))
    }

    /// Fetches the event `id` from `bot`'s relay with `{ids:[id]}`. Only a verified event with
    /// that id counts.
    async fn fetch_one(&mut self, bot: &BotName, id: &EventId) -> Option<Event> {
        let relay = self.relays.get(bot)?.clone();
        let nostr_id = nostr::EventId::from_hex(id.as_str()).ok()?;
        match relay.query(vec![Filter::new().id(nostr_id)]).await {
            Ok(events) => events
                .into_iter()
                .find(|event| event.id == nostr_id && event.verify().is_ok()),
            Err(error) => {
                tracing::warn!(%error, event_id = %id, bot = %bot, "cannot fetch an event");
                None
            }
        }
    }

    /// Fetches the thread at `root` from `bot`'s relay, as `buzz-acp`'s thread fetch does:
    /// `{ids:[root]}` plus `{kinds:[9], "#e":[root]}`. Returns the rebuild of the verified kind-9
    /// events ordered strictly before `current` by `(created_at, id)`, and remembers them as
    /// forwarded. A failed fetch is logged and gives `None`, so the event is routed against a
    /// fresh state (design 6.4).
    async fn fetch_thread(
        &mut self,
        bot: &BotName,
        root: &EventId,
        current: &Event,
    ) -> Option<IngestOutput> {
        let relay = self.relays.get(bot)?.clone();
        let root_id = nostr::EventId::from_hex(root.as_str()).ok()?;
        let filters = vec![
            Filter::new().id(root_id),
            Filter::new()
                .kind(Kind::Custom(KIND_MESSAGE))
                .event(root_id),
        ];
        let fetched = match relay.query(filters).await {
            Ok(events) => events,
            Err(error) => {
                tracing::warn!(%error, root = %root, bot = %bot, "thread fetch failed; routing against a fresh thread");
                return None;
            }
        };
        let current_key = order_key(current);
        let mut events: Vec<Event> = fetched
            .into_iter()
            .filter(|event| {
                event.kind.as_u16() == KIND_MESSAGE
                    && order_key(event) < current_key
                    && event.verify().is_ok()
            })
            .collect();
        events.sort_by_key(order_key);
        events.dedup_by_key(|event| event.id);
        if events.is_empty() {
            return None;
        }
        for event in &events {
            self.forwarded.insert(
                EventId::from_nostr(&event.id),
                Known {
                    root: root.clone(),
                    author: Pubkey::from_nostr(&event.pubkey),
                },
            );
        }
        Some(IngestOutput::RebuildThread {
            root: root.clone(),
            events,
        })
    }
}

/// The `(created_at, id)` order key of an event.
fn order_key(event: &Event) -> (u64, String) {
    (event.created_at.as_secs(), event.id.to_hex())
}

/// The raw tag arrays of an event.
fn tags_of(event: &Event) -> Vec<Vec<String>> {
    event
        .tags
        .iter()
        .map(|tag| tag.as_slice().to_vec())
        .collect()
}

/// The `router_core` view of a signature-verified event in `channel`.
fn in_event(event: &Event, channel: ChannelId) -> InEvent {
    InEvent {
        id: EventId::from_nostr(&event.id),
        pubkey: Pubkey::from_nostr(&event.pubkey),
        kind: event.kind.as_u16(),
        created_at: i64::try_from(event.created_at.as_secs()).unwrap_or(i64::MAX),
        channel,
        content: event.content.clone(),
        tags: tags_of(event),
    }
}

/// The message an edit edits: the unmarked `["e", <id>]` tag that
/// `buzz_sdk::builders::build_edit` writes (design 6.3, step 4).
fn edit_target_id(tags: &[Vec<String>]) -> Option<EventId> {
    tags.iter().find_map(|tag| match tag.as_slice() {
        [name, id] if name == "e" => EventId::from_hex(id).ok(),
        _ => None,
    })
}
