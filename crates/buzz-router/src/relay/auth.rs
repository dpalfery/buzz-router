//! NIP-42 authentication events (design section 10.1).
//!
//! This follows `examples/countdown-bot`: without an auth tag the event is
//! `EventBuilder::auth(challenge, relay_url)`; with one it is kind 22242
//! carrying the `relay`, `challenge` and `auth` tags. Either way it is signed
//! by the bot's keys.

use super::RelayError;

/// Builds the NIP-42 auth event answering `challenge` on `relay_url`.
///
/// `auth_tag` is the configured NIP-OA tag string, sent verbatim as the
/// second element of the `auth` tag.
pub fn build_auth_event(
    keys: &nostr::Keys,
    relay_url: &str,
    challenge: &str,
    auth_tag: Option<&str>,
) -> Result<nostr::Event, RelayError> {
    if let Some(tag) = auth_tag {
        let tags = vec![
            nostr::Tag::parse(["relay", relay_url])
                .map_err(|error| RelayError::Auth(format!("auth tag error: {error}")))?,
            nostr::Tag::parse(["challenge", challenge])
                .map_err(|error| RelayError::Auth(format!("auth tag error: {error}")))?,
            nostr::Tag::parse(["auth", tag])
                .map_err(|error| RelayError::Auth(format!("auth tag error: {error}")))?,
        ];
        nostr::EventBuilder::new(nostr::Kind::Authentication, "")
            .tags(tags)
            .sign_with_keys(keys)
            .map_err(|error| RelayError::Auth(format!("auth sign error: {error}")))
    } else {
        let url = nostr::RelayUrl::parse(relay_url)
            .map_err(|error| RelayError::Auth(format!("bad relay url: {error}")))?;
        nostr::EventBuilder::auth(challenge, url)
            .sign_with_keys(keys)
            .map_err(|error| RelayError::Auth(format!("auth sign error: {error}")))
    }
}
