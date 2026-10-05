//! Task 1.5 (RED): quiet hours.
//!
//! Covers R22.1, R22.2 and assumption A1 against design section 5.6:
//! `quiet_set(&Roster, DateTime<Utc>) -> BTreeSet<BotName>` returns every bot whose effective
//! `quiet_hours` range contains the wall-clock time of `now` in `owner.timezone`, using the
//! half-open interval `[start, end)`:
//!
//! - `start < end`: quiet when `start <= t < end`;
//! - `start > end`: the range wraps past midnight, quiet when `t >= start || t < end`;
//! - `start == end`: the interval is empty, so never quiet.
//!
//! The roster is built through `parse_roster` with the owner timezone `America/Chicago` and the
//! bots `A`, `B` and `C`. Unless a test says otherwise `quiet_hours` is omitted, so every bot has
//! the R2.5 default range `23:00-07:00`. Every key is a deterministic fixture key,
//! `sha256("buzz-router-fixture:" + name)`.
//!
//! Each instant in a comment is given in UTC and in `America/Chicago` local time: July is on
//! daylight time (UTC-5) and January on standard time (UTC-6).
//!
//! Beyond the five instants and the three "also" cases of the task's Test contract, this file
//! adds cases that the same design section states and the contract's table leaves out:
//!
//! - one more January instant, which a fixed UTC-5 offset would misread (`daylight_offset_is_not_applied_in_winter`);
//! - a non-wrapping per-bot range, which exercises the `start < end` branch and its half-open end;
//! - an empty `quiet_hours` set on one bot (R22.2 says "a bot's"), next to the same setting in `[limits]`.

#![allow(
    clippy::expect_used,
    reason = "helpers in an integration-test crate fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod common;

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use common::{bot_set, pubkey_hex};
use router_core::config::{parse_roster, Roster};
use router_core::ids::BotName;
use router_core::quiet::quiet_set;

/// A roster in `America/Chicago` with bots `A`, `B` and `C`, each covering every channel.
///
/// `global_quiet` is the `quiet_hours` of the `[limits]` table, or `None` to omit the table so
/// the default range applies. `bot_quiet` lists `(bot, range)` pairs: each gives that bot a
/// `limits = { quiet_hours = ".." }` override.
fn chicago_roster(global_quiet: Option<&str>, bot_quiet: &[(&str, &str)]) -> Roster {
    let mut source = format!(
        r#"version = 1

[owner]
name = "Fixture Owner"
pubkeys = ["{owner}"]
timezone = "America/Chicago"
"#,
        owner = pubkey_hex("O"),
    );
    if let Some(range) = global_quiet {
        source.push_str(&format!("\n[limits]\nquiet_hours = \"{range}\"\n"));
    }
    for name in ["A", "B", "C"] {
        let limits = bot_quiet
            .iter()
            .find(|(bot, _)| *bot == name)
            .map_or_else(String::new, |(_, range)| {
                format!("limits = {{ quiet_hours = \"{range}\" }}\n")
            });
        source.push_str(&format!(
            r#"
[[bots]]
name = "{name}"
pubkey = "{pubkey}"
channels = ["*"]
respond_to = "owner-only"
{limits}"#,
            pubkey = pubkey_hex(name),
        ));
    }
    parse_roster(&source).expect("the fixture roster is valid")
}

/// The roster whose bots all have the default range `23:00-07:00`.
fn default_roster() -> Roster {
    chicago_roster(None, &[])
}

/// An RFC 3339 instant such as `2026-07-01T04:00:00Z`.
fn utc(instant: &str) -> DateTime<Utc> {
    instant
        .parse()
        .expect("the fixture instant is an RFC 3339 UTC time")
}

/// The bots of `roster` that are quiet at `instant`.
fn quiet_at(roster: &Roster, instant: &str) -> BTreeSet<BotName> {
    quiet_set(roster, utc(instant))
}

/// Every fixture bot.
fn everyone() -> BTreeSet<BotName> {
    bot_set(&["A", "B", "C"])
}

/// No bot.
fn nobody() -> BTreeSet<BotName> {
    BTreeSet::new()
}

// ---------------------------------------------------------------------------------------------
// The contract's table: America/Chicago, default range 23:00-07:00
// ---------------------------------------------------------------------------------------------

/// 2026-07-01T03:59:59Z is 22:59:59 local, one second before the range starts. In UTC this
/// instant would be quiet, so the case also fails an implementation that ignores the timezone.
#[test]
fn local_22_59_59_is_not_quiet() {
    assert_eq!(
        quiet_at(&default_roster(), "2026-07-01T03:59:59Z"),
        nobody()
    );
}

/// 2026-07-01T04:00:00Z is 23:00 local: the start is inside the interval (A1).
#[test]
fn local_23_00_is_quiet() {
    assert_eq!(
        quiet_at(&default_roster(), "2026-07-01T04:00:00Z"),
        everyone()
    );
}

