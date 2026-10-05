//! Task 1.3 (RED): NIP-10 thread position.
//!
//! Covers R5.1 to R5.4 (types), R8.2 and R62.4, against design section 5.2:
//! `thread_position(&[Vec<String>]) -> ThreadPos` reads an event's raw tag arrays through
//! `buzz_core::nip10::parse_thread_markers_from_parts` and `ThreadMarkers::resolve()`.
//!
//! - A `root` and a `reply` marker give `Reply { root, parent }`.
//! - A `reply` marker alone gives a direct reply, with `parent == root`.
//! - No `reply` marker, a lone `root` marker, or a marker whose event id is not 64 hex
//!   characters, gives `TopLevel`.
//!
//! Event ids are deterministic fake 64-hex values from `common::event_id_hex`; a literal
//! `"R"` would not be a valid NIP-10 id, so `R` and `P` below are names, not ids.
//!
//! Design 5.2 fixes the shape of `ThreadPos` but not its derives, so each test reads the result
//! through `observe`, which copies it into a local enum. The assertions then depend only on the
//! two variants and their fields.

mod common;

use common::{event_id_hex, h_tag};
use router_core::thread::{thread_position, ThreadPos};

/// A local copy of `ThreadPos` that carries the derives the assertions need.
#[derive(Debug, PartialEq, Eq)]
enum Position {
    TopLevel,
    Reply { root: String, parent: String },
}

fn observe(position: &ThreadPos) -> Position {
    match position {
        ThreadPos::TopLevel => Position::TopLevel,
        ThreadPos::Reply { root, parent } => Position::Reply {
            root: root.as_str().to_owned(),
            parent: parent.as_str().to_owned(),
        },
    }
}

fn position_of(tags: &[Vec<String>]) -> Position {
    observe(&thread_position(tags))
}

/// An `["e", <id>, "", <marker>]` tag.
fn e_tag(id: &str, marker: &str) -> Vec<String> {
    vec![
        "e".to_owned(),
        id.to_owned(),
        String::new(),
        marker.to_owned(),
    ]
}

fn reply(root: &str, parent: &str) -> Position {
    Position::Reply {
        root: event_id_hex(root),
        parent: event_id_hex(parent),
    }
}

// ---------------------------------------------------------------------------------------------
// The contract's behaviour bullets
// ---------------------------------------------------------------------------------------------

#[test]
fn a_reply_marker_alone_is_a_direct_reply_to_the_root() {
    let tags = vec![e_tag(&event_id_hex("R"), "reply")];

    assert_eq!(position_of(&tags), reply("R", "R"));
}

#[test]
fn root_and_reply_markers_give_the_root_and_the_parent() {
    let tags = vec![
        e_tag(&event_id_hex("R"), "root"),
        e_tag(&event_id_hex("P"), "reply"),
    ];

    assert_eq!(position_of(&tags), reply("R", "P"));
}

#[test]
fn a_lone_root_marker_is_top_level() {
    let tags = vec![e_tag(&event_id_hex("R"), "root")];

    assert_eq!(position_of(&tags), Position::TopLevel);
}

#[test]
fn no_e_tag_is_top_level() {
    let tags = vec![h_tag(), vec!["p".to_owned(), common::pubkey_hex("A")]];

    assert_eq!(position_of(&tags), Position::TopLevel);
}

#[test]
fn a_reply_marker_with_an_invalid_event_id_is_top_level() {
    let tags = vec![e_tag("bad", "reply")];

    assert_eq!(position_of(&tags), Position::TopLevel);
}

// ---------------------------------------------------------------------------------------------
// Edge cases of the same rule (R62.4): the NIP-10 resolution in buzz-core, not a router rule
// ---------------------------------------------------------------------------------------------

#[test]
fn an_empty_tag_list_is_top_level() {
    assert_eq!(position_of(&[]), Position::TopLevel);
}

#[test]
fn a_valid_root_marker_with_an_invalid_reply_id_is_top_level() {
    let tags = vec![e_tag(&event_id_hex("R"), "root"), e_tag("bad", "reply")];

    assert_eq!(position_of(&tags), Position::TopLevel);
}

#[test]
fn an_invalid_root_id_is_ignored_and_a_valid_reply_marker_is_a_direct_reply() {
    let tags = vec![e_tag("bad", "root"), e_tag(&event_id_hex("P"), "reply")];

    assert_eq!(position_of(&tags), reply("P", "P"));
}

#[test]
fn an_e_tag_without_a_marker_is_ignored() {
    let tags = vec![vec!["e".to_owned(), event_id_hex("R")]];

    assert_eq!(position_of(&tags), Position::TopLevel);
}

#[test]
fn tags_other_than_markers_do_not_change_the_position() {
    let tags = vec![
        h_tag(),
        e_tag(&event_id_hex("R"), "root"),
        vec!["p".to_owned(), common::pubkey_hex("B")],
        e_tag(&event_id_hex("P"), "reply"),
        vec!["e".to_owned(), event_id_hex("unmarked")],
    ];

    assert_eq!(position_of(&tags), reply("R", "P"));
}
