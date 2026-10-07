//! The `threads` and `turns` tables (requirements 18.5 and 34.3).
//!
//! A thread row holds everything in [`ThreadState`] except `turns_used`, which lives in `turns`
//! keyed by round. [`Threads::load`] fills it from the current round's `turns` rows.

use std::collections::{BTreeMap, BTreeSet};

use router_core::ids::{BotName, ChannelId, EventId};
use router_core::thread::{RoundMode, ThreadState};
use rusqlite::{params, Connection, OptionalExtension};

use super::{parse_column, StoreError};

fn round_mode_str(mode: RoundMode) -> &'static str {
    match mode {
        RoundMode::Direct => "direct",
        RoundMode::Discussion => "discussion",
    }
}

fn parse_round_mode(text: &str) -> Result<RoundMode, StoreError> {
    match text {
        "direct" => Ok(RoundMode::Direct),
        "discussion" => Ok(RoundMode::Discussion),
        other => Err(StoreError::Corrupt {
            column: "threads.round_mode",
            message: format!("unknown round mode {other:?}"),
        }),
    }
}

/// The `threads` repository.
pub struct Threads<'c> {
    conn: &'c Connection,
}

impl<'c> Threads<'c> {
    /// A repository over `conn`.
    pub fn new(conn: &'c Connection) -> Self {
        Self { conn }
    }

    /// Inserts or replaces the thread row for `state.root_id`. `turns_used` is not written; use
    /// [`Turns`] for that.
    pub fn upsert(&self, state: &ThreadState) -> Result<(), StoreError> {
        let participants: Vec<&str> = state.participants.iter().map(BotName::as_str).collect();
        let participants =
            parse_column("threads.participants", serde_json::to_string(&participants))?;
        self.conn.execute(
            "INSERT INTO threads (root_id, channel_id, participants, discussion, round_id, round_mode, round_started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(root_id) DO UPDATE SET
               channel_id = excluded.channel_id,
               participants = excluded.participants,
               discussion = excluded.discussion,
               round_id = excluded.round_id,
               round_mode = excluded.round_mode,
               round_started_at = excluded.round_started_at",
            params![
                state.root_id.as_str(),
                state.channel_id.to_string(),
                participants,
                state.discussion,
                state.round_id.as_str(),
                round_mode_str(state.round_mode),
                state.round_started_at,
            ],
        )?;
        Ok(())
    }

    /// Whether a thread row exists for `root_id`.
    pub fn exists(&self, root_id: &EventId) -> Result<bool, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM threads WHERE root_id = ?1",
                [root_id.as_str()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The thread rooted at `root_id`, with `turns_used` from its current round.
    pub fn load(&self, root_id: &EventId) -> Result<Option<ThreadState>, StoreError> {
        let raw = self
            .conn
            .query_row(
                "SELECT channel_id, participants, discussion, round_id, round_mode, round_started_at
                 FROM threads WHERE root_id = ?1",
                [root_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((channel_id, participants, discussion, round_id, round_mode, round_started_at)) =
            raw
        else {
            return Ok(None);
        };
        let names: Vec<String> =
            parse_column("threads.participants", serde_json::from_str(&participants))?;
        let participants = names
            .into_iter()
            .map(|name| parse_column("threads.participants", BotName::new(name)))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let round_id = parse_column("threads.round_id", EventId::from_hex(&round_id))?;
        let turns_used = Turns::new(self.conn).used(root_id, &round_id)?;
        Ok(Some(ThreadState {
            root_id: root_id.clone(),
            channel_id: parse_column("threads.channel_id", ChannelId::parse(&channel_id))?,
            participants,
            discussion,
            round_id,
            round_mode: parse_round_mode(&round_mode)?,
            round_started_at,
            turns_used,
        }))
    }
}

/// The `turns` repository: wakes dispatched per bot per round, and the turn-cap reaction flag.
pub struct Turns<'c> {
    conn: &'c Connection,
}

impl<'c> Turns<'c> {
    /// A repository over `conn`.
    pub fn new(conn: &'c Connection) -> Self {
        Self { conn }
    }

    /// Adds one used turn for `bot` in the round and returns the new count.
    pub fn increment(
        &self,
        root_id: &EventId,
        round_id: &EventId,
        bot: &BotName,
    ) -> Result<u32, StoreError> {
        Ok(self.conn.query_row(
            "INSERT INTO turns (root_id, round_id, bot, used) VALUES (?1, ?2, ?3, 1)
             ON CONFLICT(root_id, round_id, bot) DO UPDATE SET used = used + 1
             RETURNING used",
            params![root_id.as_str(), round_id.as_str(), bot.as_str()],
            |row| row.get(0),
        )?)
    }

    /// Sets the turns `bot` used in the round, as a thread rebuild computes them.
    pub fn set_used(
        &self,
        root_id: &EventId,
        round_id: &EventId,
        bot: &BotName,
        used: u32,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO turns (root_id, round_id, bot, used) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(root_id, round_id, bot) DO UPDATE SET used = excluded.used",
            params![root_id.as_str(), round_id.as_str(), bot.as_str(), used],
        )?;
        Ok(())
    }

    /// Records that the turn-cap reaction was sent for `bot` in the round.
    pub fn set_cap_reacted(
        &self,
        root_id: &EventId,
        round_id: &EventId,
        bot: &BotName,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO turns (root_id, round_id, bot, cap_reacted) VALUES (?1, ?2, ?3, 1)
             ON CONFLICT(root_id, round_id, bot) DO UPDATE SET cap_reacted = 1",
            params![root_id.as_str(), round_id.as_str(), bot.as_str()],
        )?;
        Ok(())
    }

    /// Whether the turn-cap reaction was sent for `bot` in the round.
    pub fn cap_reacted(
        &self,
        root_id: &EventId,
        round_id: &EventId,
        bot: &BotName,
    ) -> Result<bool, StoreError> {
        let flag: Option<bool> = self
            .conn
            .query_row(
                "SELECT cap_reacted FROM turns WHERE root_id = ?1 AND round_id = ?2 AND bot = ?3",
                params![root_id.as_str(), round_id.as_str(), bot.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(flag.unwrap_or(false))
    }

    /// Turns used per bot in the round (design section 9.3).
    pub fn used(
        &self,
        root_id: &EventId,
        round_id: &EventId,
    ) -> Result<BTreeMap<BotName, u32>, StoreError> {
        let mut statement = self
            .conn
            .prepare("SELECT bot, used FROM turns WHERE root_id = ?1 AND round_id = ?2")?;
        let rows = statement.query_map(params![root_id.as_str(), round_id.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;
        let mut used = BTreeMap::new();
        for row in rows {
            let (bot, count) = row?;
            used.insert(parse_column("turns.bot", BotName::new(bot))?, count);
        }
        Ok(used)
    }
}
