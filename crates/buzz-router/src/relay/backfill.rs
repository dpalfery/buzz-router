//! Backfill from the cursor (design section 10.4, requirements R48.1,
//! R48.2).
//!
//! Per channel, [`backfill_since`] queries `{kinds: [9, 40003], "#h":
//! [uuid], since, limit: 500}`. Paging rides on [`RestClient::query`], which
//! follows full pages with `until` and `before_id` taken from the oldest
//! event of the page until a short page, and returns everything ascending by
//! `(created_at, id)`. There is no event cap.

use nostr::{Alphabet, Filter, Kind, SingleLetterTag, Timestamp};
use uuid::Uuid;

use super::{rest::RestClient, RelayError};

/// How far before the cursor backfill starts, in seconds.
pub const OVERLAP_SECS: u64 = 300;

/// Fetches the kind-9 and kind-40003 events of `channel` since `since_secs`
/// (unix seconds), paged to completion and ascending by `(created_at, id)`.
pub async fn backfill_since(
    rest: &RestClient,
    channel: Uuid,
    since_secs: u64,
) -> Result<Vec<nostr::Event>, RelayError> {
    let filter = Filter::new()
        .kinds([Kind::Custom(9), Kind::Custom(40003)])
        .custom_tag(SingleLetterTag::lowercase(Alphabet::H), channel.to_string())
        .since(Timestamp::from(since_secs))
        .limit(500);
    rest.query(vec![filter]).await
}
