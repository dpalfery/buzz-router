//! The SQLite store `state.sqlite3` (design section 9, requirement 47).
//!
//! [`Store`] owns one connection and runs migration 1 on open. Each table has a repository type
//! in its own module, borrowed from the store ([`Store::events`] and so on) or built over any
//! [`Connection`], so the same repositories work inside a [`rusqlite::Transaction`].

pub mod cursors;
pub mod events;
pub mod halts;
pub mod posts;
mod schema;
pub mod threads;
pub mod wakes;

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use cursors::Cursors;
use events::Events;
use halts::Halts;
use posts::Posts;
use threads::{Threads, Turns};
use wakes::Wakes;

/// The database file name in the data directory (design section 9.1).
pub const FILE_NAME: &str = "state.sqlite3";

/// A store failure.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite reported an error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A stored value could not be read back as its type.
    #[error("corrupt value in column {column}: {message}")]
    Corrupt {
        /// The column holding the bad value.
        column: &'static str,
        /// What was wrong with it.
        message: String,
    },
    /// The database has a schema version newer than this build knows.
    #[error("database schema version {0} is newer than this build supports")]
    UnknownVersion(i64),
}

/// The router's SQLite database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating if needed) the database at `path` for reading and writing, sets the
    /// connection pragmas and runs any pending migration.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::setup(Connection::open(path)?)
    }

    /// Opens an existing database read-only, as the ingest task does (design section 9.1).
    pub fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        Ok(Self { conn })
    }

    /// Opens a migrated in-memory database (tests and replay).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(mut conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        schema::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// The schema version from `PRAGMA user_version`.
    pub fn user_version(&self) -> Result<i64, StoreError> {
        schema::user_version(&self.conn)
    }

    /// The underlying connection, for transactions and ad-hoc queries.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// The underlying connection, mutably, for starting a transaction.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// The `events` repository.
    pub fn events(&self) -> Events<'_> {
        Events::new(&self.conn)
    }

    /// The `threads` repository.
    pub fn threads(&self) -> Threads<'_> {
        Threads::new(&self.conn)
    }

    /// The `turns` repository.
    pub fn turns(&self) -> Turns<'_> {
        Turns::new(&self.conn)
    }

    /// The `wakes` repository.
    pub fn wakes(&self) -> Wakes<'_> {
        Wakes::new(&self.conn)
    }

    /// The `posts` repository.
    pub fn posts(&self) -> Posts<'_> {
        Posts::new(&self.conn)
    }

    /// The `halts` repository.
    pub fn halts(&self) -> Halts<'_> {
        Halts::new(&self.conn)
    }

    /// The `cursors` repository.
    pub fn cursors(&self) -> Cursors<'_> {
        Cursors::new(&self.conn)
    }
}

/// Parses a stored identifier, reporting a failure as [`StoreError::Corrupt`].
pub(crate) fn parse_column<T, E: std::fmt::Display>(
    column: &'static str,
    parsed: Result<T, E>,
) -> Result<T, StoreError> {
    parsed.map_err(|error| StoreError::Corrupt {
        column,
        message: error.to_string(),
    })
}
