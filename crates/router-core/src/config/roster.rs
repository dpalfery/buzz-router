//! The shared roster (`roster.toml`): the resolved types and the raw file shape (design 4.2).

use std::collections::{BTreeMap, BTreeSet};

use chrono_tz::Tz;
use serde::Deserialize;
use toml::Table;

use super::limits::{Limits, LimitsFile};
use super::table::{index_path, Issues, TableReader};
use crate::ids::{BotName, ChannelId, Pubkey};

/// A validated roster. It is built only by [`parse_roster`](super::parse_roster).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Roster {
    /// The owner whose messages the router obeys.
    pub owner: Owner,
    /// Every `[[channels]]` entry, by channel id.
    pub channels: BTreeMap<ChannelId, Channel>,
    /// Every bot, by exact name.
    pub bots: BTreeMap<BotName, Bot>,
    /// Maps the lowercased name and every lowercased alias of each bot to that bot's exact name
    /// (R2.11).
    pub name_index: BTreeMap<String, BotName>,
}

/// The `[owner]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Owner {
    /// The owner's display name.
    pub name: String,
    /// Every key that counts as the owner (R2.2).
    pub pubkeys: BTreeSet<Pubkey>,
    /// The zone in which quiet hours are read.
    pub timezone: Tz,
}

/// A `[[channels]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Channel {
    /// The channel's UUID.
    pub id: ChannelId,
    /// The channel's name.
    pub name: String,
    /// The bot that answers the owner's untagged top-level messages here, if one is set.
    pub default_bot: Option<BotName>,
}

/// A `[[bots]]` entry with its effective limits. `machine` is informational (R2.9) and is not
/// kept.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Bot {
    /// The exact Buzz display name.
    pub name: BotName,
    /// The bot's public key.
    pub pubkey: Pubkey,
    /// Extra `@names` that address the bot, as written.
    pub aliases: Vec<String>,
    /// The channels the bot covers.
    pub channels: ChannelScope,
    /// Whose messages the bot answers.
    pub respond_to: RespondTo,
    /// The bot's limits: its own values over `[limits]` over the defaults (R2.6).
    pub limits: Limits,
}

/// The channels a bot covers (R2.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelScope {
    /// `["*"]`: every channel the bot is a member of.
    All,
    /// A list of channel UUIDs: only these channels. An empty list covers none.
    Only(BTreeSet<ChannelId>),
}

/// Whose messages a bot answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RespondTo {
    /// Only the owner's.
    OwnerOnly,
    /// Anyone's.
    Anyone,
}

/// The roster file as written. A field is `None` (or an empty list) when its key is missing or
/// ill-typed, and an issue has already been recorded for it.
#[derive(Debug, Default)]
pub(super) struct RosterFile {
    pub(super) version: Option<u32>,
    pub(super) owner: Option<OwnerFile>,
    pub(super) limits: LimitsFile,
    pub(super) channels: Vec<ChannelFile>,
    pub(super) bots: Vec<BotFile>,
}

/// `[owner]` as written.
#[derive(Debug, Default)]
pub(super) struct OwnerFile {
    pub(super) name: Option<String>,
    /// The well-formed keys, each with its position in the array.
    pub(super) pubkeys: Vec<(usize, Pubkey)>,
    pub(super) timezone: Option<String>,
}

/// A `[[channels]]` entry as written. An omitted `default_bot` is the empty string.
#[derive(Debug, Default)]
pub(super) struct ChannelFile {
    pub(super) id: Option<ChannelId>,
    pub(super) name: Option<String>,
    pub(super) default_bot: String,
}

/// A `[[bots]]` entry as written.
#[derive(Debug, Default)]
pub(super) struct BotFile {
    pub(super) name: Option<BotName>,
    pub(super) pubkey: Option<Pubkey>,
    /// The well-formed aliases, each with its position in the array.
    pub(super) aliases: Vec<(usize, BotName)>,
    /// `"*"` or channel UUIDs, each with its position in the array.
    pub(super) channels: Option<Vec<(usize, String)>>,
    pub(super) respond_to: Option<RespondTo>,
    pub(super) limits: LimitsFile,
}

impl RosterFile {
    /// Reads the whole file, recording an issue for every structural problem.
    pub(super) fn read(root: &Table, issues: &mut Issues) -> Self {
        let mut reader = TableReader::new(root, "");
        let version = reader.required("version", issues);
        let owner = reader
            .required_table("owner", issues)
            .map(|table| OwnerFile::read(table, issues));
        let limits = reader
            .optional_table("limits", issues)
            .map(|table| LimitsFile::read(table, "limits", issues))
            .unwrap_or_default();
        let channels = reader
            .optional_tables("channels", issues)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(index, table)| {
                table
                    .map(|table| ChannelFile::read(table, &index_path("channels", index), issues))
                    .unwrap_or_default()
            })
            .collect();
        let bots = reader
            .required_tables("bots", issues)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(index, table)| {
                table
                    .map(|table| BotFile::read(table, &index_path("bots", index), issues))
                    .unwrap_or_default()
            })
            .collect();
        reader.finish(issues);
        Self {
            version,
            owner,
            limits,
            channels,
            bots,
        }
    }
}

impl OwnerFile {
    fn read(table: &Table, issues: &mut Issues) -> Self {
        let mut reader = TableReader::new(table, "owner");
        let file = Self {
            name: reader.required("name", issues),
            pubkeys: reader.required_list("pubkeys", issues).unwrap_or_default(),
            timezone: reader.required("timezone", issues),
        };
        reader.finish(issues);
        file
    }
}

impl ChannelFile {
    fn read(table: &Table, path: &str, issues: &mut Issues) -> Self {
        let mut reader = TableReader::new(table, path);
        let file = Self {
            id: reader.required("id", issues),
            name: reader.required("name", issues),
            default_bot: reader.optional("default_bot", issues).unwrap_or_default(),
        };
        reader.finish(issues);
        file
    }
}

impl BotFile {
    fn read(table: &Table, path: &str, issues: &mut Issues) -> Self {
        let mut reader = TableReader::new(table, path);
        let name = reader.required("name", issues);
        let pubkey = reader.required("pubkey", issues);
        let aliases = reader.optional_list("aliases", issues).unwrap_or_default();
        let channels = reader.required_list("channels", issues);
        let respond_to = reader.required("respond_to", issues);
        // `machine` is informational (R2.9): it is accepted so it is not an unknown key, and
        // dropped.
        let _machine: Option<String> = reader.optional("machine", issues);
        let limits = reader
            .optional_table("limits", issues)
            .map(|table| LimitsFile::read(table, &reader.path_of("limits"), issues))
            .unwrap_or_default();
        reader.finish(issues);
        Self {
            name,
            pubkey,
            aliases,
            channels,
            respond_to,
            limits,
        }
    }
}
