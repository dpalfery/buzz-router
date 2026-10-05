//! Shared fixtures for the router-core integration tests.
//!
//! Declared with `mod common;` by each test file that needs it (task 1.3 starts it; the
//! conformance and parse tasks extend it). Everything here is deterministic and fake:
//!
//! - a key is `nostr::Keys` whose secret is `sha256("buzz-router-fixture:" + name)`, so a name
//!   always yields the same key and no real key, pubkey or relay URL appears anywhere;
//! - an event id and a channel id are derived from a name the same way;
//! - [`roster`] is built through `parse_roster`, never by constructing the resolved types.

#![allow(
    dead_code,
    reason = "every test crate compiles this module on its own and uses only part of it"
)]
#![allow(
    clippy::expect_used,
    reason = "helpers in a shared test module fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::collections::BTreeSet;

use buzz_sdk::nip_oa::compute_auth_tag;
use nostr::nips::nip19::Nip19Profile;
use nostr::{Keys, RelayUrl, SecretKey, ToBech32};
use router_core::config::{parse_roster, Roster};
use router_core::ids::{BotName, ChannelId, EventId, Pubkey};
use router_core::route::{InEvent, KIND_MESSAGE};
use sha2::{Digest, Sha256};

/// The `created_at` of every fixture event, in unix seconds.
pub const FIXTURE_CREATED_AT: i64 = 1_700_000_000;

/// `sha256("buzz-router-fixture:" + name)`, the repository's fixture-key convention.
fn fixture_digest(name: &str) -> [u8; 32] {
    Sha256::digest(format!("buzz-router-fixture:{name}")).into()
}

/// The deterministic fixture key for `name`.
pub fn keys(name: &str) -> Keys {
    let secret = SecretKey::from_slice(&fixture_digest(name))
        .expect("a sha256 digest is a valid secret key");
    Keys::new(secret)
}

/// The 64-character lowercase hex public key of the fixture key `name`.
pub fn pubkey_hex(name: &str) -> String {
    keys(name).public_key().to_hex()
}

/// The public key of the fixture key `name`, as the router's identifier type.
pub fn pubkey(name: &str) -> Pubkey {
    pubkey_of(&keys(name))
}

/// The public key of `keys`, as the router's identifier type.
pub fn pubkey_of(keys: &Keys) -> Pubkey {
    Pubkey::from_hex(&keys.public_key().to_hex()).expect("a public key is 64 hex characters")
}

/// A deterministic 64-character lowercase hex event id for `name`. Event ids in NIP-10 `e`
/// tags must be 64 hex characters, so a bare `"R"` would not be a thread link.
pub fn event_id_hex(name: &str) -> String {
    hex::encode(fixture_digest(&format!("event:{name}")))
}

/// The same deterministic event id as [`event_id_hex`], as the router's identifier type.
pub fn event_id(name: &str) -> EventId {
    EventId::from_hex(&event_id_hex(name)).expect("a fixture event id is 64 hex characters")
}

/// A deterministic channel UUID for `name`, in its hyphenated lowercase form.
pub fn channel_uuid(name: &str) -> String {
    let digest = fixture_digest(&format!("channel:{name}"));
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Uuid::from_bytes(bytes).to_string()
}

/// The deterministic channel id for `name`.
pub fn channel(name: &str) -> ChannelId {
    ChannelId::parse(&channel_uuid(name)).expect("a fixture channel id is a UUID")
}

/// A fixture roster: owner `O` with two keys (`O` and `O2`), one channel (`room`), and bots `A`,
/// `B` and `C`, each covering every channel. Built through `parse_roster`.
pub fn roster() -> Roster {
    let source = format!(
        r#"version = 1

[owner]
name = "Fixture Owner"
pubkeys = ["{owner_1}", "{owner_2}"]
timezone = "UTC"

[[channels]]
id = "{room}"
name = "fixture-room"

[[bots]]
name = "A"
pubkey = "{bot_a}"
channels = ["*"]
respond_to = "owner-only"

[[bots]]
name = "B"
pubkey = "{bot_b}"
channels = ["*"]
respond_to = "owner-only"

[[bots]]
name = "C"
pubkey = "{bot_c}"
channels = ["*"]
respond_to = "owner-only"
"#,
        owner_1 = pubkey_hex("O"),
        owner_2 = pubkey_hex("O2"),
        room = channel_uuid("room"),
        bot_a = pubkey_hex("A"),
        bot_b = pubkey_hex("B"),
        bot_c = pubkey_hex("C"),
    );
    parse_roster(&source).expect("the fixture roster is valid")
}

