//! Quiet hours (design section 5.6, requirements 22.1, 22.2 and 22.6, assumption A1).
//!
//! The engine calls [`quiet_set`] each time it builds a `Snapshot`, so `route` reads no clock and
//! decides quiet hours from the instant it is given (DD-4).

use std::cmp::Ordering;
use std::collections::BTreeSet;

use chrono::{DateTime, NaiveTime, Utc};

use crate::config::{QuietHours, Roster};
use crate::ids::BotName;

/// Returns every roster bot whose `quiet_hours` range contains the wall-clock time of `now` in
/// the owner's timezone.
///
/// The range is the half-open interval `[start, end)`. It wraps past midnight when `start` is
/// later than `end`, and it is empty, so never quiet, when `start == end`. A bot whose
/// `quiet_hours` is disabled (`""`) is never quiet. Each bot is read against its own effective
/// range, so a per-bot override applies to that bot only.
pub fn quiet_set(roster: &Roster, now: DateTime<Utc>) -> BTreeSet<BotName> {
    let local = now.with_timezone(&roster.owner.timezone).time();
    roster
        .bots
        .iter()
        .filter(|(_, bot)| {
            bot.limits
                .quiet_hours
                .is_some_and(|range| contains(range, local))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Whether the time of day `time` falls in `range`, which is half-open: `start` is inside and
/// `end` is outside.
fn contains(range: QuietHours, time: NaiveTime) -> bool {
    match range.start.cmp(&range.end) {
        Ordering::Less => range.start <= time && time < range.end,
        Ordering::Greater => time >= range.start || time < range.end,
        Ordering::Equal => false,
    }
}
