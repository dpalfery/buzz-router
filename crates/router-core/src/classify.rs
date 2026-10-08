//! Author classification (design section 5.3, requirement 4).

use buzz_sdk::nip_oa::verify_auth_tag;

use crate::config::Roster;
use crate::ids::{BotName, Pubkey};
use crate::route::InEvent;

/// The one class an event's author falls into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorClass {
    /// The author's key is one of `owner.pubkeys`.
    Owner,
    /// The author's key is a roster bot's.
    Bot(BotName),
    /// The author is neither of the above but carries a valid NIP-OA `auth` tag.
    ForeignBot {
        /// Whether the tag's owner key is one of `owner.pubkeys` (assumption A12).
        owner_is_ours: bool,
    },
    /// Everyone else, including an author whose `auth` tag fails verification (requirement 4.3).
    Human,
}

/// Classifies the author of `ev`. The tests run in this order and the first match wins: owner,
/// roster bot, foreign bot, human (requirement 4.2).
pub fn classify(ev: &InEvent, roster: &Roster) -> AuthorClass {
    if roster.owner.pubkeys.contains(&ev.pubkey) {
        return AuthorClass::Owner;
    }
    if let Some(bot) = roster.bots.values().find(|bot| bot.pubkey == ev.pubkey) {
        return AuthorClass::Bot(bot.name.clone());
    }
    match verified_auth_owner(ev) {
        Some(owner) => AuthorClass::ForeignBot {
            owner_is_ours: roster.owner.pubkeys.contains(&owner),
        },
        None => AuthorClass::Human,
    }
}

/// The owner key named by the event's first `auth` tag, if NIP-OA verifies that tag for the
/// event's author. A missing tag, a malformed one and a failed signature all give `None`.
fn verified_auth_owner(ev: &InEvent) -> Option<Pubkey> {
    let tag = ev
        .tags
        .iter()
        .find(|tag| tag.first().map(String::as_str) == Some("auth"))?;
    let json = serde_json::to_string(tag).ok()?;
    let author = ev.pubkey.to_nostr().ok()?;
    let owner = verify_auth_tag(&json, &author).ok()?;
    Some(Pubkey::from_nostr(&owner))
}
