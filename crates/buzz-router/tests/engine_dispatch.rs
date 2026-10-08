//! Task 3.4: dispatch steps, typing and payload context (design 6.6 and 7, R35, R40.5, DD-17).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use buzz_router::ingest::Source;
use buzz_router::store::wakes::WakeState;
use buzz_router::store::Store;
use nostr::Event;
use router_core::ids::{BotName, ChannelId, EventId};
use router_core::thread::{RoundMode, ThreadState};
use sha2::{Digest, Sha256};
use support::{
    base_secs, base_time, channel, emoji, keys, reply, spawn_test_core_with, top_level,
    FakeAdapter, Step, TestCoreOptions,
};

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

fn id(event: &Event) -> EventId {
    EventId::from_nostr(&event.id)
}

fn options(adapter: &FakeAdapter) -> TestCoreOptions {
    TestCoreOptions {
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    }
}

#[tokio::test(start_paused = true)]
async fn dispatch_stores_the_token_hash_sets_the_deadline_and_counts_a_turn() {
    let adapter = FakeAdapter::new(vec![Step::Wait(Duration::from_secs(5)), Step::Exit(0)]);
    let (core, _relay, store) = spawn_test_core_with(options(&adapter));

    let ask = top_level(&keys("owner"), channel(), "@A hi", base_secs());
    core.ingest(bot("A"), ask.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    let dispatch = adapter.dispatches().pop().unwrap();
    let wake = store.wakes().get(&dispatch.wake_id).unwrap().unwrap();
    assert_eq!(wake.state, WakeState::Running);
    let expected = hex::encode(Sha256::digest(dispatch.token.as_bytes()));
    assert_eq!(wake.token_hash.as_deref(), Some(expected.as_str()));
    assert_ne!(wake.token_hash.as_deref(), Some(dispatch.token.as_str()));
    assert_eq!(dispatch.token.len(), 64);
    let started = wake.started_at.unwrap();
    assert_eq!(started, base_time().timestamp_millis());
    assert_eq!(wake.deadline, Some(started + 20 * 60_000));
    let used = store.turns().used(&id(&ask), &id(&ask)).unwrap();
    assert_eq!(used.get(&bot("A")), Some(&1));
}

#[tokio::test(start_paused = true)]
async fn an_owner_trigger_gets_eyes_on_the_reaction_target() {
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, relay, _store) = spawn_test_core_with(options(&adapter));

    let ask = top_level(&keys("owner"), channel(), "@A hi", base_secs());
    core.ingest(bot("A"), ask.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    core.flush().await;

    assert_eq!(relay.reactions(emoji::EYES), vec![ask.id]);
    let eyes = relay
        .published()
        .into_iter()
        .find(|event| event.content == emoji::EYES)
        .unwrap();
    assert_eq!(eyes.pubkey, keys("A").public_key());
}

#[tokio::test(start_paused = true)]
async fn a_bot_caused_wake_gets_no_eyes() {
    let root = top_level(&keys("owner"), channel(), "@everyone discuss", base_secs());
    let seeded = root.clone();
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, relay, _store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        seed: Some(Box::new(move |store: &Store| {
            store
                .threads()
                .upsert(&ThreadState {
                    root_id: id(&seeded),
                    channel_id: ChannelId::from(channel()),
                    participants: ["A", "B", "C"].into_iter().map(bot).collect(),
                    discussion: true,
                    round_id: id(&seeded),
                    round_mode: RoundMode::Discussion,
                    round_started_at: seeded.created_at.as_secs() as i64,
                    turns_used: BTreeMap::new(),
                })
                .unwrap();
        })),
        ..TestCoreOptions::default()
    });

    let post = reply(&keys("B"), channel(), "my take", &root, &root, base_secs());
    core.ingest(bot("B"), post, Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(30)).await;
    core.flush().await;

    assert!(!adapter.dispatches().is_empty(), "the other bots are woken");
    assert!(relay.reactions(emoji::EYES).is_empty());
}

