//! Task 1.3 (RED): author classification.
//!
//! Covers R4.2 and R4.3, and assumption A12 (the `owner_is_ours` flag), against design
//! section 5.3: `classify(&InEvent, &Roster) -> AuthorClass` tests, in this order,
//!
//! 1. `Owner`: the author's pubkey is in `owner.pubkeys`;
//! 2. `Bot(name)`: the author's pubkey is a roster bot's;
//! 3. `ForeignBot { owner_is_ours }`: the event carries an `auth` tag that
//!    `buzz_sdk::nip_oa::verify_auth_tag` accepts for the author's pubkey, and `owner_is_ours`
//!    is whether the verified owner pubkey is in `owner.pubkeys`;
//! 4. `Human`: anything else, including an `auth` tag that fails verification.
//!
//! The roster comes from `common::roster()` (built through `parse_roster`): owner keys `O` and
//! `O2`, bots `A`, `B` and `C`. Every key is a deterministic fixture key,
//! `sha256("buzz-router-fixture:" + name)`. The `auth` tags are real NIP-OA tags computed with
//! `buzz_sdk::nip_oa::compute_auth_tag`.
//!
//! Design 5.3 fixes the shape of `AuthorClass` but not its derives, so each test reads the
//! result through `observe`, which copies it into a local enum. The assertions then depend only
//! on the four variants and their fields.

mod common;

use common::{auth_tag, in_event, keys, roster};
use nostr::Keys;
use router_core::classify::{classify, AuthorClass};

/// A local copy of `AuthorClass` that carries the derives the assertions need.
#[derive(Debug, PartialEq, Eq)]
enum Class {
    Owner,
    Bot(String),
    ForeignBot { owner_is_ours: bool },
    Human,
}

fn observe(class: &AuthorClass) -> Class {
    match class {
        AuthorClass::Owner => Class::Owner,
        AuthorClass::Bot(name) => Class::Bot(name.as_str().to_owned()),
        AuthorClass::ForeignBot { owner_is_ours } => Class::ForeignBot {
            owner_is_ours: *owner_is_ours,
        },
        AuthorClass::Human => Class::Human,
    }
}

/// The class of a kind-9 event authored by `author` and carrying `tags` after its `h` tag.
fn class_of(author: &Keys, tags: Vec<Vec<String>>) -> Class {
    observe(&classify(&in_event(author, tags), &roster()))
}

fn foreign_bot(owner_is_ours: bool) -> Class {
    Class::ForeignBot { owner_is_ours }
}

// ---------------------------------------------------------------------------------------------
// The contract's behaviour bullets
// ---------------------------------------------------------------------------------------------

#[test]
fn an_owner_pubkey_is_owner() {
    assert_eq!(class_of(&keys("O"), vec![]), Class::Owner);
}

#[test]
fn every_owner_pubkey_is_owner() {
    assert_eq!(class_of(&keys("O2"), vec![]), Class::Owner);
}

#[test]
fn a_roster_bot_is_that_bot() {
    assert_eq!(class_of(&keys("A"), vec![]), Class::Bot("A".to_owned()));
    assert_eq!(class_of(&keys("B"), vec![]), Class::Bot("B".to_owned()));
}

#[test]
fn an_unknown_author_with_an_auth_tag_from_an_owner_key_is_a_foreign_bot_with_owner_is_ours() {
    let agent = keys("foreign-agent");
    let tags = vec![auth_tag(&keys("O"), &agent, "")];

    assert_eq!(class_of(&agent, tags), foreign_bot(true));
}

#[test]
fn an_auth_tag_from_the_owners_second_key_also_sets_owner_is_ours() {
    let agent = keys("foreign-agent");
    let tags = vec![auth_tag(&keys("O2"), &agent, "")];

    assert_eq!(class_of(&agent, tags), foreign_bot(true));
}

#[test]
fn an_unknown_author_with_an_auth_tag_from_a_stranger_is_a_foreign_bot_without_owner_is_ours() {
    let agent = keys("foreign-agent");
    let tags = vec![auth_tag(&keys("stranger"), &agent, "")];

    assert_eq!(class_of(&agent, tags), foreign_bot(false));
}

#[test]
fn an_auth_tag_computed_for_a_different_agent_pubkey_is_human() {
    let author = keys("foreign-agent");
    let other_agent = keys("some-other-agent");
    let tags = vec![auth_tag(&keys("O"), &other_agent, "")];

    assert_eq!(class_of(&author, tags), Class::Human);
}

#[test]
fn no_auth_tag_is_human() {
    assert_eq!(class_of(&keys("foreign-agent"), vec![]), Class::Human);
}

// ---------------------------------------------------------------------------------------------
// The order of the tests (R4.2) and the fall-through to human (R4.3)
// ---------------------------------------------------------------------------------------------

#[test]
fn an_owner_carrying_a_valid_auth_tag_is_still_owner() {
    let owner = keys("O");
    let tags = vec![auth_tag(&keys("stranger"), &owner, "")];

    assert_eq!(class_of(&owner, tags), Class::Owner);
}

#[test]
fn a_roster_bot_carrying_a_valid_auth_tag_is_still_that_bot() {
    let bot = keys("A");
    let tags = vec![auth_tag(&keys("O"), &bot, "")];

    assert_eq!(class_of(&bot, tags), Class::Bot("A".to_owned()));
}

#[test]
fn a_malformed_auth_tag_is_human() {
    let author = keys("foreign-agent");
    let tags = vec![vec!["auth".to_owned(), "not-a-pubkey".to_owned()]];

    assert_eq!(class_of(&author, tags), Class::Human);
}

#[test]
fn an_auth_tag_among_other_tags_is_still_found() {
    let agent = keys("foreign-agent");
    let tags = vec![
        vec!["p".to_owned(), common::pubkey_hex("A")],
        auth_tag(&keys("O"), &agent, ""),
        vec!["e".to_owned(), common::event_id_hex("R")],
    ];

    assert_eq!(class_of(&agent, tags), foreign_bot(true));
}
