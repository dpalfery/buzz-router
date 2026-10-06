//! Task 1.11 (RED): `buzz-router route --replay FILE [--roster P]`.
//!
//! Covers requirements 55.2 to 55.4 against design 12.1 and 5.8, and the output shape the
//! architect fixed in decision D2. Each test runs the real binary,
//! `env!("CARGO_BIN_EXE_buzz-router")`, in a sandbox of temporary directories, on a JSON Lines
//! file of signed events, the form `capture` writes (R55.3):
//!
//! - one JSON line per valid signed event, with its `event_id`, `created_at`, `class`, `control`
//!   and `decisions` (`prints_one_json_line_per_valid_signed_event_with_its_decisions`);
//! - a line with a bad signature is skipped (`a_line_with_a_bad_signature_is_skipped`), and so is
//!   every other line D2 lists as unreplayable (`lines_that_cannot_be_replayed_print_nothing`);
//! - the events are replayed sorted by `(created_at, id)`
//!   (`events_are_replayed_in_created_at_then_id_order`), and the replayer carries its state from
//!   one event to the next (`a_stop_before_a_mention_changes_that_mentions_decisions` and the
//!   cap, budget and quiet tests);
//! - no `router.toml`, no network, no database: the command reads the roster it is given and
//!   leaves the config and data directories empty
//!   (`replay_needs_no_router_toml_and_writes_nothing_to_the_config_or_data_dirs`). There is no
//!   test for the absence of network traffic itself: with no `router.toml` the command has no
//!   relay address to reach;
//! - `--roster P` names the roster, and without it the configured roster is used (R55.4): here,
//!   the `roster.toml` in the config dir when there is no `router.toml`. Whether `route --replay`
//!   reads `roster_path` from a `router.toml` that is next to it is not fixed by the spec, so no
//!   test puts a `router.toml` next to a replay.
//!
//! **What is asserted of the output (decision D2).** Each line is compared as text, so the key
//! order, the presence of every key and the compact form are all pinned. The line is
//!
//! ```text
//! {"event_id":"<64 lowercase hex>","created_at":<unix seconds>,"class":<CLASS>,"control":<CONTROL>,"decisions":[<DECISION>,...]}
//! ```
//!
//! - `CLASS` is `"owner"`, `"bot"`, `"foreign_bot"` or `"human"` (`classes_print_as_...`);
//! - `CONTROL` is `null`, or `{"stop":SCOPE}`, `{"resume":SCOPE}` or `{"cancel":SCOPE}`, with
//!   `SCOPE` either `"all"` or `{"bots":[...]}` with the names sorted ascending
//!   (`control_commands_print_...`);
//! - a wake is `{"wake":{"bot":B,"reason":R,"priority":P,"debounce":D}}` (the owner's reasons
//!   `mention`, `everyone`, `default_bot`, `participant` and `reply_target`; a human's mention; a
//!   bot's `discussion` and `bot_mention`), and a suppression is
//!   `{"suppress":{"bot":B,"why":W}}` with `W` one of `halted`, `cap`, `quiet`, `budget` and
//!   `respond_to` (one test for each). The decisions keep the order `route` gives, which is by
//!   bot name, and an event with none prints `[]`.
//!
//! The expected texts are written out by the small helpers `line`, `wake` and `suppress` below,
//! which only format strings; they do not call the router.
//!
//! The roster is built from deterministic fixture keys, `sha256("buzz-router-fixture:" + name)`:
//! owner `O`, bots `A`, `B` and `C`, all owner-only unless a test says otherwise, covering every
//! channel, and a stranger `H`, all in one channel. Every event is a signed post in that channel
//! at midday UTC, outside the default quiet hours, unless a test says otherwise. No real key,
//! pubkey or relay address appears.

#![allow(
    clippy::expect_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use nostr::secp256k1::Message;
use nostr::{Event, EventBuilder, JsonUtil, Keys, Kind, SecretKey, Tag, Timestamp};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// 2023-11-14T12:00:00Z in unix seconds. Every event time is an offset from it.
const NOON: u64 = 1_699_963_200;

/// 2023-11-14T02:00:00Z, inside the default quiet hours `23:00-07:00` of a UTC owner.
const TWO_AM: u64 = NOON - 10 * 3_600;

