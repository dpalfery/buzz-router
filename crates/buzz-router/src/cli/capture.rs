//! `capture --channel ID --since DUR`: a channel's recent events as JSON Lines (design 12.1,
//! requirements 55.1 and 57).
//!
//! The command loads the configuration, finds the first local bot (in name order) that is a
//! member of the channel by the discovery query of design 10.2 (kind 39002 with `#p` = the bot's
//! pubkey), and REST-pages `{kinds:[9,40003], #h:[channel], since: now − DUR}` as that bot. It
//! writes the raw signed events to stdout, one per line, ascending by `(created_at, id)`.

use std::fs;
use std::io::{self, BufWriter, Write};

use nostr::{Alphabet, Filter, JsonUtil, Kind, SingleLetterTag, Timestamp};
use router_core::config::parse_router;
use router_core::ids::ChannelId;
use router_core::route::{KIND_EDIT, KIND_MESSAGE};

use super::roster::{load as load_roster, roster_path};
use super::{write_error, CliError};
use crate::keys::{load_key, KeySource};
use crate::paths::Dirs;
use crate::relay::rest::RestClient;
use crate::relay::RelayError;

/// The router configuration file, in the config directory.
const ROUTER_TOML: &str = "router.toml";

/// The kind of a channel's member list, used for discovery (design 10.2).
const KIND_MEMBERS: u16 = 39002;

/// The page size for the channel query (design 10.4).
const PAGE_LIMIT: usize = 500;

/// Runs `capture`.
pub(super) fn run(dirs: &Dirs, channel: &str, since: &str) -> Result<(), CliError> {
    let window = parse_since(since)?;
    let channel = ChannelId::parse(channel)
        .map_err(|error| CliError::bad_input(format!("invalid --channel: {error}")))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::other(format!("cannot start the async runtime: {error}")))?;
    let events = runtime.block_on(fetch(dirs, channel, window))?;

    let mut out = BufWriter::new(io::stdout().lock());
    for event in events {
        writeln!(out, "{}", event.as_json()).map_err(write_error)?;
    }
    out.flush().map_err(write_error)
}

/// Parses `<n>m`, `<n>h` or `<n>d` into seconds.
fn parse_since(text: &str) -> Result<u64, CliError> {
    let invalid = || {
        CliError::bad_input(format!(
            "invalid --since {text:?}: expected a number and one of m, h or d, such as 7d"
        ))
    };
    let unit = match text.chars().last() {
        Some('m') => 60,
        Some('h') => 3_600,
        Some('d') => 86_400,
        _ => return Err(invalid()),
    };
    let digits = &text[..text.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let count: u64 = digits.parse().map_err(|_| invalid())?;
    count.checked_mul(unit).ok_or_else(invalid)
}

/// Loads the configuration, picks the member bot and queries the channel.
async fn fetch(
    dirs: &Dirs,
    channel: ChannelId,
    window: u64,
) -> Result<Vec<nostr::Event>, CliError> {
    let (roster, _) = load_roster(&roster_path(&dirs.config_dir)?)?;
    let router_toml = dirs.config_dir.join(ROUTER_TOML);
    let text = fs::read_to_string(&router_toml).map_err(|error| {
        CliError::bad_input(format!("cannot read {}: {error}", router_toml.display()))
    })?;
    let config = parse_router(&text, &roster).map_err(|errors| {
        CliError::bad_input(format!("invalid {}: {errors}", router_toml.display()))
    })?;

    let mut unavailable = Vec::new();
    for (name, bot) in &config.bots {
        let keys = match load_key(&KeySource::from_config(&bot.key, &dirs.config_dir), name) {
            Ok(keys) => keys,
            Err(error) => {
                unavailable.push(format!("{name}: {error}"));
                continue;
            }
        };
        let auth_tag = bot
            .auth_tag
            .as_ref()
            .and_then(|tag| serde_json::to_string(tag.as_slice()).ok());
        let client = RestClient::new(&config.relay_url, keys.clone(), auth_tag);
        if !is_member(&client, &keys, channel).await? {
            continue;
        }
        let since = now_secs().saturating_sub(window);
        let filter = Filter::new()
            .kinds([Kind::Custom(KIND_MESSAGE), Kind::Custom(KIND_EDIT)])
            .custom_tag(SingleLetterTag::lowercase(Alphabet::H), channel.to_string())
            .since(Timestamp::from(since))
            .limit(PAGE_LIMIT);
        return client.query(vec![filter]).await.map_err(relay_error);
    }
    let mut message = format!("no local bot is a member of channel {channel}");
    if !unavailable.is_empty() {
        message.push_str(&format!(
            "; bots whose key did not load: {}",
            unavailable.join("; ")
        ));
    }
    Err(CliError::bad_input(message))
}

/// Whether the bot signing as `keys` is a member of `channel`: a kind-39002 list with `#p` = its
/// pubkey has a `d` tag naming the channel (design 10.2).
async fn is_member(
    client: &RestClient,
    keys: &nostr::Keys,
    channel: ChannelId,
) -> Result<bool, CliError> {
    let filter = Filter::new()
        .kind(Kind::Custom(KIND_MEMBERS))
        .pubkey(keys.public_key());
    let lists = client.query(vec![filter]).await.map_err(relay_error)?;
    Ok(lists.iter().any(|list| {
        list.tags.iter().any(|tag| match tag.as_slice() {
            [name, id, ..] if name == "d" => {
                ChannelId::parse(id).is_ok_and(|listed| listed == channel)
            }
            _ => false,
        })
    }))
}

/// Maps a relay failure to the CLI's error kinds: rejected auth is `Auth`, an unreachable relay is
/// `Network`, anything else is `Other`.
fn relay_error(error: RelayError) -> CliError {
    match error {
        RelayError::Status(401 | 403) => CliError::auth(error.to_string()),
        RelayError::Transport(_) => CliError::network(error.to_string()),
        RelayError::Status(_)
        | RelayError::Decode(_)
        | RelayError::Auth(_)
        | RelayError::Rejected(_) => CliError::other(error.to_string()),
    }
}

/// The current unix time in seconds.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::parse_since;

    #[test]
    fn since_accepts_minutes_hours_and_days() {
        assert_eq!(parse_since("30m").ok(), Some(1_800));
        assert_eq!(parse_since("2h").ok(), Some(7_200));
        assert_eq!(parse_since("7d").ok(), Some(604_800));
    }

    #[test]
    fn since_rejects_anything_else() {
        for text in ["7x", "h", "", "-1h", "1.5h", "2 h", "99999999999999999999d"] {
            assert!(parse_since(text).is_err(), "{text:?}");
        }
    }
}
