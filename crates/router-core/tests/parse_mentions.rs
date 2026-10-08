//! Task 1.4 (RED): mention parsing, `@everyone` detection and reply mentions.
//!
//! Covers R7.1, R17.1 to R17.5 and R44.4, and assumption A18, against design sections 5.4 and
//! 6.8 (and DD-18). The functions under test are the `router_core::parse` ones fixed by "Interfaces
//! fixed for test-first":
//!
//! - `mention_text(content)`: removes code regions, then replaces every line that starts with `>`
//!   by an empty line;
//! - `mentioned_bots(ev, class, roster, author)`: the union of text mentions, `nostr:npub1...` and
//!   `nostr:nprofile1...` URI mentions, and (for `Owner` and `Human` authors only) `p`-tag
//!   mentions, minus the author;
//! - `contains_everyone(text)`: the regex `(?i)(^|\s)@everyone\b`, run on `mention_text` output;
//! - `mentions_for_reply(text, roster)`: the pubkeys a bot's reply tags with `p`.
//!
//! The control-command parser is covered by `parse_control.rs`.
//!
//! Rosters: `common::roster()` (owner keys `O` and `O2`, bots `A`, `B`, `C`) for the cases that
//! mirror the CONFORM rows, and `common::roster_with` for the names the contract uses
//! (`dp-grok-bot`, `sf-GrokBot`, `dp-kyber-bot`, owner `David`). Every key is a deterministic
//! fixture key, `sha256("buzz-router-fixture:" + name)`. The `nostr:` URIs are built in the tests
//! with `ToBech32` (`PublicKey`) and `Nip19Profile::to_bech32`.
//!
//! The contract bullet `mentions_for_reply("thanks David and @dp-kyber-bot")` has a bare `David`.
//! Design 6.8, R44.4, DD-18 and brief 9.6 speak of `@mentions`, so the artifacts disagree (run
//! ledger finding F6). The conductor chose the contract-literal reading, which these tests pin:
//! a reply tags the owner's keys for `@David` and for a bare `David` (a whole word, any case,
//! after code regions and quoted lines are removed), and tags a bot only for an `@` mention. A
//! bare bot name or alias tags nobody.

mod common;

use std::collections::BTreeSet;

use common::{
    bot_name, bot_set, keys, message, nprofile, npub, p_tag, pubkey, roster, roster_with,
};
use nostr::Keys;
use router_core::classify::AuthorClass;
use router_core::config::Roster;
use router_core::ids::{BotName, Pubkey};
use router_core::parse::{contains_everyone, mention_text, mentioned_bots, mentions_for_reply};

/// A relay hint for the `nprofile` tests. The `.invalid` top-level domain is reserved and never
/// resolves.
const RELAY_HINT: &str = "wss://relay.invalid";

/// The roster of the contract's bullets: owner `David` (keys `O` and `O2`) and bots
/// `dp-grok-bot`, `sf-GrokBot` and `dp-kyber-bot`, which also answers to the alias `kyber`.
fn named_roster() -> Roster {
    roster_with(
        "David",
        &[
            ("dp-grok-bot", &[]),
            ("sf-GrokBot", &[]),
            ("dp-kyber-bot", &["kyber"]),
        ],
    )
}

/// The bots the owner's message `content` mentions, with no `p` tags.
fn owner_mentions(roster: &Roster, content: &str) -> BTreeSet<BotName> {
    mentioned_bots(
        &message(&keys("O"), content, vec![]),
        &AuthorClass::Owner,
        roster,
        None,
    )
}

/// The bots a message authored by `author`, of class `class`, with `tags` mentions.
fn mentions_by(
    author: &Keys,
    class: &AuthorClass,
    roster: &Roster,
    tags: Vec<Vec<String>>,
) -> BTreeSet<BotName> {
    mentioned_bots(&message(author, "hello", tags), class, roster, None)
}

fn nobody() -> BTreeSet<BotName> {
    BTreeSet::new()
}

/// The pubkeys of `names` (fixture key names), sorted, for comparison with `mentions_for_reply`.
fn sorted_pubkeys(names: &[&str]) -> Vec<Pubkey> {
    let mut pubkeys: Vec<Pubkey> = names.iter().copied().map(pubkey).collect();
    pubkeys.sort();
    pubkeys
}