/// The `["h", <channel>]` tag naming the fixture channel `room`.
pub fn h_tag() -> Vec<String> {
    vec!["h".to_owned(), channel_uuid("room")]
}

/// A kind-9 event in the fixture channel `room`, authored by `author`, carrying `tags` after
/// its `h` tag. The id is the same for every call, so a test that needs two distinct events
/// builds the second by hand.
pub fn in_event(author: &Keys, tags: Vec<Vec<String>>) -> InEvent {
    let mut all_tags = vec![h_tag()];
    all_tags.extend(tags);
    InEvent {
        id: event_id("event-under-test"),
        pubkey: pubkey_of(author),
        kind: KIND_MESSAGE,
        created_at: FIXTURE_CREATED_AT,
        channel: channel("room"),
        content: "hello".to_owned(),
        tags: all_tags,
    }
}

/// The parts of the NIP-OA tag `["auth", <owner>, <conditions>, <sig>]` that `owner` signs for
/// `agent`, computed with `buzz_sdk::nip_oa::compute_auth_tag`. This is the raw tag array an
/// `InEvent` carries.
pub fn auth_tag(owner: &Keys, agent: &Keys, conditions: &str) -> Vec<String> {
    let json = compute_auth_tag(owner, &agent.public_key(), conditions)
        .expect("the owner and agent keys differ and the conditions are valid");
    serde_json::from_str(&json).expect("compute_auth_tag returns a JSON array of strings")
}

// ---------------------------------------------------------------------------------------------
// Added by task 1.4 for the parser tests. Nothing above this line changed.
// ---------------------------------------------------------------------------------------------

/// A fixture roster like [`roster`], except that the owner's display name is `owner_name` and
/// the bots are the given `(name, aliases)` pairs, in order. Each bot's key is the fixture key
/// named after the bot (`keys(name)`) and each bot covers every channel. Built through
/// `parse_roster`.
pub fn roster_with(owner_name: &str, bots: &[(&str, &[&str])]) -> Roster {
    let mut source = format!(
        r#"version = 1

[owner]
name = "{owner_name}"
pubkeys = ["{owner_1}", "{owner_2}"]
timezone = "UTC"

[[channels]]
id = "{room}"
name = "fixture-room"
"#,
        owner_1 = pubkey_hex("O"),
        owner_2 = pubkey_hex("O2"),
        room = channel_uuid("room"),
    );
    for (name, aliases) in bots {
        let aliases = aliases
            .iter()
            .map(|alias| format!("\"{alias}\""))
            .collect::<Vec<_>>()
            .join(", ");
        source.push_str(&format!(
            r#"
[[bots]]
name = "{name}"
pubkey = "{pubkey}"
aliases = [{aliases}]
channels = ["*"]
respond_to = "owner-only"
"#,
            pubkey = pubkey_hex(name),
        ));
    }
    parse_roster(&source).expect("the fixture roster is valid")
}

/// The bot name `name`, as the router's identifier type.
pub fn bot_name(name: &str) -> BotName {
    BotName::new(name).expect("a fixture bot name is not blank")
}

/// The set of the bot names in `names`.
pub fn bot_set(names: &[&str]) -> BTreeSet<BotName> {
    names.iter().copied().map(bot_name).collect()
}

/// A `["p", <pubkey>]` tag naming the fixture key `name`.
pub fn p_tag(name: &str) -> Vec<String> {
    vec!["p".to_owned(), pubkey_hex(name)]
}

/// A kind-9 event in the fixture channel `room`, authored by `author`, with the text `content`
/// and `tags` after its `h` tag.
pub fn message(author: &Keys, content: &str, tags: Vec<Vec<String>>) -> InEvent {
    InEvent {
        content: content.to_owned(),
        ..in_event(author, tags)
    }
}

/// The `npub1...` encoding of the fixture key `name`.
pub fn npub(name: &str) -> String {
    keys(name)
        .public_key()
        .to_bech32()
        .expect("encoding a public key as bech32 cannot fail")
}

/// The `nprofile1...` encoding of the fixture key `name` with the given relay hints, built with
/// `Nip19Profile::to_bech32`.
pub fn nprofile(name: &str, relays: &[&str]) -> String {
    let relays = relays
        .iter()
        .map(|url| RelayUrl::parse(url).expect("a fixture relay URL parses"));
    Nip19Profile::new(keys(name).public_key(), relays)
        .to_bech32()
        .expect("an nprofile with a short relay list encodes")
}