/// `control` for an event that is not a control command.
const NO_CONTROL: &str = "null";

/// `sha256("buzz-router-fixture:" + name)`, the repository's fixture-key convention.
fn fixture_digest(name: &str) -> [u8; 32] {
    Sha256::digest(format!("buzz-router-fixture:{name}")).into()
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The deterministic fixture key for `name`.
fn keys(name: &str) -> Keys {
    let secret = SecretKey::from_slice(&fixture_digest(name))
        .expect("a sha256 digest is a valid secret key");
    Keys::new(secret)
}

/// The 64-character lowercase hex public key of the fixture key `name`.
fn pubkey_hex(name: &str) -> String {
    keys(name).public_key().to_hex()
}

/// The fixture channel's UUID, in its hyphenated lowercase form.
fn room() -> String {
    let hex = hex(&fixture_digest("channel:room")[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A roster with the owner `O`, one channel and the given bots as `(name, respond_to)`, each
/// covering every channel. `default_bot` names the channel's default bot, and `limits` is the
/// body of a `[limits]` table, empty for the defaults.
fn roster_text(bots: &[(&str, &str)], default_bot: Option<&str>, limits: &str) -> String {
    let mut text = format!(
        r#"version = 1

[owner]
name = "Fixture Owner"
pubkeys = ["{owner}"]
timezone = "UTC"

[[channels]]
id = "{room}"
name = "fixture-room"
"#,
        owner = pubkey_hex("O"),
        room = room(),
    );
    if let Some(bot) = default_bot {
        text.push_str(&format!("default_bot = \"{bot}\"\n"));
    }
    for (bot, respond_to) in bots {
        text.push_str(&format!(
            r#"
[[bots]]
name = "{bot}"
pubkey = "{key}"
channels = ["*"]
respond_to = "{respond_to}"
"#,
            key = pubkey_hex(bot),
        ));
    }
    if !limits.is_empty() {
        text.push_str(&format!("\n[limits]\n{limits}\n"));
    }
    text
}

/// A valid roster with one owner-only bot for each name in `bots`.
fn roster_with(bots: &[&str]) -> String {
    let bots: Vec<(&str, &str)> = bots.iter().map(|bot| (*bot, "owner-only")).collect();
    roster_text(&bots, None, "")
}

/// The `h` tag of the fixture channel.
fn h_tag() -> Tag {
    Tag::parse(["h".to_owned(), room()]).expect("an h tag")
}

/// An unmarked or marked `e` tag.
fn e_tag(id: &str, marker: Option<&str>) -> Tag {
    let mut parts = vec!["e".to_owned(), id.to_owned()];
    if let Some(marker) = marker {
        parts.push(String::new());
        parts.push(marker.to_owned());
    }
    Tag::parse(parts).expect("an e tag")
}

/// A signed event of `kind` by `author` at `created_at`, with `tags` after the `h` tag of the
/// fixture channel.
fn signed(author: &Keys, kind: u16, content: &str, created_at: u64, tags: Vec<Tag>) -> Event {
    let mut all = vec![h_tag()];
    all.extend(tags);
    EventBuilder::new(Kind::Custom(kind), content)
        .tags(all)
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(author)
        .expect("the event signs")
}

/// A signed top-level kind-9 post in the fixture channel by the fixture key `author`.
fn event(author: &str, content: &str, created_at: u64) -> Event {
    signed(&keys(author), 9, content, created_at, vec![])
}

/// A signed kind-9 reply by the fixture key `author` to `parent`, in the thread rooted at `root`,
/// tagged as NIP-10 and `buzz_sdk` write it: a direct reply (`parent` is `root`) is one `reply`
/// marker, a nested one a `root` marker and a `reply` marker.
fn reply(author: &str, content: &str, created_at: u64, root: &Event, parent: &Event) -> Event {
    let root_id = root.id.to_hex();
    let tags = if root.id == parent.id {
        vec![e_tag(&root_id, Some("reply"))]
    } else {
        vec![
            e_tag(&root_id, Some("root")),
            e_tag(&parent.id.to_hex(), Some("reply")),
        ]
    };
    signed(&keys(author), 9, content, created_at, tags)
}

/// The NIP-OA tag `["auth", <owner>, "", <sig>]` that `owner` signs for `agent`: a BIP-340
/// signature over the SHA-256 of `nostr:agent-auth:<agent pubkey hex>:` (no conditions). It is
/// computed here from that definition, not with the router's own verifier.
fn auth_tag(owner: &Keys, agent: &Keys) -> Tag {
    let preimage = format!("nostr:agent-auth:{}:", agent.public_key().to_hex());
    let digest: [u8; 32] = Sha256::digest(preimage).into();
    let signature = owner.sign_schnorr(&Message::from_digest(digest));
    Tag::parse([
        "auth".to_owned(),
        owner.public_key().to_hex(),
        String::new(),
        signature.to_string(),
    ])
    .expect("an auth tag")
}

/// `event` as a line that carries the signature of `donor`, which does not sign it.
fn with_the_signature_of(event: &Event, donor: &Event) -> String {
    let mut value = serde_json::to_value(event).expect("an event is JSON");
    let donor = serde_json::to_value(donor).expect("an event is JSON");
    value["sig"] = donor["sig"].clone();
    value.to_string()
}

/// The lines as a JSON Lines file body, each ended by a newline.
fn jsonl(lines: &[String]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

/// The events as the lines of a JSON Lines file.
fn jsonl_of(events: &[&Event]) -> String {
    jsonl(
        &events
            .iter()
            .map(|event| event.as_json())
            .collect::<Vec<_>>(),
    )
}

/// The line `route --replay` must print for `event` (decision D2): its id, its time, the class of
/// its author as the text `class`, the `control` text (`null` or an object) and the `decisions`.
fn line(event: &Event, class: &str, control: &str, decisions: &[String]) -> String {
    format!(
        r#"{{"event_id":"{}","created_at":{},"class":"{class}","control":{control},"decisions":[{}]}}"#,
        event.id.to_hex(),
        event.created_at.as_secs(),
        decisions.join(",")
    )
}

/// One wake decision (decision D2).
fn wake(bot: &str, reason: &str, priority: &str, debounce: bool) -> String {
    format!(
        r#"{{"wake":{{"bot":"{bot}","reason":"{reason}","priority":"{priority}","debounce":{debounce}}}}}"#
    )
}

/// One suppress decision (decision D2).
fn suppress(bot: &str, why: &str) -> String {
    format!(r#"{{"suppress":{{"bot":"{bot}","why":"{why}"}}}}"#)
}

/// Temporary directories for one test: a config dir and a data dir, both empty, a home dir, and a
/// directory for the roster and event files, which is outside the config dir.
struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let sandbox = Self {
            root: tempfile::tempdir().expect("a temporary directory"),
        };
        for dir in [
            sandbox.config(),
            sandbox.data(),
            sandbox.home(),
            sandbox.files(),
        ] {
            fs::create_dir(dir).expect("a sandbox directory");
        }
        sandbox
    }

    fn config(&self) -> PathBuf {
        self.root.path().join("config")
    }

    fn data(&self) -> PathBuf {
        self.root.path().join("data")
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    fn files(&self) -> PathBuf {
        self.root.path().join("files")
    }

    /// Writes `contents` to `name` in the files dir and returns its path.
    fn write_file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.files().join(name);
        fs::write(&path, contents).expect("a file");
        path
    }

    /// Writes `contents` to `name` in the config dir.
    fn write_config(&self, name: &str, contents: &str) {
        fs::write(self.config().join(name), contents).expect("a config file");
    }

    /// The binary, with no router variables inherited and a home directory inside the sandbox, so
    /// that nothing the test does reaches the user's own directories.
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_buzz-router"));
        command
            .env("HOME", self.home())
            .env("USERPROFILE", self.home())
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("BUZZ_ROUTER_CONFIG_DIR")
            .env_remove("BUZZ_ROUTER_DATA_DIR")
            .env_remove("BUZZ_ROUTER_LOG")
            .stdin(Stdio::null());
        command
    }

    /// Runs `buzz-router route --replay <events> [--roster <roster>]` against the sandbox dirs.
    fn replay(&self, events: &Path, roster: Option<&Path>) -> Output {
        let mut command = self.command();
        command
            .arg("--config-dir")
            .arg(self.config())
            .arg("--data-dir")
            .arg(self.data())
            .args(["route", "--replay"])
            .arg(events);
        if let Some(roster) = roster {
            command.arg("--roster").arg(roster);
        }
        command.output().expect("the binary starts")
    }

    /// Replays `events` against `roster_text` and returns the lines printed. Asserts exit 0.
    fn replay_events(&self, roster_text: &str, events: &[&Event]) -> Vec<String> {
        let roster = self.write_file("r.toml", roster_text);
        let file = self.write_file("events.jsonl", &jsonl_of(events));
        printed(&self.replay(&file, Some(&roster)))
    }
}

/// The lines of the command's stdout. Asserts that the command exited 0 and that stdout, if it
/// has anything, ends with a newline.
fn printed(output: &Output) -> Vec<String> {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "exit code; stdout: {stdout:?}, stderr: {stderr:?}"
    );
    assert!(
        stdout.is_empty() || stdout.ends_with('\n'),
        "every line ends with a newline: {stdout:?}"
    );
    stdout.lines().map(str::to_owned).collect()
}

/// The event ids of the printed lines, in the order printed.
fn event_ids(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            let value: Value = serde_json::from_str(line).expect("every stdout line is JSON");
            value["event_id"].as_str().unwrap_or_default().to_owned()
        })
        .collect()
}