/// `bech32` with its last character changed to another valid bech32 character, which breaks the
/// checksum.
fn with_broken_checksum(bech32: &str) -> String {
    let mut text = bech32.to_owned();
    let last = text.pop();
    text.push(if last == Some('q') { 'p' } else { 'q' });
    text
}

/// `pubkeys`, sorted.
fn sorted(mut pubkeys: Vec<Pubkey>) -> Vec<Pubkey> {
    pubkeys.sort();
    pubkeys
}

/// What `mentions_for_reply` returns for `text`, sorted.
fn reply_mentions(roster: &Roster, text: &str) -> Vec<Pubkey> {
    sorted(mentions_for_reply(text, roster))
}

// ---------------------------------------------------------------------------------------------
// mention_text (R17.1): code regions and quoted lines are removed
// ---------------------------------------------------------------------------------------------

#[test]
fn a_code_span_is_removed() {
    let text = mention_text("ask `@dp-grok-bot` later");

    assert!(!text.contains("@dp-grok-bot"), "got {text:?}");
    assert!(text.contains("ask"), "got {text:?}");
    assert!(text.contains("later"), "got {text:?}");
}

#[test]
fn a_fenced_code_block_is_removed() {
    let text = mention_text("before\n```\n@dp-grok-bot\n```\nafter");

    assert!(!text.contains("@dp-grok-bot"), "got {text:?}");
    assert!(text.contains("before"), "got {text:?}");
    assert!(text.contains("after"), "got {text:?}");
}

#[test]
fn a_line_that_starts_with_a_quote_marker_becomes_an_empty_line() {
    assert_eq!(
        mention_text("before\n> quoted @dp-grok-bot\nafter"),
        "before\n\nafter"
    );
}

#[test]
fn every_quoted_line_becomes_an_empty_line() {
    assert_eq!(mention_text("> one\n> two\nthree"), "\n\nthree");
}

#[test]
fn a_quote_marker_inside_a_line_leaves_the_line_alone() {
    assert_eq!(mention_text("a > b @dp-grok-bot"), "a > b @dp-grok-bot");
}

#[test]
fn text_without_code_or_quotes_is_unchanged() {
    assert_eq!(
        mention_text("plain @dp-grok-bot text\nsecond line"),
        "plain @dp-grok-bot text\nsecond line"
    );
}

// ---------------------------------------------------------------------------------------------
// mentioned_bots: text mentions (R17.2)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_text_mention_maps_to_its_bot() {
    assert_eq!(
        owner_mentions(&named_roster(), "@dp-kyber-bot hi"),
        bot_set(&["dp-kyber-bot"])
    );
}

#[test]
fn dp_grok_bot_matches_only_dp_grok_bot_when_sf_grokbot_also_exists() {
    let roster = named_roster();

    assert_eq!(
        owner_mentions(&roster, "@dp-grok-bot hi"),
        bot_set(&["dp-grok-bot"])
    );
    assert_eq!(
        owner_mentions(&roster, "@sf-GrokBot hi"),
        bot_set(&["sf-GrokBot"])
    );
}

#[test]
fn matching_is_case_insensitive() {
    assert_eq!(
        owner_mentions(&named_roster(), "@DP-Grok-BOT hi"),
        bot_set(&["dp-grok-bot"])
    );
}

#[test]
fn a_lower_case_mention_matches_a_mixed_case_roster_name() {
    assert_eq!(
        owner_mentions(&named_roster(), "@sf-grokbot hi"),
        bot_set(&["sf-GrokBot"])
    );
}

#[test]
fn an_alias_matches_its_bot() {
    assert_eq!(
        owner_mentions(&named_roster(), "@kyber hi"),
        bot_set(&["dp-kyber-bot"])
    );
}

#[test]
fn an_unknown_name_is_ignored() {
    assert_eq!(owner_mentions(&named_roster(), "@bob hi"), nobody());
}

#[test]
fn an_unknown_name_does_not_hide_a_known_one() {
    assert_eq!(
        owner_mentions(&named_roster(), "@bob and @dp-kyber-bot"),
        bot_set(&["dp-kyber-bot"])
    );
}

