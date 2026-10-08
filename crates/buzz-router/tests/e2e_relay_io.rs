//! Relay I/O against a local Buzz relay (task 2.9, requirements R35.5, R44,
//! R50.1, R61.1–R61.5).
//!
//! Every test provisions its own channel and mints fresh identities, so the
//! tests share one relay without interfering. All tests are ignored by
//! default and fail without `BUZZ_E2E=1`, and [`e2e_support::relay_url`]
//! refuses anything but a local
//! relay. Run with:
//!
//! ```sh
//! BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=ws://127.0.0.1:3000 \
//!   cargo test -p buzz-router --test e2e_relay_io -- --ignored --test-threads=1
//! ```

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod e2e_support;
mod support;

use std::collections::BTreeSet;
use std::future::Future;

use buzz_router::ingest::Source;
use buzz_router::relay::conn::{spawn_connection, spawn_synced_connection, ConnParams, SyncParams};
use buzz_router::relay::discovery::discover_channels;
use buzz_router::store::Store;
use router_core::ids::{BotName, ChannelId};

use e2e_support::{
    channel_message, fresh_identities, provision_channel, relay_url, reply_to, rest_for, TIMEOUT,
};

/// Runs `fut`, failing the test instead of hanging when the relay does not
/// answer within [`TIMEOUT`].
async fn within<T>(what: &str, fut: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// A synced connection with its sink and membership tap.
type SyncedConn = (
    buzz_router::relay::conn::Connection,
    tokio::sync::mpsc::UnboundedReceiver<buzz_router::relay::conn::Delivered>,
    tokio::sync::mpsc::UnboundedReceiver<(BotName, BTreeSet<ChannelId>)>,
);

/// A synced connection for `bot_keys` with its own cursor store, sink and
/// membership tap, reporting into a throwaway test core.
fn synced(url: &str, bot_keys: &nostr::Keys) -> SyncedConn {
    let (core, _, _) = support::spawn_test_core();
    let (sink_tx, sink_rx) = tokio::sync::mpsc::unbounded_channel();
    let (tap_tx, tap_rx) = tokio::sync::mpsc::unbounded_channel();
    let store = Store::open_in_memory().expect("in-memory store opens");
    let conn = spawn_synced_connection(SyncParams {
        conn: ConnParams::new(url.to_owned(), bot_keys.clone(), None),
        rest: rest_for(bot_keys, url),
        store,
        bot: BotName::new("A").expect("valid bot name"),
        sink: sink_tx,
        core,
        membership_tap: Some(tap_tx),
    });
    (conn, sink_rx, tap_rx)
}

#[tokio::test]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn nip42_auth_standalone_succeeds() {
    let url = relay_url();
    let ids = fresh_identities();
    let conn = spawn_connection(ConnParams::new(url, ids.bots[0].clone(), None));
    within("standalone NIP-42 auth", conn.wait_up()).await;
}

#[tokio::test]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn owner_attested_auth_succeeds() {
    let url = relay_url();
    let ids = fresh_identities();
    let tag = buzz_sdk::nip_oa::compute_auth_tag(&ids.owner, &ids.bots[0].public_key(), "")
        .expect("owner attests the bot");
    let conn = spawn_connection(ConnParams::new(url, ids.bots[0].clone(), Some(tag)));
    within("owner-attested NIP-42 auth", conn.wait_up()).await;
}

#[tokio::test]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn discovery_finds_the_provisioned_channel() {
    let url = relay_url();
    let ids = fresh_identities();
    let owner_rest = rest_for(&ids.owner, &url);
    let channel = provision_channel(&owner_rest, &ids.owner, &ids.bots, "e2e-discovery").await;
    let bot_rest = rest_for(&ids.bots[0], &url);
    let channels = within(
        "channel discovery",
        discover_channels(&bot_rest, &ids.bots[0].public_key().to_hex()),
    )
    .await
    .expect("discovery succeeds");
    assert!(
        channels
            .iter()
            .any(|found| found.id == channel && found.name == "e2e-discovery"),
        "discovery finds the provisioned channel, got {channels:?}"
    );
}