/// How many entries `dir` holds.
fn entries_in(dir: &Path) -> usize {
    fs::read_dir(dir).expect("a readable directory").count()
}

#[test]
fn prints_one_json_line_per_valid_signed_event_with_its_decisions() {
    let sandbox = Sandbox::new();
    // The owner names bot A; a stranger names nobody; the owner names both bots.
    let first = event("O", "@A hi", NOON);
    let second = event("H", "just chatting", NOON + 60);
    let third = event("O", "@A @B go", NOON + 120);

    let lines = sandbox.replay_events(&roster_with(&["A", "B"]), &[&first, &second, &third]);

    // The first line is also written out in full, so that the `line` helper cannot hide a typo.
    assert_eq!(
        lines.first().map(String::as_str),
        Some(
            format!(
                r#"{{"event_id":"{}","created_at":{},"class":"owner","control":null,"decisions":[{{"wake":{{"bot":"A","reason":"mention","priority":"owner","debounce":false}}}}]}}"#,
                first.id.to_hex(),
                NOON
            )
            .as_str()
        )
    );
    assert_eq!(
        lines,
        [
            line(
                &first,
                "owner",
                NO_CONTROL,
                &[wake("A", "mention", "owner", false)]
            ),
            line(&second, "human", NO_CONTROL, &[]),
            line(
                &third,
                "owner",
                NO_CONTROL,
                &[
                    wake("A", "mention", "owner", false),
                    wake("B", "mention", "owner", false)
                ]
            ),
        ]
    );
}

