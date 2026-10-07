//! The `wakes` table: one row per wake, queued to ended (requirement 47.2).

use router_core::ids::{BotName, EventId};
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

use super::{parse_column, StoreError};

/// A wake's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeState {
    /// Waiting to dispatch.
    Queued,
    /// The adapter is running.
    Running,
    /// Ended after posting.
    Posted,
    /// Ended with a pass.
    Passed,
    /// Ended at its deadline.
    Timeout,
    /// Killed by a stop or cancel.
    Killed,
    /// The adapter failed.
    Failed,
    /// Was running when the router stopped.
    Interrupted,
}

impl WakeState {
    /// The stored text, such as `queued`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Posted => "posted",
            Self::Passed => "passed",
            Self::Timeout => "timeout",
            Self::Killed => "killed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "posted" => Ok(Self::Posted),
            "passed" => Ok(Self::Passed),
            "timeout" => Ok(Self::Timeout),
            "killed" => Ok(Self::Killed),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            other => Err(StoreError::Corrupt {
                column: "wakes.state",
                message: format!("unknown wake state {other:?}"),
            }),
        }
    }
}

/// One `wakes` row. Times are unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRow {
    /// The wake id.
    pub id: Uuid,
    /// The bot woken.
    pub bot: BotName,
    /// The thread root.
    pub root_id: EventId,
    /// The round the wake belongs to.
    pub round_id: EventId,
    /// The snake_case wake reason.
    pub reason: String,
    /// `owner`, `human` or `bot`.
    pub priority: String,
    /// The trigger list as a JSON array (design section 6.5).
    pub triggers: String,
    /// The lifecycle state.
    pub state: WakeState,
    /// Hex SHA-256 of the wake token; `None` while queued.
    pub token_hash: Option<String>,
    /// The attempt number, from 1.
    pub attempt: u32,
    /// When the wake was queued.
    pub created_at: i64,
    /// The earliest dispatch time.
    pub dispatch_after: i64,
    /// When the wake started.
    pub started_at: Option<i64>,
    /// When the wake times out.
    pub deadline: Option<i64>,
    /// When the wake ended.
    pub ended_at: Option<i64>,
    /// The outcome JSON.
    pub outcome: Option<String>,
}

const COLUMNS: &str = "id, bot, root_id, round_id, reason, priority, triggers, state, token_hash, \
     attempt, created_at, dispatch_after, started_at, deadline, ended_at, outcome";

/// The `wakes` repository.
pub struct Wakes<'c> {
    conn: &'c Connection,
}

impl<'c> Wakes<'c> {
    /// A repository over `conn`.
    pub fn new(conn: &'c Connection) -> Self {
        Self { conn }
    }

    /// Inserts a new wake.
    pub fn insert(&self, row: &WakeRow) -> Result<(), StoreError> {
        self.conn.execute(
            &format!(
                "INSERT INTO wakes ({COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
            ),
            params![
                row.id.to_string(),
                row.bot.as_str(),
                row.root_id.as_str(),
                row.round_id.as_str(),
                row.reason,
                row.priority,
                row.triggers,
                row.state.as_str(),
                row.token_hash,
                row.attempt,
                row.created_at,
                row.dispatch_after,
                row.started_at,
                row.deadline,
                row.ended_at,
                row.outcome,
            ],
        )?;
        Ok(())
    }