#[test]
fn several_mentions_give_every_bot_once() {
    assert_eq!(
        owner_mentions(&named_roster(), "@dp-grok-bot @sf-GrokBot @dp-grok-bot"),
        bot_set(&["dp-grok-bot", "sf-GrokBot"])
    );
}

#[test]
fn a_name_followed_by_punctuation_still_matches() {
    assert_eq!(
        owner_mentions(&named_roster(), "hey @dp-grok-bot, thoughts?"),
        bot_set(&["dp-grok-bot"])
    );
}

#[test]
fn a_name_must_be_a_whole_word() {
    assert_eq!(
        owner_mentions(&named_roster(), "@dp-grok-botx hi"),
        nobody()
    );
}

#[test]
fn an_at_sign_inside_a_word_is_not_a_mention() {
    assert_eq!(
        owner_mentions(&named_roster(), "mail foo@dp-grok-bot please"),
        nobody()
    );
}

#[test]
fn a_longer_name_wins_over_a_name_that_is_its_prefix() {
    let roster = roster_with("David", &[("scout", &[]), ("scout.pro", &[])]);

    assert_eq!(
        owner_mentions(&roster, "@scout.pro hi"),
        bot_set(&["scout.pro"])
    );
    assert_eq!(owner_mentions(&roster, "@scout hi"), bot_set(&["scout"]));
}

#[test]
fn everyone_is_not_a_bot_mention() {
    assert_eq!(
        owner_mentions(&named_roster(), "@everyone thoughts?"),
        nobody()
    );
}

// ---------------------------------------------------------------------------------------------
// mentioned_bots: code regions and quoted lines (R17.1, R17.5, CONFORM 32 and 33)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_mention_that_is_only_inside_code_is_ignored() {
    // CONFORM case 32: O posts ```@A``` and nobody is mentioned.
    assert_eq!(owner_mentions(&roster(), "```@A```"), nobody());
}

#[test]
fn a_mention_after_a_space_inside_a_fence_line_is_ignored() {
    // Case 32 again, with a space before the `@`. A mention must follow whitespace to count, so
    // the case-32 text alone would pass even if code regions were not removed.
    assert_eq!(owner_mentions(&roster(), "``` @A ```"), nobody());
}

#[test]
fn a_mention_inside_a_code_span_is_ignored() {
    // The space before the `@` makes this a mention unless the code span is removed.
    assert_eq!(
        owner_mentions(&named_roster(), "run `ping @dp-grok-bot` later"),
        nobody()
    );
}

#[test]
fn a_mention_inside_a_fenced_block_is_ignored() {
    assert_eq!(
        owner_mentions(&named_roster(), "see:\n```\n@dp-grok-bot\n```\ndone"),
        nobody()
    );
}

#[test]
fn a_mention_in_a_quoted_line_is_ignored_and_one_on_the_next_line_counts() {
    // CONFORM case 33: the first line quotes "@everyone", the next line mentions B.
    assert_eq!(
        owner_mentions(&roster(), "> @everyone said hi\nwhat do you think @B"),
        bot_set(&["B"])
    );
}

#[test]
fn a_mention_that_is_only_in_a_quoted_line_is_ignored() {
    assert_eq!(
        owner_mentions(&named_roster(), "> @dp-grok-bot said so\nthanks"),
        nobody()
    );
}

#[test]
fn an_npub_uri_inside_code_is_ignored() {
    let content = format!("`nostr:{}`", npub("dp-grok-bot"));

    assert_eq!(owner_mentions(&named_roster(), &content), nobody());
}

#[test]
fn an_nprofile_uri_inside_code_is_ignored() {
    let content = format!("`nostr:{}`", nprofile("dp-grok-bot", &[]));

    assert_eq!(owner_mentions(&named_roster(), &content), nobody());
}

#[test]
fn an_npub_uri_in_a_quoted_line_is_ignored() {
    let content = format!("> nostr:{} said so", npub("dp-grok-bot"));

    assert_eq!(owner_mentions(&named_roster(), &content), nobody());
}

#[test]
fn an_nprofile_uri_in_a_quoted_line_is_ignored() {
    let content = format!("> nostr:{} said so", nprofile("dp-grok-bot", &[]));

    assert_eq!(owner_mentions(&named_roster(), &content), nobody());
}

