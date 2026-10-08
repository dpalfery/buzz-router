//! Validation (design 4.3): turns a raw file into a resolved type and records every rule that is
//! broken, so one run reports every problem. The rules are R1.7, R1.8, R1.14, R2.3, R2.12, R22.3
//! and assumptions A2 and A3. Structural problems (missing keys, wrong types, unknown keys, values
//! outside a listed set, malformed ids and quiet hours) were recorded while reading the file.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

use buzz_sdk::nip_oa::parse_auth_tag;
use chrono_tz::Tz;

use super::limits::Limits;
use super::roster::RosterFile;
use super::roster::{Bot, BotFile, Channel, ChannelFile, ChannelScope, Owner, OwnerFile, Roster};
use super::router::{
    resolve_roster_path, AdapterConfig, AdapterFile, KeySource, RouterBot, RouterBotFile,
    RouterConfig, RouterFile, WebhookMode, DEFAULT_API_BIND, DEFAULT_MAX_CONCURRENT,
};
use super::table::{index_path, key_path, Issues};
use super::ConfigErrors;
use crate::ids::{BotName, ChannelId, Pubkey};

/// The only roster `version` this router reads (A3).
const SUPPORTED_VERSION: u32 = 1;

/// Ends validation: the value if nothing was recorded, otherwise every issue.
fn finish<T>(issues: Issues, value: Option<T>) -> Result<T, ConfigErrors> {
    match value {
        Some(value) if issues.is_empty() => Ok(value),
        _ => Err(ConfigErrors::from_issues(issues.into_vec())),
    }
}

// ---------------------------------------------------------------------------------------------
// Roster
// ---------------------------------------------------------------------------------------------

/// Validates a roster read from `roster.toml`. `issues` holds what reading already recorded.
pub(super) fn validate_roster(
    file: &RosterFile,
    mut issues: Issues,
) -> Result<Roster, ConfigErrors> {
    if let Some(version) = file.version.filter(|version| *version != SUPPORTED_VERSION) {
        issues.add(
            "version",
            format!("unsupported version {version}; this router reads version {SUPPORTED_VERSION}"),
        );
    }
    let (owner_keys, owner) = validate_owner(file.owner.as_ref(), &mut issues);
    let global_limits = file.limits.layer(Limits::default());
    let bots = validate_bots(&file.bots, &owner_keys, global_limits, &mut issues);
    let channels = validate_channels(&file.channels, &file.bots, &mut issues);

    let name_index = name_index(&bots);
    let value = owner.map(|owner| Roster {
        owner,
        channels: channels
            .into_iter()
            .map(|channel| (channel.id, channel))
            .collect(),
        bots: bots
            .into_iter()
            .map(|bot| (bot.name.clone(), bot))
            .collect(),
        name_index,
    });
    finish(issues, value)
}

/// Checks the owner's keys and timezone. Returns the well-formed owner keys even when the owner
/// as a whole is rejected, because the bots are checked against them.
fn validate_owner(
    owner: Option<&OwnerFile>,
    issues: &mut Issues,
) -> (BTreeSet<Pubkey>, Option<Owner>) {
    let Some(owner) = owner else {
        return (BTreeSet::new(), None);
    };
    let mut keys = BTreeSet::new();
    for (index, key) in &owner.pubkeys {
        if !keys.insert(key.clone()) {
            issues.add(
                index_path("owner.pubkeys", *index),
                "this owner key is listed more than once",
            );
        }
    }
    let timezone = owner.timezone.as_deref().and_then(|text| {
        Tz::from_str(text)
            .map_err(|_| {
                issues.add(
                    "owner.timezone",
                    format!("{text:?} is not an IANA zone name"),
                )
            })
            .ok()
    });
    let resolved = owner
        .name
        .clone()
        .zip(timezone)
        .map(|(name, timezone)| Owner {
            name,
            pubkeys: keys.clone(),
            timezone,
        });
    (keys, resolved)
}

