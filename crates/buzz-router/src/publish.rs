//! Outbound events: replies, status notes, reactions and typing (design
//! section 6.8, requirements 30.5, 35.5, 44, 45.2, 46.2, 46.3 and 59.3).
//!
//! Every event is signed with the sending bot's keys. Replies and status
//! notes go out over the bot's WebSocket with one REST fallback (DD-7), and
//! their `posts` row is written before sending so the relay echo is never
//! counted as unmanaged (DD-6); a final failure deletes the row. Replies and
//! status notes for a halted bot are refused before anything is written or
//! sent (R30.5). Reactions are best-effort with the same transports but no
//! `posts` row; typing goes over the WebSocket only and its caller decides
//! what a failure means.

use std::sync::Arc;

use router_core::config::Roster;
use router_core::ids::{BotName, ChannelId, EventId};
use router_core::parse::mentions_for_reply;
use uuid::Uuid;

use crate::relay::rest::RestClient;
use crate::relay::{RelayError, RelayPort};
use crate::store::halts::HaltScope;
use crate::store::posts::PostRow;
use crate::store::Store;

/// What publishing needs: the database (for `posts` and halts), the bot's
/// WebSocket port, the REST fallback, the roster, the bot's keys, and the
/// configured NIP-OA auth tag (added to replies and status notes when set).
pub struct PublishDeps {
    /// The store, for `posts` rows and the halt check.
    pub store: Store,
    /// The bot's WebSocket port (or a fake in tests).
    pub relay: Arc<dyn RelayPort>,
    /// The REST fallback, tried once when the socket publish fails.
    pub rest: RestClient,
    /// The roster, for reply mentions.
    pub roster: Roster,
    /// The sending bot's signing keys.
    pub keys: nostr::Keys,
    /// The NIP-OA auth tag, added as-is when present.
    pub auth_tag: Option<nostr::Tag>,
}

/// A failure to build or publish an outbound event.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// The bot is halted: nothing was written or sent.
    #[error("the bot is halted")]
    Halted,
    /// The event could not be built or neither transport delivered it.
    #[error("cannot publish: {0}")]
    Failed(String),
    /// The database write failed.
    #[error("store error: {0}")]
    Store(String),
}

fn store_failed(error: crate::store::StoreError) -> PublishError {
    PublishError::Store(error.to_string())
}

/// A build failure at `what`: the underlying error renders with `Display`.
fn failed(what: &str, error: impl std::fmt::Display) -> PublishError {
    PublishError::Failed(format!("{what}: {error}"))
}

/// The status-note text: `On it, this will take a bit.`, or
/// `On it, about {eta}.` when an estimate is set (R46.2).
pub fn build_status_text(eta: Option<&str>) -> String {
    match eta {
        Some(eta) => format!("On it, about {eta}."),
        None => "On it, this will take a bit.".to_string(),
    }
}

/// Builds a kind-9 reply: the `h` tag, the direct or nested `e` shape from
/// `buzz_sdk`'s `thread_tags`, `p` tags from [`mentions_for_reply`], the
/// configured auth tag, and the `buzz-router` marker, signed by `keys`.
pub fn build_reply(
    roster: &Roster,
    keys: &nostr::Keys,
    auth_tag: Option<&nostr::Tag>,
    channel: &ChannelId,
    root: &EventId,
    parent: &EventId,
    text: &str,
) -> Result<nostr::Event, PublishError> {
    build_threaded(&Threaded {
        roster,
        keys,
        auth_tag,
        channel,
        root,
        parent,
        text: text.to_string(),
        marker: "reply",
    })
}

/// Builds a kind-9 status note: like [`build_reply`] with the status text and
/// the `status` marker, threaded under the reaction target (R46.2, R46.3).
pub fn build_status_note(
    roster: &Roster,
    keys: &nostr::Keys,
    auth_tag: Option<&nostr::Tag>,
    channel: &ChannelId,
    root: &EventId,
    parent: &EventId,
    eta: Option<&str>,
) -> Result<nostr::Event, PublishError> {
    build_threaded(&Threaded {
        roster,
        keys,
        auth_tag,
        channel,
        root,
        parent,
        text: build_status_text(eta),
        marker: "status",
    })
}

