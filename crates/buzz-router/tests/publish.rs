//! RED tests for T2.7: publisher replies, status notes, reactions, typing
//! (design §6.8).
//!
//! The implementation (`buzz_router::publish`) does not exist yet. These tests
//! pin its contract: event shapes, the `posts` row written before sending,
//! the REST fallback, and the halt refusal.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "assertions and mock handlers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use buzz_router::publish::{
    build_reply, build_status_text, publish_reaction, publish_reply, publish_status_note,
    publish_typing, PublishDeps, PublishError,
};
use buzz_router::relay::rest::RestClient;
use buzz_router::relay::RelayPort;
use buzz_router::store::halts::HaltScope;
use buzz_router::store::Store;
use router_core::config::parse_roster;
use router_core::ids::{BotName, ChannelId, EventId};
use uuid::Uuid;

/// A file database in a tempdir (the `TempDir` must stay alive), a roster with
/// bots A/B/C plus `Owner`, and deps for `bot_name` sending.
fn deps(
    bot_name: &str,
    relay: &support::FakeRelay,
    rest_url: &str,
    auth_tag: Option<nostr::Tag>,
) -> (PublishDeps, Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite3");
    let assert_store = Store::open(&path).unwrap();
    let roster = parse_roster(&support::roster_toml("")).unwrap();
    let bot_keys = support::keys(bot_name);
    let relay_arc: Arc<dyn RelayPort> = Arc::new(relay.clone());
    let rest = RestClient::new(rest_url, bot_keys.clone(), None);
    let publish_store = Store::open(&path).unwrap();
    let deps = PublishDeps {
        store: publish_store,
        relay: relay_arc,
        rest,
        roster,
        keys: bot_keys,
        auth_tag,
    };
    (deps, assert_store, dir)
}

fn bot_a() -> BotName {
    BotName::new("A").unwrap()
}

fn channel_id() -> ChannelId {
    ChannelId::from(support::channel())
}

/// A root event plus a nested parent reply, with their ids.
fn thread() -> (nostr::Event, nostr::Event, EventId, EventId) {
    let chan = support::channel();
    let root = support::top_level(&support::keys("owner"), chan, "root", 1_700_000_000);
    let parent = support::reply(
        &support::keys("B"),
        chan,
        "parent",
        &root,
        &root,
        1_700_000_001,
    );
    let root_id = EventId::from_nostr(&root.id);
    let parent_id = EventId::from_nostr(&parent.id);
    (root, parent, root_id, parent_id)
}

fn tag_vecs(event: &nostr::Event) -> Vec<Vec<String>> {
    event.tags.iter().map(|t| t.as_slice().to_vec()).collect()
}

fn has_tag(event: &nostr::Event, want: &[&str]) -> bool {
    let want: Vec<String> = want.iter().map(|s| (*s).to_string()).collect();
    tag_vecs(event).contains(&want)
}

fn p_tags(event: &nostr::Event) -> Vec<String> {
    tag_vecs(event)
        .iter()
        .filter(|t| t.first().is_some_and(|k| k == "p"))
        .filter_map(|t| t.get(1).cloned())
        .collect()
}

fn e_tags(event: &nostr::Event) -> Vec<Vec<String>> {
    tag_vecs(event)
        .into_iter()
        .filter(|t| t.first().is_some_and(|k| k == "e"))
        .collect()
}

/// Serve `app` on 127.0.0.1:0. Returns the `ws://` relay URL for
/// `RestClient::new` and the server task.
async fn spawn_mock(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("ws://127.0.0.1:{port}"), handle)
}

