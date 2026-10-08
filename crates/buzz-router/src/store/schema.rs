//! Schema migrations (design section 9.2). The version lives in `PRAGMA user_version`.

use rusqlite::Connection;

use super::StoreError;

/// Migration 1: the DDL of design section 9.2, verbatim.
const MIGRATION_1: &str = "
CREATE TABLE events (
  id            TEXT PRIMARY KEY,
  channel_id    TEXT NOT NULL,
  root_id       TEXT NOT NULL,
  author        TEXT NOT NULL,
  class         TEXT NOT NULL,
  kind          INTEGER NOT NULL,
  created_at    INTEGER NOT NULL,
  processed_at  INTEGER
);
CREATE INDEX events_root ON events(root_id, created_at);

CREATE TABLE cursors (
  bot             TEXT NOT NULL,
  relay_url       TEXT NOT NULL,
  last_created_at INTEGER NOT NULL,
  PRIMARY KEY (bot, relay_url)
);

CREATE TABLE threads (
  root_id          TEXT PRIMARY KEY,
  channel_id       TEXT NOT NULL,
  participants     TEXT NOT NULL DEFAULT '[]',
  discussion       INTEGER NOT NULL DEFAULT 0,
  round_id         TEXT NOT NULL,
  round_mode       TEXT NOT NULL,
  round_started_at INTEGER NOT NULL
);

CREATE TABLE turns (
  root_id     TEXT NOT NULL,
  round_id    TEXT NOT NULL,
  bot         TEXT NOT NULL,
  used        INTEGER NOT NULL DEFAULT 0,
  cap_reacted INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (root_id, round_id, bot)
);

CREATE TABLE wakes (
  id             TEXT PRIMARY KEY,
  bot            TEXT NOT NULL,
  root_id        TEXT NOT NULL,
  round_id       TEXT NOT NULL,
  reason         TEXT NOT NULL,
  priority       TEXT NOT NULL,
  triggers       TEXT NOT NULL,
  state          TEXT NOT NULL,
  token_hash     TEXT,
  attempt        INTEGER NOT NULL DEFAULT 1,
  created_at     INTEGER NOT NULL,
  dispatch_after INTEGER NOT NULL,
  started_at     INTEGER,
  deadline       INTEGER,
  ended_at       INTEGER,
  outcome        TEXT
);
CREATE INDEX wakes_state   ON wakes(state, bot);
CREATE INDEX wakes_thread  ON wakes(bot, root_id, state);
CREATE INDEX wakes_started ON wakes(bot, started_at);
CREATE UNIQUE INDEX wakes_token ON wakes(token_hash) WHERE token_hash IS NOT NULL;

CREATE TABLE posts (
  event_id   TEXT PRIMARY KEY,
  bot        TEXT NOT NULL,
  wake_id    TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX posts_wake ON posts(wake_id);

CREATE TABLE halts (
  scope        TEXT PRIMARY KEY,
  set_by_event TEXT,
  set_at       INTEGER NOT NULL
);
";

/// The schema version this build creates.
const LATEST: i64 = 1;

pub(super) fn user_version(conn: &Connection) -> Result<i64, StoreError> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Runs migration 1 in one transaction when `user_version = 0`.
pub(super) fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    match user_version(conn)? {
        0 => {
            let tx = conn.transaction()?;
            tx.execute_batch(MIGRATION_1)?;
            tx.pragma_update(None, "user_version", LATEST)?;
            tx.commit()?;
            Ok(())
        }
        LATEST => Ok(()),
        other => Err(StoreError::UnknownVersion(other)),
    }
}