/// Checks the bots against each other and against the owner, and resolves the ones that are
/// whole.
fn validate_bots(
    files: &[BotFile],
    owner_keys: &BTreeSet<Pubkey>,
    global_limits: Limits,
    issues: &mut Issues,
) -> Vec<Bot> {
    let mut claimed_names = BTreeSet::new();
    let mut claimed_keys = BTreeSet::new();
    let mut bots = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let path = index_path("bots", index);
        if let Some(name) = &file.name {
            let name_path = key_path(&path, "name");
            claim_name(&mut claimed_names, name, name_path.clone(), issues);
            // Owner decision O2: `all` is the halt scope that covers every bot (design 6.7),
            // stored as one `halts` row, so no bot may be named `all`, in any case.
            if name.as_str().eq_ignore_ascii_case("all") {
                issues.add(
                    name_path,
                    "\"all\" is reserved for the halt scope covering every bot",
                );
            }
        }
        let aliases_path = key_path(&path, "aliases");
        for (alias_index, alias) in &file.aliases {
            claim_name(
                &mut claimed_names,
                alias,
                index_path(&aliases_path, *alias_index),
                issues,
            );
        }
        if let Some(pubkey) = &file.pubkey {
            let pubkey_path = key_path(&path, "pubkey");
            if owner_keys.contains(pubkey) {
                issues.add(pubkey_path, "this key is also an owner key");
            } else if !claimed_keys.insert(pubkey) {
                issues.add(pubkey_path, "this key already belongs to another bot");
            }
        }
        let scope = file
            .channels
            .as_deref()
            .and_then(|entries| resolve_scope(entries, &key_path(&path, "channels"), issues));
        if let (Some(name), Some(pubkey), Some(channels), Some(respond_to)) =
            (&file.name, &file.pubkey, scope, file.respond_to)
        {
            bots.push(Bot {
                name: name.clone(),
                pubkey: pubkey.clone(),
                aliases: file
                    .aliases
                    .iter()
                    .map(|(_, alias)| alias.as_str().to_owned())
                    .collect(),
                channels,
                respond_to,
                limits: file.limits.layer(global_limits),
            });
        }
    }
    bots
}

/// Claims a name or alias, which must be unique across the roster ignoring ASCII case (A3).
fn claim_name(claimed: &mut BTreeSet<String>, name: &BotName, path: String, issues: &mut Issues) {
    if !claimed.insert(name.as_str().to_ascii_lowercase()) {
        issues.add(
            path,
            format!(
                "{:?} repeats another name or alias; names are compared ignoring case",
                name.as_str()
            ),
        );
    }
}

/// Resolves a bot's `channels`: `["*"]` alone, or a list of channel UUIDs (R2.10).
fn resolve_scope(
    entries: &[(usize, String)],
    path: &str,
    issues: &mut Issues,
) -> Option<ChannelScope> {
    if entries.iter().any(|(_, entry)| entry == "*") {
        if entries.len() > 1 {
            issues.add(
                path,
                "\"*\" must be the only entry; otherwise list channel UUIDs",
            );
            return None;
        }
        return Some(ChannelScope::All);
    }
    let mut ids = BTreeSet::new();
    let mut whole = true;
    for (index, entry) in entries {
        match ChannelId::parse(entry) {
            Ok(id) => {
                ids.insert(id);
            }
            Err(error) => {
                issues.add(
                    index_path(path, *index),
                    format!("expected \"*\" or a channel UUID: {error}"),
                );
                whole = false;
            }
        }
    }
    whole.then_some(ChannelScope::Only(ids))
}

/// Checks the channels and resolves the ones that are whole. A `default_bot` that is set must
/// name a bot exactly (A3).
fn validate_channels(
    files: &[ChannelFile],
    bot_files: &[BotFile],
    issues: &mut Issues,
) -> Vec<Channel> {
    let bot_names: BTreeMap<&str, &BotName> = bot_files
        .iter()
        .filter_map(|bot| bot.name.as_ref())
        .map(|name| (name.as_str(), name))
        .collect();
    // Owner decision O2: channel ids identify threads and cursors, so each one may appear once.
    let mut seen_ids = BTreeSet::new();
    let mut channels = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let path = index_path("channels", index);
        if let Some(id) = file.id {
            if !seen_ids.insert(id) {
                issues.add(key_path(&path, "id"), "this channel id is already used");
            }
        }
        let default_bot = if file.default_bot.is_empty() {
            Some(None)
        } else if let Some(name) = bot_names.get(file.default_bot.as_str()) {
            Some(Some((*name).clone()))
        } else {
            issues.add(
                key_path(&path, "default_bot"),
                format!("{:?} is not a bot in this roster", file.default_bot),
            );
            None
        };
        if let (Some(id), Some(name), Some(default_bot)) =
            (file.id, file.name.as_ref(), default_bot)
        {
            channels.push(Channel {
                id,
                name: name.clone(),
                default_bot,
            });
        }
    }
    channels
}

