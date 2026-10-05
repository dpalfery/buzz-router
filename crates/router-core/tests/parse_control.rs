//! Task 1.4 (RED): control-command parsing.
//!
//! Covers R29.2 to R29.6 (and the CONFORM rows 17 to 20 that name them) against design section
//! 5.4: `parse_control(content, roster, mentioned) -> Option<Control>`.
//!
//! 1. `s = strip_code_regions(content)`.
//! 2. Replace `@everyone`, every roster `@name` and alias, and every `nostr:` URI with spaces.
//!    A name is tried longest first, case-insensitively, after an `@` that starts the text or
//!    follows whitespace, and must end at a word boundary.
//! 3. Lowercase, then turn every character that is neither alphanumeric nor `!` into a space.
//! 4. Split into words.
//! 5. One word `!cancel` is `Cancel`; one word `resume` is `Resume`; one to five words, one of
//!    them `stop`, `halt` or `!shutdown`, is `Stop`. Otherwise there is no command.
//! 6. The scope is `Scope::All` when `mentioned` is empty, otherwise `Scope::Bots(mentioned)`.
//!
//! `mentioned` is the set `mentioned_bots` computed for the message, so these tests pass it in
//! directly: the scope depends on it and nothing else.
//!
//! Rosters: `common::roster()` (bots `A`, `B`, `C`) for most cases, and `common::roster_with` for
//! the alias and the prefix-name cases. Every key is a deterministic fixture key. The `nostr:` URIs
//! are built in the tests from fixture keys.

mod common;

use std::collections::BTreeSet;

use common::{bot_set, nprofile, npub, roster, roster_with};
use router_core::ids::BotName;
use router_core::parse::parse_control;
use router_core::route::{Control, Scope};

/// The control command in `content` when no bot is mentioned.
fn control_of(content: &str) -> Option<Control> {
    parse_control(content, &roster(), &BTreeSet::new())
}

/// The control command in `content` when the message mentions `mentioned`.
fn control_mentioning(content: &str, mentioned: &[&str]) -> Option<Control> {
    parse_control(content, &roster(), &bot_set(mentioned))
}

fn stop_all() -> Option<Control> {
    Some(Control::Stop(Scope::All))
}

fn bots(names: &[&str]) -> Scope {
    let set: BTreeSet<BotName> = bot_set(names);
    Scope::Bots(set)
}

// ---------------------------------------------------------------------------------------------
// Stop to everyone: the contract's list (R29.3, R29.9, R29.10)
// ---------------------------------------------------------------------------------------------

#[test]
fn stop_alone_stops_everyone() {
    assert_eq!(control_of("stop"), stop_all());
}

#[test]
fn stop_it_stops_everyone() {
    assert_eq!(control_of("stop it"), stop_all());
}

#[test]
fn stop_with_a_trailing_bracket_stops_everyone() {
    // CONFORM case 18.
    assert_eq!(control_of("fucking stop["), stop_all());
}

#[test]
fn everyone_stop_stops_everyone() {
    // CONFORM case 17: `@everyone` is removed and is not a mention, so the scope is every bot.
    assert_eq!(control_of("@everyone stop"), stop_all());
}

#[test]
fn please_stop_now_stops_everyone() {
    assert_eq!(control_of("please stop now"), stop_all());
}

#[test]
fn stop_the_dev_server_stops_everyone() {
    assert_eq!(control_of("stop the dev server"), stop_all());
}

#[test]
fn shutdown_stops_everyone() {
    assert_eq!(control_of("!shutdown"), stop_all());
}

// ---------------------------------------------------------------------------------------------
// Stop: the other words and the word limit (R29.2, R29.3)
// ---------------------------------------------------------------------------------------------

#[test]
fn halt_stops_everyone() {
    assert_eq!(control_of("halt"), stop_all());
}

#[test]
fn the_words_are_lowercased_before_matching() {
    assert_eq!(control_of("STOP"), stop_all());
    assert_eq!(control_of("Please HALT now"), stop_all());
}

#[test]
fn five_words_containing_stop_are_a_stop() {
    assert_eq!(control_of("please stop the dev server"), stop_all());
}

#[test]
fn six_words_containing_stop_are_not_a_stop() {
    assert_eq!(control_of("please stop the dev server now"), None);
}

#[test]
fn stop_the_dev_server_and_restart_it_is_not_a_command() {
    // CONFORM case 20: eight words.
    assert_eq!(
        control_of("please stop the dev server and restart it"),
        None
    );
}

#[test]
fn an_empty_message_is_not_a_command() {
    assert_eq!(control_of(""), None);
    assert_eq!(control_of("  \n "), None);
}

#[test]
fn a_message_without_a_command_word_is_not_a_command() {
    assert_eq!(control_of("hello there"), None);
}