// ---------------------------------------------------------------------------------------------
// mentioned_bots: URI mentions (R17.3, A18)
// ---------------------------------------------------------------------------------------------

#[test]
fn an_npub_uri_maps_to_its_bot() {
    let content = format!("hey nostr:{} look", npub("dp-grok-bot"));

    assert_eq!(
        owner_mentions(&named_roster(), &content),
        bot_set(&["dp-grok-bot"])
    );
}

#[test]
fn an_nprofile_uri_maps_to_its_bot() {
    let content = format!("hey nostr:{} look", nprofile("dp-grok-bot", &[]));

    assert_eq!(
        owner_mentions(&named_roster(), &content),
        bot_set(&["dp-grok-bot"])
    );
}

#[test]
fn an_nprofile_uri_with_a_relay_hint_maps_to_its_bot() {
    let content = format!("hey nostr:{} look", nprofile("sf-GrokBot", &[RELAY_HINT]));

    assert_eq!(
        owner_mentions(&named_roster(), &content),
        bot_set(&["sf-GrokBot"])
    );
}

#[test]
fn an_nprofile_uri_with_upper_case_data_maps_to_its_bot() {
    // Design 5.4 lowercases the candidate before decoding, as NIP-19 allows upper case.
    let bech32 = nprofile("dp-grok-bot", &[]);
    let data = bech32.trim_start_matches("nprofile1").to_ascii_uppercase();
    let content = format!("hey nostr:nprofile1{data} look");

    assert_eq!(
        owner_mentions(&named_roster(), &content),
        bot_set(&["dp-grok-bot"])
    );
}

#[test]
fn an_nprofile_uri_followed_by_punctuation_maps_to_its_bot() {
    let content = format!("thanks nostr:{}, great", nprofile("dp-grok-bot", &[]));

    assert_eq!(
        owner_mentions(&named_roster(), &content),
        bot_set(&["dp-grok-bot"])
    );
}

#[test]
fn an_npub_and_an_nprofile_of_different_bots_both_count() {
    let content = format!(
        "nostr:{} and nostr:{}",
        npub("dp-grok-bot"),
        nprofile("dp-kyber-bot", &[])
    );

    assert_eq!(
        owner_mentions(&named_roster(), &content),
        bot_set(&["dp-grok-bot", "dp-kyber-bot"])
    );
}

#[test]
fn the_uris_of_a_key_outside_the_roster_are_ignored() {
    let content = format!(
        "nostr:{} nostr:{}",
        npub("stranger"),
        nprofile("stranger", &[])
    );

    assert_eq!(owner_mentions(&named_roster(), &content), nobody());
}

#[test]
fn an_nprofile_uri_that_does_not_decode_is_ignored() {
    let broken = with_broken_checksum(&nprofile("dp-grok-bot", &[]));
    let content = format!("hey nostr:{broken} look");

    assert_eq!(owner_mentions(&named_roster(), &content), nobody());
}

#[test]
fn an_nprofile_prefix_with_no_data_is_ignored() {
    assert_eq!(
        owner_mentions(&named_roster(), "see nostr:nprofile1 for details"),
        nobody()
    );
}

// ---------------------------------------------------------------------------------------------
// mentioned_bots: p-tag mentions (R17.4, CONFORM 36)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_p_tag_counts_for_an_owner_author() {
    assert_eq!(
        mentions_by(&keys("O"), &AuthorClass::Owner, &roster(), vec![p_tag("A")]),
        bot_set(&["A"])
    );
}

#[test]
fn a_p_tag_counts_for_a_human_author() {
    assert_eq!(
        mentions_by(
            &keys("stranger"),
            &AuthorClass::Human,
            &roster(),
            vec![p_tag("A"), p_tag("C")]
        ),
        bot_set(&["A", "C"])
    );
}

#[test]
fn a_p_tag_does_not_count_for_a_bot_author() {
    assert_eq!(
        mentions_by(
            &keys("B"),
            &AuthorClass::Bot(bot_name("B")),
            &roster(),
            vec![p_tag("A")]
        ),
        nobody()
    );
}

