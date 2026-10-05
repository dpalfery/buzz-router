//! Control-command parsing: `stop`, `resume` and `!cancel` (design section 5.4, requirement 29).

use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::sync::LazyLock;

use buzz_sdk::mentions::strip_code_regions;
use regex::Regex;

use super::everyone::blank_everyone;
use super::mentions::roster_names;
use super::text::blank;
use crate::config::Roster;
use crate::ids::BotName;
use crate::route::{Control, Scope};

/// A `nostr:` URI: the scheme and the bech32 characters that follow it (requirement 29.2).
/// Compiled once. See [`blank`] for why the static holds an `Option`.
static NOSTR_URI: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)nostr:[0-9a-z]+").ok());

/// The most words a message may have and still count as a stop (requirement 29.3).
const MAX_STOP_WORDS: usize = 5;

/// The control command in `content`, if any (requirement 29).
///
/// This is called for owner kind-9 messages only. `mentioned` is the set of bots the message
/// mentions in any form, as `mentioned_bots` computes it. It sets the scope of the command:
/// every bot when it is empty, including when the message has `@everyone`.
///
/// The text is normalised first:
///
/// 1. code regions are removed (quoted lines are not);
/// 2. `@everyone`, every roster `@name` and alias, and every `nostr:` URI are replaced by spaces;
/// 3. the text is lowercased, and every character that is neither alphanumeric nor `!` becomes a
///    space;
/// 4. the text is split into words.
///
/// Then, in this order:
///
/// - the single word `!cancel` is `Cancel`;
/// - the single word `resume` is `Resume`;
/// - one to five words, any of them `stop`, `halt` or `!shutdown`, is `Stop`.
pub fn parse_control(
    content: &str,
    roster: &Roster,
    mentioned: &BTreeSet<BotName>,
) -> Option<Control> {
    let stripped = strip_code_regions(content);
    let text = blank_everyone(&stripped);
    let text = blank_roster_names(&text, roster);
    let text = blank(NOSTR_URI.as_ref(), &text);
    let normalised = normalise(&text);
    let words: Vec<&str> = normalised.split_whitespace().collect();

    let scope = || {
        if mentioned.is_empty() {
            Scope::All
        } else {
            Scope::Bots(mentioned.clone())
        }
    };
    match words.as_slice() {
        ["!cancel"] => Some(Control::Cancel(scope())),
        ["resume"] => Some(Control::Resume(scope())),
        words
            if words.len() <= MAX_STOP_WORDS
                && words
                    .iter()
                    .any(|word| matches!(*word, "stop" | "halt" | "!shutdown")) =>
        {
            Some(Control::Stop(scope()))
        }
        _ => None,
    }
}

/// Lowercases `text` and turns every character that is neither alphanumeric nor `!` into a
/// space (requirement 29.2, step 3).
fn normalise(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '!' {
                c
            } else {
                ' '
            }
        })
        .collect()
}

/// `text` with every roster `@name` and alias replaced by a space.
///
/// Names match the way `buzz_sdk::mentions::extract_at_mentions_with_known` matches them: the
/// `@` is at the start of the text or follows ASCII whitespace, then a name is tried longest
/// first, case-insensitively, and must be followed by the end of the text, whitespace or one of
/// `, ; . ! ? : ) ] }`. A longer name wins, so `@scout.pro` is removed whole when both `scout`
/// and `scout.pro` are roster names. An `@token` that is not a roster name stays.
fn blank_roster_names(text: &str, roster: &Roster) -> String {
    let mut names = roster_names(roster);
    names.retain(|name| !name.trim().is_empty());
    names.sort_by_key(|name| Reverse(name.len()));

    let mut blanked = String::with_capacity(text.len());
    // Everything before `copied` is already in `blanked` or was blanked.
    let mut copied = 0;
    for (at, _) in text.match_indices('@') {
        if at < copied || !follows_space(text, at) {
            continue;
        }
        let Some(rest) = text.get(at + 1..) else {
            continue;
        };
        if let Some(len) = names.iter().find_map(|name| name_len_at(rest, name)) {
            blanked.push_str(text.get(copied..at).unwrap_or_default());
            blanked.push(' ');
            copied = at + 1 + len;
        }
    }
    blanked.push_str(text.get(copied..).unwrap_or_default());
    blanked
}

/// Whether the `@` at byte `at` of `text` starts the text or follows ASCII whitespace.
fn follows_space(text: &str, at: usize) -> bool {
    at.checked_sub(1).is_none_or(|previous| {
        text.as_bytes()
            .get(previous)
            .is_some_and(u8::is_ascii_whitespace)
    })
}

/// The byte length of `name` when `rest` starts with it, ignoring ASCII case, and the name ends
/// at a word boundary.
fn name_len_at(rest: &str, name: &str) -> Option<usize> {
    let candidate = rest.get(..name.len())?;
    let after = rest.get(name.len()..)?;
    (candidate.eq_ignore_ascii_case(name) && at_word_boundary(after)).then_some(name.len())
}

/// Whether a name may end right before `after`. This is the boundary rule of
/// `extract_at_mentions_with_known`, which keeps the function private.
fn at_word_boundary(after: &str) -> bool {
    after.chars().next().is_none_or(|c| {
        c.is_ascii_whitespace() || matches!(c, ',' | ';' | '.' | '!' | '?' | ':' | ')' | ']' | '}')
    })
}
