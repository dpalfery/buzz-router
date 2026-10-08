//! Task 2.1: the SQLite store, its migration and one repository per table (design section 9).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::collections::{BTreeMap, BTreeSet};

use buzz_router::store::cursors::Cursors;
use buzz_router::store::events::{EventClass, EventRow};
use buzz_router::store::halts::HaltScope;
use buzz_router::store::posts::PostRow;
use buzz_router::store::wakes::{WakeRow, WakeState};
use buzz_router::store::Store;
use router_core::ids::{BotName, ChannelId, EventId, Pubkey};
use router_core::thread::{RoundMode, ThreadState};

fn hex64(seed: u8) -> String {
    format!("{seed:02x}").repeat(32)
}

fn event_id(seed: u8) -> EventId {
    EventId::from_hex(&hex64(seed)).unwrap()
}

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

fn channel() -> ChannelId {
    ChannelId::parse("6f1c7a52-0d4e-4b8a-9a43-1f2e3d4c5b6a").unwrap()
}

fn pubkey() -> Pubkey {
    let keys = nostr::Keys::parse(&hex64(7)).unwrap();
    Pubkey::from_nostr(&keys.public_key())
}

fn columns(store: &Store, table: &str) -> Vec<(String, String, bool, Option<String>, bool)> {
    let conn = store.connection();
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? != 0,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)? != 0,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn col(
    name: &str,
    ty: &str,
    not_null: bool,
    default: Option<&str>,
    pk: bool,
) -> (String, String, bool, Option<String>, bool) {
    (
        name.to_owned(),
        ty.to_owned(),
        not_null,
        default.map(str::to_owned),
        pk,
    )
}

fn indexes(store: &Store) -> BTreeMap<String, String> {
    let conn = store.connection();
    let mut statement = conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn pragma_text(store: &Store, name: &str) -> String {
    store
        .connection()
        .query_row(&format!("PRAGMA {name}"), [], |row| {
            row.get::<_, rusqlite::types::Value>(0)
        })
        .map(|value| match value {
            rusqlite::types::Value::Integer(n) => n.to_string(),
            rusqlite::types::Value::Text(text) => text,
            other => format!("{other:?}"),
        })
        .unwrap()
}

#[test]
fn file_database_migrates_once_and_sets_pragmas() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite3");

    let store = Store::open(&path).unwrap();
    assert_eq!(store.user_version().unwrap(), 1);
    assert_eq!(pragma_text(&store, "journal_mode"), "wal");
    // synchronous = NORMAL is 1.
    assert_eq!(pragma_text(&store, "synchronous"), "1");
    assert_eq!(pragma_text(&store, "busy_timeout"), "5000");
    store.halts().set(&HaltScope::All, Some("cli"), 10).unwrap();
    drop(store);

    // Reopening must not re-run migration 1, which would fail on the existing tables or wipe data.
    let store = Store::open(&path).unwrap();
    assert_eq!(store.user_version().unwrap(), 1);
    assert_eq!(store.halts().list().unwrap().len(), 1);
}

#[test]
fn in_memory_database_is_migrated() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.user_version().unwrap(), 1);
}

