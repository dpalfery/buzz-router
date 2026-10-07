//! The `posts` table: events the router published, written before sending (requirement 47.2).

use router_core::ids::{BotName, EventId};
use rusqlite::{params, Connection};
use uuid::Uuid;

use super::StoreError;

/// One `posts` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostRow {
    /// The published event's id.
    pub event_id: EventId,
    /// The bot that signed it.
    pub bot: BotName,
    /// The wake it was posted for, if any.
    pub wake_id: Option<Uuid>,
    /// The event's `created_at`, in unix seconds.
    pub created_at: i64,
}

/// The `posts` repository.
pub struct Posts<'c> {
    conn: &'c Connection,
}

impl<'c> Posts<'c> {
    /// A repository over `conn`.
    pub fn new(conn: &'c Connection) -> Self {
        Self { conn }
    }

    /// Inserts a post.
    pub fn insert(&self, row: &PostRow) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO posts (event_id, bot, wake_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                row.event_id.as_str(),
                row.bot.as_str(),
                row.wake_id.map(|id| id.to_string()),
                row.created_at,
            ],
        )?;
        Ok(())
    }

    /// Whether the router published `event_id`.
    pub fn exists(&self, event_id: &EventId) -> Result<bool, StoreError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM posts WHERE event_id = ?1)",
            [event_id.as_str()],
            |row| row.get(0),
        )?)
    }

    /// Deletes the post, as after a failed publish.
    pub fn delete(&self, event_id: &EventId) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM posts WHERE event_id = ?1", [event_id.as_str()])?;
        Ok(())
    }

    /// Posts recorded for a wake (design section 9.3; recovery only).
    pub fn count_for_wake(&self, wake_id: &Uuid) -> Result<u32, StoreError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM posts WHERE wake_id = ?1",
            [wake_id.to_string()],
            |row| row.get(0),
        )?)
    }
}
