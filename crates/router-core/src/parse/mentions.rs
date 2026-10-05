//! Mention extraction: which roster bots a message addresses, and whom a reply tags (design
//! sections 5.4 and 6.8, requirements 17 and 44.4).

use std::collections::BTreeSet;

use buzz_sdk::mentions::{extract_at_mentions_with_known, extract_nostr_uris};

use super::nip19::nprofile_pubkeys;
use super::text::mention_text;
use crate::classify::AuthorClass;
use crate::config::Roster;
use crate::ids::{BotName, Pubkey};
use crate::route::InEvent;

/// The roster bots the event `ev` mentions, other than `author`.
///
/// The result is the union of three sets (design 5.4), computed on [`mention_text`] where text
/// is involved, so a mention inside code or a quoted line never counts (requirements 17.1 and
/// 17.5):
///
/// 1. `@name` mentions of a roster name or alias, matched case-insensitively, as whole words,
///    longest name first (requirement 17.2). Other `@` tokens are ignored.
/// 2. `nostr:npub1...` and `nostr:nprofile1...` URIs whose key is a roster bot's (requirement
///    17.3, assumption A18).
/// 3. `p` tags naming a roster bot, only when `class` is `Owner` or `Human`, and never a `p` tag
///    that carries the event author's own pubkey (requirement 17.4).
///
/// `author` is the bot that wrote the event, when it is one: a bot never mentions itself.
pub fn mentioned_bots(
    ev: &InEvent,
    class: &AuthorClass,
    roster: &Roster,
    author: Option<&BotName>,
) -> BTreeSet<BotName> {
    let text = mention_text(&ev.content);

    let mut bots = at_mentioned_bots(&text, roster);
    bots.extend(uri_mentioned_bots(&text, roster));
    if matches!(class, AuthorClass::Owner | AuthorClass::Human) {
        bots.extend(p_tag_bots(ev, roster));
    }
    if let Some(author) = author {
        bots.remove(author);
    }
    bots
}

/// The pubkeys a reply with the text `text` tags with `p` (requirement 44.4, design 6.8).
///
/// The text is read like a message (see [`mention_text`]). A bot is tagged when the text
/// `@mentions` its name or an alias; a bare bot name tags nobody. The owner is tagged, with every
/// owner pubkey (DD-18), when the text `@mentions` the owner's name or names the owner as a bare
/// whole word, in any case. The result holds each pubkey once, in pubkey order, so it is the same
/// on every call.
pub fn mentions_for_reply(text: &str, roster: &Roster) -> Vec<Pubkey> {
    let text = mention_text(text);
    let mut names = roster_names(roster);
    names.push(roster.owner.name.as_str());
    let owner_name = roster.owner.name.to_ascii_lowercase();

    let mut tagged: BTreeSet<Pubkey> = BTreeSet::new();
    let mut owner_tagged = false;
    for name in extract_at_mentions_with_known(&text, &names) {
        owner_tagged |= name == owner_name;
        if let Some(bot) = roster
            .name_index
            .get(&name)
            .and_then(|bot| roster.bots.get(bot))
        {
            tagged.insert(bot.pubkey.clone());
        }
    }
    // Interim reading of an inconsistency in the artifacts (run ledger finding F6): the `||`
    // term below is the whole effect of `owner_named_bare`. To go back to `@`-only owner
    // mentions, delete the term and the function.
    if owner_tagged || owner_named_bare(&text, &roster.owner.name) {
        tagged.extend(roster.owner.pubkeys.iter().cloned());
    }
    tagged.into_iter().collect()
}

/// Whether `text` names the owner as a bare word: `owner_name` without an `@`, in any ASCII
/// case, with no letter, digit or underscore next to it on either side.
///
/// This exists because task 1.4's contract has `mentions_for_reply("thanks David and
/// @dp-kyber-bot")` tag the owner, while design 6.8, requirement 44.4 and DD-18 speak of
/// `@mentions` only (finding F6). It applies to the owner only: a bare bot name or alias tags
/// nobody. `text` is already free of code regions and quoted lines.
///
/// It does nothing beyond whole-word matching. In particular, `David's` and a name inside a
/// longer `@dp-david-bot` do count, because `'` and `-` are not word characters. Nothing pins
/// either case.
fn owner_named_bare(text: &str, owner_name: &str) -> bool {
    if owner_name.trim().is_empty() {
        return false;
    }
    // ASCII lowercasing keeps every byte offset, so `start` indexes `text` as well.
    let haystack = text.to_ascii_lowercase();
    let needle = owner_name.to_ascii_lowercase();
    haystack.match_indices(&needle).any(|(start, found)| {
        let before = haystack
            .get(..start)
            .and_then(|head| head.chars().next_back());
        let after = haystack
            .get(start + found.len()..)
            .and_then(|tail| tail.chars().next());
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    })
}

/// Whether `c` can be part of a word for [`owner_named_bare`].
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every roster name and alias, lowercased (the keys of `name_index`).
pub(super) fn roster_names(roster: &Roster) -> Vec<&str> {
    roster.name_index.keys().map(String::as_str).collect()
}

/// The bots `@mentioned` in `text` (requirement 17.2).
fn at_mentioned_bots(text: &str, roster: &Roster) -> BTreeSet<BotName> {
    extract_at_mentions_with_known(text, &roster_names(roster))
        .iter()
        .filter_map(|name| roster.name_index.get(name))
        .cloned()
        .collect()
}

/// The bots named by a `nostr:npub1...` or `nostr:nprofile1...` URI in `text` (requirement 17.3).
fn uri_mentioned_bots(text: &str, roster: &Roster) -> BTreeSet<BotName> {
    let npubs = extract_nostr_uris(text)
        .iter()
        .filter_map(|hex| Pubkey::from_hex(hex).ok())
        .collect::<Vec<_>>();
    npubs
        .iter()
        .chain(nprofile_pubkeys(text).iter())
        .filter_map(|pubkey| bot_with_pubkey(roster, pubkey))
        .cloned()
        .collect()
}

/// The bots named by the `p` tags of `ev`, without the tag that carries the author's own key
/// (requirement 17.4). The caller decides whether the author's class lets `p` tags count.
pub(crate) fn p_tag_bots(ev: &InEvent, roster: &Roster) -> BTreeSet<BotName> {
    ev.tags
        .iter()
        .filter_map(|tag| match tag.as_slice() {
            [kind, value, ..] if kind == "p" => Pubkey::from_hex(value).ok(),
            _ => None,
        })
        .filter(|pubkey| *pubkey != ev.pubkey)
        .filter_map(|pubkey| bot_with_pubkey(roster, &pubkey))
        .cloned()
        .collect()
}

/// The roster bot whose key is `pubkey`.
fn bot_with_pubkey<'r>(roster: &'r Roster, pubkey: &Pubkey) -> Option<&'r BotName> {
    roster
        .bots
        .values()
        .find(|bot| bot.pubkey == *pubkey)
        .map(|bot| &bot.name)
}