#[tokio::test]
async fn reply_top_level_shape() {
    let relay = support::FakeRelay::new();
    let (deps, store, _dir) = deps("A", &relay, "ws://127.0.0.1:9", None);
    let (_root, _parent, root_id, _parent_id) = thread();
    let channel = channel_id();
    let wake_id = Uuid::new_v4();

    let event = publish_reply(
        &deps,
        &bot_a(),
        &channel,
        &root_id,
        &root_id,
        Some(wake_id),
        "hello @B",
    )
    .await
    .unwrap();

    assert_eq!(u16::from(event.kind), 9);
    assert!(has_tag(&event, &["h", &support::channel().to_string()]));
    assert_eq!(
        e_tags(&event),
        vec![vec![
            "e".to_string(),
            root_id.as_str().to_string(),
            String::new(),
            "reply".to_string(),
        ]],
        "a top-level reply carries exactly one reply e-tag"
    );
    assert!(
        p_tags(&event).contains(&support::pubkey_hex("B")),
        "mentioned bots get p-tags"
    );
    assert!(has_tag(
        &event,
        &["buzz-router", env!("CARGO_PKG_VERSION"), "reply"]
    ));
    event.verify().unwrap();
    assert_eq!(event.pubkey, support::keys("A").public_key());
    assert_eq!(relay.published().len(), 1);
    assert_eq!(relay.published()[0].id, event.id);
    let published_id = EventId::from_nostr(&event.id);
    assert!(
        store.posts().exists(&published_id).unwrap(),
        "the posts row survives a successful publish"
    );
}

#[tokio::test]
async fn reply_nested_shape() {
    let relay = support::FakeRelay::new();
    let (deps, _store, _dir) = deps("A", &relay, "ws://127.0.0.1:9", None);
    let (_root, _parent, root_id, parent_id) = thread();

    let event = publish_reply(
        &deps,
        &bot_a(),
        &channel_id(),
        &root_id,
        &parent_id,
        None,
        "nested hello @B",
    )
    .await
    .unwrap();

    assert_eq!(
        e_tags(&event),
        vec![
            vec![
                "e".to_string(),
                root_id.as_str().to_string(),
                String::new(),
                "root".to_string(),
            ],
            vec![
                "e".to_string(),
                parent_id.as_str().to_string(),
                String::new(),
                "reply".to_string(),
            ],
        ]
    );
}

#[tokio::test]
async fn reply_mentions_owner_and_auth_tag() {
    let relay = support::FakeRelay::new();
    let auth_tag = nostr::Tag::parse(["auth", "t"]).unwrap();
    let (deps, _store, _dir) = deps("A", &relay, "ws://127.0.0.1:9", Some(auth_tag));
    let (_root, _parent, root_id, _parent_id) = thread();

    let event = publish_reply(
        &deps,
        &bot_a(),
        &channel_id(),
        &root_id,
        &root_id,
        None,
        "thanks Owner",
    )
    .await
    .unwrap();

    assert!(
        p_tags(&event).contains(&support::pubkey_hex("owner")),
        "naming the owner p-tags every owner pubkey"
    );
    assert!(has_tag(&event, &["auth", "t"]));

    // The pure builder pins the same shape without sending.
    let built = build_reply(
        &deps.roster,
        &support::keys("A"),
        Some(&nostr::Tag::parse(["auth", "t"]).unwrap()),
        &channel_id(),
        &root_id,
        &root_id,
        "thanks Owner",
    )
    .unwrap();
    assert!(has_tag(&built, &["auth", "t"]));
    assert!(p_tags(&built).contains(&support::pubkey_hex("owner")));
}

#[tokio::test]
async fn posts_row_visible_at_receipt() {
    let relay = support::FakeRelay::new();
    let (deps, _store, dir) = deps("A", &relay, "ws://127.0.0.1:9", None);
    let (_root, _parent, root_id, _parent_id) = thread();
    let path = dir.path().join("state.sqlite3");
    relay.on_publish(Box::new(move |incoming| {
        let second = Store::open(&path).unwrap();
        let incoming_id = EventId::from_nostr(&incoming.id);
        assert!(
            second.posts().exists(&incoming_id).unwrap(),
            "the posts row exists when the relay receives the event"
        );
    }));

    publish_reply(
        &deps,
        &bot_a(),
        &channel_id(),
        &root_id,
        &root_id,
        None,
        "hook sees the row",
    )
    .await
    .unwrap();
}