#[test]
fn table_columns_match_design() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(
        columns(&store, "events"),
        vec![
            col("id", "TEXT", false, None, true),
            col("channel_id", "TEXT", true, None, false),
            col("root_id", "TEXT", true, None, false),
            col("author", "TEXT", true, None, false),
            col("class", "TEXT", true, None, false),
            col("kind", "INTEGER", true, None, false),
            col("created_at", "INTEGER", true, None, false),
            col("processed_at", "INTEGER", false, None, false),
        ]
    );
    assert_eq!(
        columns(&store, "cursors"),
        vec![
            col("bot", "TEXT", true, None, true),
            col("relay_url", "TEXT", true, None, true),
            col("last_created_at", "INTEGER", true, None, false),
        ]
    );
    assert_eq!(
        columns(&store, "threads"),
        vec![
            col("root_id", "TEXT", false, None, true),
            col("channel_id", "TEXT", true, None, false),
            col("participants", "TEXT", true, Some("'[]'"), false),
            col("discussion", "INTEGER", true, Some("0"), false),
            col("round_id", "TEXT", true, None, false),
            col("round_mode", "TEXT", true, None, false),
            col("round_started_at", "INTEGER", true, None, false),
        ]
    );
    assert_eq!(
        columns(&store, "turns"),
        vec![
            col("root_id", "TEXT", true, None, true),
            col("round_id", "TEXT", true, None, true),
            col("bot", "TEXT", true, None, true),
            col("used", "INTEGER", true, Some("0"), false),
            col("cap_reacted", "INTEGER", true, Some("0"), false),
        ]
    );
    assert_eq!(
        columns(&store, "wakes"),
        vec![
            col("id", "TEXT", false, None, true),
            col("bot", "TEXT", true, None, false),
            col("root_id", "TEXT", true, None, false),
            col("round_id", "TEXT", true, None, false),
            col("reason", "TEXT", true, None, false),
            col("priority", "TEXT", true, None, false),
            col("triggers", "TEXT", true, None, false),
            col("state", "TEXT", true, None, false),
            col("token_hash", "TEXT", false, None, false),
            col("attempt", "INTEGER", true, Some("1"), false),
            col("created_at", "INTEGER", true, None, false),
            col("dispatch_after", "INTEGER", true, None, false),
            col("started_at", "INTEGER", false, None, false),
            col("deadline", "INTEGER", false, None, false),
            col("ended_at", "INTEGER", false, None, false),
            col("outcome", "TEXT", false, None, false),
        ]
    );
    assert_eq!(
        columns(&store, "posts"),
        vec![
            col("event_id", "TEXT", false, None, true),
            col("bot", "TEXT", true, None, false),
            col("wake_id", "TEXT", false, None, false),
            col("created_at", "INTEGER", true, None, false),
        ]
    );
    assert_eq!(
        columns(&store, "halts"),
        vec![
            col("scope", "TEXT", false, None, true),
            col("set_by_event", "TEXT", false, None, false),
            col("set_at", "INTEGER", true, None, false),
        ]
    );
}

#[test]
fn indexes_match_design() {
    let store = Store::open_in_memory().unwrap();
    let expected: BTreeMap<String, String> = [
        (
            "events_root",
            "CREATE INDEX events_root ON events(root_id, created_at)",
        ),
        (
            "wakes_state",
            "CREATE INDEX wakes_state   ON wakes(state, bot)",
        ),
        (
            "wakes_thread",
            "CREATE INDEX wakes_thread  ON wakes(bot, root_id, state)",
        ),
        (
            "wakes_started",
            "CREATE INDEX wakes_started ON wakes(bot, started_at)",
        ),
        (
            "wakes_token",
            "CREATE UNIQUE INDEX wakes_token ON wakes(token_hash) WHERE token_hash IS NOT NULL",
        ),
        ("posts_wake", "CREATE INDEX posts_wake ON posts(wake_id)"),
    ]
    .into_iter()
    .map(|(name, sql)| (name.to_owned(), sql.to_owned()))
    .collect();
    assert_eq!(indexes(&store), expected);
}

fn event_row(seed: u8, root: u8, created_at: i64) -> EventRow {
    EventRow {
        id: event_id(seed),
        channel_id: channel(),
        root_id: event_id(root),
        author: pubkey(),
        class: EventClass::Owner,
        kind: 9,
        created_at,
        processed_at: None,
    }
}

