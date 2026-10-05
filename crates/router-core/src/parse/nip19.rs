//! `nostr:nprofile1` URI decoding (design section 5.4, assumption A18).
//!
//! `buzz_sdk::mentions::extract_nostr_uris` decodes only `nostr:npub1` URIs at the pinned Buzz
//! commit, so the router decodes `nprofile` URIs itself with the `nostr` crate's NIP-19 support.

use nostr::nips::nip19::Nip19Profile;
use nostr::FromBech32;

use crate::ids::Pubkey;

/// The URI scheme that introduces a NIP-27 reference.
const SCHEME: &str = "nostr:";
/// What follows the scheme for a profile reference. Lower case, as `extract_nostr_uris` expects
/// of `npub1`.
const PROFILE_PREFIX: &str = "nprofile1";

/// The pubkeys named by the `nostr:nprofile1...` URIs in `text`, each once, in order of first
/// appearance.
///
/// The data after `nprofile1` is the longest run of ASCII letters and digits. It is lowercased
/// before decoding, because NIP-19 allows upper case. A candidate that does not decode (bad
/// checksum, wrong kind, no data) is dropped. The relay hints inside a profile are ignored.
pub(super) fn nprofile_pubkeys(text: &str) -> Vec<Pubkey> {
    let mut found: Vec<Pubkey> = Vec::new();
    for segment in text.split(SCHEME).skip(1) {
        if !segment.starts_with(PROFILE_PREFIX) {
            continue;
        }
        let end = segment
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(segment.len());
        let Some(candidate) = segment.get(..end) else {
            continue;
        };
        let Ok(profile) = Nip19Profile::from_bech32(&candidate.to_ascii_lowercase()) else {
            continue;
        };
        let pubkey = Pubkey::from_nostr(&profile.public_key);
        if !found.contains(&pubkey) {
            found.push(pubkey);
        }
    }
    found
}
