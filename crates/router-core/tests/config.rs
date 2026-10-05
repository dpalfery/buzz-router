//! Task 1.2 (RED): roster and router configuration types, validation and `roster_hash`.
//!
//! Covers R1.2, R1.3, R1.6 to R1.14, R2.1 to R2.12, R22.3 and the assumptions A2 and A3
//! (the rules that need no I/O), using the binding interface decisions D1 to D4 recorded for
//! this task:
//!
//! - every config type is imported from the `router_core::config` root and every identifier
//!   from `router_core::ids`;
//! - the resolved types are built only through `parse_roster` and `parse_router`;
//! - a rejection test asserts the exact list of issue paths (sorted by path, as `ConfigErrors`
//!   guarantees) and never the message text;
//! - every key, pubkey and channel id is a deterministic fake derived from
//!   `sha256("buzz-router-fixture:" + name)`; there are no real keys, pubkeys or relay URLs.
//!
//! The sections follow the Test contract: roster, roster rejections, router, router rejections,
//! `roster_hash`, then the identifier and error-collection behaviour the decision record fixes.

#![allow(
    clippy::expect_used,
    reason = "helpers in an integration-test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::PathBuf;

use buzz_sdk::nip_oa::parse_auth_tag;
use chrono::NaiveTime;
use nostr::{Keys, SecretKey};
use router_core::config::{
    parse_roster, parse_router, roster_hash, router_roster_path, AdapterConfig, ChannelScope,
    ConfigErrors, ConfigIssue, KeySource, Limits, PromptMode, QuietHours, ReplyMode, RespondTo,
    Roster, RouterConfig, WebhookMode,
};
use router_core::ids::{BotName, ChannelId, EventId, IdError, Pubkey};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------------------------
// Deterministic fixtures
// ---------------------------------------------------------------------------------------------

/// `sha256("buzz-router-fixture:" + name)`, the repository's fixture-key convention.
fn fixture_digest(name: &str) -> [u8; 32] {
    Sha256::digest(format!("buzz-router-fixture:{name}")).into()
}

fn fixture_keys(name: &str) -> Keys {
    let secret = SecretKey::from_slice(&fixture_digest(name))
        .expect("a sha256 digest is a valid secret key");
    Keys::new(secret)
}

/// The 64-character lowercase hex public key of the fixture key `name`.
fn fixture_pubkey(name: &str) -> String {
    fixture_keys(name).public_key().to_hex()
}