/// The inputs to the shared reply/status-note builder.
struct Threaded<'a> {
    roster: &'a Roster,
    keys: &'a nostr::Keys,
    auth_tag: Option<&'a nostr::Tag>,
    channel: &'a ChannelId,
    root: &'a EventId,
    parent: &'a EventId,
    text: String,
    /// `reply` or `status`.
    marker: &'static str,
}

/// The shared reply/status-note builder.
fn build_threaded(p: &Threaded<'_>) -> Result<nostr::Event, PublishError> {
    let mentions: Vec<String> = mentions_for_reply(&p.text, p.roster)
        .iter()
        .map(|pubkey| pubkey.as_str().to_string())
        .collect();
    let mention_refs: Vec<&str> = mentions.iter().map(String::as_str).collect();
    let thread_ref = buzz_sdk::ThreadRef {
        root_event_id: nostr::EventId::from_hex(p.root.as_str())
            .map_err(|error| PublishError::Failed(format!("bad root id: {error}")))?,
        parent_event_id: nostr::EventId::from_hex(p.parent.as_str())
            .map_err(|error| PublishError::Failed(format!("bad parent id: {error}")))?,
    };
    let mut builder = buzz_sdk::builders::build_message(
        p.channel.uuid(),
        &p.text,
        Some(&thread_ref),
        &mention_refs,
        false,
        &[],
        &[],
    )
    .map_err(|error| PublishError::Failed(format!("build error: {error}")))?;
    if let Some(tag) = p.auth_tag {
        builder = builder.tag(tag.clone());
    }
    let marker_tag = nostr::Tag::parse(["buzz-router", env!("CARGO_PKG_VERSION"), p.marker])
        .map_err(|error| failed("marker tag", error))?;
    builder
        .tag(marker_tag)
        .sign_with_keys(p.keys)
        .map_err(|error| PublishError::Failed(format!("sign error: {error}")))
}

/// Whether `bot` may publish: a halt for every bot or for this bot refuses.
fn ensure_not_halted(deps: &PublishDeps, bot: &BotName) -> Result<(), PublishError> {
    let halts = deps.store.halts().list().map_err(store_failed)?;
    let halted = halts
        .iter()
        .any(|halt| halt.scope == HaltScope::All || halt.scope == HaltScope::Bot(bot.clone()));
    if halted {
        return Err(PublishError::Halted);
    }
    Ok(())
}

/// Sends `event` over the socket, falling back once to REST (DD-7).
/// There is no `posts` row here; callers that need one manage it.
async fn send_event(deps: &PublishDeps, event: &nostr::Event) -> Result<(), PublishError> {
    match deps.relay.publish(event.clone()).await {
        Ok(()) => Ok(()),
        Err(first) => match deps.rest.submit_event(event).await {
            Ok(()) => Ok(()),
            Err(second) => Err(PublishError::Failed(format!(
                "socket publish failed ({first}); REST fallback failed ({second})"
            ))),
        },
    }
}

/// Sends `event` with its `posts` row: the row is written before sending and
/// deleted when neither transport delivers it.
async fn send_tracked(
    deps: &PublishDeps,
    bot: &BotName,
    wake_id: Option<Uuid>,
    event: nostr::Event,
) -> Result<nostr::Event, PublishError> {
    let row = PostRow {
        event_id: EventId::from_nostr(&event.id),
        bot: bot.clone(),
        wake_id,
        created_at: event.created_at.as_secs() as i64,
    };
    deps.store.posts().insert(&row).map_err(store_failed)?;
    match send_event(deps, &event).await {
        Ok(()) => Ok(event),
        Err(error) => {
            let _ = deps.store.posts().delete(&row.event_id);
            Err(error)
        }
    }
}