#[test]
fn events_insert_or_ignore_then_mark_processed() {
    let store = Store::open_in_memory().unwrap();
    let events = store.events();
    let row = event_row(1, 1, 1_700_000_000);

    assert!(!events.is_processed(&row.id).unwrap());
    assert!(events.insert_or_ignore(&row).unwrap());
    let mut other = row.clone();
    other.class = EventClass::Human;
    assert!(!events.insert_or_ignore(&other).unwrap());
    assert_eq!(events.get(&row.id).unwrap(), Some(row.clone()));
    assert!(!events.is_processed(&row.id).unwrap());

    events.mark_processed(&row.id, 1_700_000_000_123).unwrap();
    assert!(events.is_processed(&row.id).unwrap());
    let stored = events.get(&row.id).unwrap().unwrap();
    assert_eq!(stored.processed_at, Some(1_700_000_000_123));

    for class in [EventClass::Bot, EventClass::ForeignBot, EventClass::Human] {
        let mut row = event_row(class as u8 + 10, 1, 1);
        row.class = class;
        row.kind = 40003;
        events.insert_or_ignore(&row).unwrap();
        assert_eq!(events.get(&row.id).unwrap(), Some(row));
    }
    assert_eq!(events.get(&event_id(99)).unwrap(), None);
}

