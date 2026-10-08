//! The per-machine router config (`router.toml`): the resolved types and the raw file shape
//! (design 4.2).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use serde::Deserialize;
use toml::Table;

use super::table::{index_path, Issues, TableReader};
use crate::ids::BotName;

/// What `roster_path` is when `router.toml` omits it (R1.2).
const DEFAULT_ROSTER_PATH: &str = "roster.toml";

/// The loopback API address when `api_bind` is omitted (R1.3).
pub(super) const DEFAULT_API_BIND: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 47821);

/// `max_concurrent` when a bot omits it (R1.10).
pub(super) const DEFAULT_MAX_CONCURRENT: u32 = 1;

/// A validated per-machine config. It is built only by [`parse_router`](super::parse_router).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RouterConfig {
    /// The relay's WebSocket URL.
    pub relay_url: String,
    /// The loopback address of the API for agents and the CLI.
    pub api_bind: SocketAddr,
    /// The address of the tailnet listener, if one is configured.
    pub tailnet_bind: Option<SocketAddr>,
    /// The base URL given to async webhook agents, without a trailing slash.
    pub public_url: Option<String>,
    /// The roster file, exactly as written: the caller resolves it against the config directory.
    pub roster_path: PathBuf,
    /// The bots this machine serves. A bot must be in the roster but need not be here.
    pub bots: BTreeMap<BotName, RouterBot>,
}

/// One locally served bot.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RouterBot {
    /// Where the bot's secret key lives.
    pub key: KeySource,
    /// The bot's NIP-OA tag, parsed with `buzz_sdk::nip_oa::parse_auth_tag`.
    pub auth_tag: Option<nostr::Tag>,
    /// How many wakes of this bot may run at once.
    pub max_concurrent: u32,
    /// How the bot is woken.
    pub adapter: AdapterConfig,
}

/// Where a bot's secret key is stored (R1.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// The OS keychain entry `buzz-router/<name>`.
    Keychain,
    /// A key file, which must have 0600 permissions on Unix (checked by the daemon).
    File(PathBuf),
}

/// How a bot is woken (R1.11, R1.12). A leading `~` in `cwd` is expanded by the daemon, which
/// can read the home directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterConfig {
    /// Run a local command.
    Command {
        /// The program and its arguments.
        command: Vec<String>,
        /// The working directory, exactly as written.
        cwd: String,
        /// Extra environment variables.
        env: BTreeMap<String, String>,
        /// How the prompt reaches the command.
        prompt_mode: PromptMode,
        /// How the reply gets back.
        reply_mode: ReplyMode,
        /// A prompt template file, or `None` for the built-in template.
        prompt_template: Option<PathBuf>,
    },
    /// Call a webhook.
    Webhook {
        /// The endpoint.
        url: String,
        /// The name of the environment variable that holds the HMAC secret.
        secret_env: String,
        /// Whether the agent calls back or replies in the HTTP response.
        mode: WebhookMode,
        /// A best-effort cancel endpoint.
        cancel_url: Option<String>,
    },
}

/// How a command adapter receives its prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptMode {
    /// On standard input.
    Stdin,
    /// In a file, whose path is in `BUZZ_ROUTER_PROMPT_FILE`.
    File,
}

/// How a command adapter's reply gets back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplyMode {
    /// The router posts what the command prints.
    Stdout,
    /// The agent calls `buzz-router post`.
    Api,
}

/// How a webhook adapter's reply gets back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookMode {
    /// The agent calls back later.
    Async,
    /// The agent replies in the HTTP response.
    Sync,
}

/// What `roster_path` resolves to: the value as written, or `roster.toml` when it is omitted or
/// empty.
pub(super) fn resolve_roster_path(written: Option<String>) -> PathBuf {
    match written {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(DEFAULT_ROSTER_PATH),
    }
}

/// Reads only `roster_path` from the root table and ignores every other key, so `roster check`
/// works on a `router.toml` that is not valid yet (R2.15).
pub(super) fn read_roster_path(root: &Table, issues: &mut Issues) -> PathBuf {
    let mut reader = TableReader::new(root, "");
    resolve_roster_path(reader.optional("roster_path", issues))
}