/// Maps each lowercased name and alias to its bot (R2.11).
fn name_index(bots: &[Bot]) -> BTreeMap<String, BotName> {
    let mut index = BTreeMap::new();
    for bot in bots {
        index.insert(bot.name.as_str().to_ascii_lowercase(), bot.name.clone());
        for alias in &bot.aliases {
            index.insert(alias.to_ascii_lowercase(), bot.name.clone());
        }
    }
    index
}

// ---------------------------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------------------------

/// Validates a router config against the roster it will run with. `issues` holds what reading
/// already recorded.
pub(super) fn validate_router(
    file: &RouterFile,
    roster: &Roster,
    mut issues: Issues,
) -> Result<RouterConfig, ConfigErrors> {
    let api_bind = file.api_bind.unwrap_or(DEFAULT_API_BIND);
    if !api_bind.ip().to_canonical().is_loopback() {
        issues.add("api_bind", "must be a loopback address such as 127.0.0.1");
    }
    let tailnet_bind = validate_tailnet_bind(&file.tailnet_bind, &mut issues);
    let public_url = Some(file.public_url.trim_end_matches('/'))
        .filter(|url| !url.is_empty())
        .map(str::to_owned);
    validate_async_webhooks(file, public_url.is_some(), &mut issues);
    let bots = validate_router_bots(&file.bots, roster, &mut issues);

    let value = file.relay_url.clone().map(|relay_url| RouterConfig {
        relay_url,
        api_bind,
        tailnet_bind,
        public_url,
        roster_path: resolve_roster_path(file.roster_path.clone()),
        bots,
    });
    finish(issues, value)
}

/// `tailnet_bind` is `""` (off) or an address that is not a wildcard (A3).
fn validate_tailnet_bind(text: &str, issues: &mut Issues) -> Option<SocketAddr> {
    if text.is_empty() {
        return None;
    }
    match text.parse::<SocketAddr>() {
        Ok(address) if address.ip().to_canonical().is_unspecified() => {
            issues.add(
                "tailnet_bind",
                "must not be a wildcard address; use this machine's tailnet IP",
            );
            None
        }
        Ok(address) => Some(address),
        Err(_) => {
            issues.add(
                "tailnet_bind",
                format!("{text:?} is not an address and port such as 100.64.0.1:47821"),
            );
            None
        }
    }
}

/// An async webhook bot needs `public_url` and `tailnet_bind` (A3), so its agent can call back.
fn validate_async_webhooks(file: &RouterFile, has_public_url: bool, issues: &mut Issues) {
    let Some(bot) = file.bots.iter().find(|bot| {
        matches!(
            bot.adapter,
            Some(AdapterFile::Webhook {
                mode: Some(WebhookMode::Async),
                ..
            })
        )
    }) else {
        return;
    };
    let bot_name = bot.name.as_ref().map_or("a bot", BotName::as_str);
    if !has_public_url {
        issues.add(
            "public_url",
            format!("required, because {bot_name} uses an async webhook"),
        );
    }
    if file.tailnet_bind.is_empty() {
        issues.add(
            "tailnet_bind",
            format!("required, because {bot_name} uses an async webhook"),
        );
    }
}

