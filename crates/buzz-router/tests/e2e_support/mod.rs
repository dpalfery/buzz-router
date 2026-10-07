//! Throwaway harness for the relay integration tests (task 2.9, design §16.4).
//!
//! Every identity is generated fresh per test run and every channel UUID is
//! random, so repeated runs against one relay never collide. Nothing here
//! touches a live relay or a real key: [`relay_url`] only accepts a local
//! `ws://127.0.0.1:` (or `ws://localhost:`) URL, as printed by
//! `scripts/e2e-relay.sh up`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers in a test support module fail the test by panicking"
)]

use std::time::Duration;

use buzz_router::relay::rest::RestClient;
use uuid::Uuid;

/// Whether the relay integration tests run. Everything is skipped unless
/// `BUZZ_E2E=1`.
pub fn e2e_enabled() -> bool {
    std::env::var("BUZZ_E2E").as_deref() == Ok("1")
}

/// The relay URL under test, from `BUZZ_E2E_RELAY_URL`. Refuses anything that
/// is not a local relay.
pub fn relay_url() -> String {
    let url = std::env::var("BUZZ_E2E_RELAY_URL")
        .expect("BUZZ_E2E_RELAY_URL must be set (scripts/e2e-relay.sh up prints it)");
    assert!(
        url.starts_with("ws://127.0.0.1:") || url.starts_with("ws://localhost:"),
        "the e2e relay must be local, got {url}"
    );
    url
}

/// How long relay operations may take before a test fails instead of hanging.
pub const TIMEOUT: Duration = Duration::from_secs(20);

/// Four throwaway identities: one owner and three bots, all generated fresh.
pub struct Identities {
    /// The channel owner, which provisions channels and posts owner messages.
    pub owner: nostr::Keys,
    /// The three local bots.
    pub bots: Vec<nostr::Keys>,
}

/// Generates one owner and three bot identities.
pub fn fresh_identities() -> Identities {
    Identities {
        owner: nostr::Keys::generate(),
        bots: vec![
            nostr::Keys::generate(),
            nostr::Keys::generate(),
            nostr::Keys::generate(),
        ],
    }
}

/// A REST client signing as `keys` against the e2e relay.
pub fn rest_for(keys: &nostr::Keys, url: &str) -> RestClient {
    RestClient::new(url, keys.clone(), None)
}

/// Creates a private channel called `name` owned by `owner` and adds every
/// key in `members`, submitting `build_create_channel` and `build_add_member`
/// over REST. Returns the fresh channel UUID.
pub async fn provision_channel(
    owner_rest: &RestClient,
    owner: &nostr::Keys,
    members: &[nostr::Keys],
    name: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    let create = buzz_sdk::builders::build_create_channel(
        id,
        name,
        Some(buzz_sdk::Visibility::Private),
        None,
        None,
        None,
    )
    .expect("valid channel create");
    let create = create.sign_with_keys(owner).expect("channel create signs");
    owner_rest
        .submit_event(&create)
        .await
        .expect("the relay accepts the channel create");
    for member in members {
        let add = buzz_sdk::builders::build_add_member(
            id,
            &member.public_key().to_hex(),
            Some(buzz_sdk::MemberRole::Member),
        )
        .expect("valid add-member");
        let add = add.sign_with_keys(owner).expect("add-member signs");
        owner_rest
            .submit_event(&add)
            .await
            .expect("the relay accepts the add-member");
    }
    id
}

/// Signs a top-level kind-9 channel message.
pub fn channel_message(author: &nostr::Keys, channel: Uuid, text: &str) -> nostr::Event {
    buzz_sdk::builders::build_message(channel, text, None, &[], false, &[], &[])
        .expect("valid channel message")
        .sign_with_keys(author)
        .expect("channel message signs")
}

/// Signs a kind-9 reply to `parent` under `root`.
pub fn reply_to(
    author: &nostr::Keys,
    channel: Uuid,
    text: &str,
    root: &nostr::Event,
    parent: &nostr::Event,
) -> nostr::Event {
    let thread = buzz_sdk::ThreadRef {
        root_event_id: root.id,
        parent_event_id: parent.id,
    };
    buzz_sdk::builders::build_message(channel, text, Some(&thread), &[], false, &[], &[])
        .expect("valid reply")
        .sign_with_keys(author)
        .expect("reply signs")
}
