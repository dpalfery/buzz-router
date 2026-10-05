//! Task 1.2 (RED): the placeholder example files `roster.example.toml` and
//! `router.example.toml` at the repository root (R1.16, R2.17; brief section 4).
//!
//! Two kinds of check:
//!
//! 1. The files hold placeholders only: no 64-hex literal, no `nsec1`, no `npub1`, and no
//!    `wss://` host other than `<relay-host>`.
//! 2. After every `<...>` placeholder is replaced with a deterministic fake value, the roster
//!    example parses and validates, and the router example validates against it.
//!
//! # How placeholders are filled
//!
//! A placeholder is `<` followed by letters, digits, `.`, `_` or `-`, then `>`. The fake value
//! depends on the TOML key on the same line and on the dash-separated words of the placeholder
//! name, so the example files may name their placeholders freely:
//!
//! - the TOML key is `pubkey` or `pubkeys`, or a word is `hex` or `pubkey`: a unique 64-hex string;
//! - the TOML key is `id`, or a word is `uuid`, `guid` or `id`: a UUID;
//! - a word is `ip`, `ipv4`, `addr` or `address`: a tailnet IPv4 address;
//! - a word is `port`: `47821`;
//! - anything else: `fixture-<n>`.
//!
//! When a placeholder has both an `ip` word and a `port` word, the value is `ip:port`. Each
//! occurrence gets its own value, derived from `sha256("buzz-router-fixture:" + ...)`, so two
//! `<hex>` placeholders never collide. The router example is validated against the roster
//! example, so every bot it names must exist in the roster example, and an async webhook bot
//! needs non-empty `public_url` and `tailnet_bind` values (A3).

#![allow(
    clippy::expect_used,
    reason = "helpers in an integration-test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::{Captures, Regex};
use router_core::config::{parse_roster, parse_router};
use sha2::{Digest, Sha256};

const ROSTER_EXAMPLE: &str = "roster.example.toml";
const ROUTER_EXAMPLE: &str = "router.example.toml";

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<[A-Za-z0-9._-]+>").expect("the placeholder pattern is a valid regex")
});

static HEX_64: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9a-fA-F]{64}").expect("the hex pattern is a valid regex"));

static WSS_HOST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"wss://([^\s"'/:]+)"#).expect("the wss pattern is a valid regex")
});

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_example(name: &str) -> String {
    let path = repository_root().join(name);
    std::fs::read_to_string(&path).unwrap_or_default()
}

fn fixture_digest(name: &str) -> [u8; 32] {
    Sha256::digest(format!("buzz-router-fixture:{name}")).into()
}

/// The fake value for one placeholder occurrence. `key` is the TOML key on the placeholder's
/// line (empty when the line has no `=`), `name` is the text between the angle brackets, and
/// `index` is the occurrence number within the file.
fn fake_value(key: &str, name: &str, index: usize) -> String {
    let words: Vec<String> = name
        .split(['-', '_', '.'])
        .map(str::to_ascii_lowercase)
        .collect();
    let has_word = |wanted: &[&str]| words.iter().any(|word| wanted.contains(&word.as_str()));
    let digest = fixture_digest(&format!("example:{index}:{name}"));

    if matches!(key, "pubkey" | "pubkeys") || has_word(&["hex", "pubkey"]) {
        return hex::encode(digest);
    }
    if key == "id" || has_word(&["uuid", "guid", "id"]) {
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        return uuid::Uuid::from_bytes(bytes).to_string();
    }
    let ip = format!("100.64.{}.{}", (index / 250) % 250, index % 250 + 1);
    match (
        has_word(&["ip", "ipv4", "addr", "address"]),
        has_word(&["port"]),
    ) {
        (true, true) => format!("{ip}:47821"),
        (true, false) => ip,
        (false, true) => "47821".to_string(),
        (false, false) => format!("fixture-{index}"),
    }
}

/// Replaces every `<...>` placeholder in `text` with a deterministic fake value.
fn fill_placeholders(text: &str) -> String {
    let mut index = 0_usize;
    let filled = PLACEHOLDER.replace_all(text, |captures: &Captures<'_>| {
        let found = captures.get(0).map_or(0..0, |whole| whole.range());
        let line_start = text[..found.start]
            .rfind('\n')
            .map_or(0, |newline| newline + 1);
        let key = text[line_start..found.start]
            .split_once('=')
            .map_or("", |(key, _)| key.trim());
        let name = &text[found.start + 1..found.end - 1];
        let value = fake_value(key, name, index);
        index += 1;
        value
    });
    filled.into_owned()
}