/// A deterministic channel UUID in its hyphenated lowercase form.
fn fixture_uuid(name: &str) -> String {
    let digest = fixture_digest(name);
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn pubkey(name: &str) -> Pubkey {
    Pubkey::from_hex(&fixture_pubkey(name)).expect("a fixture public key is 64 hex characters")
}

fn pubkeys(names: &[&str]) -> BTreeSet<Pubkey> {
    names.iter().map(|name| pubkey(name)).collect()
}

fn channel(name: &str) -> ChannelId {
    ChannelId::parse(&fixture_uuid(name)).expect("a fixture channel id is a UUID")
}

fn bot_name(name: &str) -> BotName {
    BotName::new(name).expect("a fixture bot name is not empty")
}

fn hm(hour: u32, minute: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(hour, minute, 0).expect("a valid wall-clock time")
}

/// Replaces the first occurrence of `from`, and fails the test when it is not there so a
/// stale fixture cannot turn a rejection test into a vacuous one.
fn edit(source: &str, from: &str, to: &str) -> String {
    assert!(
        source.contains(from),
        "fixture edit target not found: {from}"
    );
    source.replacen(from, to, 1)
}

// ---------------------------------------------------------------------------------------------
// Roster fixtures
// ---------------------------------------------------------------------------------------------

/// The roster of brief section 4.1, verbatim, with its placeholders still in place.
const BRIEF_ROSTER: &str = r#"version = 1

[owner]
name = "David"
pubkeys = ["<david-key-1-hex>", "<david-key-2-hex>"]   # both of David's keys
timezone = "America/Chicago"                            # IANA zone, used for quiet hours

[limits]                      # defaults for every bot; per-bot overrides allowed
turns_per_round = 4
wakes_per_hour = 20
wakes_per_day = 100
quiet_hours = "23:00-07:00"   # owner timezone; "" disables
discussion_debounce_secs = 20
discussion_debounce_max_secs = 90
max_wake_minutes = 20
max_posts_per_wake = 3
status_note_after_secs = 20

[[channels]]
id = "<channel-uuid>"
name = "work-for-david"
default_bot = ""              # optional: bot that answers David's untagged top-level messages here

[[bots]]
name = "dp-grok-bot"          # exact Buzz display name
pubkey = "<hex>"
aliases = []                  # extra @names that address this bot, matched as whole words
channels = ["*"]              # "*" = every channel the bot is a member of, or a list of channel uuids
respond_to = "owner-only"     # owner-only | anyone
machine = "dp-box"            # informational only
# limits = { turns_per_round = 4 }   # optional override
"#;

/// A small valid roster with two owner keys, three channels and two bots. The rejection tests
/// each break exactly one thing in it.
const BASE_ROSTER: &str = r#"version = 1

[owner]
name = "Fixture Owner"
pubkeys = ["<owner-1>", "<owner-2>"]
timezone = "America/Chicago"

[limits]
turns_per_round = 4
quiet_hours = "23:00-07:00"

[[channels]]
id = "<channel-1>"
name = "alpha-room"
default_bot = "alpha-bot"

[[channels]]
id = "<channel-2>"
name = "beta-room"

[[channels]]
id = "<channel-3>"
name = "gamma-room"
default_bot = ""

[[bots]]
name = "alpha-bot"
pubkey = "<alpha-key>"
aliases = ["alpha"]
channels = ["*"]
respond_to = "owner-only"

[[bots]]
name = "beta-bot"
pubkey = "<beta-key>"
aliases = []
channels = ["<channel-1>"]
respond_to = "anyone"
machine = "box-2"
"#;

/// The version and owner of a roster, for tests that append their own `[[bots]]` blocks.
const OWNER_ONLY_HEADER: &str = r#"version = 1

[owner]
name = "Fixture Owner"
pubkeys = ["<owner-1>"]
timezone = "UTC"
"#;

/// A roster that omits `[limits]`, `[[channels]]` and `aliases` (all optional, A2). The
/// `<limits-table>` and `<alpha-extra>` slots take whatever the limits tests need.
const MINIMAL_ROSTER: &str = r#"version = 1

[owner]
name = "Fixture Owner"
pubkeys = ["<owner-1>"]
timezone = "UTC"

<limits-table>

[[bots]]
name = "alpha-bot"
pubkey = "<alpha-key>"
channels = ["*"]
respond_to = "owner-only"
<alpha-extra>

[[bots]]
name = "beta-bot"
pubkey = "<beta-key>"
channels = ["*"]
respond_to = "owner-only"
"#;

fn fill(template: &str) -> String {
    template
        .replace("<owner-1>", &fixture_pubkey("owner-1"))
        .replace("<owner-2>", &fixture_pubkey("owner-2"))
        .replace("<alpha-key>", &fixture_pubkey("alpha-key"))
        .replace("<beta-key>", &fixture_pubkey("beta-key"))
        .replace("<channel-1>", &fixture_uuid("channel-1"))
        .replace("<channel-2>", &fixture_uuid("channel-2"))
        .replace("<channel-3>", &fixture_uuid("channel-3"))
}

fn base_roster() -> String {
    fill(BASE_ROSTER)
}

fn minimal_roster(limits_table: &str, alpha_extra: &str) -> String {
    fill(
        &MINIMAL_ROSTER
            .replace("<limits-table>", limits_table)
            .replace("<alpha-extra>", alpha_extra),
    )
}

/// One `[[bots]]` block for `name`, with a fixture key derived from the name itself.
fn bot_block(name: &str, aliases: &[&str]) -> String {
    let aliases = aliases
        .iter()
        .map(|alias| format!("\"{alias}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\n[[bots]]\nname = \"{name}\"\npubkey = \"{}\"\naliases = [{aliases}]\nchannels = [\"*\"]\nrespond_to = \"owner-only\"\n",
        fixture_pubkey(name)
    )
}

fn parsed_base_roster() -> Roster {
    parse_roster(&base_roster()).expect("the base roster is valid")
}

/// The issue paths of a rejected roster, in the order `ConfigErrors` reports them.
fn roster_error_paths(source: &str) -> Vec<String> {
    let errors = parse_roster(source).expect_err("the roster should be rejected");
    errors.iter().map(|issue| issue.path.clone()).collect()
}

/// R2.5 spelled out value by value, so a wrong `Limits::default()` cannot hide behind itself.
fn r2_5_defaults() -> Limits {
    Limits {
        turns_per_round: 4,
        wakes_per_hour: 20,
        wakes_per_day: 100,
        quiet_hours: Some(QuietHours {
            start: hm(23, 0),
            end: hm(7, 0),
        }),
        discussion_debounce_secs: 20,
        discussion_debounce_max_secs: 90,
        max_wake_minutes: 20,
        max_posts_per_wake: 3,
        status_note_after_secs: 20,
    }
}

// ---------------------------------------------------------------------------------------------
// Roster: valid input
// ---------------------------------------------------------------------------------------------

#[test]
fn brief_section_4_1_roster_with_fixture_keys_parses() {
    let source = BRIEF_ROSTER
        .replace("<david-key-1-hex>", &fixture_pubkey("david-key-1"))
        .replace("<david-key-2-hex>", &fixture_pubkey("david-key-2"))
        .replace("<channel-uuid>", &fixture_uuid("work-for-david"))
        .replace("<hex>", &fixture_pubkey("dp-grok-bot"));

    let roster = parse_roster(&source).expect("the brief section 4.1 roster is valid");

    assert_eq!(roster.owner.name, "David");
    assert_eq!(
        roster.owner.pubkeys,
        pubkeys(&["david-key-1", "david-key-2"]),
        "both owner keys are kept (R2.2)"
    );
    assert_eq!(roster.owner.timezone, chrono_tz::America::Chicago);

    let channel_id = channel("work-for-david");
    assert_eq!(roster.channels.len(), 1);
    assert_eq!(roster.channels[&channel_id].id, channel_id);
    assert_eq!(roster.channels[&channel_id].name, "work-for-david");
    assert_eq!(
        roster.channels[&channel_id].default_bot, None,
        "an empty default_bot means no default bot"
    );

    assert_eq!(roster.bots.len(), 1);
    let bot = &roster.bots["dp-grok-bot"];
    assert_eq!(bot.name, bot_name("dp-grok-bot"));
    assert_eq!(bot.pubkey, pubkey("dp-grok-bot"));
    assert!(bot.aliases.is_empty());
    assert_eq!(bot.channels, ChannelScope::All);
    assert_eq!(bot.respond_to, RespondTo::OwnerOnly);
    assert_eq!(bot.limits, r2_5_defaults());
}

#[test]
fn base_roster_parses_and_keeps_names_channels_and_respond_to() {
    let roster = parsed_base_roster();

    assert_eq!(roster.owner.name, "Fixture Owner");
    assert_eq!(roster.channels.len(), 3);
    assert_eq!(
        roster.channels[&channel("channel-1")].default_bot,
        Some(bot_name("alpha-bot"))
    );
    assert_eq!(roster.channels[&channel("channel-2")].default_bot, None);
    assert_eq!(roster.channels[&channel("channel-3")].default_bot, None);

    assert_eq!(
        roster.bots.keys().map(BotName::as_str).collect::<Vec<_>>(),
        ["alpha-bot", "beta-bot"]
    );
    assert_eq!(roster.bots["alpha-bot"].pubkey, pubkey("alpha-key"));
    assert_eq!(roster.bots["alpha-bot"].aliases, ["alpha"]);
    assert_eq!(roster.bots["alpha-bot"].respond_to, RespondTo::OwnerOnly);
    assert_eq!(roster.bots["beta-bot"].pubkey, pubkey("beta-key"));
    assert_eq!(roster.bots["beta-bot"].respond_to, RespondTo::Anyone);
}

#[test]
fn omitted_limits_take_the_r2_5_defaults() {
    let roster = parse_roster(&minimal_roster("", "")).expect("a roster without limits is valid");

    assert_eq!(roster.bots["alpha-bot"].limits, r2_5_defaults());
    assert_eq!(roster.bots["beta-bot"].limits, r2_5_defaults());
    assert_eq!(Limits::default(), r2_5_defaults());
    assert!(
        roster.channels.is_empty(),
        "omitted channels are an empty set"
    );
}

#[test]
fn a_limits_table_with_some_keys_leaves_the_others_at_their_defaults() {
    let source = minimal_roster("[limits]\nturns_per_round = 6\nwakes_per_hour = 10", "");

    let roster = parse_roster(&source).expect("a partial [limits] table is valid");

    let expected = Limits {
        turns_per_round: 6,
        wakes_per_hour: 10,
        ..r2_5_defaults()
    };
    assert_eq!(roster.bots["alpha-bot"].limits, expected);
    assert_eq!(roster.bots["beta-bot"].limits, expected);
}

#[test]
fn every_limits_key_is_accepted_in_the_global_table() {
    let source = minimal_roster(
        "[limits]\nturns_per_round = 7\nwakes_per_hour = 11\nwakes_per_day = 111\n\
         quiet_hours = \"22:30-06:15\"\ndiscussion_debounce_secs = 21\n\
         discussion_debounce_max_secs = 91\nmax_wake_minutes = 21\nmax_posts_per_wake = 4\n\
         status_note_after_secs = 22",
        "",
    );

    let roster = parse_roster(&source).expect("every limits key is accepted (R2.4)");

    let expected = Limits {
        turns_per_round: 7,
        wakes_per_hour: 11,
        wakes_per_day: 111,
        quiet_hours: Some(QuietHours {
            start: hm(22, 30),
            end: hm(6, 15),
        }),
        discussion_debounce_secs: 21,
        discussion_debounce_max_secs: 91,
        max_wake_minutes: 21,
        max_posts_per_wake: 4,
        status_note_after_secs: 22,
    };
    assert_eq!(roster.bots["alpha-bot"].limits, expected);
    assert_eq!(roster.bots["beta-bot"].limits, expected);
}

#[test]
fn a_per_bot_override_wins_over_the_defaults_for_that_bot_only() {
    let source = minimal_roster("", "limits = { turns_per_round = 2 }");

    let roster = parse_roster(&source).expect("a per-bot override is valid");

    assert_eq!(
        roster.bots["alpha-bot"].limits,
        Limits {
            turns_per_round: 2,
            ..Limits::default()
        }
    );
    assert_eq!(roster.bots["beta-bot"].limits, Limits::default());
}

#[test]
fn a_per_bot_override_merges_key_by_key_over_the_global_limits_over_the_defaults() {
    let source = minimal_roster(
        "[limits]\nturns_per_round = 6\nwakes_per_hour = 10",
        "limits = { turns_per_round = 2, max_posts_per_wake = 1 }",
    );

    let roster = parse_roster(&source).expect("a per-bot override over [limits] is valid");

    assert_eq!(
        roster.bots["alpha-bot"].limits,
        Limits {
            turns_per_round: 2,
            wakes_per_hour: 10,
            max_posts_per_wake: 1,
            ..r2_5_defaults()
        },
        "the bot's value, then [limits], then the default (R2.6)"
    );
    assert_eq!(
        roster.bots["beta-bot"].limits,
        Limits {
            turns_per_round: 6,
            wakes_per_hour: 10,
            ..r2_5_defaults()
        }
    );
}

#[test]
fn every_limits_key_is_accepted_in_a_per_bot_override() {
    let source = minimal_roster(
        "[limits]\nturns_per_round = 6",
        "limits = { turns_per_round = 8, wakes_per_hour = 12, wakes_per_day = 112, \
         quiet_hours = \"21:00-05:00\", discussion_debounce_secs = 23, \
         discussion_debounce_max_secs = 93, max_wake_minutes = 23, max_posts_per_wake = 5, \
         status_note_after_secs = 24 }",
    );

    let roster = parse_roster(&source).expect("every limits key is accepted per bot (R2.6)");

    assert_eq!(
        roster.bots["alpha-bot"].limits,
        Limits {
            turns_per_round: 8,
            wakes_per_hour: 12,
            wakes_per_day: 112,
            quiet_hours: Some(QuietHours {
                start: hm(21, 0),
                end: hm(5, 0),
            }),
            discussion_debounce_secs: 23,
            discussion_debounce_max_secs: 93,
            max_wake_minutes: 23,
            max_posts_per_wake: 5,
            status_note_after_secs: 24,
        }
    );
    assert_eq!(
        roster.bots["beta-bot"].limits,
        Limits {
            turns_per_round: 6,
            ..r2_5_defaults()
        }
    );
}

#[test]
fn an_empty_global_quiet_hours_disables_quiet_hours_for_every_bot() {
    let source = minimal_roster("[limits]\nquiet_hours = \"\"", "");

    let roster = parse_roster(&source).expect("an empty quiet_hours is valid (R22.2)");

    assert_eq!(roster.bots["alpha-bot"].limits.quiet_hours, None);
    assert_eq!(roster.bots["beta-bot"].limits.quiet_hours, None);
}

#[test]
fn an_empty_per_bot_quiet_hours_overrides_a_global_range() {
    let source = minimal_roster(
        "[limits]\nquiet_hours = \"22:00-06:00\"",
        "limits = { quiet_hours = \"\" }",
    );

    let roster = parse_roster(&source).expect("a per-bot empty quiet_hours is valid");

    assert_eq!(
        roster.bots["alpha-bot"].limits.quiet_hours, None,
        "the bot's empty value is its own value, not an omission"
    );
    assert_eq!(
        roster.bots["beta-bot"].limits.quiet_hours,
        Some(QuietHours {
            start: hm(22, 0),
            end: hm(6, 0),
        })
    );
}

#[test]
fn a_star_channel_list_resolves_to_all_channels() {
    let roster = parsed_base_roster();

    assert_eq!(roster.bots["alpha-bot"].channels, ChannelScope::All);
}

#[test]
fn a_uuid_channel_list_resolves_to_exactly_those_channels() {
    let source = edit(
        &base_roster(),
        &format!("channels = [\"{}\"]", fixture_uuid("channel-1")),
        &format!(
            "channels = [\"{}\", \"{}\"]",
            fixture_uuid("channel-1"),
            fixture_uuid("channel-3")
        ),
    );

    let roster = parse_roster(&source).expect("a UUID channel list is valid (R2.10)");

    assert_eq!(
        roster.bots["beta-bot"].channels,
        ChannelScope::Only(BTreeSet::from([channel("channel-1"), channel("channel-3")])),
        "channel-2 is defined but not listed, so it is not covered"
    );
}

#[test]
fn an_empty_channel_list_resolves_to_no_channels() {
    let source = edit(
        &base_roster(),
        &format!("channels = [\"{}\"]", fixture_uuid("channel-1")),
        "channels = []",
    );

    let roster = parse_roster(&source).expect("an empty channel list is valid");

    assert_eq!(
        roster.bots["beta-bot"].channels,
        ChannelScope::Only(BTreeSet::new())
    );
}

#[test]
fn an_alias_resolves_through_the_lowercase_name_index() {
    let roster = parsed_base_roster();

    assert_eq!(roster.name_index["alpha"].as_str(), "alpha-bot");
    assert_eq!(roster.name_index["alpha-bot"].as_str(), "alpha-bot");
    assert_eq!(roster.name_index["beta-bot"].as_str(), "beta-bot");
    assert_eq!(
        roster.name_index.len(),
        3,
        "two names and one alias, nothing else"
    );
}

#[test]
fn name_index_keys_are_lowercase_while_the_values_keep_the_exact_name() {
    let source = format!(
        "{}{}{}",
        fill(OWNER_ONLY_HEADER),
        bot_block("Hal.macbook-pro", &["Grok", "HAL"]),
        bot_block("ARR Ride Guide", &[]),
    );

    let roster = parse_roster(&source).expect("mixed-case names and aliases are valid");

    assert_eq!(
        roster
            .name_index
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["arr ride guide", "grok", "hal", "hal.macbook-pro"]
    );
    assert_eq!(roster.name_index["grok"].as_str(), "Hal.macbook-pro");
    assert_eq!(roster.name_index["hal"].as_str(), "Hal.macbook-pro");
    assert_eq!(
        roster.name_index["hal.macbook-pro"].as_str(),
        "Hal.macbook-pro"
    );
    assert_eq!(
        roster.name_index["arr ride guide"].as_str(),
        "ARR Ride Guide"
    );
    assert!(roster.bots.contains_key("Hal.macbook-pro"));
    assert!(!roster.bots.contains_key("hal.macbook-pro"));
}

#[test]
fn an_uppercase_hex_pubkey_is_accepted_and_stored_lowercase() {
    let upper = fixture_pubkey("alpha-key").to_ascii_uppercase();
    let source = edit(&base_roster(), &fixture_pubkey("alpha-key"), &upper);

    let roster = parse_roster(&source).expect("any-case hex is accepted (D1)");

    assert_eq!(roster.bots["alpha-bot"].pubkey, pubkey("alpha-key"));
    assert_eq!(
        roster.bots["alpha-bot"].pubkey.as_str(),
        fixture_pubkey("alpha-key")
    );
}

// ---------------------------------------------------------------------------------------------
// Roster: rejections (one test each, asserting the issue path)
// ---------------------------------------------------------------------------------------------

#[test]
fn rejects_version_2() {
    let source = edit(&base_roster(), "version = 1", "version = 2");

    assert_eq!(roster_error_paths(&source), ["version"]);
}

#[test]
fn rejects_an_unknown_timezone() {
    let source = edit(&base_roster(), "America/Chicago", "Mars/Olympus");

    assert_eq!(roster_error_paths(&source), ["owner.timezone"]);
}

#[test]
fn rejects_quiet_hours_that_are_not_a_time_range() {
    let source = edit(
        &base_roster(),
        "quiet_hours = \"23:00-07:00\"",
        "quiet_hours = \"23-7\"",
    );

    assert_eq!(roster_error_paths(&source), ["limits.quiet_hours"]);
}

#[test]
fn rejects_quiet_hours_with_an_invalid_time() {
    let source = edit(
        &base_roster(),
        "quiet_hours = \"23:00-07:00\"",
        "quiet_hours = \"25:00-07:00\"",
    );

    assert_eq!(roster_error_paths(&source), ["limits.quiet_hours"]);
}

#[test]
fn rejects_per_bot_quiet_hours_that_are_not_a_time_range() {
    let source = edit(
        &base_roster(),
        "aliases = []",
        "aliases = []\nlimits = { quiet_hours = \"23-7\" }",
    );

    assert_eq!(roster_error_paths(&source), ["bots[1].limits.quiet_hours"]);
}

#[test]
fn rejects_an_alias_that_duplicates_another_bots_name_in_different_case() {
    let source = edit(&base_roster(), "aliases = []", "aliases = [\"ALPHA-Bot\"]");

    assert_eq!(roster_error_paths(&source), ["bots[1].aliases[0]"]);
}

#[test]
fn rejects_an_alias_that_duplicates_another_bots_alias_in_different_case() {
    let source = edit(&base_roster(), "aliases = []", "aliases = [\"ALPHA\"]");

    assert_eq!(roster_error_paths(&source), ["bots[1].aliases[0]"]);
}

#[test]
fn rejects_a_bot_name_that_duplicates_another_bots_name_in_different_case() {
    let source = edit(
        &base_roster(),
        "name = \"beta-bot\"",
        "name = \"ALPHA-BOT\"",
    );

    assert_eq!(roster_error_paths(&source), ["bots[1].name"]);
}

#[test]
fn rejects_two_bots_that_share_a_pubkey() {
    let source = edit(
        &base_roster(),
        &fixture_pubkey("beta-key"),
        &fixture_pubkey("alpha-key"),
    );

    assert_eq!(roster_error_paths(&source), ["bots[1].pubkey"]);
}

#[test]
fn rejects_an_owner_key_reused_as_a_bot_key() {
    let source = edit(
        &base_roster(),
        &fixture_pubkey("beta-key"),
        &fixture_pubkey("owner-2"),
    );

    assert_eq!(roster_error_paths(&source), ["bots[1].pubkey"]);
}

#[test]
fn rejects_an_owner_key_listed_twice() {
    let source = edit(
        &base_roster(),
        &fixture_pubkey("owner-2"),
        &fixture_pubkey("owner-1"),
    );

    assert_eq!(roster_error_paths(&source), ["owner.pubkeys[1]"]);
}

#[test]
fn rejects_an_unknown_default_bot() {
    let source = edit(
        &base_roster(),
        "default_bot = \"alpha-bot\"",
        "default_bot = \"ghost-bot\"",
    );

    assert_eq!(roster_error_paths(&source), ["channels[0].default_bot"]);
}

#[test]
fn rejects_a_default_bot_that_differs_from_the_bot_name_in_case() {
    let source = edit(
        &base_roster(),
        "default_bot = \"alpha-bot\"",
        "default_bot = \"Alpha-Bot\"",
    );

    assert_eq!(roster_error_paths(&source), ["channels[0].default_bot"]);
}

#[test]
fn rejects_the_unknown_key_turns_per_rnd() {
    let source = edit(&base_roster(), "turns_per_round = 4", "turns_per_rnd = 4");

    assert_eq!(roster_error_paths(&source), ["limits.turns_per_rnd"]);
}

#[test]
fn rejects_a_missing_respond_to() {
    let source = edit(&base_roster(), "respond_to = \"owner-only\"\n", "");

    assert_eq!(roster_error_paths(&source), ["bots[0].respond_to"]);
}

#[test]
fn rejects_a_respond_to_value_outside_the_listed_options() {
    let source = edit(
        &base_roster(),
        "respond_to = \"owner-only\"",
        "respond_to = \"everyone\"",
    );

    assert_eq!(roster_error_paths(&source), ["bots[0].respond_to"]);
}

#[test]
fn rejects_a_bot_pubkey_that_is_not_64_hex_characters() {
    let valid = fixture_pubkey("alpha-key");
    let too_short = valid[..63].to_string();
    let too_long = format!("{valid}0");
    let not_hex = format!("z{}", &valid[1..]);

    for bad in [
        "abc123",
        too_short.as_str(),
        too_long.as_str(),
        not_hex.as_str(),
    ] {
        let source = edit(&base_roster(), &valid, bad);

        assert_eq!(
            roster_error_paths(&source),
            ["bots[0].pubkey"],
            "pubkey {bad:?} should be rejected"
        );
    }
}

#[test]
fn rejects_an_owner_pubkey_that_is_not_64_hex_characters() {
    let source = edit(&base_roster(), &fixture_pubkey("owner-2"), "abc123");

    assert_eq!(roster_error_paths(&source), ["owner.pubkeys[1]"]);
}

#[test]
fn rejects_a_channel_entry_that_is_neither_star_nor_a_uuid() {
    let source = edit(
        &base_roster(),
        &format!("channels = [\"{}\"]", fixture_uuid("channel-1")),
        "channels = [\"general\"]",
    );

    let paths = roster_error_paths(&source);

    assert!(
        !paths.is_empty()
            && paths
                .iter()
                .all(|path| path.starts_with("bots[1].channels")),
        "expected only issues at or under bots[1].channels, got {paths:?}"
    );
}

#[test]
fn rejects_an_empty_or_blank_bot_name() {
    for bad in ["", "   "] {
        let source = edit(
            &base_roster(),
            "name = \"beta-bot\"",
            &format!("name = \"{bad}\""),
        );

        assert_eq!(
            roster_error_paths(&source),
            ["bots[1].name"],
            "bot name {bad:?} should be rejected"
        );
    }
}

#[test]
fn rejects_a_roster_that_is_not_valid_toml_with_one_issue_at_the_empty_path() {
    assert_eq!(roster_error_paths("version = = 1"), [""]);
}

#[test]
fn a_roster_with_several_defects_reports_every_one_sorted_by_path() {
    let source = edit(&base_roster(), "version = 1", "version = 2");
    let source = edit(&source, "America/Chicago", "Mars/Olympus");
    let source = edit(&source, "turns_per_round = 4", "turns_per_rnd = 4");
    let source = edit(&source, "respond_to = \"owner-only\"\n", "");

    assert_eq!(
        roster_error_paths(&source),
        [
            "bots[0].respond_to",
            "limits.turns_per_rnd",
            "owner.timezone",
            "version"
        ],
        "structural and validation issues are collected together (D4)"
    );
}

#[test]
fn config_errors_expose_find_iteration_display_and_the_error_trait() {
    let source = edit(&base_roster(), "version = 1", "version = 2");
    let source = edit(&source, "America/Chicago", "Mars/Olympus");

    let errors: ConfigErrors = parse_roster(&source).expect_err("the roster should be rejected");

    let version: &ConfigIssue = errors.find("version").expect("an issue at version");
    assert_eq!(version.path, "version");
    assert!(errors.find("owner.timezone").is_some());
    assert!(errors.find("owner.name").is_none());
    assert_eq!(errors.0.len(), 2);
    assert_eq!(IntoIterator::into_iter(&errors).count(), errors.0.len());
    assert!(
        errors.0.windows(2).all(|pair| pair[0] <= pair[1]),
        "issues are sorted by (path, message)"
    );
    assert!(!errors.to_string().is_empty());
    let as_error: &dyn std::error::Error = &errors;
    assert!(!as_error.to_string().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Router fixtures
// ---------------------------------------------------------------------------------------------

/// The top of brief section 4.2, with every optional key written out empty.
const ROUTER_HEADER: &str = r#"relay_url = "wss://relay.invalid"
api_bind = "127.0.0.1:47821"
tailnet_bind = ""
public_url = ""
roster_path = "roster.toml"
"#;

/// A command bot with every optional key written out empty.
const ALPHA_COMMAND_BOT: &str = r#"
[[bots]]
name = "alpha-bot"
key = "keychain"
auth_tag = ""
max_concurrent = 1

[bots.adapter]
type = "command"
command = ["agent", "--text"]
cwd = "~/fixture/alpha"
env = {}
prompt_mode = "stdin"
reply_mode = "stdout"
prompt_template = ""
"#;

/// A sync webhook bot, which needs neither `public_url` nor `tailnet_bind`.
const BETA_WEBHOOK_BOT: &str = r#"
[[bots]]
name = "beta-bot"
key = "file:/fixture/beta.key"
max_concurrent = 2

[bots.adapter]
type = "webhook"
url = "https://routine.invalid/hook"
secret_env = "BETA_WEBHOOK_SECRET"
mode = "sync"
cancel_url = ""
"#;

/// Only the keys A2 makes required: every optional key is omitted.
const MINIMAL_ROUTER: &str = r#"relay_url = "wss://relay.invalid"

[[bots]]
name = "alpha-bot"
key = "keychain"

[bots.adapter]
type = "command"
command = ["agent"]
cwd = "."
prompt_mode = "file"
reply_mode = "api"
"#;

/// Every optional key given a non-empty value.
const FULL_ROUTER: &str = r#"relay_url = "wss://relay.invalid"
api_bind = "127.0.0.1:47822"
tailnet_bind = "100.64.0.1:47821"
public_url = "http://box.tailnet.invalid:47821/"
roster_path = "shared/roster.toml"

[[bots]]
name = "alpha-bot"
key = "keychain"
auth_tag = '<auth-tag>'
max_concurrent = 3

[bots.adapter]
type = "command"
command = ["agent", "--text"]
cwd = "~/fixture/alpha"
env = { ALPHA_MODE = "fixture" }
prompt_mode = "stdin"
reply_mode = "stdout"
prompt_template = "templates/alpha.txt"

[[bots]]
name = "beta-bot"
key = "file:/fixture/beta.key"

[bots.adapter]
type = "webhook"
url = "https://routine.invalid/hook"
secret_env = "BETA_WEBHOOK_SECRET"
mode = "async"
cancel_url = "https://routine.invalid/cancel"
"#;

fn base_router() -> String {
    format!("{ROUTER_HEADER}{ALPHA_COMMAND_BOT}{BETA_WEBHOOK_BOT}")
}

/// A structurally valid NIP-OA tag with a fabricated signature. `parse_auth_tag` checks the
/// structure only, so the router config accepts it.
fn fixture_auth_tag_json() -> String {
    let signature = format!(
        "{}{}",
        hex::encode(fixture_digest("auth-signature-1")),
        hex::encode(fixture_digest("auth-signature-2"))
    );
    format!(
        "[\"auth\",\"{}\",\"\",\"{signature}\"]",
        fixture_pubkey("auth-owner")
    )
}

fn parsed(source: &str) -> RouterConfig {
    parse_router(source, &parsed_base_roster()).expect("the router config is valid")
}

/// The issue paths of a rejected router config, in the order `ConfigErrors` reports them.
fn router_error_paths(source: &str) -> Vec<String> {
    let errors = parse_router(source, &parsed_base_roster())
        .expect_err("the router config should be rejected");
    errors.iter().map(|issue| issue.path.clone()).collect()
}

fn alpha_command_adapter() -> AdapterConfig {
    AdapterConfig::Command {
        command: vec!["agent".to_string(), "--text".to_string()],
        cwd: "~/fixture/alpha".to_string(),
        env: BTreeMap::new(),
        prompt_mode: PromptMode::Stdin,
        reply_mode: ReplyMode::Stdout,
        prompt_template: None,
    }
}

// ---------------------------------------------------------------------------------------------
// Router: valid input and defaults
// ---------------------------------------------------------------------------------------------

#[test]
fn router_defaults_apply_when_the_optional_keys_are_omitted() {
    let config = parsed(MINIMAL_ROUTER);

    assert_eq!(config.api_bind.to_string(), "127.0.0.1:47821");
    assert_eq!(config.bots["alpha-bot"].max_concurrent, 1);
    assert_eq!(config.roster_path, PathBuf::from("roster.toml"));
    assert_eq!(config.tailnet_bind, None);
    assert_eq!(config.public_url, None);
    assert_eq!(config.relay_url, "wss://relay.invalid");
    assert_eq!(config.bots["alpha-bot"].key, KeySource::Keychain);
    assert_eq!(config.bots["alpha-bot"].auth_tag, None);
    assert_eq!(
        config.bots["alpha-bot"].adapter,
        AdapterConfig::Command {
            command: vec!["agent".to_string()],
            cwd: ".".to_string(),
            env: BTreeMap::new(),
            prompt_mode: PromptMode::File,
            reply_mode: ReplyMode::Api,
            prompt_template: None,
        }
    );
}

#[test]
fn router_resolves_the_brief_section_4_2_shapes_for_a_command_bot_and_a_sync_webhook_bot() {
    let config = parsed(&base_router());

    assert_eq!(config.api_bind, SocketAddr::from(([127, 0, 0, 1], 47821)));
    assert_eq!(config.roster_path, PathBuf::from("roster.toml"));
    assert_eq!(config.bots["alpha-bot"].key, KeySource::Keychain);
    assert_eq!(config.bots["alpha-bot"].max_concurrent, 1);
    assert_eq!(
        config.bots["alpha-bot"].adapter,
        alpha_command_adapter(),
        "cwd stays exactly as written; home expansion belongs to the daemon"
    );
    assert_eq!(
        config.bots["beta-bot"].key,
        KeySource::File(PathBuf::from("/fixture/beta.key"))
    );
    assert_eq!(config.bots["beta-bot"].max_concurrent, 2);
    assert_eq!(
        config.bots["beta-bot"].adapter,
        AdapterConfig::Webhook {
            url: "https://routine.invalid/hook".to_string(),
            secret_env: "BETA_WEBHOOK_SECRET".to_string(),
            mode: WebhookMode::Sync,
            cancel_url: None,
        }
    );
}

#[test]
fn router_empty_strings_in_optional_fields_become_none() {
    let config = parsed(&base_router());

    assert_eq!(config.tailnet_bind, None);
    assert_eq!(config.public_url, None);
    assert_eq!(config.bots["alpha-bot"].auth_tag, None);
    assert!(matches!(
        &config.bots["alpha-bot"].adapter,
        AdapterConfig::Command {
            prompt_template: None,
            ..
        }
    ));
    assert!(matches!(
        &config.bots["beta-bot"].adapter,
        AdapterConfig::Webhook {
            cancel_url: None,
            ..
        }
    ));
}

#[test]
fn router_keeps_every_optional_value_that_is_given() {
    let auth_tag_json = fixture_auth_tag_json();
    let source = FULL_ROUTER.replace("<auth-tag>", &auth_tag_json);

    let config = parsed(&source);

    assert_eq!(config.api_bind.to_string(), "127.0.0.1:47822");
    assert_eq!(
        config.tailnet_bind,
        Some(SocketAddr::from(([100, 64, 0, 1], 47821)))
    );
    assert_eq!(
        config.public_url.as_deref(),
        Some("http://box.tailnet.invalid:47821"),
        "a trailing slash is dropped"
    );
    assert_eq!(config.roster_path, PathBuf::from("shared/roster.toml"));
    assert_eq!(config.bots["alpha-bot"].max_concurrent, 3);
    assert_eq!(
        config.bots["alpha-bot"].auth_tag,
        Some(parse_auth_tag(&auth_tag_json).expect("the fixture auth tag is well formed")),
        "auth_tag is parsed with buzz_sdk::nip_oa::parse_auth_tag (R1.9)"
    );
    assert_eq!(
        config.bots["alpha-bot"].adapter,
        AdapterConfig::Command {
            command: vec!["agent".to_string(), "--text".to_string()],
            cwd: "~/fixture/alpha".to_string(),
            env: BTreeMap::from([("ALPHA_MODE".to_string(), "fixture".to_string())]),
            prompt_mode: PromptMode::Stdin,
            reply_mode: ReplyMode::Stdout,
            prompt_template: Some(PathBuf::from("templates/alpha.txt")),
        }
    );
    assert_eq!(config.bots["beta-bot"].max_concurrent, 1);
    assert_eq!(
        config.bots["beta-bot"].adapter,
        AdapterConfig::Webhook {
            url: "https://routine.invalid/hook".to_string(),
            secret_env: "BETA_WEBHOOK_SECRET".to_string(),
            mode: WebhookMode::Async,
            cancel_url: Some("https://routine.invalid/cancel".to_string()),
        }
    );
}

#[test]
fn router_accepts_key_file_with_a_path() {
    let source = edit(&base_router(), "key = \"keychain\"", "key = \"file:/x\"");

    let config = parsed(&source);

    assert_eq!(
        config.bots["alpha-bot"].key,
        KeySource::File(PathBuf::from("/x"))
    );
}

#[test]
fn router_accepts_an_ipv6_loopback_api_bind() {
    let source = edit(
        &base_router(),
        "api_bind = \"127.0.0.1:47821\"",
        "api_bind = \"[::1]:47821\"",
    );

    let config = parsed(&source);

    assert!(config.api_bind.ip().is_loopback());
    assert_eq!(config.api_bind.port(), 47821);
}

#[test]
fn router_may_serve_a_subset_of_the_roster_and_the_local_bots_are_its_keys() {
    let source = format!("{ROUTER_HEADER}{BETA_WEBHOOK_BOT}");

    let config = parsed(&source);

    assert_eq!(
        config.bots.keys().map(BotName::as_str).collect::<Vec<_>>(),
        ["beta-bot"],
        "alpha-bot is in the roster but not served on this machine"
    );
}

#[test]
fn a_sync_webhook_bot_needs_neither_public_url_nor_tailnet_bind() {
    let config = parsed(&base_router());

    assert_eq!(config.tailnet_bind, None);
    assert_eq!(config.public_url, None);
    assert!(matches!(
        &config.bots["beta-bot"].adapter,
        AdapterConfig::Webhook {
            mode: WebhookMode::Sync,
            ..
        }
    ));
}

#[test]
fn router_roster_path_is_roster_toml_when_the_key_is_omitted() {
    let source = edit(&base_router(), "roster_path = \"roster.toml\"\n", "");

    assert_eq!(
        router_roster_path(&source).expect("the router config is valid"),
        PathBuf::from("roster.toml")
    );
}

#[test]
fn router_roster_path_is_returned_as_written() {
    let source = edit(
        &base_router(),
        "roster_path = \"roster.toml\"",
        "roster_path = \"shared/roster.toml\"",
    );

    assert_eq!(
        router_roster_path(&source).expect("the router config is valid"),
        PathBuf::from("shared/roster.toml")
    );
}

#[test]
fn router_roster_path_rejects_text_that_is_not_toml_with_one_issue_at_the_empty_path() {
    let errors = router_roster_path("roster_path = = 1").expect_err("not TOML");

    assert_eq!(
        errors
            .iter()
            .map(|issue| issue.path.as_str())
            .collect::<Vec<_>>(),
        [""]
    );
}

// ---------------------------------------------------------------------------------------------
// Router: rejections (one test each, asserting the issue path)
// ---------------------------------------------------------------------------------------------

#[test]
fn router_rejects_a_non_loopback_api_bind() {
    let source = edit(
        &base_router(),
        "api_bind = \"127.0.0.1:47821\"",
        "api_bind = \"0.0.0.0:47821\"",
    );

    assert_eq!(router_error_paths(&source), ["api_bind"]);
}

#[test]
fn router_rejects_a_wildcard_tailnet_bind() {
    let source = edit(
        &base_router(),
        "tailnet_bind = \"\"",
        "tailnet_bind = \"0.0.0.0:1\"",
    );

    assert_eq!(router_error_paths(&source), ["tailnet_bind"]);
}

#[test]
fn router_rejects_an_ipv6_wildcard_tailnet_bind() {
    let source = edit(
        &base_router(),
        "tailnet_bind = \"\"",
        "tailnet_bind = \"[::]:1\"",
    );

    assert_eq!(router_error_paths(&source), ["tailnet_bind"]);
}

#[test]
fn router_rejects_an_async_webhook_with_an_empty_public_url() {
    let source = edit(&base_router(), "mode = \"sync\"", "mode = \"async\"");
    let source = edit(
        &source,
        "tailnet_bind = \"\"",
        "tailnet_bind = \"100.64.0.1:47821\"",
    );

    assert_eq!(router_error_paths(&source), ["public_url"]);
}

#[test]
fn router_rejects_an_async_webhook_with_an_empty_tailnet_bind() {
    let source = edit(&base_router(), "mode = \"sync\"", "mode = \"async\"");
    let source = edit(
        &source,
        "public_url = \"\"",
        "public_url = \"http://box.tailnet.invalid:47821\"",
    );

    assert_eq!(router_error_paths(&source), ["tailnet_bind"]);
}

#[test]
fn router_reports_both_missing_settings_of_an_async_webhook() {
    let source = edit(&base_router(), "mode = \"sync\"", "mode = \"async\"");

    assert_eq!(router_error_paths(&source), ["public_url", "tailnet_bind"]);
}

#[test]
fn router_accepts_an_async_webhook_when_public_url_and_tailnet_bind_are_set() {
    let source = edit(&base_router(), "mode = \"sync\"", "mode = \"async\"");
    let source = edit(
        &source,
        "tailnet_bind = \"\"",
        "tailnet_bind = \"100.64.0.1:47821\"",
    );
    let source = edit(
        &source,
        "public_url = \"\"",
        "public_url = \"http://box.tailnet.invalid:47821\"",
    );

    let config = parsed(&source);

    assert!(matches!(
        &config.bots["beta-bot"].adapter,
        AdapterConfig::Webhook {
            mode: WebhookMode::Async,
            ..
        }
    ));
}

#[test]
fn router_rejects_key_vault() {
    let source = edit(&base_router(), "key = \"keychain\"", "key = \"vault\"");

    assert_eq!(router_error_paths(&source), ["bots[0].key"]);
}

#[test]
fn router_rejects_a_file_key_with_an_empty_path() {
    let source = edit(&base_router(), "key = \"keychain\"", "key = \"file:\"");

    assert_eq!(router_error_paths(&source), ["bots[0].key"]);
}

#[test]
fn router_rejects_a_bot_name_missing_from_the_roster() {
    let source = edit(
        &base_router(),
        "name = \"alpha-bot\"",
        "name = \"ghost-bot\"",
    );

    assert_eq!(router_error_paths(&source), ["bots[0].name"]);
}

#[test]
fn router_rejects_a_bot_name_that_differs_from_the_roster_name_in_case() {
    let source = edit(
        &base_router(),
        "name = \"alpha-bot\"",
        "name = \"Alpha-Bot\"",
    );

    assert_eq!(router_error_paths(&source), ["bots[0].name"]);
}

#[test]
fn router_rejects_prompt_mode_pipe() {
    let source = edit(
        &base_router(),
        "prompt_mode = \"stdin\"",
        "prompt_mode = \"pipe\"",
    );

    assert_eq!(router_error_paths(&source), ["bots[0].adapter.prompt_mode"]);
}

#[test]
fn router_rejects_an_auth_tag_that_is_not_a_nip_oa_tag() {
    let source = edit(&base_router(), "auth_tag = \"\"", "auth_tag = 'not a tag'");

    assert_eq!(router_error_paths(&source), ["bots[0].auth_tag"]);
}

#[test]
fn router_rejects_an_unknown_key() {
    let source = edit(&base_router(), "max_concurrent = 1", "max_concurent = 1");

    assert_eq!(router_error_paths(&source), ["bots[0].max_concurent"]);
}

#[test]
fn router_rejects_a_missing_relay_url() {
    let source = edit(&base_router(), "relay_url = \"wss://relay.invalid\"\n", "");

    assert_eq!(router_error_paths(&source), ["relay_url"]);
}

#[test]
fn router_rejects_a_missing_key() {
    let source = edit(&base_router(), "key = \"keychain\"\n", "");

    assert_eq!(router_error_paths(&source), ["bots[0].key"]);
}

#[test]
fn router_rejects_text_that_is_not_valid_toml_with_one_issue_at_the_empty_path() {
    assert_eq!(router_error_paths("relay_url = = 1"), [""]);
}

#[test]
fn a_router_file_with_three_errors_reports_three_issues() {
    let source = edit(
        &base_router(),
        "api_bind = \"127.0.0.1:47821\"",
        "api_bind = \"0.0.0.0:47821\"",
    );
    let source = edit(&source, "key = \"keychain\"", "key = \"vault\"");
    let source = edit(&source, "name = \"beta-bot\"", "name = \"ghost-bot\"");

    let paths = router_error_paths(&source);

    assert_eq!(paths.len(), 3, "got {paths:?}");
    assert_eq!(paths, ["api_bind", "bots[0].key", "bots[1].name"]);
}

#[test]
fn router_collects_structural_and_validation_issues_together() {
    let source = edit(
        &base_router(),
        "prompt_mode = \"stdin\"",
        "prompt_mode = \"pipe\"",
    );
    let source = edit(
        &source,
        "api_bind = \"127.0.0.1:47821\"",
        "api_bind = \"0.0.0.0:47821\"",
    );
    let source = edit(&source, "name = \"beta-bot\"", "name = \"ghost-bot\"");

    assert_eq!(
        router_error_paths(&source),
        ["api_bind", "bots[0].adapter.prompt_mode", "bots[1].name"],
        "structural and validation issues are collected together (D4)"
    );
}

// ---------------------------------------------------------------------------------------------
// roster_hash
// ---------------------------------------------------------------------------------------------

#[test]
fn roster_hash_of_abc_is_the_sha256_test_vector() {
    assert_eq!(
        roster_hash(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn roster_hash_of_nothing_is_the_sha256_of_the_empty_input() {
    assert_eq!(
        roster_hash(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn roster_hash_is_the_lowercase_hex_sha256_of_the_exact_bytes() {
    let bytes = base_roster().into_bytes();

    let hash = roster_hash(&bytes);

    assert_eq!(hash, hex::encode(Sha256::digest(&bytes)));
    assert_eq!(hash.len(), 64);
    assert_eq!(hash, hash.to_ascii_lowercase());
    assert_ne!(
        hash,
        roster_hash(&[bytes.as_slice(), b"\n"].concat()),
        "one more byte changes the hash"
    );
}

// ---------------------------------------------------------------------------------------------
// Identifiers (decision record D1)
// ---------------------------------------------------------------------------------------------

#[test]
fn pubkey_from_hex_accepts_any_case_and_stores_lowercase() {
    let lower = fixture_pubkey("owner-1");

    let from_upper = Pubkey::from_hex(&lower.to_ascii_uppercase()).expect("any-case hex");

    assert_eq!(from_upper.as_str(), lower);
    assert_eq!(from_upper, Pubkey::from_hex(&lower).expect("lowercase hex"));
}

#[test]
fn pubkey_and_event_id_reject_anything_that_is_not_64_hex_characters() {
    let valid = fixture_pubkey("owner-1");
    let too_short = valid[..63].to_string();
    let too_long = format!("{valid}0");
    let not_hex = format!("g{}", &valid[1..]);

    for bad in [
        "",
        "abc123",
        too_short.as_str(),
        too_long.as_str(),
        not_hex.as_str(),
    ] {
        assert!(
            matches!(Pubkey::from_hex(bad), Err(IdError::NotHex64(_))),
            "Pubkey::from_hex({bad:?}) should be NotHex64"
        );
        assert!(
            matches!(EventId::from_hex(bad), Err(IdError::NotHex64(_))),
            "EventId::from_hex({bad:?}) should be NotHex64"
        );
    }
}

#[test]
fn event_id_from_hex_stores_lowercase_and_round_trips_a_nostr_event_id() {
    let hex_id = hex::encode(fixture_digest("event-1"));

    let id = EventId::from_hex(&hex_id.to_ascii_uppercase()).expect("any-case hex");

    assert_eq!(id.as_str(), hex_id);
    let nostr_id = nostr::EventId::from_hex(&hex_id).expect("a fixture event id");
    assert_eq!(EventId::from_nostr(&nostr_id), id);
}

#[test]
fn pubkey_round_trips_a_nostr_public_key() {
    let nostr_key = fixture_keys("owner-1").public_key();

    let key = Pubkey::from_nostr(&nostr_key);

    assert_eq!(key.as_str(), nostr_key.to_hex());
    assert_eq!(
        key.to_nostr().expect("a fixture key is a nostr key"),
        nostr_key
    );
}

#[test]
fn bot_name_rejects_empty_and_whitespace_only_names() {
    for bad in ["", " ", "\t\n"] {
        assert_eq!(
            BotName::new(bad),
            Err(IdError::EmptyName),
            "bot name {bad:?} should be rejected"
        );
    }
    assert_eq!(
        BotName::new("Hal.macbook-pro")
            .expect("a plain name")
            .as_str(),
        "Hal.macbook-pro"
    );
}

#[test]
fn channel_id_parses_a_uuid_and_rejects_anything_else() {
    let text = fixture_uuid("channel-1");

    let id = ChannelId::parse(&text).expect("a fixture UUID");

    assert_eq!(id.uuid().to_string(), text);
    assert_eq!(ChannelId::from(id.uuid()), id);
    assert!(matches!(
        ChannelId::parse("not-a-uuid"),
        Err(IdError::NotUuid(_))
    ));
}

#[test]
fn ids_display_and_from_str_round_trip() {
    let key = pubkey("owner-1");
    let id = EventId::from_hex(&hex::encode(fixture_digest("event-1"))).expect("hex id");
    let channel_id = channel("channel-1");
    let name = bot_name("alpha-bot");

    assert_eq!(key.to_string().parse::<Pubkey>(), Ok(key.clone()));
    assert_eq!(id.to_string().parse::<EventId>(), Ok(id.clone()));
    assert_eq!(channel_id.to_string().parse::<ChannelId>(), Ok(channel_id));
    assert_eq!(name.to_string().parse::<BotName>(), Ok(name.clone()));
    assert!(matches!(
        "nope".parse::<Pubkey>(),
        Err(IdError::NotHex64(_))
    ));
}

#[test]
fn ids_serialise_as_plain_strings_and_validate_when_deserialised() {
    let key = pubkey("owner-1");
    let channel_id = channel("channel-1");

    assert_eq!(
        serde_json::to_string(&key).expect("serialise"),
        format!("\"{}\"", key.as_str())
    );
    assert_eq!(
        serde_json::to_string(&channel_id).expect("serialise"),
        format!("\"{}\"", fixture_uuid("channel-1"))
    );
    assert_eq!(
        serde_json::from_str::<Pubkey>(&format!("\"{}\"", key.as_str())).expect("deserialise"),
        key
    );
    assert!(serde_json::from_str::<Pubkey>("\"abc\"").is_err());
    assert!(serde_json::from_str::<EventId>("\"abc\"").is_err());
    assert!(serde_json::from_str::<ChannelId>("\"not-a-uuid\"").is_err());
    assert!(serde_json::from_str::<BotName>("\"\"").is_err());
    assert_eq!(
        serde_json::from_str::<BotName>("\"alpha-bot\"").expect("deserialise"),
        bot_name("alpha-bot")
    );
}