/// Checks the local bots and resolves the ones that are whole.
fn validate_router_bots(
    files: &[RouterBotFile],
    roster: &Roster,
    issues: &mut Issues,
) -> BTreeMap<BotName, RouterBot> {
    let mut bots = BTreeMap::new();
    for (index, file) in files.iter().enumerate() {
        let path = index_path("bots", index);
        let name = file
            .name
            .as_ref()
            .filter(|name| match local_bot_problem(name, roster) {
                Some(problem) => {
                    issues.add(key_path(&path, "name"), problem);
                    false
                }
                None => true,
            });
        let key = file.key.as_deref().and_then(|text| {
            parse_key_source(text)
                .map_err(|message| issues.add(key_path(&path, "key"), message))
                .ok()
        });
        let auth_tag =
            parse_optional_auth_tag(&file.auth_tag, &key_path(&path, "auth_tag"), issues);
        // Owner decision O2: zero concurrency would wedge the bot's queue behind a wake that can
        // never dispatch.
        if file.max_concurrent == Some(0) {
            issues.add(key_path(&path, "max_concurrent"), "must be at least 1");
        }
        let adapter_path = key_path(&path, "adapter");
        let adapter = file
            .adapter
            .as_ref()
            .and_then(|file| resolve_adapter(file, &adapter_path, issues));
        if let (Some(name), Some(key), Some(auth_tag), Some(adapter)) =
            (name, key, auth_tag, adapter)
        {
            bots.insert(
                name.clone(),
                RouterBot {
                    key,
                    auth_tag,
                    max_concurrent: file.max_concurrent.unwrap_or(DEFAULT_MAX_CONCURRENT),
                    adapter,
                },
            );
        }
    }
    bots
}

/// What is wrong with serving `name` here, if anything: it must be a roster bot (R1.7).
fn local_bot_problem(name: &BotName, roster: &Roster) -> Option<String> {
    if roster.bots.contains_key(name) {
        None
    } else {
        Some(format!("{:?} is not a bot in the roster", name.as_str()))
    }
}

/// `keychain`, or `file:<path>` with a non-empty path (R1.8).
fn parse_key_source(text: &str) -> Result<KeySource, String> {
    if text == "keychain" {
        return Ok(KeySource::Keychain);
    }
    match text.strip_prefix("file:") {
        Some(path) if !path.trim().is_empty() => Ok(KeySource::File(PathBuf::from(path))),
        Some(_) => Err("a file key needs a path after \"file:\"".to_owned()),
        None => Err("must be \"keychain\" or \"file:<path>\"".to_owned()),
    }
}

/// An empty `auth_tag` is no tag. Otherwise it must be a NIP-OA tag (R1.9). The outer `None`
/// means the tag was rejected.
fn parse_optional_auth_tag(
    text: &str,
    path: &str,
    issues: &mut Issues,
) -> Option<Option<nostr::Tag>> {
    if text.is_empty() {
        return Some(None);
    }
    match parse_auth_tag(text) {
        Ok(tag) => Some(Some(tag)),
        Err(error) => {
            issues.add(path, format!("not a NIP-OA auth tag: {error}"));
            None
        }
    }
}

/// Turns a whole adapter table into its resolved form. An empty optional string is `None`.
/// An empty command is rejected (owner decision O2): there would be no program to run.
fn resolve_adapter(file: &AdapterFile, path: &str, issues: &mut Issues) -> Option<AdapterConfig> {
    match file {
        AdapterFile::Command {
            command,
            cwd,
            env,
            prompt_mode,
            reply_mode,
            prompt_template,
        } => {
            let command = command.clone()?;
            if command.is_empty() {
                issues.add(key_path(path, "command"), "must name a program to run");
                return None;
            }
            Some(AdapterConfig::Command {
                command,
                cwd: cwd.clone()?,
                env: env.clone(),
                prompt_mode: (*prompt_mode)?,
                reply_mode: (*reply_mode)?,
                prompt_template: non_empty(prompt_template).map(PathBuf::from),
            })
        }
        AdapterFile::Webhook {
            url,
            secret_env,
            mode,
            cancel_url,
        } => Some(AdapterConfig::Webhook {
            url: url.clone()?,
            secret_env: secret_env.clone()?,
            mode: (*mode)?,
            cancel_url: non_empty(cancel_url).map(str::to_owned),
        }),
    }
}

fn non_empty(text: &str) -> Option<&str> {
    Some(text).filter(|text| !text.is_empty())
}
