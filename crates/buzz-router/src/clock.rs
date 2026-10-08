//! The wall clock the daemon reads (design section 6.1).
//!
//! Every timer uses `tokio::time`. [`VirtualClock`] derives the wall clock from tokio's clock, so
//! tests that pause tokio time (`#[tokio::test(start_paused = true)]`) control both.

use chrono::{DateTime, Utc};

/// A source of the current wall-clock time.
pub trait Clock: Send + Sync {
    /// The current time.
    fn now(&self) -> DateTime<Utc>;
}

/// The real clock: [`Utc::now`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that starts at `base` and advances with tokio's clock.
#[derive(Debug, Clone, Copy)]
pub struct VirtualClock {
    base: DateTime<Utc>,
    start: tokio::time::Instant,
}

impl VirtualClock {
    /// A clock reading `base` now, advancing as tokio time advances.
    pub fn new(base: DateTime<Utc>) -> Self {
        Self {
            base,
            start: tokio::time::Instant::now(),
        }
    }
}

impl Clock for VirtualClock {
    fn now(&self) -> DateTime<Utc> {
        let elapsed = tokio::time::Instant::now().saturating_duration_since(self.start);
        chrono::Duration::from_std(elapsed)
            .ok()
            .and_then(|elapsed| self.base.checked_add_signed(elapsed))
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
    }
}