/// The router file as written. A field is `None` (or an empty list) when its key is missing or
/// ill-typed, and an issue has already been recorded for it. An omitted optional string is the
/// empty string.
#[derive(Debug, Default)]
pub(super) struct RouterFile {
    pub(super) relay_url: Option<String>,
    pub(super) api_bind: Option<SocketAddr>,
    pub(super) tailnet_bind: String,
    pub(super) public_url: String,
    pub(super) roster_path: Option<String>,
    pub(super) bots: Vec<RouterBotFile>,
}

/// A `[[bots]]` entry of `router.toml` as written.
#[derive(Debug, Default)]
pub(super) struct RouterBotFile {
    pub(super) name: Option<BotName>,
    pub(super) key: Option<String>,
    pub(super) auth_tag: String,
    pub(super) max_concurrent: Option<u32>,
    pub(super) adapter: Option<AdapterFile>,
}

/// A `[bots.adapter]` table as written, picked by its `type`.
#[derive(Debug)]
pub(super) enum AdapterFile {
    Command {
        command: Option<Vec<String>>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
        prompt_mode: Option<PromptMode>,
        reply_mode: Option<ReplyMode>,
        prompt_template: String,
    },
    Webhook {
        url: Option<String>,
        secret_env: Option<String>,
        mode: Option<WebhookMode>,
        cancel_url: String,
    },
}

/// The `type` of an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AdapterKind {
    Command,
    Webhook,
}

impl RouterFile {
    /// Reads the whole file, recording an issue for every structural problem.
    pub(super) fn read(root: &Table, issues: &mut Issues) -> Self {
        let mut reader = TableReader::new(root, "");
        let relay_url = reader.required("relay_url", issues);
        let api_bind = reader.optional("api_bind", issues);
        let tailnet_bind = reader.optional("tailnet_bind", issues).unwrap_or_default();
        let public_url = reader.optional("public_url", issues).unwrap_or_default();
        let roster_path = reader.optional("roster_path", issues);
        let bots = reader
            .required_tables("bots", issues)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(index, table)| {
                table
                    .map(|table| RouterBotFile::read(table, &index_path("bots", index), issues))
                    .unwrap_or_default()
            })
            .collect();
        reader.finish(issues);
        Self {
            relay_url,
            api_bind,
            tailnet_bind,
            public_url,
            roster_path,
            bots,
        }
    }
}

impl RouterBotFile {
    fn read(table: &Table, path: &str, issues: &mut Issues) -> Self {
        let mut reader = TableReader::new(table, path);
        let name = reader.required("name", issues);
        let key = reader.required("key", issues);
        let auth_tag = reader.optional("auth_tag", issues).unwrap_or_default();
        let max_concurrent = reader.optional("max_concurrent", issues);
        let adapter = reader
            .required_table("adapter", issues)
            .and_then(|table| AdapterFile::read(table, &reader.path_of("adapter"), issues));
        reader.finish(issues);
        Self {
            name,
            key,
            auth_tag,
            max_concurrent,
            adapter,
        }
    }
}

impl AdapterFile {
    /// Reads the adapter table. When `type` is missing or unknown the other keys cannot be
    /// judged, so nothing else is reported and the result is `None`.
    fn read(table: &Table, path: &str, issues: &mut Issues) -> Option<Self> {
        let mut reader = TableReader::new(table, path);
        let adapter = match reader.required::<AdapterKind>("type", issues)? {
            AdapterKind::Command => Self::Command {
                command: reader.required("command", issues),
                cwd: reader.required("cwd", issues),
                env: reader.optional("env", issues).unwrap_or_default(),
                prompt_mode: reader.required("prompt_mode", issues),
                reply_mode: reader.required("reply_mode", issues),
                prompt_template: reader
                    .optional("prompt_template", issues)
                    .unwrap_or_default(),
            },
            AdapterKind::Webhook => Self::Webhook {
                url: reader.required("url", issues),
                secret_env: reader.required("secret_env", issues),
                mode: reader.required("mode", issues),
                cancel_url: reader.optional("cancel_url", issues).unwrap_or_default(),
            },
        };
        reader.finish(issues);
        Some(adapter)
    }
}
