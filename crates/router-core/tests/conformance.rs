//! Task 1.6 (RED): the routing conformance harness (design section 16.1, requirement 64).
//! Task 1.7 (RED) registers the bot-message fixtures and adds one format key, `roster.channels`.
//! Task 1.8 (RED) registers the human, foreign-bot, edit and status-tag fixtures and adds four
//! format changes: `edit_target`, `event.tags`, `expect.diagnostics`, and a `quiet` that may be
//! omitted.
//! Task 1.9 (RED) registers the control fixtures (stop, resume, cancel); the format needed no
//! change.
//!
//! Every `*.json` file under `fixtures/conformance/` is one case: a roster, a snapshot, one event
//! (or a list of steps) and the expected result. The harness loads it, builds the roster and the
//! `Snapshot`, calls `route`, and compares the result exactly.
//!
//! # The fixture format
//!
//! Section 16.1 gives one example (case 7). The harness parses exactly that shape with
//! `deny_unknown_fields`, so a misspelt key is an error, not a silent default.
//!
//! - `case`, `requirement`, `title`: the brief 15.1 case number (the file name starts with it),
//!   the CONFORM criterion the case pins, and a one-line description.
//! - `roster`: `{"owner", "bots", "respond_to", "default_bot"}`. The owner and every bot are
//!   symbolic names. The roster has one channel, `room`, and every event is posted in it. Every
//!   bot covers every channel unless `channels` says otherwise. `respond_to` maps a bot to
//!   `"owner-only"` (the default) or `"anyone"`. `default_bot` is the channel's default bot or
//!   `null`. Two optional maps from a bot to a list, neither in the 16.1 example: `aliases` (extra
//!   `@names`, extra 108) and `channels` (the channels the bot covers, as symbolic channel names
//!   such as `"lobby"`; a bot that is not listed covers every channel, `["*"]`; extra 109).
//! - `local_bots`, `members`: the snapshot's `local_bots` and `local_members`.
//! - `now`: the time `route` is called with.
//! - `quiet`, `halts`, `wake_counts`: the snapshot's `quiet` set, `halts` and `wake_counts`
//!   (a map from bot to `{"hour", "day"}`). `halts` and `wake_counts` may be omitted for "none".
//!   An explicit `quiet` is used as written, for every step. When `quiet` is omitted, the harness
//!   derives it for each step the way the engine does, with `router_core::quiet::quiet_set` over
//!   the roster and that step's `now` (task 1.8, extra 118).
//! - `thread`, `parent_author`, `edit_target`: the snapshot's thread (`root`, `participants`,
//!   `discussion`, `round_id`, `round_mode`, `turns_used`), the author of the reply parent, and
//!   the message an edit edits, `{"message": LABEL}`. All three optional. `edit_target` is not in
//!   the 16.1 example; the edit fixtures need it (task 1.8).
//! - `event`: `{"label", "author", "kind", "content", "reply"?, "p"?, "auth"?, "tags"?}`. `tags`
//!   is a list of raw tag arrays appended after the others; it is how a fixture writes the status
//!   tag `["buzz-router", "0.1.0", "status"]` (design 5.5, requirement 4.5), and is not in the
//!   16.1 example.
//! - `expect`: `{"control"?, "decisions", "thread_update"?, "wake_mode"?, "diagnostics"?}`.
//! - `steps`: instead of `event` and `expect`, a list of `{"now"?, "event", "expect"}`.
//!
//! # Symbolic identities
//!
//! - A name (`O`, `A`, `B`, `C`, `H`, `F`, or a roster name) is the fixture key
//!   `sha256("buzz-router-fixture:" + name)`; its public key is that key's.
//! - A label (an event, a thread root or a round) is the event id `sha256("event:" + label)`,
//!   written exactly as design 16.1 words it. `common::event_id` (task 1.3) hashes
//!   `"buzz-router-fixture:event:" + name` instead. No fixture file contains an id, so the two
//!   never meet.
//! - `reply` is `{"root", "parent"}` in labels. It becomes the NIP-10 `e` tags `buzz_sdk` writes.
//!   `p` is a list of names and becomes `p` tags. `auth` is `{"owner": "X"}` and becomes a NIP-OA
//!   `auth` tag signed by `X` for the author, from `compute_auth_tag(&keys(X),
//!   &keys(author).public_key(), "")`.
//! - An event's `created_at` is the unix time of its step's `now`.
//!
//! # The expected result
//!
//! `control` and `decisions` are always compared, exactly: an absent or `null` `control` means
//! the result has none, and `decisions` lists every decision in the order `route` returns them
//! (sorted by bot name). `thread_update` and `wake_mode` are compared exactly when present and
//! skipped when absent, and so is `diagnostics` (it is not in the 16.1 example either; task 1.8
//! adds it): when present it lists every `Diagnostic` `route` returns, in order. Inside
//! `thread_update`, an omitted part means none of it: no `create`, no `add_participants`,
//! `set_discussion` false, no `new_round`.
//!
//! The JSON shapes follow what `#[derive(Deserialize)]` with `rename_all = "snake_case"` would give
//! the router types, as the `wake` example in 16.1 shows:
//!
//! - `{"wake": {"bot", "reason", "priority", "debounce"}}` and `{"suppress": {"bot", "why"}}`,
//!   with `why` one of `halted`, `cap`, `quiet`, `budget`, `respond_to`;
//! - `control` is `{"stop": SCOPE}`, `{"resume": SCOPE}` or `{"cancel": SCOPE}`, and `SCOPE` is
//!   `"all"` or `{"bots": ["A"]}`;
//! - `thread_update.create` is `{"root": LABEL}` (the channel is always `room`), and
//!   `thread_update.new_round` is `{"round_id": LABEL, "mode": "direct" | "discussion"}`; its
//!   `started_at` is the event's `created_at`;
//! - a diagnostic is `"status_tag_ignored"` or `{"roster_drift": {"pubkey": NAME}}`, where `NAME`
//!   is the symbolic name of the foreign bot's key.
//!
//! # Steps
//!
//! A fixture with `steps` runs them in order against one evolving set of threads. After each step
//! the harness applies that step's `thread_update` (creating the thread, adding participants,
//! setting `discussion`, and starting a round, which resets `turns_used`) and adds one turn to
//! `turns_used` for every `Wake`, as if it had been dispatched. A step's `now` overrides the
//! fixture's. The snapshot's `thread` for a reply is the thread with the reply's `root`; the
//! snapshot's `thread` for an edit (kind 40003) is the thread the fixture's `thread` names (the
//! edited message's thread); any other top-level event has none. `parent_author`, `edit_target`,
//! an explicit `quiet`, `halts` and `wake_counts` do not change between steps.
//!
//! # Types without serde
//!
//! `Decision`, `Control`, `Scope`, `SuppressWhy` and `RoundMode` do not derive `Deserialize`
//! (`Reason` and `Priority` do), and `InEvent`, `Snapshot`, `ThreadState`, `ThreadUpdate`,
//! `NewThread`, `NewRound`, `Halts` and `WakeCounts` have no serde either. The harness therefore
//! parses the fixture into the mirror structs below and converts them. Identifiers (`Pubkey`,
//! `EventId`) are derived from names and labels, so they never come from JSON. `BotName` does
//! deserialize and is used directly.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "helpers in an integration-test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use common::{auth_tag, channel, h_tag, keys, p_tag, pubkey, pubkey_of, reply_tags};
use common::{roster_full, RosterBot};
use router_core::config::Roster;
use router_core::ids::{BotName, EventId};
use router_core::quiet::quiet_set;
use router_core::route::{
    route, Control, Decision, Diagnostic, EditTarget, Halts, InEvent, NewRound, NewThread,
    Priority, Reason, RouteResult, Scope, Snapshot, SuppressWhy, ThreadUpdate, WakeCounts,
    KIND_EDIT,
};
use router_core::thread::{RoundMode, ThreadState};
use serde::Deserialize;
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------------------------
// The fixture format: test-side mirrors of the router types
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    case: u32,
    requirement: String,
    title: String,
    roster: RosterSpec,
    local_bots: BTreeSet<BotName>,
    members: BTreeSet<BotName>,
    now: DateTime<Utc>,
    /// The snapshot's `quiet` set. Omitted: derived from each step's `now` with `quiet_set`.
    quiet: Option<BTreeSet<BotName>>,
    #[serde(default)]
    halts: HaltsSpec,
    #[serde(default)]
    wake_counts: BTreeMap<BotName, WakeCountsSpec>,
    thread: Option<ThreadSpec>,
    parent_author: Option<String>,
    /// The message an edit event edits. Not in the 16.1 example; the edit fixtures need it.
    edit_target: Option<EditTargetSpec>,
    event: Option<EventSpec>,
    expect: Option<ExpectSpec>,
    steps: Option<Vec<StepSpec>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RosterSpec {
    owner: String,
    bots: Vec<String>,
    #[serde(default)]
    respond_to: BTreeMap<String, RespondToSpec>,
    default_bot: Option<String>,
    /// Extra `@names` per bot. Not in the 16.1 example; extra 108 needs it.
    #[serde(default)]
    aliases: BTreeMap<String, Vec<String>>,
    /// The channels a bot covers, by symbolic channel name. Not in the 16.1 example; extra 109
    /// needs it. A bot with no entry covers every channel. Events are always in `room`.
    #[serde(default)]
    channels: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RespondToSpec {
    OwnerOnly,
    Anyone,
}

impl RespondToSpec {
    /// The spelling `roster.toml` uses.
    fn as_str(self) -> &'static str {
        match self {
            Self::OwnerOnly => "owner-only",
            Self::Anyone => "anyone",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HaltsSpec {
    all: bool,
    bots: BTreeSet<BotName>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WakeCountsSpec {
    hour: u32,
    day: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThreadSpec {
    root: String,
    participants: BTreeSet<BotName>,
    discussion: bool,
    round_id: String,
    round_mode: RoundModeSpec,
    turns_used: BTreeMap<BotName, u32>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RoundModeSpec {
    Direct,
    Discussion,
}

impl From<RoundModeSpec> for RoundMode {
    fn from(mode: RoundModeSpec) -> Self {
        match mode {
            RoundModeSpec::Direct => Self::Direct,
            RoundModeSpec::Discussion => Self::Discussion,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditTargetSpec {
    /// The label of the edited kind-9 message.
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventSpec {
    label: String,
    author: String,
    kind: u16,
    content: String,
    reply: Option<ReplySpec>,
    #[serde(default)]
    p: Vec<String>,
    auth: Option<AuthSpec>,
    /// Raw tag arrays appended after the `h`, `e`, `p` and `auth` tags. Not in the 16.1 example;
    /// the status-tag fixture (case 28) needs it.
    #[serde(default)]
    tags: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplySpec {
    root: String,
    parent: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthSpec {
    owner: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepSpec {
    now: Option<DateTime<Utc>>,
    event: EventSpec,
    expect: ExpectSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectSpec {
    control: Option<ControlSpec>,
    decisions: Vec<DecisionSpec>,
    thread_update: Option<ThreadUpdateSpec>,
    wake_mode: Option<RoundModeSpec>,
    /// Not in the 16.1 example. When present, `route`'s diagnostics must equal it exactly.
    diagnostics: Option<Vec<DiagnosticSpec>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ControlSpec {
    Stop(ScopeSpec),
    Resume(ScopeSpec),
    Cancel(ScopeSpec),
}

impl From<ControlSpec> for Control {
    fn from(control: ControlSpec) -> Self {
        match control {
            ControlSpec::Stop(scope) => Self::Stop(scope.into()),
            ControlSpec::Resume(scope) => Self::Resume(scope.into()),
            ControlSpec::Cancel(scope) => Self::Cancel(scope.into()),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScopeSpec {
    All,
    Bots(BTreeSet<BotName>),
}

impl From<ScopeSpec> for Scope {
    fn from(scope: ScopeSpec) -> Self {
        match scope {
            ScopeSpec::All => Self::All,
            ScopeSpec::Bots(bots) => Self::Bots(bots),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum DecisionSpec {
    Wake {
        bot: BotName,
        reason: Reason,
        priority: Priority,
        debounce: bool,
    },
    Suppress {
        bot: BotName,
        why: SuppressWhySpec,
    },
}

impl From<DecisionSpec> for Decision {
    fn from(decision: DecisionSpec) -> Self {
        match decision {
            DecisionSpec::Wake {
                bot,
                reason,
                priority,
                debounce,
            } => Self::Wake {
                bot,
                reason,
                priority,
                debounce,
            },
            DecisionSpec::Suppress { bot, why } => Self::Suppress {
                bot,
                why: why.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SuppressWhySpec {
    Halted,
    Cap,
    Quiet,
    Budget,
    RespondTo,
}

impl From<SuppressWhySpec> for SuppressWhy {
    fn from(why: SuppressWhySpec) -> Self {
        match why {
            SuppressWhySpec::Halted => Self::Halted,
            SuppressWhySpec::Cap => Self::Cap,
            SuppressWhySpec::Quiet => Self::Quiet,
            SuppressWhySpec::Budget => Self::Budget,
            SuppressWhySpec::RespondTo => Self::RespondTo,
        }
    }
}

/// A `Diagnostic`. The foreign bot's key is a symbolic name, like every identity in a fixture.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum DiagnosticSpec {
    RosterDrift { pubkey: String },
    StatusTagIgnored,
}

impl From<DiagnosticSpec> for Diagnostic {
    fn from(diagnostic: DiagnosticSpec) -> Self {
        match diagnostic {
            DiagnosticSpec::RosterDrift { pubkey: name } => Self::RosterDrift {
                pubkey: pubkey(&name),
            },
            DiagnosticSpec::StatusTagIgnored => Self::StatusTagIgnored,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThreadUpdateSpec {
    create: Option<CreateSpec>,
    #[serde(default)]
    add_participants: BTreeSet<BotName>,
    #[serde(default)]
    set_discussion: bool,
    new_round: Option<NewRoundSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSpec {
    root: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NewRoundSpec {
    round_id: String,
    mode: RoundModeSpec,
}

// ---------------------------------------------------------------------------------------------
// Identities
// ---------------------------------------------------------------------------------------------

/// The 64 hex characters of `sha256("event:" + label)`.
fn label_hex(label: &str) -> String {
    let digest: [u8; 32] = Sha256::digest(format!("event:{label}")).into();
    hex::encode(digest)
}

/// The event id of `label`.
fn label_id(label: &str) -> EventId {
    EventId::from_hex(&label_hex(label)).expect("a label digest is 64 hex characters")
}

// ---------------------------------------------------------------------------------------------
// Building the router's inputs from a fixture
// ---------------------------------------------------------------------------------------------

fn build_roster(spec: &RosterSpec) -> Roster {
    for name in spec
        .respond_to
        .keys()
        .chain(spec.aliases.keys())
        .chain(spec.channels.keys())
    {
        assert!(
            spec.bots.contains(name),
            "`{name}` is in roster.respond_to, roster.aliases or roster.channels but not in roster.bots"
        );
    }
    let aliases: Vec<Vec<&str>> = spec
        .bots
        .iter()
        .map(|name| {
            spec.aliases
                .get(name)
                .map(|aliases| aliases.iter().map(String::as_str).collect())
                .unwrap_or_default()
        })
        .collect();
    let channel_lists: Vec<Option<Vec<&str>>> = spec
        .bots
        .iter()
        .map(|name| {
            spec.channels
                .get(name)
                .map(|channels| channels.iter().map(String::as_str).collect())
        })
        .collect();
    let bots: Vec<RosterBot<'_>> = spec
        .bots
        .iter()
        .zip(&aliases)
        .zip(&channel_lists)
        .map(|((name, aliases), channels)| RosterBot {
            name,
            aliases,
            respond_to: spec
                .respond_to
                .get(name)
                .map_or("owner-only", |respond_to| respond_to.as_str()),
            channels: channels.as_deref(),
        })
        .collect();
    roster_full(&spec.owner, spec.default_bot.as_deref(), &bots)
}

fn build_event(spec: &EventSpec, now: DateTime<Utc>) -> InEvent {
    let author = keys(&spec.author);
    let mut tags = vec![h_tag()];
    if let Some(reply) = &spec.reply {
        tags.extend(reply_tags(
            &label_hex(&reply.root),
            &label_hex(&reply.parent),
        ));
    }
    tags.extend(spec.p.iter().map(|name| p_tag(name)));
    if let Some(auth) = &spec.auth {
        tags.push(auth_tag(&keys(&auth.owner), &author, ""));
    }
    tags.extend(spec.tags.iter().cloned());
    InEvent {
        id: label_id(&spec.label),
        pubkey: pubkey_of(&author),
        kind: spec.kind,
        created_at: now.timestamp(),
        channel: channel("room"),
        content: spec.content.clone(),
        tags,
    }
}

fn build_thread(spec: &ThreadSpec, now: DateTime<Utc>) -> ThreadState {
    ThreadState {
        root_id: label_id(&spec.root),
        channel_id: channel("room"),
        participants: spec.participants.clone(),
        discussion: spec.discussion,
        round_id: label_id(&spec.round_id),
        round_mode: spec.round_mode.into(),
        // The 16.1 `thread` object has no start time and no owner rule reads one, so it is `now`.
        round_started_at: now.timestamp(),
        turns_used: spec.turns_used.clone(),
    }
}

fn expected_thread_update(spec: &ThreadUpdateSpec, ev: &InEvent) -> ThreadUpdate {
    ThreadUpdate {
        create: spec.create.as_ref().map(|create| NewThread {
            root_id: label_id(&create.root),
            channel_id: ev.channel,
        }),
        add_participants: spec.add_participants.clone(),
        set_discussion: spec.set_discussion,
        new_round: spec.new_round.as_ref().map(|round| NewRound {
            round_id: label_id(&round.round_id),
            mode: round.mode.into(),
            started_at: ev.created_at,
        }),
    }
}

// ---------------------------------------------------------------------------------------------
// Running a fixture
// ---------------------------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/conformance"
    ))
}

/// The `NN` of a file named `NN-slug.json`.
fn file_number(file: &str) -> &str {
    file.split('-').next().unwrap_or(file)
}

fn load(test: &str, file: &str) -> Fixture {
    let path = fixture_dir().join(file);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let fixture: Fixture = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{file} is not a valid fixture: {error}"));
    let number = file_number(file);
    assert_eq!(
        number.parse::<u32>().ok(),
        Some(fixture.case),
        "{file}: the `case` field must be the number the file name starts with"
    );
    assert_eq!(
        test,
        format!("case_{number}"),
        "{file} is registered under the wrong test name"
    );
    assert!(
        !fixture.requirement.is_empty() && !fixture.title.is_empty(),
        "{file}: `requirement` and `title` must not be empty"
    );
    fixture
}

/// The steps of a fixture: its single `event` and `expect`, or its `steps`.
fn steps_of(file: &str, fixture: &mut Fixture) -> Vec<StepSpec> {
    match (
        fixture.event.take(),
        fixture.expect.take(),
        fixture.steps.take(),
    ) {
        (Some(event), Some(expect), None) => vec![StepSpec {
            now: None,
            event,
            expect,
        }],
        (None, None, Some(steps)) if !steps.is_empty() => steps,
        _ => panic!("{file}: a fixture has either `event` and `expect`, or a non-empty `steps`"),
    }
}

/// Applies a step's result to the thread rooted at `root`, as the engine would after dispatching
/// the step's wakes.
fn apply(
    threads: &mut BTreeMap<EventId, ThreadState>,
    root: &EventId,
    ev: &InEvent,
    result: &RouteResult,
) {
    if let Some(create) = &result.thread_update.create {
        threads
            .entry(create.root_id.clone())
            .or_insert_with(|| ThreadState {
                root_id: create.root_id.clone(),
                channel_id: create.channel_id,
                participants: BTreeSet::new(),
                discussion: false,
                round_id: create.root_id.clone(),
                round_mode: RoundMode::Direct,
                round_started_at: ev.created_at,
                turns_used: BTreeMap::new(),
            });
    }
    let Some(thread) = threads.get_mut(root) else {
        return;
    };
    thread
        .participants
        .extend(result.thread_update.add_participants.iter().cloned());
    if result.thread_update.set_discussion {
        thread.discussion = true;
    }
    if let Some(round) = &result.thread_update.new_round {
        thread.round_id = round.round_id.clone();
        thread.round_mode = round.mode;
        thread.round_started_at = round.started_at;
        thread.turns_used.clear();
    }
    for decision in &result.decisions {
        if let Decision::Wake { bot, .. } = decision {
            *thread.turns_used.entry(bot.clone()).or_insert(0) += 1;
        }
    }
}

fn run_fixture(test: &str, file: &str) {
    let mut fixture = load(test, file);
    let steps = steps_of(file, &mut fixture);
    let multi_step = steps.len() > 1;

    let roster = build_roster(&fixture.roster);
    let halts = Halts {
        all: fixture.halts.all,
        bots: fixture.halts.bots.clone(),
    };
    let wake_counts: BTreeMap<BotName, WakeCounts> = fixture
        .wake_counts
        .iter()
        .map(|(bot, counts)| {
            (
                bot.clone(),
                WakeCounts {
                    hour: counts.hour,
                    day: counts.day,
                },
            )
        })
        .collect();
    let parent_author = fixture.parent_author.as_deref().map(pubkey);
    let edit_target = fixture.edit_target.as_ref().map(|target| EditTarget {
        message_id: label_id(&target.message),
    });
    let mut threads: BTreeMap<EventId, ThreadState> = BTreeMap::new();
    if let Some(spec) = &fixture.thread {
        let thread = build_thread(spec, fixture.now);
        threads.insert(thread.root_id.clone(), thread);
    }

    for (index, step) in steps.into_iter().enumerate() {
        let StepSpec { now, event, expect } = step;
        let now = now.unwrap_or(fixture.now);
        let context = if multi_step {
            format!(
                "{file} (case {}: {}), step {}",
                fixture.case,
                fixture.title,
                index + 1
            )
        } else {
            format!("{file} (case {}: {})", fixture.case, fixture.title)
        };
        let ev = build_event(&event, now);
        // A reply belongs to the thread of its root and an edit to the thread of the message it
        // edits, which the fixture's `thread` names. Any other top-level event has no thread yet.
        let is_edit = event.kind == KIND_EDIT;
        let root = match (&event.reply, &fixture.thread) {
            (Some(reply), _) => label_id(&reply.root),
            (None, Some(thread)) if is_edit => label_id(&thread.root),
            (None, _) => ev.id.clone(),
        };
        let thread = (event.reply.is_some() || is_edit)
            .then(|| threads.get(&root).cloned())
            .flatten();
        let snap = Snapshot {
            roster: &roster,
            local_bots: &fixture.local_bots,
            local_members: fixture.members.clone(),
            halts: &halts,
            thread,
            parent_author: parent_author.clone(),
            edit_target: edit_target.clone(),
            wake_counts: &wake_counts,
            quiet: fixture
                .quiet
                .clone()
                .unwrap_or_else(|| quiet_set(&roster, now)),
        };

        let result = route(&ev, &snap, now);

        assert_eq!(
            result.control,
            expect.control.map(Control::from),
            "{context}: control"
        );
        let expected_decisions: Vec<Decision> =
            expect.decisions.into_iter().map(Decision::from).collect();
        assert_eq!(result.decisions, expected_decisions, "{context}: decisions");
        if let Some(thread_update) = &expect.thread_update {
            assert_eq!(
                result.thread_update,
                expected_thread_update(thread_update, &ev),
                "{context}: thread_update"
            );
        }
        if let Some(wake_mode) = expect.wake_mode {
            assert_eq!(
                result.wake_mode,
                RoundMode::from(wake_mode),
                "{context}: wake_mode"
            );
        }
        if let Some(diagnostics) = expect.diagnostics {
            let expected_diagnostics: Vec<Diagnostic> =
                diagnostics.into_iter().map(Diagnostic::from).collect();
            assert_eq!(
                result.diagnostics, expected_diagnostics,
                "{context}: diagnostics"
            );
        }

        apply(&mut threads, &root, &ev, &result);
    }
}

// ---------------------------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------------------------

/// Registers each fixture file as one `#[test]` and lists them all in `REGISTERED`, which
/// `every_fixture_file_is_registered` checks against the directory.
macro_rules! conformance_case {
    ($($name:ident => $file:literal),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                run_fixture(stringify!($name), $file);
            }
        )+

        /// Every fixture file a test above runs.
        const REGISTERED: &[&str] = &[$($file),+];
    };
}

conformance_case! {
    case_01 => "01-owner-mention-top-level.json",
    case_02 => "02-owner-untagged-top-level.json",
    case_03 => "03-default-bot.json",
    case_04 => "04-everyone-top-level.json",
    case_05 => "05-owner-untagged-in-thread.json",
    case_06 => "06-everyone-thread-new-round.json",
    case_07 => "07-reply-target.json",
    case_08 => "08-mention-in-discussion-thread.json",
    case_09 => "09-mention-beats-reply-target.json",
    case_10 => "10-discussion-bot-post.json",
    case_11 => "11-discussion-cap.json",
    case_12 => "12-direct-bot-reply-p-owner.json",
    case_13 => "13-bot-mention.json",
    case_14 => "14-bot-p-tag-ignored.json",
    case_15 => "15-bot-everyone-plain.json",
    case_16 => "16-bot-only-thread-cap.json",
    case_17 => "17-everyone-stop.json",
    case_18 => "18-fucking-stop.json",
    case_19 => "19-scoped-stop.json",
    case_20 => "20-long-sentence-not-stop.json",
    case_21 => "21-scoped-cancel.json",
    case_22 => "22-resume-all.json",
    case_23 => "23-halted-owner-mention.json",
    case_24 => "24-quiet-hours-bot-caused.json",
    case_25 => "25-quiet-owner-mention.json",
    case_26 => "26-budget-owner-mention.json",
    case_27 => "27-budget-bot-caused.json",
    case_28 => "28-status-tag.json",
    case_29 => "29-human-owner-only.json",
    case_30 => "30-human-anyone.json",
    case_31 => "31-thread-without-participants.json",
    case_32 => "32-mention-inside-code.json",
    case_33 => "33-quoted-everyone.json",
    case_34 => "34-longest-name.json",
    case_35 => "35-owner-edit-adds-p.json",
    case_36 => "36-self-p-tag-ignored.json",
    case_37 => "37-foreign-bot.json",
    case_101 => "101-human-reply-target.json",
    case_102 => "102-human-mention-beats-reply.json",
    case_103 => "103-mention-and-discussion-single-decision.json",
    case_104 => "104-edit-over-cap.json",
    case_105 => "105-edit-ignores-quiet-budget.json",
    case_106 => "106-nprofile-mention.json",
    case_107 => "107-npub-mention.json",
    case_108 => "108-alias-mention.json",
    case_109 => "109-outside-channels-list.json",
    case_110 => "110-halted-owner-only-respond-to.json",
    case_111 => "111-everyone-with-mention.json",
    case_112 => "112-non-owner-edit-ignored.json",
    case_113 => "113-shutdown.json",
    case_114 => "114-scoped-resume-two-bots.json",
    case_115 => "115-default-bot-empty-thread.json",
    case_116 => "116-foreign-roster-drift.json",
    case_117 => "117-human-everyone-plain.json",
    case_118 => "118-quiet-boundary.json",
    case_119 => "119-daily-budget-bot-caused.json",
    case_120 => "120-daily-budget-owner-not-blocked.json",
    case_121 => "121-bot-stop-not-control.json",
}

#[test]
fn every_fixture_file_is_registered() {
    let dir = fixture_dir();
    let mut unregistered: Vec<String> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", dir.display()))
        .map(|entry| entry.expect("a directory entry is readable").file_name())
        .filter_map(|name| name.into_string().ok())
        .filter(|name| name.ends_with(".json") && !REGISTERED.contains(&name.as_str()))
        .collect();
    unregistered.sort();
    assert!(
        unregistered.is_empty(),
        "fixture files with no conformance_case! entry: {unregistered:?}"
    );
}
