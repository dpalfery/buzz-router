//! The `cursors` table: per bot and relay, the newest `created_at` seen (requirement 47.4).

use router_core::ids::BotName;
use rusqlite::{params, Connection, OptionalExtension};

use super::StoreError;

/// The `cursors` repository.
pub struct Cursors<'c> {
    conn: &'c Connection,
}

impl<'c> Cursors<'c> {
    /// A repository over `conn`.
    pub fn new(conn: &'c Connection) -> Self {
        Self { conn }
    }

    /// The cursor for (`bot`, `relay_url`) in unix seconds, if one is stored.
    pub fn get(&self, bot: &BotName, relay_url: &str) -> Result<Option<i64>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT last_created_at FROM cursors WHERE bot = ?1 AND relay_url = ?2",
                params![bot.as_str(), relay_url],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Moves the cursor to `created_at`, unless it is already later. It never moves backwards.
    pub fn advance(
        &self,
        bot: &BotName,
        relay_url: &str,
        created_at: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO cursors (bot, relay_url, last_created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(bot, relay_url)
             DO UPDATE SET last_created_at = MAX(last_created_at, excluded.last_created_at)",
            params![bot.as_str(), relay_url, created_at],
        )?;
        Ok(())
    }
}
