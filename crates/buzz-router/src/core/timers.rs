//! Running-wake timers: typing, the deadline and the status note (design 6.6, timers).
//!
//! Each running wake carries its own due times. The core loop sleeps until the earliest one (or
//! its 1-second tick) and then fires every timer that is due on the wall clock.

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use uuid::Uuid;

use super::apply::Core;
use crate::store::wakes::WakeState;

/// The typing indicator's cadence, `buzz-acp`'s (R35.5).
const TYPING_EVERY: TimeDelta = TimeDelta::seconds(3);

/// When a running wake's timers fall due.
pub(super) struct WakeTimers {
    typing: DateTime<Utc>,
    deadline: DateTime<Utc>,
    /// `None` once sent, or for a wake that never gets a note.
    status_note: Option<DateTime<Utc>>,
}

impl WakeTimers {
    /// Timers for a wake dispatched at `now`, whose first typing indicator was just sent.
    pub(super) fn new(
        now: DateTime<Utc>,
        deadline: DateTime<Utc>,
        status_note: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            typing: now + TYPING_EVERY,
            deadline,
            status_note,
        }
    }

    fn next(&self) -> DateTime<Utc> {
        let next = self.typing.min(self.deadline);
        self.status_note.map_or(next, |note| next.min(note))
    }
}

impl Core {
    /// How long until the earliest running-wake timer, if any.
    pub(super) fn next_timer_in(&self) -> Option<Duration> {
        let now = self.clock.now();
        self.running
            .values()
            .map(|running| running.timers.next())
            .min()
            .map(|due| (due - now).to_std().unwrap_or(Duration::ZERO))
    }

    /// Fires every timer that is due (design 6.6): the deadline ends the wake as `timeout` and
    /// cancels its runner; the status note goes out once if the agent has neither posted nor
    /// passed; typing is re-sent.
    pub(super) fn fire_timers(&mut self) {
        let now = self.clock.now();
        let due: Vec<Uuid> = self
            .running
            .iter()
            .filter(|(_, running)| running.timers.next() <= now)
            .map(|(id, _)| *id)
            .collect();
        for wake_id in due {
            let Some(running) = self.running.get_mut(&wake_id) else {
                continue;
            };
            if running.timers.deadline <= now {
                running.cancel.cancel();
                self.finish(&wake_id, WakeState::Timeout, "deadline");
                continue;
            }
            let note = running.timers.status_note.filter(|at| *at <= now).is_some();
            if note {
                running.timers.status_note = None;
            }
            let quiet = running.posts > 0 || running.passed;
            let typing = running.timers.typing <= now;
            if typing {
                running.timers.typing = now + TYPING_EVERY;
            }
            if note && !quiet {
                self.send_status_note(&wake_id);
            }
            if typing {
                self.send_typing(&wake_id);
            }
        }
    }
}
