//! Per-bot limits and quiet hours (R2.4 to R2.6, R22.3).

use std::str::FromStr;

use chrono::NaiveTime;
use serde::Deserialize;
use thiserror::Error;
use toml::Table;

use super::table::{Issues, TableReader};

/// A daily window, in the owner's timezone, during which bot-caused and human-caused wakes are
/// dropped. The window is the half-open interval `[start, end)` and wraps past midnight when
/// `start` is later than `end` (R22.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    /// First wall-clock time inside the window.
    pub start: NaiveTime,
    /// First wall-clock time after the window.
    pub end: NaiveTime,
}

/// Why a `quiet_hours` value was rejected (R22.3).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QuietHoursError {
    /// The text is not of the form `HH:MM-HH:MM`.
    #[error("expected \"HH:MM-HH:MM\" or an empty string, got {0:?}")]
    Format(String),
    /// The text has the right shape but one of its times does not exist, such as `25:00`.
    #[error("{0:?} is not a valid time of day")]
    Time(String),
}

impl FromStr for QuietHours {
    type Err = QuietHoursError;

    /// Parses `HH:MM-HH:MM`: two digits, a colon, two digits, a hyphen, and the same again.
    fn from_str(text: &str) -> Result<Self, QuietHoursError> {
        let (start, end) = text
            .split_once('-')
            .ok_or_else(|| QuietHoursError::Format(text.to_owned()))?;
        Ok(Self {
            start: parse_time(start, text)?,
            end: parse_time(end, text)?,
        })
    }
}

/// Parses one `HH:MM` half of a range. `whole` is the full text, for the error message.
fn parse_time(part: &str, whole: &str) -> Result<NaiveTime, QuietHoursError> {
    let &[tens_h, units_h, b':', tens_m, units_m] = part.as_bytes() else {
        return Err(QuietHoursError::Format(whole.to_owned()));
    };
    if ![tens_h, units_h, tens_m, units_m]
        .iter()
        .all(u8::is_ascii_digit)
    {
        return Err(QuietHoursError::Format(whole.to_owned()));
    }
    let hour = u32::from(tens_h - b'0') * 10 + u32::from(units_h - b'0');
    let minute = u32::from(tens_m - b'0') * 10 + u32::from(units_m - b'0');
    NaiveTime::from_hms_opt(hour, minute, 0).ok_or_else(|| QuietHoursError::Time(part.to_owned()))
}

/// How a `quiet_hours` key reads: `""` disables quiet hours, anything else is a range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(super) enum QuietSetting {
    Disabled,
    Range(QuietHours),
}

impl TryFrom<String> for QuietSetting {
    type Error = QuietHoursError;

    fn try_from(text: String) -> Result<Self, QuietHoursError> {
        if text.is_empty() {
            Ok(Self::Disabled)
        } else {
            text.parse().map(Self::Range)
        }
    }
}

/// The effective limits of one bot (R2.5, R2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// How many bot turns a discussion round allows.
    pub turns_per_round: u32,
    /// Wakes dispatched in the trailing 60 minutes.
    pub wakes_per_hour: u32,
    /// Wakes dispatched in the trailing 24 hours.
    pub wakes_per_day: u32,
    /// The quiet-hours window, or `None` when quiet hours are disabled.
    pub quiet_hours: Option<QuietHours>,
    /// Seconds a discussion wake waits for more triggers.
    pub discussion_debounce_secs: u64,
    /// The longest a discussion wake may keep waiting, in seconds.
    pub discussion_debounce_max_secs: u64,
    /// Minutes before a running wake is killed.
    pub max_wake_minutes: u64,
    /// Replies one wake may publish.
    pub max_posts_per_wake: u32,
    /// Seconds into a wake before the router posts a status note.
    pub status_note_after_secs: u64,
}

impl Default for Limits {
    /// The defaults of R2.5.
    fn default() -> Self {
        Self {
            turns_per_round: 4,
            wakes_per_hour: 20,
            wakes_per_day: 100,
            quiet_hours: Some(QuietHours {
                start: time_of_day(23, 0),
                end: time_of_day(7, 0),
            }),
            discussion_debounce_secs: 20,
            discussion_debounce_max_secs: 90,
            max_wake_minutes: 20,
            max_posts_per_wake: 3,
            status_note_after_secs: 20,
        }
    }
}

/// A time of day from literals that are known to be valid. The midnight fallback is unreachable
/// and exists only so the function cannot panic.
const fn time_of_day(hour: u32, minute: u32) -> NaiveTime {
    match NaiveTime::from_hms_opt(hour, minute, 0) {
        Some(time) => time,
        None => NaiveTime::MIN,
    }
}

/// A `[limits]` table or a bot's `limits` table, as written: every key is optional.
#[derive(Debug, Default)]
pub(super) struct LimitsFile {
    turns_per_round: Option<u32>,
    wakes_per_hour: Option<u32>,
    wakes_per_day: Option<u32>,
    quiet_hours: Option<QuietSetting>,
    discussion_debounce_secs: Option<u64>,
    discussion_debounce_max_secs: Option<u64>,
    max_wake_minutes: Option<u64>,
    max_posts_per_wake: Option<u32>,
    status_note_after_secs: Option<u64>,
}

impl LimitsFile {
    /// Reads the table at `path`, recording an issue for every unknown or ill-typed key.
    pub(super) fn read(table: &Table, path: &str, issues: &mut Issues) -> Self {
        let mut reader = TableReader::new(table, path);
        let file = Self {
            turns_per_round: reader.optional("turns_per_round", issues),
            wakes_per_hour: reader.optional("wakes_per_hour", issues),
            wakes_per_day: reader.optional("wakes_per_day", issues),
            quiet_hours: reader.optional("quiet_hours", issues),
            discussion_debounce_secs: reader.optional("discussion_debounce_secs", issues),
            discussion_debounce_max_secs: reader.optional("discussion_debounce_max_secs", issues),
            max_wake_minutes: reader.optional("max_wake_minutes", issues),
            max_posts_per_wake: reader.optional("max_posts_per_wake", issues),
            status_note_after_secs: reader.optional("status_note_after_secs", issues),
        };
        reader.finish(issues);
        file
    }

    /// Lays this table over `base`, key by key: a key written here wins, an omitted key keeps
    /// the value from `base`.
    pub(super) fn layer(&self, base: Limits) -> Limits {
        Limits {
            turns_per_round: self.turns_per_round.unwrap_or(base.turns_per_round),
            wakes_per_hour: self.wakes_per_hour.unwrap_or(base.wakes_per_hour),
            wakes_per_day: self.wakes_per_day.unwrap_or(base.wakes_per_day),
            quiet_hours: match self.quiet_hours {
                Some(QuietSetting::Disabled) => None,
                Some(QuietSetting::Range(range)) => Some(range),
                None => base.quiet_hours,
            },
            discussion_debounce_secs: self
                .discussion_debounce_secs
                .unwrap_or(base.discussion_debounce_secs),
            discussion_debounce_max_secs: self
                .discussion_debounce_max_secs
                .unwrap_or(base.discussion_debounce_max_secs),
            max_wake_minutes: self.max_wake_minutes.unwrap_or(base.max_wake_minutes),
            max_posts_per_wake: self.max_posts_per_wake.unwrap_or(base.max_posts_per_wake),
            status_note_after_secs: self
                .status_note_after_secs
                .unwrap_or(base.status_note_after_secs),
        }
    }
}
