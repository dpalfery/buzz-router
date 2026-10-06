//! `route --replay`: offline decisions for captured events (design 12.1 and 5.8, requirements
//! 55.2 to 55.4).
//!
//! The command reads a JSON Lines file of signed events, as `capture` writes it, and replays them
//! through `router_core::replay::Replayer` with a simulated clock. It uses no network and no
//! database, and writes nothing but its output lines. For each event it prints one line:
//!
//! ```text
//! {"event_id":"<hex>","created_at":<secs>,"class":<class>,"control":<control>,"decisions":[...]}
//! ```
//!
//! The lines go through the private types below, which mirror `router-core`'s result and own the
//! JSON shape, so the core needs no serialisation of its own.

use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use buzz_sdk::builders::extract_channel_id;
use nostr::{Event, JsonUtil};
use router_core::classify::{classify, AuthorClass};
use router_core::ids::{ChannelId, EventId, Pubkey};
use router_core::replay::Replayer;
use router_core::route::{
    Control, Decision, InEvent, Priority, Reason, RouteResult, Scope, SuppressWhy, KIND_EDIT,
    KIND_MESSAGE,
};
use serde::Serialize;

use super::{roster, write_error, CliError};
use crate::paths::Dirs;

/// Replays the events in `events_file` against the roster at `roster_file`, or against the
/// configured roster when none is given (requirement 55.4), and prints the decisions.
pub(super) fn run(
    dirs: &Dirs,
    events_file: &Path,
    roster_file: Option<&Path>,
) -> Result<(), CliError> {
    let roster_file = match roster_file {
        Some(path) => path.to_path_buf(),
        None => roster::roster_path(&dirs.config_dir)?,
    };
    let (roster, _) = roster::load(&roster_file)?;
    let contents = fs::read(events_file).map_err(|error| {
        CliError::bad_input(format!(
            "cannot read the events file {}: {error}",
            events_file.display()
        ))
    })?;

    let mut events = replayable_events(&contents);
    events.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));

    let mut replayer = Replayer::new(roster.clone());
    let mut out = BufWriter::new(io::stdout().lock());
    for event in &events {
        let class = classify(event, &roster);
        let result = replayer.step(event);
        let line = Line::new(event, &class, &result);
        let text = serde_json::to_string(&line)
            .map_err(|error| CliError::other(format!("cannot encode a decision line: {error}")))?;
        writeln!(out, "{text}").map_err(write_error)?;
    }
    out.flush().map_err(write_error)
}

/// The events of a JSON Lines file that the router would route: lines that parse as events with a
/// valid signature, of kind 9 or 40003, with an `h` tag that holds a channel UUID. Every other
/// line is skipped without a word.
fn replayable_events(contents: &[u8]) -> Vec<InEvent> {
    contents
        .split(|byte| *byte == b'\n')
        .filter_map(replayable_event)
        .collect()
}

/// The routable event on one line of the file, if there is one.
fn replayable_event(line: &[u8]) -> Option<InEvent> {
    let event = Event::from_json(line).ok()?;
    event.verify().ok()?;
    let kind = event.kind.as_u16();
    if kind != KIND_MESSAGE && kind != KIND_EDIT {
        return None;
    }
    let channel = ChannelId::from(extract_channel_id(&event)?);
    Some(InEvent {
        id: EventId::from_nostr(&event.id),
        pubkey: Pubkey::from_nostr(&event.pubkey),
        kind,
        created_at: i64::try_from(event.created_at.as_secs()).unwrap_or(i64::MAX),
        channel,
        content: event.content,
        tags: event
            .tags
            .iter()
            .map(|tag| tag.as_slice().to_vec())
            .collect(),
    })
}

/// One output line. The field order is the key order.
#[derive(Serialize)]
struct Line<'a> {
    event_id: &'a str,
    created_at: i64,
    class: Class,
    control: Option<ControlJson<'a>>,
    decisions: Vec<DecisionJson<'a>>,
}

impl<'a> Line<'a> {
    fn new(event: &'a InEvent, class: &AuthorClass, result: &'a RouteResult) -> Self {
        Self {
            event_id: event.id.as_str(),
            created_at: event.created_at,
            class: Class::from(class),
            control: result.control.as_ref().map(ControlJson::from),
            decisions: result.decisions.iter().map(DecisionJson::from).collect(),
        }
    }
}

/// The author class.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Class {
    Owner,
    Bot,
    ForeignBot,
    Human,
}

impl From<&AuthorClass> for Class {
    fn from(class: &AuthorClass) -> Self {
        match class {
            AuthorClass::Owner => Self::Owner,
            AuthorClass::Bot(_) => Self::Bot,
            AuthorClass::ForeignBot { .. } => Self::ForeignBot,
            AuthorClass::Human => Self::Human,
        }
    }
}

/// A control command: `{"stop": <scope>}`, `{"resume": <scope>}` or `{"cancel": <scope>}`.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ControlJson<'a> {
    Stop(ScopeJson<'a>),
    Resume(ScopeJson<'a>),
    Cancel(ScopeJson<'a>),
}

impl<'a> From<&'a Control> for ControlJson<'a> {
    fn from(control: &'a Control) -> Self {
        match control {
            Control::Stop(scope) => Self::Stop(ScopeJson::from(scope)),
            Control::Resume(scope) => Self::Resume(ScopeJson::from(scope)),
            Control::Cancel(scope) => Self::Cancel(ScopeJson::from(scope)),
        }
    }
}

/// The bots a control applies to: `"all"` or `{"bots": [...]}`, with the names sorted.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ScopeJson<'a> {
    All,
    Bots(Vec<&'a str>),
}

impl<'a> From<&'a Scope> for ScopeJson<'a> {
    fn from(scope: &'a Scope) -> Self {
        match scope {
            Scope::All => Self::All,
            // The core keeps the names in a sorted set, so they come out ascending.
            Scope::Bots(bots) => Self::Bots(bots.iter().map(|bot| bot.as_str()).collect()),
        }
    }
}

/// One decision: `{"wake": {...}}` or `{"suppress": {...}}`.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum DecisionJson<'a> {
    Wake {
        bot: &'a str,
        reason: Reason,
        priority: Priority,
        debounce: bool,
    },
    Suppress {
        bot: &'a str,
        why: Why,
    },
}

impl<'a> From<&'a Decision> for DecisionJson<'a> {
    fn from(decision: &'a Decision) -> Self {
        match decision {
            Decision::Wake {
                bot,
                reason,
                priority,
                debounce,
            } => Self::Wake {
                bot: bot.as_str(),
                reason: *reason,
                priority: *priority,
                debounce: *debounce,
            },
            Decision::Suppress { bot, why } => Self::Suppress {
                bot: bot.as_str(),
                why: Why::from(*why),
            },
        }
    }
}

/// Why a bot was not woken.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Why {
    Halted,
    Cap,
    Quiet,
    Budget,
    RespondTo,
}

impl From<SuppressWhy> for Why {
    fn from(why: SuppressWhy) -> Self {
        match why {
            SuppressWhy::Halted => Self::Halted,
            SuppressWhy::Cap => Self::Cap,
            SuppressWhy::Quiet => Self::Quiet,
            SuppressWhy::Budget => Self::Budget,
            SuppressWhy::RespondTo => Self::RespondTo,
        }
    }
}
