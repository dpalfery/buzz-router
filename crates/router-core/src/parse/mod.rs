//! Parsers for message text (design section 5.4, requirements 7.1, 17 and 29.2 to 29.6).
//!
//! - [`mention_text`] removes the parts of a message that never count as mentions: code regions
//!   and quoted lines.
//! - [`mentioned_bots`] finds the roster bots a message addresses by `@name`, by `nostr:npub1`
//!   and `nostr:nprofile1` URI, and by `p` tag.
//! - [`contains_everyone`] detects `@everyone`. The caller counts it only for owner messages.
//! - [`parse_control`] recognises the `stop`, `resume` and `!cancel` commands.
//! - [`mentions_for_reply`] finds the pubkeys a bot's reply tags with `p`.
//!
//! Everything here is pure. The `@name` matching comes from `buzz_sdk::mentions`, so the router
//! agrees with Buzz about what a mention is.

mod control;
mod everyone;
mod mentions;
mod nip19;
mod text;

pub use control::parse_control;
pub use everyone::contains_everyone;
pub(crate) use mentions::p_tag_bots;
pub use mentions::{mentioned_bots, mentions_for_reply};
pub use text::mention_text;