#[tokio::test]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn live_owner_message_arrives() {
    let url = relay_url();
    let ids = fresh_identities();
    let owner_rest = rest_for(&ids.owner, &url);
    let channel = provision_channel(&owner_rest, &ids.owner, &ids.bots, "e2e-live").await;
    let (_conn, mut sink, mut tap) = synced(&url, &ids.bots[0]);
    within("the membership report", tap.recv())
        .await
        .expect("discovery reports memberships");
    let sent = channel_message(&ids.owner, channel, "hello from the owner");
    within("the owner publish", owner_rest.submit_event(&sent))
        .await
        .expect("the relay accepts the owner message");
    let delivered = within("the live delivery", sink.recv())
        .await
        .expect("the live message is delivered");
    assert!(matches!(delivered.source, Source::Live));
    assert_eq!(delivered.channel, channel);
    assert_eq!(delivered.event.id, sent.id);
}

#[tokio::test]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn missed_messages_arrive_through_backfill() {
    let url = relay_url();
    let ids = fresh_identities();
    let owner_rest = rest_for(&ids.owner, &url);
    let channel = provision_channel(&owner_rest, &ids.owner, &ids.bots, "e2e-backfill").await;
    let root = channel_message(&ids.owner, channel, "the root");
    let reply = reply_to(&ids.owner, channel, "the reply", &root, &root);
    let nested = reply_to(&ids.owner, channel, "the nested reply", &root, &reply);
    for event in [&root, &reply, &nested] {
        within("the backfill seed publish", owner_rest.submit_event(event))
            .await
            .expect("the relay accepts the seed message");
    }
    // Simulate a client that was offline while those messages were sent: its
    // cursor predates them, so the next sync must backfill them.
    let (core, _, _) = support::spawn_test_core();
    let (sink_tx, mut sink) = tokio::sync::mpsc::unbounded_channel();
    let (tap_tx, mut tap) = tokio::sync::mpsc::unbounded_channel();
    let store = Store::open_in_memory().expect("in-memory store opens");
    let bot = BotName::new("A").expect("valid bot name");
    let stale = root.created_at.as_secs().saturating_sub(3_600);
    store
        .cursors()
        .advance(&bot, &url, stale as i64)
        .expect("the stale cursor is stored");
    let _conn = spawn_synced_connection(SyncParams {
        conn: ConnParams::new(url.clone(), ids.bots[0].clone(), None),
        rest: rest_for(&ids.bots[0], &url),
        store,
        bot,
        sink: sink_tx,
        core,
        membership_tap: Some(tap_tx),
    });
    within("the membership report", tap.recv())
        .await
        .expect("discovery reports memberships");
    let mut backfilled = Vec::new();
    for _ in 0..3 {
        let delivered = within("a backfilled message", sink.recv())
            .await
            .expect("backfill delivers the missed messages");
        assert!(
            matches!(delivered.source, Source::Backfill),
            "missed messages arrive as backfill, got {:?}",
            delivered.source
        );
        assert_eq!(delivered.channel, channel);
        backfilled.push(delivered.event);
    }
    let mut ids: Vec<String> = backfilled.iter().map(|event| event.id.to_hex()).collect();
    ids.sort();
    let mut expected = [root.id.to_hex(), reply.id.to_hex(), nested.id.to_hex()];
    expected.sort();
    assert_eq!(ids, expected);
    let order: Vec<(u64, String)> = backfilled
        .iter()
        .map(|event| (event.created_at.as_secs(), event.id.to_hex()))
        .collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted, "backfill arrives oldest first");
}