#[test]
fn threads_upsert_and_load_with_participants_as_json() {
    let store = Store::open_in_memory().unwrap();
    let threads = store.threads();
    let mut state = ThreadState {
        root_id: event_id(1),
        channel_id: channel(),
        participants: BTreeSet::from([bot("A"), bot("B")]),
        discussion: false,
        round_id: event_id(1),
        round_mode: RoundMode::Direct,
        round_started_at: 1_700_000_000,
        turns_used: BTreeMap::new(),
    };
    assert_eq!(threads.load(&state.root_id).unwrap(), None);
    threads.upsert(&state).unwrap();
    assert_eq!(threads.load(&state.root_id).unwrap(), Some(state.clone()));

    let raw: String = store
        .connection()
        .query_row(
            "SELECT participants FROM threads WHERE root_id = ?1",
            [state.root_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw, r#"["A","B"]"#);

    state.discussion = true;
    state.round_id = event_id(2);
    state.round_mode = RoundMode::Discussion;
    state.round_started_at = 1_700_000_100;
    state.participants.insert(bot("C"));
    threads.upsert(&state).unwrap();
    assert_eq!(threads.load(&state.root_id).unwrap(), Some(state));
}

#[test]
fn turns_increment_and_cap_reacted() {
    let store = Store::open_in_memory().unwrap();
    let turns = store.turns();
    let (root, round) = (event_id(1), event_id(2));

    assert_eq!(turns.used(&root, &round).unwrap(), BTreeMap::new());
    assert_eq!(turns.increment(&root, &round, &bot("A")).unwrap(), 1);
    assert_eq!(turns.increment(&root, &round, &bot("A")).unwrap(), 2);
    assert_eq!(turns.increment(&root, &round, &bot("B")).unwrap(), 1);
    assert_eq!(turns.increment(&root, &event_id(3), &bot("A")).unwrap(), 1);
    assert_eq!(
        turns.used(&root, &round).unwrap(),
        BTreeMap::from([(bot("A"), 2), (bot("B"), 1)])
    );

    assert!(!turns.cap_reacted(&root, &round, &bot("A")).unwrap());
    turns.set_cap_reacted(&root, &round, &bot("A")).unwrap();
    assert!(turns.cap_reacted(&root, &round, &bot("A")).unwrap());
    assert!(!turns.cap_reacted(&root, &round, &bot("B")).unwrap());
    // Reacting before any turn was used still records the flag.
    turns.set_cap_reacted(&root, &round, &bot("C")).unwrap();
    assert!(turns.cap_reacted(&root, &round, &bot("C")).unwrap());
    assert_eq!(turns.used(&root, &round).unwrap().get(&bot("C")), Some(&0));
}

#[test]
fn thread_load_includes_turns_for_the_current_round() {
    let store = Store::open_in_memory().unwrap();
    let state = ThreadState {
        root_id: event_id(1),
        channel_id: channel(),
        participants: BTreeSet::from([bot("A")]),
        discussion: false,
        round_id: event_id(2),
        round_mode: RoundMode::Direct,
        round_started_at: 5,
        turns_used: BTreeMap::new(),
    };
    store.threads().upsert(&state).unwrap();
    store
        .turns()
        .increment(&state.root_id, &event_id(1), &bot("A"))
        .unwrap();
    store
        .turns()
        .increment(&state.root_id, &state.round_id, &bot("A"))
        .unwrap();
    let loaded = store.threads().load(&state.root_id).unwrap().unwrap();
    assert_eq!(loaded.turns_used, BTreeMap::from([(bot("A"), 1)]));
}

fn wake_row(id: uuid::Uuid, name: &str, root: u8) -> WakeRow {
    WakeRow {
        id,
        bot: bot(name),
        root_id: event_id(root),
        round_id: event_id(root),
        reason: "mention".to_owned(),
        priority: "owner".to_owned(),
        triggers: format!(r#"["{}"]"#, hex64(root)),
        state: WakeState::Queued,
        token_hash: None,
        attempt: 1,
        created_at: 1_000,
        dispatch_after: 1_500,
        started_at: None,
        deadline: None,
        ended_at: None,
        outcome: None,
    }
}

#[test]
fn wakes_insert_update_find_and_count() {
    let store = Store::open_in_memory().unwrap();
    let wakes = store.wakes();
    let first = wake_row(uuid::Uuid::from_u128(1), "A", 1);
    wakes.insert(&first).unwrap();
    assert_eq!(wakes.get(&first.id).unwrap(), Some(first.clone()));

    assert_eq!(
        wakes.find_queued(&bot("A"), &event_id(1)).unwrap(),
        Some(first.clone())
    );
    assert_eq!(wakes.find_queued(&bot("B"), &event_id(1)).unwrap(), None);
    assert_eq!(wakes.find_queued(&bot("A"), &event_id(2)).unwrap(), None);

    wakes
        .start(&first.id, "aa11", 2_000, 2_000 + 600_000)
        .unwrap();
    let started = wakes.get(&first.id).unwrap().unwrap();
    assert_eq!(started.state, WakeState::Running);
    assert_eq!(started.token_hash.as_deref(), Some("aa11"));
    assert_eq!(started.started_at, Some(2_000));
    assert_eq!(started.deadline, Some(602_000));
    assert_eq!(wakes.find_queued(&bot("A"), &event_id(1)).unwrap(), None);
    assert_eq!(wakes.find_by_token_hash("aa11").unwrap(), Some(started));
    assert_eq!(wakes.find_by_token_hash("bb22").unwrap(), None);

    wakes
        .finish(
            &first.id,
            WakeState::Posted,
            3_000,
            Some(r#"{"posted":[],"detail":null,"dropped":false}"#),
        )
        .unwrap();
    let ended = wakes.get(&first.id).unwrap().unwrap();
    assert_eq!(ended.state, WakeState::Posted);
    assert_eq!(ended.ended_at, Some(3_000));
    assert_eq!(
        ended.outcome.as_deref(),
        Some(r#"{"posted":[],"detail":null,"dropped":false}"#)
    );

    wakes.set_state(&first.id, WakeState::Interrupted).unwrap();
    assert_eq!(
        wakes.get(&first.id).unwrap().unwrap().state,
        WakeState::Interrupted
    );

    let second = wake_row(uuid::Uuid::from_u128(2), "A", 2);
    wakes.insert(&second).unwrap();
    wakes.start(&second.id, "cc33", 5_000, 6_000).unwrap();
    let other_bot = wake_row(uuid::Uuid::from_u128(3), "B", 2);
    wakes.insert(&other_bot).unwrap();
    wakes.start(&other_bot.id, "dd44", 5_000, 6_000).unwrap();
    wakes
        .insert(&wake_row(uuid::Uuid::from_u128(4), "A", 3))
        .unwrap();

    assert_eq!(wakes.count_started_since(&bot("A"), 0).unwrap(), 2);
    assert_eq!(wakes.count_started_since(&bot("A"), 2_000).unwrap(), 2);
    assert_eq!(wakes.count_started_since(&bot("A"), 2_001).unwrap(), 1);
    assert_eq!(wakes.count_started_since(&bot("A"), 5_001).unwrap(), 0);
    assert_eq!(wakes.count_started_since(&bot("B"), 0).unwrap(), 1);
}

#[test]
fn wake_states_round_trip() {
    let store = Store::open_in_memory().unwrap();
    let wakes = store.wakes();
    let all = [
        WakeState::Queued,
        WakeState::Running,
        WakeState::Posted,
        WakeState::Passed,
        WakeState::Timeout,
        WakeState::Killed,
        WakeState::Failed,
        WakeState::Interrupted,
    ];
    for (n, state) in all.into_iter().enumerate() {
        let mut row = wake_row(uuid::Uuid::from_u128(n as u128 + 10), "A", 1);
        row.state = state;
        wakes.insert(&row).unwrap();
        assert_eq!(wakes.get(&row.id).unwrap().unwrap().state, state);
    }
    let names: Vec<&str> = all.iter().map(|state| state.as_str()).collect();
    assert_eq!(
        names,
        [
            "queued",
            "running",
            "posted",
            "passed",
            "timeout",
            "killed",
            "failed",
            "interrupted"
        ]
    );
}

#[test]
fn posts_insert_exists_delete() {
    let store = Store::open_in_memory().unwrap();
    let posts = store.posts();
    let row = PostRow {
        event_id: event_id(5),
        bot: bot("A"),
        wake_id: Some(uuid::Uuid::from_u128(1)),
        created_at: 1_700_000_000,
    };
    assert!(!posts.exists(&row.event_id).unwrap());
    posts.insert(&row).unwrap();
    assert!(posts.exists(&row.event_id).unwrap());
    assert_eq!(posts.count_for_wake(&uuid::Uuid::from_u128(1)).unwrap(), 1);
    posts.delete(&row.event_id).unwrap();
    assert!(!posts.exists(&row.event_id).unwrap());

    let unmanaged = PostRow {
        wake_id: None,
        ..row
    };
    posts.insert(&unmanaged).unwrap();
    assert!(posts.exists(&unmanaged.event_id).unwrap());
}

#[test]
fn halts_set_clear_and_list() {
    let store = Store::open_in_memory().unwrap();
    let halts = store.halts();
    assert!(halts.list().unwrap().is_empty());

    halts.set(&HaltScope::All, Some(&hex64(9)), 100).unwrap();
    halts
        .set(&HaltScope::Bot(bot("A")), Some("admin-api"), 200)
        .unwrap();
    let listed = halts.list().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].scope, HaltScope::All);
    assert_eq!(listed[0].set_by_event.as_deref(), Some(hex64(9).as_str()));
    assert_eq!(listed[0].set_at, 100);
    assert_eq!(listed[1].scope, HaltScope::Bot(bot("A")));
    assert_eq!(listed[1].set_by_event.as_deref(), Some("admin-api"));

    // Setting again replaces the row rather than adding one.
    halts.set(&HaltScope::All, Some("cli"), 300).unwrap();
    assert_eq!(halts.list().unwrap().len(), 2);

    halts.clear(&HaltScope::All).unwrap();
    let listed = halts.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].scope, HaltScope::Bot(bot("A")));
    halts.clear(&HaltScope::Bot(bot("A"))).unwrap();
    assert!(halts.list().unwrap().is_empty());
}

#[test]
fn cursors_get_and_advance_never_backwards() {
    let store = Store::open_in_memory().unwrap();
    let cursors: Cursors<'_> = store.cursors();
    let relay = "wss://relay.example.invalid";
    assert_eq!(cursors.get(&bot("A"), relay).unwrap(), None);

    cursors.advance(&bot("A"), relay, 100).unwrap();
    assert_eq!(cursors.get(&bot("A"), relay).unwrap(), Some(100));
    cursors.advance(&bot("A"), relay, 50).unwrap();
    assert_eq!(cursors.get(&bot("A"), relay).unwrap(), Some(100));
    cursors.advance(&bot("A"), relay, 150).unwrap();
    assert_eq!(cursors.get(&bot("A"), relay).unwrap(), Some(150));

    assert_eq!(cursors.get(&bot("B"), relay).unwrap(), None);
    assert_eq!(cursors.get(&bot("A"), "ws://other.invalid").unwrap(), None);
}