#[test]
fn a_p_tag_does_not_count_for_a_foreign_bot_author() {
    let author = keys("foreign-agent");

    for owner_is_ours in [true, false] {
        assert_eq!(
            mentions_by(
                &author,
                &AuthorClass::ForeignBot { owner_is_ours },
                &roster(),
                vec![p_tag("A")]
            ),
            nobody(),
            "owner_is_ours = {owner_is_ours}"
        );
    }
}

#[test]
fn a_bot_authors_text_mention_still_counts() {
    let ev = message(&keys("B"), "over to you @C", vec![p_tag("A")]);

    assert_eq!(
        mentioned_bots(&ev, &AuthorClass::Bot(bot_name("B")), &roster(), None),
        bot_set(&["C"])
    );
}

#[test]
fn a_bot_authors_npub_uri_still_counts() {
    let content = format!("over to you nostr:{}", npub("C"));
    let ev = message(&keys("B"), &content, vec![]);

    assert_eq!(
        mentioned_bots(&ev, &AuthorClass::Bot(bot_name("B")), &roster(), None),
        bot_set(&["C"])
    );
}

#[test]
fn a_p_tag_for_a_key_outside_the_roster_is_ignored() {
    assert_eq!(
        mentions_by(
            &keys("O"),
            &AuthorClass::Owner,
            &roster(),
            vec![p_tag("stranger")]
        ),
        nobody()
    );
}

#[test]
fn a_malformed_p_tag_is_ignored() {
    let tags = vec![
        vec!["p".to_owned()],
        vec!["p".to_owned(), "not-a-pubkey".to_owned()],
        p_tag("B"),
    ];

    assert_eq!(
        mentions_by(&keys("O"), &AuthorClass::Owner, &roster(), tags),
        bot_set(&["B"])
    );
}

#[test]
fn a_p_tag_carrying_the_authors_own_key_is_ignored() {
    // CONFORM case 36: O posts a message whose only p tag is O's own key.
    assert_eq!(
        mentions_by(&keys("O"), &AuthorClass::Owner, &roster(), vec![p_tag("O")]),
        nobody()
    );
}

#[test]
fn the_authors_key_is_skipped_even_when_it_is_a_roster_bot_key() {
    // The class is a parameter, so a test can pair a roster bot's key with `Human`: the p tag
    // naming the author is skipped (R17.4) and the one naming B counts.
    assert_eq!(
        mentions_by(
            &keys("A"),
            &AuthorClass::Human,
            &roster(),
            vec![p_tag("A"), p_tag("B")]
        ),
        bot_set(&["B"])
    );
}

// ---------------------------------------------------------------------------------------------
// mentioned_bots: the union, minus the author (design 5.4)
// ---------------------------------------------------------------------------------------------

#[test]
fn text_uri_and_p_tag_mentions_are_united() {
    let content = format!("@A and nostr:{}", npub("B"));
    let ev = message(&keys("O"), &content, vec![p_tag("C")]);

    assert_eq!(
        mentioned_bots(&ev, &AuthorClass::Owner, &roster(), None),
        bot_set(&["A", "B", "C"])
    );
}

#[test]
fn a_bot_named_in_two_forms_appears_once() {
    let content = format!("@A and nostr:{}", npub("A"));
    let ev = message(&keys("O"), &content, vec![p_tag("A")]);

    assert_eq!(
        mentioned_bots(&ev, &AuthorClass::Owner, &roster(), None),
        bot_set(&["A"])
    );
}

#[test]
fn the_author_bot_is_removed_from_the_result() {
    let ev = message(&keys("A"), "@A and @B", vec![]);
    let author = bot_name("A");

    assert_eq!(
        mentioned_bots(
            &ev,
            &AuthorClass::Bot(author.clone()),
            &roster(),
            Some(&author)
        ),
        bot_set(&["B"])
    );
    assert_eq!(
        mentioned_bots(&ev, &AuthorClass::Bot(author), &roster(), None),
        bot_set(&["A", "B"])
    );
}

// ---------------------------------------------------------------------------------------------
// contains_everyone (R7.1)
// ---------------------------------------------------------------------------------------------

#[test]
fn everyone_at_the_start_is_detected() {
    assert!(contains_everyone("@everyone thoughts?"));
}

