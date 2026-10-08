//! The `halts` table: active stops, for all bots or one (requirement 30.2).

use router_core::ids::BotName;
use rusqlite::{params, Connection};

use super::{parse_column, StoreError};

/// What a halt covers. Stored as `'all'` or the bot name; configuration rejects a bot named
/// `all`, so the two cannot collide.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum HaltScope {
    /// Every bot.
    All,
    /// One bot.
    Bot(BotName),
}

impl HaltScope {
    fn as_str(&self) -> &str {
        match self {
            Self::All => "all",
            Self::Bot(bot) => bot.as_str(),
        }
    }

    fn parse(text: String) -> Result<Self, StoreError> {
        if text == "all" {
            Ok(Self::All)
        } else {
            Ok(Self::Bot(parse_column("halts.scope", BotName::new(text))?))
        }
    }
}

/// One `halts` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HaltRow {
    /// What is halted.
    pub scope: HaltScope,
    /// The control event id, `cli` or `admin-api`.
    pub set_by_event: Option<String>,
    /// When the halt was set, in unix milliseconds.
    pub set_at: i64,
}

/// The `halts` repository.
pub struct Halts<'c> {
    conn: &'c Connection,
}

impl<'c> Halts<'c> {
    /// A repository over `conn`.
    pub fn new(conn: &'c Connection) -> Self {
        Self { conn }
    }

    /// Sets (or replaces) the halt for `scope`.
    pub fn set(
        &self,
        scope: &HaltScope,
        set_by_event: Option<&str>,
        set_at: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO halts (scope, set_by_event, set_at) VALUES (?1, ?2, ?3)",
            params![scope.as_str(), set_by_event, set_at],
        )?;
        Ok(())
    }

    /// Clears the halt for `scope`, if set.
    pub fn clear(&self, scope: &HaltScope) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM halts WHERE scope = ?1", [scope.as_str()])?;
        Ok(())
    }

    /// Every active halt, `all` first, then bots by name.
    pub fn list(&self) -> Result<Vec<HaltRow>, StoreError> {
        let mut statement = self
            .conn
            .prepare("SELECT scope, set_by_event, set_at FROM halts")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut halts = Vec::new();
        for row in rows {
            let (scope, set_by_event, set_at) = row?;
            halts.push(HaltRow {
                scope: HaltScope::parse(scope)?,
                set_by_event,
                set_at,
            });
        }
        halts.sort_by(|a, b| a.scope.cmp(&b.scope));
        Ok(halts)
    }
}