// ---------------------------------------------------------------------------------------------
// Code regions (R29.2 step 1)
// ---------------------------------------------------------------------------------------------

#[test]
fn stop_inside_a_code_span_is_not_a_command() {
    assert_eq!(control_of("`stop`"), None);
}

#[test]
fn stop_inside_a_fenced_block_is_not_a_command() {
    assert_eq!(control_of("```\nstop\n```"), None);
}

// ---------------------------------------------------------------------------------------------
// Scope (R29.6, CONFORM 19)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_stop_that_mentions_a_bot_is_scoped_to_it() {
    // CONFORM case 19.
    assert_eq!(
        control_mentioning("@A stop", &["A"]),
        Some(Control::Stop(bots(&["A"])))
    );
}

#[test]
fn a_stop_takes_its_scope_from_the_mentioned_set() {
    assert_eq!(
        control_mentioning("stop", &["A", "B"]),
        Some(Control::Stop(bots(&["A", "B"])))
    );
}

#[test]
fn a_cancel_that_mentions_a_bot_is_scoped_to_it() {
    assert_eq!(
        control_mentioning("@A !cancel", &["A"]),
        Some(Control::Cancel(bots(&["A"])))
    );
}

#[test]
fn a_resume_that_mentions_a_bot_is_scoped_to_it() {
    // `@A` is removed, which leaves `resume` as the only word.
    assert_eq!(
        control_mentioning("@A resume", &["A"]),
        Some(Control::Resume(bots(&["A"])))
    );
}

// ---------------------------------------------------------------------------------------------
// Cancel and resume (R29.4, R29.5)
// ---------------------------------------------------------------------------------------------

#[test]
fn cancel_alone_cancels_everyone() {
    assert_eq!(control_of("!cancel"), Some(Control::Cancel(Scope::All)));
}

#[test]
fn cancel_must_be_the_only_word() {
    assert_eq!(control_of("!cancel now"), None);
}

#[test]
fn a_cancel_word_without_the_bang_is_not_a_command() {
    assert_eq!(control_of("cancel"), None);
}

#[test]
fn resume_alone_resumes_everyone() {
    assert_eq!(control_of("resume"), Some(Control::Resume(Scope::All)));
}

#[test]
fn resume_is_matched_case_insensitively() {
    assert_eq!(control_of("Resume"), Some(Control::Resume(Scope::All)));
}

#[test]
fn resume_must_be_the_only_word() {
    assert_eq!(control_of("resume please"), None);
}

#[test]
fn everyone_resume_resumes_everyone() {
    // `@everyone` is removed, which leaves `resume` as the only word.
    assert_eq!(
        control_of("@everyone resume"),
        Some(Control::Resume(Scope::All))
    );
}

// ---------------------------------------------------------------------------------------------
// What is removed before the words are counted (R29.2 step 2)
// ---------------------------------------------------------------------------------------------

#[test]
fn an_npub_uri_is_removed_before_matching() {
    // The URI names A, so the caller's mentioned set is {A}.
    assert_eq!(
        control_mentioning(&format!("nostr:{} stop", npub("A")), &["A"]),
        Some(Control::Stop(bots(&["A"])))
    );
}

#[test]
fn an_npub_uri_leaves_resume_as_the_only_word() {
    assert_eq!(
        control_mentioning(&format!("nostr:{} resume", npub("A")), &["A"]),
        Some(Control::Resume(bots(&["A"])))
    );
}

#[test]
fn an_nprofile_uri_leaves_cancel_as_the_only_word() {
    assert_eq!(
        control_mentioning(&format!("nostr:{} !cancel", nprofile("A", &[])), &["A"]),
        Some(Control::Cancel(bots(&["A"])))
    );
}

#[test]
fn a_uri_does_not_count_towards_the_five_words() {
    // Five words once the URI is removed (could you please stop now), seven if it were kept.
    assert_eq!(
        control_mentioning(
            &format!("could you please stop nostr:{} now", npub("A")),
            &["A"]
        ),
        Some(Control::Stop(bots(&["A"])))
    );
}

#[test]
fn an_alias_is_removed_before_matching() {
    let roster = roster_with("David", &[("A", &["alpha"]), ("B", &[])]);

    assert_eq!(
        parse_control("@alpha resume", &roster, &bot_set(&["A"])),
        Some(Control::Resume(bots(&["A"])))
    );
}

#[test]
fn the_longest_roster_name_is_removed_first() {
    // Removing `@scout` first would leave `.pro resume`, which is two words.
    let roster = roster_with("David", &[("scout", &[]), ("scout.pro", &[])]);

    assert_eq!(
        parse_control("@scout.pro resume", &roster, &bot_set(&["scout.pro"])),
        Some(Control::Resume(bots(&["scout.pro"])))
    );
}