#[test]
fn everyone_followed_by_a_comma_is_detected() {
    assert!(contains_everyone("hi @everyone,"));
}

#[test]
fn everyone_inside_a_word_is_not_detected() {
    assert!(!contains_everyone("foo@everyone"));
}

#[test]
fn everyone_inside_code_is_not_detected_once_mention_text_has_run() {
    assert!(!contains_everyone(&mention_text("`@everyone`")));
}

#[test]
fn everyone_inside_a_fenced_block_is_not_detected_once_mention_text_has_run() {
    assert!(!contains_everyone(&mention_text("```\n@everyone\n```")));
}

#[test]
fn everyone_in_a_quoted_line_is_not_detected_once_mention_text_has_run() {
    // CONFORM case 33: the quoted first line does not count.
    assert!(!contains_everyone(&mention_text(
        "> @everyone said hi\nwhat do you think @B"
    )));
}

#[test]
fn everyone_in_plain_text_survives_mention_text() {
    assert!(contains_everyone(&mention_text("@everyone thoughts?")));
}

#[test]
fn everyone_is_matched_case_insensitively() {
    assert!(contains_everyone("hello @Everyone"));
}

#[test]
fn everyone_after_a_newline_is_detected() {
    assert!(contains_everyone("first line\n@everyone second line"));
}

#[test]
fn everyone_must_be_a_whole_word() {
    assert!(!contains_everyone("hi @everyonee"));
}

#[test]
fn text_without_everyone_is_not_detected() {
    assert!(!contains_everyone("what do you think @B"));
}

// ---------------------------------------------------------------------------------------------
// mentions_for_reply (R44.4, design 6.8, DD-18)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_reply_mentioning_the_owner_and_a_bot_tags_both_owner_keys_and_the_bot() {
    // The `@David` form of the contract bullet. The bare form is the next test (finding F6).
    assert_eq!(
        reply_mentions(&named_roster(), "thanks @David and @dp-kyber-bot"),
        sorted_pubkeys(&["O", "O2", "dp-kyber-bot"])
    );
}

#[test]
fn an_owner_mention_covers_every_owner_pubkey() {
    assert_eq!(
        reply_mentions(&named_roster(), "thanks @David"),
        sorted_pubkeys(&["O", "O2"])
    );
}

#[test]
fn a_bot_mention_in_a_reply_maps_to_the_bots_pubkey() {
    assert_eq!(
        reply_mentions(&named_roster(), "over to @dp-kyber-bot"),
        sorted_pubkeys(&["dp-kyber-bot"])
    );
}

#[test]
fn an_alias_in_a_reply_maps_to_its_bots_pubkey() {
    assert_eq!(
        reply_mentions(&named_roster(), "over to @kyber"),
        sorted_pubkeys(&["dp-kyber-bot"])
    );
}

#[test]
fn a_bot_named_by_its_name_and_its_alias_is_tagged_once() {
    assert_eq!(
        reply_mentions(&named_roster(), "@dp-kyber-bot, I mean @kyber"),
        sorted_pubkeys(&["dp-kyber-bot"])
    );
}

#[test]
fn a_reply_mentioning_an_unknown_name_tags_nobody() {
    assert_eq!(reply_mentions(&named_roster(), "thanks @bob"), Vec::new());
}

#[test]
fn a_reply_with_no_mention_tags_nobody() {
    assert_eq!(
        reply_mentions(&named_roster(), "all done, nothing to add"),
        Vec::new()
    );
}

#[test]
fn a_reply_mention_inside_code_tags_nobody() {
    // R44.4 resolves reply mentions with the parser of Requirement 17, whose R17.5 ignores
    // code regions.
    assert_eq!(
        reply_mentions(&named_roster(), "run `ping @dp-kyber-bot` to start it"),
        Vec::new()
    );
}

#[test]
fn a_reply_with_a_bare_owner_name_and_an_at_mentioned_bot_tags_both_owner_keys_and_the_bot() {
    // The contract bullet, literally: no `@` before David (finding F6).
    assert_eq!(
        reply_mentions(&named_roster(), "thanks David and @dp-kyber-bot"),
        sorted_pubkeys(&["O", "O2", "dp-kyber-bot"])
    );
}