/// 2026-07-01T11:59:59Z is 06:59:59 local, one second before the range ends.
#[test]
fn local_06_59_59_is_quiet() {
    assert_eq!(
        quiet_at(&default_roster(), "2026-07-01T11:59:59Z"),
        everyone()
    );
}

/// 2026-07-01T12:00:00Z is 07:00 local: the end is outside the interval (A1).
#[test]
fn local_07_00_is_not_quiet() {
    assert_eq!(
        quiet_at(&default_roster(), "2026-07-01T12:00:00Z"),
        nobody()
    );
}

/// 2026-01-15T05:00:00Z is 23:00 CST, in winter.
#[test]
fn winter_23_00_local_is_quiet() {
    assert_eq!(
        quiet_at(&default_roster(), "2026-01-15T05:00:00Z"),
        everyone()
    );
}

/// 2026-01-15T04:59:59Z is 22:59:59 CST. Read with the July offset (UTC-5) it would be
/// 23:59:59 and quiet, so this fails an implementation that fixes the offset instead of using
/// the zone's rules for the date. Not one of the contract's five instants.
#[test]
fn daylight_offset_is_not_applied_in_winter() {
    assert_eq!(
        quiet_at(&default_roster(), "2026-01-15T04:59:59Z"),
        nobody()
    );
}

// ---------------------------------------------------------------------------------------------
// Never quiet
// ---------------------------------------------------------------------------------------------

/// `quiet_hours = ""` in `[limits]` disables quiet hours (R22.2). The instants are inside the
/// default range, 23:00, 04:00 and 06:59:59 local.
#[test]
fn an_empty_quiet_hours_means_never_quiet() {
    let roster = chicago_roster(Some(""), &[]);
    for instant in [
        "2026-07-01T04:00:00Z",
        "2026-07-01T09:00:00Z",
        "2026-07-01T11:59:59Z",
    ] {
        assert_eq!(quiet_at(&roster, instant), nobody(), "at {instant}");
    }
}

/// `quiet_hours = "12:00-12:00"` is an empty interval, so never quiet (A1, design 5.6). The
/// instants are 12:00 local itself, a second either side of it, 23:00 and 00:00 local. An
/// implementation that treats `start == end` as a wrap past midnight would call all of them
/// quiet.
#[test]
fn equal_start_and_end_means_never_quiet() {
    let roster = chicago_roster(Some("12:00-12:00"), &[]);
    for instant in [
        "2026-07-01T16:59:59Z",
        "2026-07-01T17:00:00Z",
        "2026-07-01T17:00:01Z",
        "2026-07-01T04:00:00Z",
        "2026-07-01T05:00:00Z",
    ] {
        assert_eq!(quiet_at(&roster, instant), nobody(), "at {instant}");
    }
}

// ---------------------------------------------------------------------------------------------
// A per-bot override applies to that bot only
// ---------------------------------------------------------------------------------------------

/// `C` has its own range, `12:00-13:00`; `A` and `B` keep the default. At 23:00 local the
/// default range is quiet and `C`'s is not; at 12:00 local it is the other way round.
#[test]
fn a_per_bot_override_applies_to_that_bot_only() {
    let roster = chicago_roster(None, &[("C", "12:00-13:00")]);

    assert_eq!(
        quiet_at(&roster, "2026-07-01T04:00:00Z"),
        bot_set(&["A", "B"])
    );
    assert_eq!(quiet_at(&roster, "2026-07-01T17:00:00Z"), bot_set(&["C"]));
}

/// A per-bot range that does not wrap (`start < end`) is also read as `[start, end)`: 11:59:59
/// local is before it, 12:00 and 12:59:59 are inside, 13:00 is outside.
#[test]
fn a_per_bot_range_that_does_not_wrap_is_half_open() {
    let roster = chicago_roster(None, &[("C", "12:00-13:00")]);

    for (instant, expected) in [
        ("2026-07-01T16:59:59Z", nobody()),
        ("2026-07-01T17:00:00Z", bot_set(&["C"])),
        ("2026-07-01T17:59:59Z", bot_set(&["C"])),
        ("2026-07-01T18:00:00Z", nobody()),
    ] {
        assert_eq!(quiet_at(&roster, instant), expected, "at {instant}");
    }
}

/// `A` has `quiet_hours = ""`; `B` and `C` keep the default. At 23:00 local only `B` and `C`
/// are quiet (R22.2: "a bot's `quiet_hours` is `\"\"`").
#[test]
fn an_empty_per_bot_quiet_hours_disables_quiet_hours_for_that_bot_only() {
    let roster = chicago_roster(None, &[("A", "")]);

    assert_eq!(
        quiet_at(&roster, "2026-07-01T04:00:00Z"),
        bot_set(&["B", "C"])
    );
}