// ---------------------------------------------------------------------------------------------
// The files exist and hold placeholders only
// ---------------------------------------------------------------------------------------------

#[test]
fn both_example_files_exist_at_the_repository_root() {
    for name in [ROSTER_EXAMPLE, ROUTER_EXAMPLE] {
        let path = repository_root().join(name);

        assert!(path.is_file(), "{} should exist", path.display());
        assert!(
            !read_example(name).trim().is_empty(),
            "{name} should not be empty"
        );
    }
}

#[test]
fn neither_example_file_contains_a_64_hex_literal() {
    for name in [ROSTER_EXAMPLE, ROUTER_EXAMPLE] {
        let text = read_example(name);

        assert!(!text.is_empty(), "{name} should exist and not be empty");
        assert!(
            !HEX_64.is_match(&text),
            "{name} contains a 64-hex literal; use a <placeholder>"
        );
    }
}

#[test]
fn neither_example_file_contains_nsec1_or_npub1() {
    for name in [ROSTER_EXAMPLE, ROUTER_EXAMPLE] {
        let text = read_example(name);

        assert!(!text.is_empty(), "{name} should exist and not be empty");
        assert!(!text.contains("nsec1"), "{name} contains an nsec1 key");
        assert!(!text.contains("npub1"), "{name} contains an npub1 key");
    }
}

#[test]
fn neither_example_file_names_a_wss_host_other_than_the_relay_host_placeholder() {
    for name in [ROSTER_EXAMPLE, ROUTER_EXAMPLE] {
        let text = read_example(name);

        assert!(!text.is_empty(), "{name} should exist and not be empty");
        for found in WSS_HOST.captures_iter(&text) {
            assert_eq!(
                &found[1], "<relay-host>",
                "{name} names the relay host {:?}; use <relay-host>",
                &found[1]
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The files parse and validate once the placeholders are replaced
// ---------------------------------------------------------------------------------------------

#[test]
fn placeholder_filling_leaves_no_placeholder_and_gives_each_hex_its_own_value() {
    let filled = fill_placeholders(
        "pubkeys = [\"<a-hex>\", \"<b-hex>\"]\nid = \"<c>\"\nbind = \"<tailnet-ip>:<port>\"\n",
    );

    assert!(!PLACEHOLDER.is_match(&filled), "left over: {filled}");
    let hex_values: Vec<&str> = HEX_64
        .find_iter(&filled)
        .map(|found| found.as_str())
        .collect();
    assert_eq!(hex_values.len(), 2);
    assert_ne!(hex_values[0], hex_values[1]);
    assert!(filled.contains(":47821"));
}

#[test]
fn the_roster_example_parses_and_validates_once_placeholders_are_replaced() {
    let text = read_example(ROSTER_EXAMPLE);
    assert!(
        !text.is_empty(),
        "{ROSTER_EXAMPLE} should exist and not be empty"
    );

    let result = parse_roster(&fill_placeholders(&text));

    let roster = result.expect("the roster example is a valid roster");
    assert!(!roster.owner.pubkeys.is_empty());
    assert!(!roster.bots.is_empty());
}

#[test]
fn the_router_example_validates_against_the_roster_example_once_placeholders_are_replaced() {
    let roster_text = read_example(ROSTER_EXAMPLE);
    let router_text = read_example(ROUTER_EXAMPLE);
    assert!(
        !roster_text.is_empty(),
        "{ROSTER_EXAMPLE} should exist and not be empty"
    );
    assert!(
        !router_text.is_empty(),
        "{ROUTER_EXAMPLE} should exist and not be empty"
    );
    let roster = parse_roster(&fill_placeholders(&roster_text))
        .expect("the roster example is a valid roster");

    let result = parse_router(&fill_placeholders(&router_text), &roster);

    let config = result.expect("the router example is a valid router config");
    assert!(!config.bots.is_empty());
    assert!(config.relay_url.starts_with("wss://"));
}