#[tokio::test(start_paused = true)]
async fn typing_is_sent_at_dispatch_and_every_three_seconds_until_the_wake_ends() {
    let adapter = FakeAdapter::new(vec![Step::Wait(Duration::from_secs(10)), Step::Exit(0)]);
    let (core, relay, _store) = spawn_test_core_with(options(&adapter));

    let ask = top_level(&keys("owner"), channel(), "@A hi", base_secs());
    core.ingest(bot("A"), ask.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    core.flush().await;
    assert_eq!(relay.typing().len(), 1, "typing at dispatch");

    tokio::time::sleep(Duration::from_millis(3_000)).await;
    core.flush().await;
    assert_eq!(relay.typing().len(), 2, "typing again at 3 s");

    tokio::time::sleep(Duration::from_secs(30)).await;
    core.flush().await;
    let typing = relay.typing();
    assert_eq!(
        typing.len(),
        4,
        "0, 3, 6 and 9 s, then the wake ends at 10 s"
    );
    let tags: Vec<Vec<String>> = typing[0]
        .tags
        .iter()
        .map(|tag| tag.as_slice().to_vec())
        .collect();
    assert!(tags.contains(&vec!["h".to_owned(), channel().to_string()]));
    assert!(tags.contains(&vec![
        "e".to_owned(),
        ask.id.to_hex(),
        String::new(),
        "reply".to_owned()
    ]));
    assert_eq!(typing[0].pubkey, keys("A").public_key());
}

#[tokio::test(start_paused = true)]
async fn the_context_holds_the_last_twenty_messages_oldest_first() {
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, relay, _store) = spawn_test_core_with(options(&adapter));
    let owner = keys("owner");
    let start = base_secs() - 1_000;

    let root = top_level(&owner, channel(), "kick-off", start);
    relay.seed(root.clone());
    core.ingest(bot("A"), root.clone(), Source::Live);
    core.flush().await;
    let mut seeded = vec![root.clone()];
    for n in 1..25 {
        let author = if n % 2 == 0 { keys("B") } else { keys("C") };
        let post = reply(
            &author,
            channel(),
            &format!("line {n}\nmore"),
            &root,
            &root,
            start + n,
        );
        relay.seed(post.clone());
        seeded.push(post);
    }
    let ask = reply(&owner, channel(), "@A summarise", &root, &root, start + 100);
    relay.seed(ask.clone());
    seeded.push(ask.clone());
    core.ingest(bot("A"), ask, Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    let payload = adapter.dispatches().pop().unwrap().payload;
    assert_eq!(payload.context.len(), 20);
    let expected: Vec<String> = seeded[6..].iter().map(|e| e.id.to_hex()).collect();
    let got: Vec<String> = payload.context.iter().map(|m| m.id.clone()).collect();
    assert_eq!(got, expected, "the last 20, oldest first");
    assert!(
        payload.context.iter().all(|m| m.new),
        "no previous wake: all new"
    );
    let last = payload.context.last().unwrap();
    assert_eq!(last.author, "Owner");
    assert_eq!(last.class, "owner");
    assert_eq!(payload.context[0].author, "B");
    assert_eq!(payload.context[0].text, "line 6\nmore");
    assert_eq!(
        payload.channel.name,
        channel().to_string(),
        "no name known: the UUID"
    );
}

#[tokio::test(start_paused = true)]
async fn new_flags_mark_messages_after_the_previous_wake_of_this_bot_in_the_thread() {
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, relay, _store) = spawn_test_core_with(options(&adapter));
    let owner = keys("owner");

    let root = top_level(&owner, channel(), "@A one", base_secs() - 100);
    let before = reply(
        &keys("B"),
        channel(),
        "before",
        &root,
        &root,
        base_secs() - 50,
    );
    relay.seed(root.clone());
    relay.seed(before.clone());
    core.ingest(bot("A"), root.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    core.flush().await;

    let after = reply(
        &keys("B"),
        channel(),
        "after",
        &root,
        &root,
        base_secs() + 5,
    );
    let again = reply(
        &owner,
        channel(),
        "@A again",
        &root,
        &root,
        base_secs() + 10,
    );
    relay.seed(after.clone());
    relay.seed(again.clone());
    core.ingest(bot("A"), again.clone(), Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    let dispatches = adapter.dispatches();
    assert_eq!(dispatches.len(), 2);
    let flags: Vec<(String, bool)> = dispatches[1]
        .payload
        .context
        .iter()
        .map(|m| (m.text.clone(), m.new))
        .collect();
    assert_eq!(
        flags,
        vec![
            ("@A one".to_owned(), false),
            ("before".to_owned(), false),
            ("after".to_owned(), true),
            ("@A again".to_owned(), true),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn the_channel_name_is_the_roster_name() {
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, _relay, _store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        roster_extra: format!(
            "\n[[channels]]\nid = \"{}\"\nname = \"general\"\n",
            channel()
        ),
        ..TestCoreOptions::default()
    });
    core.channel_name(ChannelId::from(channel()), "discovered".to_owned());

    core.ingest(
        bot("A"),
        top_level(&keys("owner"), channel(), "@A hi", base_secs()),
        Source::Live,
    );
    core.flush().await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(adapter.dispatches()[0].payload.channel.name, "general");
}

#[tokio::test(start_paused = true)]
async fn the_channel_name_falls_back_to_the_discovered_name() {
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, _relay, _store) = spawn_test_core_with(options(&adapter));
    core.channel_name(ChannelId::from(channel()), "discovered".to_owned());

    core.ingest(
        bot("A"),
        top_level(&keys("owner"), channel(), "@A hi", base_secs()),
        Source::Live,
    );
    core.flush().await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(adapter.dispatches()[0].payload.channel.name, "discovered");
}
