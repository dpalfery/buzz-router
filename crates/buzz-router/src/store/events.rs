//! The `events` table: every event seen, and whether it has been routed (requirement 47.3).

use router_core::ids::{ChannelId, EventId, Pubkey};
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::{parse_column, StoreError};

/// The author class stored with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventClass {
    /// One of `owner.pubkeys`.
    Owner,
    /// A roster bot.
    Bot,
    /// A bot outside the roster with a NIP-OA `auth` tag.
    ForeignBot,
    /// Anyone else.
    Human,
}

impl EventClass {
    /// The stored text: `owner`, `bot`, `foreign_bot` or `human`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Bot => "bot",
            Self::ForeignBot => "foreign_bot",
            Self::Human => "human",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "owner" => Ok(Self::Owner),
            "bot" => Ok(Self::Bot),
            "foreign_bot" => Ok(Self::ForeignBot),
            "human" => Ok(Self::Human),
            other => Err(StoreError::Corrupt {
                column: "events.class",
                message: format!("unknown class {other:?}"),
            }),
        }
    }
}

/// One `events` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    /// The event id.
    pub id: EventId,
    /// The channel (`h` tag).
    pub channel_id: ChannelId,
    /// The thread root; equal to `id` for a top-level event.
    pub root_id: EventId,
    /// The author's pubkey.
    pub author: Pubkey,
    /// The author's class.
    pub class: EventClass,
    /// The event kind (9 or 40003).
    pub kind: u16,
    /// The event's `created_at`, in unix seconds.
    pub created_at: i64,
    /// When the event was routed, in unix milliseconds; `None` until then.
    pub processed_at: Option<i64>,
}

const COLUMNS: &str = "id, channel_id, root_id, author, class, kind, created_at, processed_at";

/// The `events` repository.
pub struct Events<'c> {
    conn: &'c Connection,
}

impl<'c> Events<'c> {
    /// A repository over `conn`.
    pub fn new(conn: &'c Connection) -> Self {
        Self { conn }
    }

    /// Inserts `row` unless an event with its id is already stored. Returns whether it was new.
    pub fn insert_or_ignore(&self, row: &EventRow) -> Result<bool, StoreError> {
        let inserted = self.conn.execute(
            &format!(
                "INSERT OR IGNORE INTO events ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            ),
            params![
                row.id.as_str(),
                row.channel_id.to_string(),
                row.root_id.as_str(),
                row.author.as_str(),
                row.class.as_str(),
                row.kind,
                row.created_at,
                row.processed_at,
            ],
        )?;
        Ok(inserted == 1)
    }

    /// Records that the event was routed at `at_ms` (unix milliseconds).
    pub fn mark_processed(&self, id: &EventId, at_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE events SET processed_at = ?2 WHERE id = ?1",
            params![id.as_str(), at_ms],
        )?;
        Ok(())
    }

    /// Whether the event is stored and has been routed.
    pub fn is_processed(&self, id: &EventId) -> Result<bool, StoreError> {
        let processed: Option<Option<i64>> = self
            .conn
            .query_row(
                "SELECT processed_at FROM events WHERE id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(matches!(processed, Some(Some(_))))
    }

    /// The stored row for `id`, if any.
    pub fn get(&self, id: &EventId) -> Result<Option<EventRow>, StoreError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM events WHERE id = ?1"),
                [id.as_str()],
                RawEvent::from_row,
            )
            .optional()?;
        raw.map(RawEvent::into_row).transpose()
    }
}

struct RawEvent {
    id: String,
    channel_id: String,
    root_id: String,
    author: String,
    class: String,
    kind: u16,
    created_at: i64,
    processed_at: Option<i64>,
}

impl RawEvent {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            channel_id: row.get(1)?,
            root_id: row.get(2)?,
            author: row.get(3)?,
            class: row.get(4)?,
            kind: row.get(5)?,
            created_at: row.get(6)?,
            processed_at: row.get(7)?,
        })
    }

    fn into_row(self) -> Result<EventRow, StoreError> {
        Ok(EventRow {
            id: parse_column("events.id", EventId::from_hex(&self.id))?,
            channel_id: parse_column("events.channel_id", ChannelId::parse(&self.channel_id))?,
            root_id: parse_column("events.root_id", EventId::from_hex(&self.root_id))?,
            author: parse_column("events.author", Pubkey::from_hex(&self.author))?,
            class: EventClass::parse(&self.class)?,
            kind: self.kind,
            created_at: self.created_at,
            processed_at: self.processed_at,
        })
    }
}