    /// Sets the state alone.
    pub fn set_state(&self, id: &Uuid, state: WakeState) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE wakes SET state = ?2 WHERE id = ?1",
            params![id.to_string(), state.as_str()],
        )?;
        Ok(())
    }

    /// Marks the wake running with its token hash, start time and deadline.
    pub fn start(
        &self,
        id: &Uuid,
        token_hash: &str,
        started_at: i64,
        deadline: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE wakes SET state = ?2, token_hash = ?3, started_at = ?4, deadline = ?5 WHERE id = ?1",
            params![
                id.to_string(),
                WakeState::Running.as_str(),
                token_hash,
                started_at,
                deadline
            ],
        )?;
        Ok(())
    }

    /// Ends the wake with `state`, its end time and outcome JSON.
    pub fn finish(
        &self,
        id: &Uuid,
        state: WakeState,
        ended_at: i64,
        outcome: Option<&str>,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE wakes SET state = ?2, ended_at = ?3, outcome = ?4 WHERE id = ?1",
            params![id.to_string(), state.as_str(), ended_at, outcome],
        )?;
        Ok(())
    }

    /// The wake with `id`, if any.
    pub fn get(&self, id: &Uuid) -> Result<Option<WakeRow>, StoreError> {
        self.one("id = ?1", params![id.to_string()])
    }

    /// The wake whose token hashes to `token_hash`, if any.
    pub fn find_by_token_hash(&self, token_hash: &str) -> Result<Option<WakeRow>, StoreError> {
        self.one("token_hash = ?1", params![token_hash])
    }

    /// The queued wake for (`bot`, `root_id`), if any.
    pub fn find_queued(
        &self,
        bot: &BotName,
        root_id: &EventId,
    ) -> Result<Option<WakeRow>, StoreError> {
        self.one(
            "bot = ?1 AND root_id = ?2 AND state = 'queued' ORDER BY created_at LIMIT 1",
            params![bot.as_str(), root_id.as_str()],
        )
    }

    /// Replaces a queued wake's attributes and triggers after a trigger is appended
    /// (design section 6.5).
    pub fn update_queued(
        &self,
        id: &Uuid,
        reason: &str,
        priority: &str,
        triggers: &str,
        dispatch_after: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE wakes SET reason = ?2, priority = ?3, triggers = ?4, dispatch_after = ?5 \
             WHERE id = ?1",
            params![id.to_string(), reason, priority, triggers, dispatch_after],
        )?;
        Ok(())
    }

    /// Moves the wake to `round_id`, the thread's current round at dispatch (design section 6.6).
    pub fn set_round(&self, id: &Uuid, round_id: &EventId) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE wakes SET round_id = ?2 WHERE id = ?1",
            params![id.to_string(), round_id.as_str()],
        )?;
        Ok(())
    }

    /// Every queued wake, oldest first.
    pub fn queued(&self) -> Result<Vec<WakeRow>, StoreError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM wakes WHERE state = 'queued' ORDER BY created_at, id"
        ))?;
        let raw = statement
            .query_map([], RawWake::from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        raw.into_iter().map(RawWake::into_row).collect()
    }

    /// Every wake in `state`, oldest first.
    pub fn with_state(&self, state: WakeState) -> Result<Vec<WakeRow>, StoreError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM wakes WHERE state = ?1 ORDER BY created_at, id"
        ))?;
        let raw = statement
            .query_map([state.as_str()], RawWake::from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        raw.into_iter().map(RawWake::into_row).collect()
    }

    /// The earliest `dispatch_after` of a queued wake that is later than `after_ms`.
    pub fn next_dispatch_after(&self, after_ms: i64) -> Result<Option<i64>, StoreError> {
        Ok(self.conn.query_row(
            "SELECT MIN(dispatch_after) FROM wakes WHERE state = 'queued' AND dispatch_after > ?1",
            params![after_ms],
            |row| row.get(0),
        )?)
    }

    /// Dispatched wakes per bot in one round of a thread (design section 6.3, rebuild step 3).
    pub fn started_in_round(
        &self,
        root_id: &EventId,
        round_id: &EventId,
    ) -> Result<std::collections::BTreeMap<BotName, u32>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT bot, COUNT(*) FROM wakes \
             WHERE root_id = ?1 AND round_id = ?2 AND started_at IS NOT NULL GROUP BY bot",
        )?;
        let rows = statement.query_map(params![root_id.as_str(), round_id.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;
        let mut counts = std::collections::BTreeMap::new();
        for row in rows {
            let (bot, count) = row?;
            counts.insert(parse_column("wakes.bot", BotName::new(bot))?, count);
        }
        Ok(counts)
    }

    /// The latest `started_at` of `bot`'s wakes in the thread at `root_id`, other than `except`
    /// (DD-17).
    pub fn previous_start(
        &self,
        bot: &BotName,
        root_id: &EventId,
        except: &Uuid,
    ) -> Result<Option<i64>, StoreError> {
        Ok(self.conn.query_row(
            "SELECT MAX(started_at) FROM wakes WHERE bot = ?1 AND root_id = ?2 AND id != ?3",
            params![bot.as_str(), root_id.as_str(), except.to_string()],
            |row| row.get(0),
        )?)
    }

    /// Wakes for `bot` started at or after `since_ms` (design section 9.3, assumption A10).
    pub fn count_started_since(&self, bot: &BotName, since_ms: i64) -> Result<u32, StoreError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM wakes WHERE bot = ?1 AND started_at >= ?2",
            params![bot.as_str(), since_ms],
            |row| row.get(0),
        )?)
    }

    fn one(
        &self,
        filter: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Option<WakeRow>, StoreError> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM wakes WHERE {filter}"),
                args,
                RawWake::from_row,
            )
            .optional()?;
        raw.map(RawWake::into_row).transpose()
    }
}

struct RawWake {
    id: String,
    bot: String,
    root_id: String,
    round_id: String,
    reason: String,
    priority: String,
    triggers: String,
    state: String,
    token_hash: Option<String>,
    attempt: u32,
    created_at: i64,
    dispatch_after: i64,
    started_at: Option<i64>,
    deadline: Option<i64>,
    ended_at: Option<i64>,
    outcome: Option<String>,
}

impl RawWake {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            bot: row.get(1)?,
            root_id: row.get(2)?,
            round_id: row.get(3)?,
            reason: row.get(4)?,
            priority: row.get(5)?,
            triggers: row.get(6)?,
            state: row.get(7)?,
            token_hash: row.get(8)?,
            attempt: row.get(9)?,
            created_at: row.get(10)?,
            dispatch_after: row.get(11)?,
            started_at: row.get(12)?,
            deadline: row.get(13)?,
            ended_at: row.get(14)?,
            outcome: row.get(15)?,
        })
    }

    fn into_row(self) -> Result<WakeRow, StoreError> {
        Ok(WakeRow {
            id: parse_column("wakes.id", Uuid::parse_str(&self.id))?,
            bot: parse_column("wakes.bot", BotName::new(self.bot))?,
            root_id: parse_column("wakes.root_id", EventId::from_hex(&self.root_id))?,
            round_id: parse_column("wakes.round_id", EventId::from_hex(&self.round_id))?,
            reason: self.reason,
            priority: self.priority,
            triggers: self.triggers,
            state: WakeState::parse(&self.state)?,
            token_hash: self.token_hash,
            attempt: self.attempt,
            created_at: self.created_at,
            dispatch_after: self.dispatch_after,
            started_at: self.started_at,
            deadline: self.deadline,
            ended_at: self.ended_at,
            outcome: self.outcome,
        })
    }
}