#[derive(Clone)]
struct EventsState {
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

async fn capture_event(
    axum::extract::State(state): axum::extract::State<EventsState>,
    body: Bytes,
) -> impl IntoResponse {
    state.bodies.lock().unwrap().push(body.to_vec());
    (StatusCode::OK, "{}".to_string())
}

#[tokio::test]
async fn rest_fallback_on_ws_failure() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let state = EventsState {
        bodies: bodies.clone(),
    };
    let (rest_url, _server) = spawn_mock(
        Router::new()
            .route("/events", post(capture_event))
            .with_state(state),
    )
    .await;

    let relay = support::FakeRelay::new();
    relay.fail_publishes(true);
    let (deps, store, _dir) = deps("A", &relay, &rest_url, None);
    let (_root, _parent, root_id, _parent_id) = thread();

    let event = publish_reply(
        &deps,
        &bot_a(),
        &channel_id(),
        &root_id,
        &root_id,
        None,
        "falls back to REST",
    )
    .await
    .unwrap();

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    let received: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
    assert_eq!(
        received,
        serde_json::to_value(&event).unwrap(),
        "the mock receives the exact event JSON"
    );
    assert!(
        store
            .posts()
            .exists(&EventId::from_nostr(&event.id))
            .unwrap(),
        "the posts row survives a fallback publish"
    );
}

#[derive(Clone)]
struct FailState {
    hits: Arc<AtomicUsize>,
}

async fn failing_events(
    axum::extract::State(state): axum::extract::State<FailState>,
) -> impl IntoResponse {
    state.hits.fetch_add(1, Ordering::SeqCst);
    (StatusCode::INTERNAL_SERVER_ERROR, "{}".to_string())
}

#[tokio::test]
async fn failed_publish_deletes_posts_row() {
    let hits = Arc::new(AtomicUsize::new(0));
    let state = FailState { hits: hits.clone() };
    let (rest_url, _server) = spawn_mock(
        Router::new()
            .route("/events", post(failing_events))
            .with_state(state),
    )
    .await;

    let relay = support::FakeRelay::new();
    relay.fail_publishes(true);
    let seen: Arc<Mutex<Option<EventId>>> = Arc::new(Mutex::new(None));
    {
        let seen = seen.clone();
        relay.on_publish(Box::new(move |incoming| {
            *seen.lock().unwrap() = Some(EventId::from_nostr(&incoming.id));
        }));
    }
    let (deps, store, _dir) = deps("A", &relay, &rest_url, None);
    let (_root, _parent, root_id, _parent_id) = thread();

    let result = publish_reply(
        &deps,
        &bot_a(),
        &channel_id(),
        &root_id,
        &root_id,
        None,
        "both transports fail",
    )
    .await;
    assert!(result.is_err());
    assert_eq!(hits.load(Ordering::SeqCst), 1, "REST is tried exactly once");
    let published_id = seen
        .lock()
        .unwrap()
        .clone()
        .expect("the relay saw the event");
    assert!(
        !store.posts().exists(&published_id).unwrap(),
        "a failed publish deletes the posts row"
    );
}

#[tokio::test]
async fn status_note_texts() {
    assert_eq!(build_status_text(None), "On it, this will take a bit.");
    assert_eq!(
        build_status_text(Some("10 minutes")),
        "On it, about 10 minutes."
    );

    let relay = support::FakeRelay::new();
    let (deps, store, _dir) = deps("A", &relay, "ws://127.0.0.1:9", None);
    let (_root, _parent, root_id, parent_id) = thread();

    let event = publish_status_note(
        &deps,
        &bot_a(),
        &channel_id(),
        &root_id,
        &parent_id,
        None,
        Some("10 minutes"),
    )
    .await
    .unwrap();

    assert_eq!(u16::from(event.kind), 9);
    assert_eq!(event.content, "On it, about 10 minutes.");
    assert!(has_tag(
        &event,
        &["buzz-router", env!("CARGO_PKG_VERSION"), "status"]
    ));
    assert_eq!(
        e_tags(&event),
        vec![
            vec![
                "e".to_string(),
                root_id.as_str().to_string(),
                String::new(),
                "root".to_string(),
            ],
            vec![
                "e".to_string(),
                parent_id.as_str().to_string(),
                String::new(),
                "reply".to_string(),
            ],
        ]
    );
    assert!(
        store
            .posts()
            .exists(&EventId::from_nostr(&event.id))
            .unwrap(),
        "a status note records a posts row"
    );
}