#[test]
fn a_line_with_a_bad_signature_is_skipped() {
    let sandbox = Sandbox::new();
    let roster = sandbox.write_file("r.toml", &roster_with(&["A"]));
    let before = event("O", "@A one", NOON);
    let forged = event("O", "@A forged", NOON + 60);
    let after = event("O", "@A two", NOON + 120);
    let file = sandbox.write_file(
        "events.jsonl",
        &jsonl(&[
            before.as_json(),
            with_the_signature_of(&forged, &before),
            after.as_json(),
        ]),
    );

    let output = sandbox.replay(&file, Some(&roster));

    assert_eq!(
        printed(&output),
        [
            line(
                &before,
                "owner",
                NO_CONTROL,
                &[wake("A", "mention", "owner", false)]
            ),
            line(
                &after,
                "owner",
                NO_CONTROL,
                &[wake("A", "mention", "owner", false)]
            ),
        ],
        "the forged event {} must not be replayed",
        forged.id.to_hex()
    );
}

#[test]
fn lines_that_cannot_be_replayed_print_nothing() {
    let sandbox = Sandbox::new();
    let roster = sandbox.write_file("r.toml", &roster_with(&["A"]));
    let owner = keys("O");
    let first = event("O", "@A one", NOON);
    let last = event("O", "@A two", NOON + 600);
    // Signed and valid, but of a kind other than 9 and 40003.
    let note = signed(&owner, 1, "@A a note", NOON + 60, vec![]);
    // A kind-9 post with no `h` tag, and one whose `h` tag holds no UUID.
    let untagged = EventBuilder::new(Kind::Custom(9), "@A nowhere")
        .custom_created_at(Timestamp::from_secs(NOON + 120))
        .sign_with_keys(&owner)
        .expect("the event signs");
    let not_a_uuid = EventBuilder::new(Kind::Custom(9), "@A elsewhere")
        .tags([Tag::parse(["h", "general"]).expect("an h tag")])
        .custom_created_at(Timestamp::from_secs(NOON + 180))
        .sign_with_keys(&owner)
        .expect("the event signs");
    // A kind-40003 edit is replayed. Its target was never seen, so it wakes nobody.
    let unseen = hex(&fixture_digest("message:unseen"));
    let edit = signed(
        &owner,
        40003,
        "@A edited",
        NOON + 300,
        vec![e_tag(&unseen, None)],
    );
    let file = sandbox.write_file(
        "events.jsonl",
        &jsonl(&[
            first.as_json(),
            "this is not JSON".to_owned(),
            String::new(),
            note.as_json(),
            untagged.as_json(),
            not_a_uuid.as_json(),
            edit.as_json(),
            last.as_json(),
        ]),
    );

    let output = sandbox.replay(&file, Some(&roster));

    assert_eq!(
        printed(&output),
        [
            line(
                &first,
                "owner",
                NO_CONTROL,
                &[wake("A", "mention", "owner", false)]
            ),
            line(&edit, "owner", NO_CONTROL, &[]),
            line(
                &last,
                "owner",
                NO_CONTROL,
                &[wake("A", "mention", "owner", false)]
            ),
        ]
    );
}

