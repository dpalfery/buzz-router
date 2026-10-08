//! Channel discovery (design section 10.2, requirement R61.2).
//!
//! This follows `buzz-acp`'s `discover_channels` and
//! `merge_discovered_channels`: REST-query kind 39002 with `#p` set to the
//! bot's pubkey, collect the channel UUIDs from the `d` tags, then REST-query
//! kind 39000 with `#d` set to those UUIDs for the names. Channels whose
//! metadata carries `["archived", "true"]` are skipped. Discovery reruns on
//! every reconnect.

use std::collections::BTreeMap;

use nostr::{Filter, Kind, PublicKey};
use uuid::Uuid;

use super::{rest::RestClient, RelayError};

/// A live channel the bot is a member of: its UUID and display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredChannel {
    /// The channel UUID (the `d` tag).
    pub id: Uuid,
    /// The channel name (the `name` tag of its kind-39000 metadata).
    pub name: String,
}

/// Discovers the live channels `bot_pubkey_hex` is a member of.
///
/// `bot_pubkey_hex` is the bot's lowercase hex pubkey. The returned channels
/// are ordered by UUID, so repeated discoveries agree.
pub async fn discover_channels(
    rest: &RestClient,
    bot_pubkey_hex: &str,
) -> Result<Vec<DiscoveredChannel>, RelayError> {
    let pubkey = PublicKey::from_hex(bot_pubkey_hex)
        .map_err(|error| RelayError::Decode(format!("bad bot pubkey: {error}")))?;
    let memberships = rest
        .query(vec![Filter::new().kind(Kind::Custom(39002)).pubkey(pubkey)])
        .await?;
    let mut ids: Vec<String> = Vec::new();
    for event in &memberships {
        for tag in event.tags.iter() {
            let cells = tag.as_slice();
            if cells.first().map(String::as_str) == Some("d") {
                if let Some(value) = cells.get(1) {
                    if Uuid::parse_str(value).is_ok() && !ids.iter().any(|seen| seen == value) {
                        ids.push(value.clone());
                    }
                }
            }
        }
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let metadata = rest
        .query(vec![Filter::new()
            .kind(Kind::Custom(39000))
            .identifiers(ids)])
        .await?;
    let mut named: BTreeMap<Uuid, String> = BTreeMap::new();
    for event in &metadata {
        let Some(raw) = tag_value(event, "d") else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(raw) else {
            continue;
        };
        if event.tags.iter().any(|tag| {
            let cells = tag.as_slice();
            cells.first().map(String::as_str) == Some("archived")
                && cells.get(1).map(String::as_str) == Some("true")
        }) {
            named.remove(&id);
            continue;
        }
        let Some(name) = tag_value(event, "name") else {
            continue;
        };
        named.insert(id, name.to_string());
    }
    Ok(named
        .into_iter()
        .map(|(id, name)| DiscoveredChannel { id, name })
        .collect())
}

/// The value of the first tag called `name` on `event`, if there is one.
fn tag_value<'e>(event: &'e nostr::Event, name: &str) -> Option<&'e str> {
    event.tags.iter().find_map(|tag| {
        let cells = tag.as_slice();
        if cells.first().map(String::as_str) == Some(name) {
            cells.get(1).map(String::as_str)
        } else {
            None
        }
    })
}