/// Publishes a reply and records its `posts` row (R44). Refuses halted bots.
pub async fn publish_reply(
    deps: &PublishDeps,
    bot: &BotName,
    channel: &ChannelId,
    root: &EventId,
    parent: &EventId,
    wake_id: Option<Uuid>,
    text: &str,
) -> Result<nostr::Event, PublishError> {
    ensure_not_halted(deps, bot)?;
    let event = build_reply(
        &deps.roster,
        &deps.keys,
        deps.auth_tag.as_ref(),
        channel,
        root,
        parent,
        text,
    )?;
    send_tracked(deps, bot, wake_id, event).await
}

/// Publishes a status note and records its `posts` row (R46.2, R46.3).
/// Refuses halted bots.
pub async fn publish_status_note(
    deps: &PublishDeps,
    bot: &BotName,
    channel: &ChannelId,
    root: &EventId,
    parent: &EventId,
    wake_id: Option<Uuid>,
    eta: Option<&str>,
) -> Result<nostr::Event, PublishError> {
    ensure_not_halted(deps, bot)?;
    let event = build_status_note(
        &deps.roster,
        &deps.keys,
        deps.auth_tag.as_ref(),
        channel,
        root,
        parent,
        eta,
    )?;
    send_tracked(deps, bot, wake_id, event).await
}

/// Publishes a kind-7 reaction on `target`: not recorded, best-effort over
/// both transports (R45.2).
pub async fn publish_reaction(
    deps: &PublishDeps,
    bot: &BotName,
    target: &EventId,
    emoji: &str,
) -> Result<nostr::Event, PublishError> {
    let _ = bot;
    let target_id = nostr::EventId::from_hex(target.as_str())
        .map_err(|error| PublishError::Failed(format!("bad target id: {error}")))?;
    let event = buzz_sdk::builders::build_reaction(target_id, emoji)
        .map_err(|error| PublishError::Failed(format!("build error: {error}")))?
        .sign_with_keys(&deps.keys)
        .map_err(|error| PublishError::Failed(format!("sign error: {error}")))?;
    send_event(deps, &event).await?;
    Ok(event)
}

/// Publishes a kind-20002 typing indicator like `buzz-acp`'s
/// `build_typing_event`: empty content, an `h` tag, and the nested or direct
/// `e` shape. It goes over the WebSocket only, with no `posts` row (R35.5).
pub async fn publish_typing(
    deps: &PublishDeps,
    bot: &BotName,
    channel: &ChannelId,
    root: &EventId,
    parent: &EventId,
) -> Result<nostr::Event, PublishError> {
    let _ = bot;
    let event = build_typing(&deps.keys, channel, root, parent)?;
    deps.relay
        .publish(event.clone())
        .await
        .map_err(|error: RelayError| PublishError::Failed(error.to_string()))?;
    Ok(event)
}

/// Builds a kind-20002 typing indicator like `buzz-acp`'s `build_typing_event`:
/// empty content, an `h` tag, and the nested or direct `e` shape (R35.5).
pub fn build_typing(
    keys: &nostr::Keys,
    channel: &ChannelId,
    root: &EventId,
    parent: &EventId,
) -> Result<nostr::Event, PublishError> {
    let mut tags = vec![nostr::Tag::parse(["h", &channel.uuid().to_string()])
        .map_err(|error| failed("h tag", error))?];
    if root != parent {
        tags.push(
            nostr::Tag::parse(["e", root.as_str(), "", "root"])
                .map_err(|error| failed("root tag", error))?,
        );
    }
    tags.push(
        nostr::Tag::parse(["e", parent.as_str(), "", "reply"])
            .map_err(|error| failed("reply tag", error))?,
    );
    nostr::EventBuilder::new(nostr::Kind::Custom(20002), "")
        .tags(tags)
        .sign_with_keys(keys)
        .map_err(|error| PublishError::Failed(format!("sign error: {error}")))
}