#[tokio::test]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn thread_fetch_returns_root_and_replies() {
    let url = relay_url();
    let ids = fresh_identities();
    let owner_rest = rest_for(&ids.owner, &url);
    let channel = provision_channel(&owner_rest, &ids.owner, &ids.bots, "e2e-thread").await;
    let root = channel_message(&ids.owner, channel, "thread root");
    let reply = reply_to(&ids.owner, channel, "first reply", &root, &root);
    let nested = reply_to(&ids.owner, channel, "nested reply", &root, &reply);
    for event in [&root, &reply, &nested] {
        within("the thread seed publish", owner_rest.submit_event(event))
            .await
            .expect("the relay accepts the thread message");
    }
    let reader = rest_for(&ids.bots[0], &url);
    let thread = within(
        "the thread fetch",
        reader.query(vec![nostr::Filter::new()
            .kinds([nostr::Kind::Custom(9), nostr::Kind::Custom(40_003)])
            .event(root.id)]),
    )
    .await
    .expect("the thread fetch succeeds");
    let mut found: Vec<String> = thread.iter().map(|event| event.id.to_hex()).collect();
    found.sort();
    let mut expected = [reply.id.to_hex(), nested.id.to_hex()];
    expected.sort();
    assert_eq!(found, expected, "the #e fetch returns both replies");
}

#[tokio::test]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn reply_and_reaction_are_visible_through_rest() {
    let url = relay_url();
    let ids = fresh_identities();
    let owner_rest = rest_for(&ids.owner, &url);
    let channel = provision_channel(&owner_rest, &ids.owner, &ids.bots, "e2e-publish").await;
    let root = channel_message(&ids.owner, channel, "publish root");
    within("the root publish", owner_rest.submit_event(&root))
        .await
        .expect("the relay accepts the root");
    let conn = spawn_connection(ConnParams::new(url.clone(), ids.bots[0].clone(), None));
    within("the publisher auth", conn.wait_up()).await;
    let reply = reply_to(&ids.bots[0], channel, "a real e2e reply", &root, &root);
    within("the reply publish", conn.publish(reply.clone()))
        .await
        .expect("the relay acknowledges the reply");
    let reaction = buzz_sdk::builders::build_reaction(reply.id, "👀")
        .expect("valid reaction")
        .sign_with_keys(&ids.bots[0])
        .expect("reaction signs");
    within("the reaction publish", conn.publish(reaction.clone()))
        .await
        .expect("the relay acknowledges the reaction");
    let reader = rest_for(&ids.bots[1], &url);
    let seen = within(
        "the reply query",
        reader.query(vec![nostr::Filter::new().id(reply.id)]),
    )
    .await
    .expect("the reply query succeeds");
    assert!(
        seen.iter().any(|event| event.id == reply.id),
        "the published reply is visible through a REST query"
    );
    let seen = within(
        "the reaction query",
        reader.query(vec![nostr::Filter::new().id(reaction.id)]),
    )
    .await
    .expect("the reaction query succeeds");
    assert!(
        seen.iter()
            .any(|event| event.id == reaction.id && event.content == "👀"),
        "the published reaction is visible through a REST query"
    );
}

#[tokio::test]
#[ignore = "needs a local Buzz relay: set BUZZ_E2E=1 and run with --ignored"]
async fn typing_indicator_gets_relay_ok() {
    let url = relay_url();
    let ids = fresh_identities();
    let owner_rest = rest_for(&ids.owner, &url);
    let channel = provision_channel(&owner_rest, &ids.owner, &ids.bots, "e2e-typing").await;
    let root = channel_message(&ids.owner, channel, "typing root");
    within("the root publish", owner_rest.submit_event(&root))
        .await
        .expect("the relay accepts the root");
    // The buzz-acp typing shape: empty content, an `h` tag and a direct `e` tag.
    let typing = nostr::EventBuilder::new(nostr::Kind::Custom(20_002), "")
        .tag(nostr::Tag::parse(["h", &channel.to_string()]).expect("valid h tag"))
        .tag(nostr::Tag::parse(["e", &root.id.to_hex(), "", "reply"]).expect("valid e tag"))
        .sign_with_keys(&ids.bots[0])
        .expect("typing signs");
    let conn = spawn_connection(ConnParams::new(url, ids.bots[0].clone(), None));
    within("the publisher auth", conn.wait_up()).await;
    within("the typing publish", conn.publish(typing))
        .await
        .expect("the relay acknowledges the typing indicator");
}