#[test]
fn events_are_replayed_in_created_at_then_id_order() {
    let sandbox = Sandbox::new();
    let earliest = event("O", "first", NOON);
    let latest = event("O", "fourth", NOON + 120);
    // Two events share a time, so only their ids order them.
    let tie_one = event("O", "second", NOON + 60);
    let tie_two = event("O", "third", NOON + 60);
    let (smaller, larger) = if tie_one.id.to_hex() < tie_two.id.to_hex() {
        (tie_one, tie_two)
    } else {
        (tie_two, tie_one)
    };
    // The file is in neither time order nor id order: the tied events are in descending id order,
    // so a sort on the time alone would keep them that way.
    let lines = sandbox.replay_events(
        &roster_with(&["A"]),
        &[&latest, &larger, &earliest, &smaller],
    );

    assert_eq!(
        event_ids(&lines),
        [
            earliest.id.to_hex(),
            smaller.id.to_hex(),
            larger.id.to_hex(),
            latest.id.to_hex()
        ]
    );
}

#[test]
fn a_stop_before_a_mention_changes_that_mentions_decisions() {
    let sandbox = Sandbox::new();
    let roster = roster_with(&["A"]);
    let stop = event("O", "stop", NOON);
    let resume = event("O", "resume", NOON + 30);
    let mention = event("O", "@A hi", NOON + 60);
    let woken = wake("A", "mention", "owner", false);

    let alone = sandbox.replay_events(&roster, &[&mention]);
    let halted = sandbox.replay_events(&roster, &[&stop, &mention]);
    let resumed = sandbox.replay_events(&roster, &[&stop, &resume, &mention]);

    assert_eq!(
        alone,
        [line(
            &mention,
            "owner",
            NO_CONTROL,
            std::slice::from_ref(&woken)
        )]
    );
    assert_eq!(
        halted,
        [
            line(&stop, "owner", r#"{"stop":"all"}"#, &[]),
            line(&mention, "owner", NO_CONTROL, &[suppress("A", "halted")]),
        ]
    );
    assert_eq!(
        resumed,
        [
            line(&stop, "owner", r#"{"stop":"all"}"#, &[]),
            line(&resume, "owner", r#"{"resume":"all"}"#, &[]),
            line(&mention, "owner", NO_CONTROL, &[woken]),
        ],
        "after stop and resume the mention must be decided as if neither had come"
    );
}

#[test]
fn control_commands_print_their_kind_and_a_scope_with_sorted_bots() {
    let sandbox = Sandbox::new();
    // No bot named: every bot. One bot named. Two bots named, in descending order, so that only a
    // sorted scope prints them ascending.
    let stop_all = event("O", "stop", NOON);
    let stop_one = event("O", "@A stop", NOON + 60);
    let stop_two = event("O", "@B @A stop", NOON + 120);
    let resume_all = event("O", "resume", NOON + 180);
    let resume_two = event("O", "@B @A resume", NOON + 240);
    let cancel_all = event("O", "!cancel", NOON + 300);
    let cancel_one = event("O", "@A !cancel", NOON + 360);
    let cancel_two = event("O", "@B @A !cancel", NOON + 420);

    let lines = sandbox.replay_events(
        &roster_with(&["A", "B"]),
        &[
            &stop_all,
            &stop_one,
            &stop_two,
            &resume_all,
            &resume_two,
            &cancel_all,
            &cancel_one,
            &cancel_two,
        ],
    );

    assert_eq!(
        lines,
        [
            line(&stop_all, "owner", r#"{"stop":"all"}"#, &[]),
            line(&stop_one, "owner", r#"{"stop":{"bots":["A"]}}"#, &[]),
            line(&stop_two, "owner", r#"{"stop":{"bots":["A","B"]}}"#, &[]),
            line(&resume_all, "owner", r#"{"resume":"all"}"#, &[]),
            line(
                &resume_two,
                "owner",
                r#"{"resume":{"bots":["A","B"]}}"#,
                &[]
            ),
            line(&cancel_all, "owner", r#"{"cancel":"all"}"#, &[]),
            line(&cancel_one, "owner", r#"{"cancel":{"bots":["A"]}}"#, &[]),
            line(
                &cancel_two,
                "owner",
                r#"{"cancel":{"bots":["A","B"]}}"#,
                &[]
            ),
        ]
    );
}

#[test]
fn classes_print_as_owner_bot_foreign_bot_and_human() {
    let sandbox = Sandbox::new();
    let from_owner = event("O", "hello", NOON);
    let from_bot = event("A", "hello", NOON + 60);
    // An author the roster does not know, with a valid NIP-OA tag from the owner's key.
    let agent = keys("foreign-agent");
    let from_foreign_bot = signed(
        &agent,
        9,
        "hello",
        NOON + 120,
        vec![auth_tag(&keys("O"), &agent)],
    );
    let from_human = event("H", "hello", NOON + 180);

    let lines = sandbox.replay_events(
        &roster_with(&["A"]),
        &[&from_owner, &from_bot, &from_foreign_bot, &from_human],
    );

    assert_eq!(
        lines,
        [
            line(&from_owner, "owner", NO_CONTROL, &[]),
            line(&from_bot, "bot", NO_CONTROL, &[]),
            line(&from_foreign_bot, "foreign_bot", NO_CONTROL, &[]),
            line(&from_human, "human", NO_CONTROL, &[]),
        ]
    );
}

#[test]
fn an_owner_post_prints_wakes_for_a_mention_everyone_and_the_default_bot() {
    let sandbox = Sandbox::new();
    let roster = roster_text(
        &[
            ("A", "owner-only"),
            ("B", "owner-only"),
            ("C", "owner-only"),
        ],
        Some("A"),
        "",
    );
    let mention = event("O", "@B hi", NOON);
    let everyone = event("O", "@everyone plan", NOON + 60);
    let untagged = event("O", "hey", NOON + 120);

    let lines = sandbox.replay_events(&roster, &[&mention, &everyone, &untagged]);

    assert_eq!(
        lines,
        [
            line(
                &mention,
                "owner",
                NO_CONTROL,
                &[wake("B", "mention", "owner", false)]
            ),
            line(
                &everyone,
                "owner",
                NO_CONTROL,
                &[
                    wake("A", "everyone", "owner", false),
                    wake("B", "everyone", "owner", false),
                    wake("C", "everyone", "owner", false)
                ]
            ),
            line(
                &untagged,
                "owner",
                NO_CONTROL,
                &[wake("A", "default_bot", "owner", false)]
            ),
        ]
    );
}

#[test]
fn an_owner_reply_prints_participant_and_reply_target_wakes() {
    let sandbox = Sandbox::new();
    // The owner names A, so A takes part in the thread, and A answers in it.
    let ask = event("O", "@A hi", NOON);
    let answer = reply("A", "done", NOON + 60, &ask, &ask);
    // An untagged owner reply to the thread root goes to its participant; an untagged owner reply
    // to A's own message goes to A as the reply target.
    let to_the_thread = reply("O", "one more thing", NOON + 120, &ask, &ask);
    let to_the_answer = reply("O", "why?", NOON + 180, &ask, &answer);

    let lines = sandbox.replay_events(
        &roster_with(&["A", "B"]),
        &[&ask, &answer, &to_the_thread, &to_the_answer],
    );

    assert_eq!(
        lines,
        [
            line(
                &ask,
                "owner",
                NO_CONTROL,
                &[wake("A", "mention", "owner", false)]
            ),
            line(&answer, "bot", NO_CONTROL, &[]),
            line(
                &to_the_thread,
                "owner",
                NO_CONTROL,
                &[wake("A", "participant", "owner", false)]
            ),
            line(
                &to_the_answer,
                "owner",
                NO_CONTROL,
                &[wake("A", "reply_target", "owner", false)]
            ),
        ]
    );
}

#[test]
fn a_human_mention_prints_a_human_wake_or_a_respond_to_suppression() {
    let sandbox = Sandbox::new();
    // Bot A answers anyone; bot B answers only the owner.
    let roster = roster_text(&[("A", "anyone"), ("B", "owner-only")], None, "");
    let to_a = event("H", "@A hi", NOON);
    let to_b = event("H", "@B hi", NOON + 60);

    let lines = sandbox.replay_events(&roster, &[&to_a, &to_b]);

    assert_eq!(
        lines,
        [
            line(
                &to_a,
                "human",
                NO_CONTROL,
                &[wake("A", "mention", "human", false)]
            ),
            line(&to_b, "human", NO_CONTROL, &[suppress("B", "respond_to")]),
        ]
    );
}

#[test]
fn bot_posts_print_discussion_and_bot_mention_wakes() {
    let sandbox = Sandbox::new();
    // A discussion: the owner addresses everyone, then B posts in the thread.
    let start = event("O", "@everyone let us discuss", NOON);
    let view = reply(
        "B",
        "My view: ship it behind a flag.",
        NOON + 60,
        &start,
        &start,
    );
    // A direct round: the owner names A, then A names B in the thread.
    let ask = event("O", "@A look at this", NOON + 120);
    let hand_over = reply("A", "@B can you check X", NOON + 180, &ask, &ask);

    let lines = sandbox.replay_events(
        &roster_with(&["A", "B", "C"]),
        &[&start, &view, &ask, &hand_over],
    );

    assert_eq!(
        lines,
        [
            line(
                &start,
                "owner",
                NO_CONTROL,
                &[
                    wake("A", "everyone", "owner", false),
                    wake("B", "everyone", "owner", false),
                    wake("C", "everyone", "owner", false)
                ]
            ),
            line(
                &view,
                "bot",
                NO_CONTROL,
                &[
                    wake("A", "discussion", "bot", true),
                    wake("C", "discussion", "bot", true)
                ]
            ),
            line(
                &ask,
                "owner",
                NO_CONTROL,
                &[wake("A", "mention", "owner", false)]
            ),
            line(
                &hand_over,
                "bot",
                NO_CONTROL,
                &[wake("B", "bot_mention", "bot", true)]
            ),
        ]
    );
}

/// Replays an owner `@everyone` at `at` and then B's post in that thread a minute later, against
/// a roster of A, B and C with the given `[limits]` body. Returns the two printed lines and the
/// two events.
fn everyone_then_a_bot_post(at: u64, limits: &str) -> (Vec<String>, Event, Event) {
    let sandbox = Sandbox::new();
    let roster = roster_text(
        &[
            ("A", "owner-only"),
            ("B", "owner-only"),
            ("C", "owner-only"),
        ],
        None,
        limits,
    );
    let start = event("O", "@everyone let us discuss", at);
    let view = reply(
        "B",
        "My view: ship it behind a flag.",
        at + 60,
        &start,
        &start,
    );
    let lines = sandbox.replay_events(&roster, &[&start, &view]);
    (lines, start, view)
}

/// What the owner's `@everyone` prints: a wake for each of A, B and C, whatever the limits.
fn everyone_wakes() -> [String; 3] {
    [
        wake("A", "everyone", "owner", false),
        wake("B", "everyone", "owner", false),
        wake("C", "everyone", "owner", false),
    ]
}

#[test]
fn a_bot_post_inside_quiet_hours_prints_quiet_suppressions() {
    let (lines, start, view) = everyone_then_a_bot_post(TWO_AM, "");

    assert_eq!(
        lines,
        [
            line(&start, "owner", NO_CONTROL, &everyone_wakes()),
            line(
                &view,
                "bot",
                NO_CONTROL,
                &[suppress("A", "quiet"), suppress("C", "quiet")]
            ),
        ]
    );
}

#[test]
fn a_bot_post_over_the_turn_cap_prints_cap_suppressions() {
    // One turn a round: the owner's `@everyone` wake is each bot's turn.
    let (lines, start, view) = everyone_then_a_bot_post(NOON, "turns_per_round = 1");

    assert_eq!(
        lines,
        [
            line(&start, "owner", NO_CONTROL, &everyone_wakes()),
            line(
                &view,
                "bot",
                NO_CONTROL,
                &[suppress("A", "cap"), suppress("C", "cap")]
            ),
        ]
    );
}

#[test]
fn a_bot_post_over_the_hourly_budget_prints_budget_suppressions() {
    // One wake an hour: the owner's `@everyone` wake uses it up.
    let (lines, start, view) = everyone_then_a_bot_post(NOON, "wakes_per_hour = 1");

    assert_eq!(
        lines,
        [
            line(&start, "owner", NO_CONTROL, &everyone_wakes()),
            line(
                &view,
                "bot",
                NO_CONTROL,
                &[suppress("A", "budget"), suppress("C", "budget")]
            ),
        ]
    );
}

#[test]
fn replay_needs_no_router_toml_and_writes_nothing_to_the_config_or_data_dirs() {
    let sandbox = Sandbox::new();
    let roster = sandbox.write_file("r.toml", &roster_with(&["A"]));
    let mention = event("O", "@A hi", NOON);
    let file = sandbox.write_file("events.jsonl", &jsonl_of(&[&mention]));
    assert_eq!(
        entries_in(&sandbox.config()),
        0,
        "no router.toml, no roster"
    );

    let output = sandbox.replay(&file, Some(&roster));

    assert_eq!(printed(&output).len(), 1);
    assert_eq!(
        entries_in(&sandbox.config()),
        0,
        "the config dir stays empty"
    );
    assert_eq!(
        entries_in(&sandbox.data()),
        0,
        "no database or log file is created"
    );
}

#[test]
fn the_roster_flag_names_the_roster_to_replay_against() {
    let sandbox = Sandbox::new();
    let mention = event("O", "@A hi", NOON);

    let lines = sandbox.replay_events(&roster_with(&["A"]), &[&mention]);

    assert_eq!(
        lines,
        [line(
            &mention,
            "owner",
            NO_CONTROL,
            &[wake("A", "mention", "owner", false)]
        )]
    );
}

#[test]
fn the_roster_flag_wins_over_the_roster_in_the_config_dir() {
    let sandbox = Sandbox::new();
    // The configured roster has no bot A, so `@A hi` would be decided for nobody.
    sandbox.write_config("roster.toml", &roster_with(&["B"]));
    let mention = event("O", "@A hi", NOON);

    let lines = sandbox.replay_events(&roster_with(&["A", "B"]), &[&mention]);

    assert_eq!(
        lines,
        [line(
            &mention,
            "owner",
            NO_CONTROL,
            &[wake("A", "mention", "owner", false)]
        )]
    );
}

#[test]
fn without_the_roster_flag_the_roster_in_the_config_dir_is_used() {
    let sandbox = Sandbox::new();
    sandbox.write_config("roster.toml", &roster_with(&["A"]));
    let mention = event("O", "@A hi", NOON);
    let file = sandbox.write_file("events.jsonl", &jsonl_of(&[&mention]));
    assert!(!sandbox.config().join("router.toml").exists());

    let lines = printed(&sandbox.replay(&file, None));

    assert_eq!(
        lines,
        [line(
            &mention,
            "owner",
            NO_CONTROL,
            &[wake("A", "mention", "owner", false)]
        )]
    );
}