#[tokio::test]
async fn reaction_shape() {
    let relay = support::FakeRelay::new();
    let (deps, store, _dir) = deps("A", &relay, "ws://127.0.0.1:9", None);
    let (root, _parent, _root_id, _parent_id) = thread();
    let target = EventId::from_nostr(&root.id);

    let event = publish_reaction(&deps, &bot_a(), &target, "👍")
        .await
        .unwrap();

    assert_eq!(u16::from(event.kind), 7);
    assert_eq!(event.content, "👍");
    assert!(has_tag(&event, &["e", target.as_str()]));
    assert_eq!(relay.published().len(), 1);
    assert!(
        !store
            .posts()
            .exists(&EventId::from_nostr(&event.id))
            .unwrap(),
        "a reaction records no posts row"
    );
}

#[tokio::test]
async fn typing_shape() {
    let relay = support::FakeRelay::new();
    let (deps, store, _dir) = deps("A", &relay, "ws://127.0.0.1:9", None);
    let (_root, _parent, root_id, parent_id) = thread();

    // Nested: root plus reply e-tags.
    let nested = publish_typing(&deps, &bot_a(), &channel_id(), &root_id, &parent_id)
        .await
        .unwrap();
    assert_eq!(u16::from(nested.kind), 20002);
    assert_eq!(nested.content, "");
    assert!(has_tag(&nested, &["h", &support::channel().to_string()]));
    assert_eq!(
        e_tags(&nested),
        vec![
            vec![
                "e".to_string(),
                root_id.as_str().to_string(),
                String::new(),
                "root".to_string(),
            ],
            vec![
                "e".to_string(),
                parent_id.as_str().to_string(),
                String::new(),
                "reply".to_string(),
            ],
        ]
    );

    // Direct: only the reply e-tag.
    let direct = publish_typing(&deps, &bot_a(), &channel_id(), &root_id, &root_id)
        .await
        .unwrap();
    assert_eq!(
        e_tags(&direct),
        vec![vec![
            "e".to_string(),
            root_id.as_str().to_string(),
            String::new(),
            "reply".to_string(),
        ]]
    );

    assert_eq!(relay.published().len(), 2);
    for event in [&nested, &direct] {
        assert!(
            !store
                .posts()
                .exists(&EventId::from_nostr(&event.id))
                .unwrap(),
            "typing records no posts row"
        );
    }
}

#[tokio::test]
async fn halted_bot_refuses_reply() {
    for scope in [HaltScope::All, HaltScope::Bot(BotName::new("A").unwrap())] {
        let relay = support::FakeRelay::new();
        let (deps, store, _dir) = deps("A", &relay, "ws://127.0.0.1:9", None);
        let (_root, _parent, root_id, _parent_id) = thread();
        let wake_id = Uuid::new_v4();
        store.halts().set(&scope, None, 0).unwrap();

        let result = publish_reply(
            &deps,
            &bot_a(),
            &channel_id(),
            &root_id,
            &root_id,
            Some(wake_id),
            "halted",
        )
        .await;
        assert!(
            matches!(result, Err(PublishError::Halted)),
            "a halted bot refuses the reply"
        );
        assert!(relay.published().is_empty(), "a halted bot sends nothing");
        assert_eq!(
            store.posts().count_for_wake(&wake_id).unwrap(),
            0,
            "a halted bot inserts no posts row"
        );
    }
}