#[test]
fn a_bare_owner_name_alone_tags_both_owner_keys() {
    assert_eq!(
        reply_mentions(&named_roster(), "thanks David"),
        sorted_pubkeys(&["O", "O2"])
    );
}

#[test]
fn a_bare_owner_name_that_is_the_whole_text_tags_both_owner_keys() {
    assert_eq!(
        reply_mentions(&named_roster(), "David"),
        sorted_pubkeys(&["O", "O2"])
    );
}

#[test]
fn a_lower_case_bare_owner_name_followed_by_punctuation_tags_both_owner_keys() {
    assert_eq!(
        reply_mentions(&named_roster(), "thanks david!"),
        sorted_pubkeys(&["O", "O2"])
    );
}

#[test]
fn an_upper_case_bare_owner_name_tags_both_owner_keys() {
    assert_eq!(
        reply_mentions(&named_roster(), "DAVID, over to you"),
        sorted_pubkeys(&["O", "O2"])
    );
}

#[test]
fn a_bare_owner_name_at_the_start_of_a_longer_word_tags_nobody() {
    assert_eq!(
        reply_mentions(&named_roster(), "thanks Davidson"),
        Vec::new()
    );
}

#[test]
fn a_bare_owner_name_at_the_end_of_a_longer_word_tags_nobody() {
    assert_eq!(
        reply_mentions(&named_roster(), "thanks Eldavid"),
        Vec::new()
    );
}

#[test]
fn a_bare_owner_name_inside_a_code_span_tags_nobody() {
    assert_eq!(
        reply_mentions(&named_roster(), "run `ask David` first"),
        Vec::new()
    );
}

#[test]
fn a_bare_owner_name_inside_a_fenced_block_tags_nobody() {
    assert_eq!(
        reply_mentions(&named_roster(), "see:\n```\nask David\n```\ndone"),
        Vec::new()
    );
}

#[test]
fn a_bare_owner_name_on_a_quoted_line_tags_nobody() {
    assert_eq!(
        reply_mentions(&named_roster(), "> David said so\nall done"),
        Vec::new()
    );
}

#[test]
fn a_bare_owner_name_on_the_line_after_a_quoted_line_still_tags_the_owner() {
    assert_eq!(
        reply_mentions(&named_roster(), "> earlier text\nthanks David"),
        sorted_pubkeys(&["O", "O2"])
    );
}

#[test]
fn a_bare_bot_name_does_not_tag_the_bot() {
    assert_eq!(
        reply_mentions(&named_roster(), "thanks dp-kyber-bot"),
        Vec::new()
    );
}

#[test]
fn a_bare_bot_alias_does_not_tag_the_bot() {
    assert_eq!(reply_mentions(&named_roster(), "thanks kyber"), Vec::new());
}

#[test]
fn a_bare_owner_name_next_to_a_bare_bot_name_tags_only_the_owner() {
    assert_eq!(
        reply_mentions(&named_roster(), "David and dp-kyber-bot"),
        sorted_pubkeys(&["O", "O2"])
    );
}

#[test]
fn an_owner_named_with_and_without_an_at_sign_gets_each_key_once() {
    let tagged = mentions_for_reply("@David, thanks David", &named_roster());

    assert_eq!(tagged.len(), 2, "got {tagged:?}");
    assert_eq!(sorted(tagged), sorted_pubkeys(&["O", "O2"]));
}

#[test]
fn the_owner_and_a_bot_named_in_both_owner_forms_get_each_key_once() {
    let tagged = mentions_for_reply("David, @David and @dp-kyber-bot", &named_roster());

    assert_eq!(tagged.len(), 3, "got {tagged:?}");
    assert_eq!(sorted(tagged), sorted_pubkeys(&["O", "O2", "dp-kyber-bot"]));
}

#[test]
fn the_owner_keys_come_back_in_one_fixed_order_whatever_the_form_or_the_call() {
    let roster = named_roster();
    let expected = mentions_for_reply("@David", &roster);
    assert_eq!(expected.len(), 2, "got {expected:?}");

    for text in [
        "@David",
        "David",
        "David and @David",
        "@David, thanks david",
    ] {
        for _ in 0..32 {
            assert_eq!(mentions_for_reply(text, &roster), expected, "text {text:?}");
        }
    }
}
