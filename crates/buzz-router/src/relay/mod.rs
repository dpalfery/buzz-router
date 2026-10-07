//! The relay client (design section 10, requirements 50 and 61).
//!
//! One [`RelayConn`](crate::relay) task runs per local bot (task 2.4). This
//! module holds the shared pieces: the [`RelayPort`] trait that the core uses
//! to publish and query (tests substitute a fake), the [`RelayError`], and
//! the REST client in [`rest`] (NIP-98, retries, paging).

pub mod auth;
pub mod conn;
pub mod rest;

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// A failure to talk to the relay, over WebSocket or REST.
#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    /// The request could not be sent or completed (timeout, connect error).
    #[error("relay transport error: {0}")]
    Transport(String),
    /// The relay answered with a non-retriable, non-success status.
    #[error("relay returned HTTP {0}")]
    Status(u16),
    /// The relay's response could not be understood.
    #[error("relay response was not valid: {0}")]
    Decode(String),
    /// The relay answered `false` to a publish, with its reason.
    #[error("the relay rejected the event: {0}")]
    Rejected(String),
    /// The request could not be authenticated (NIP-98 signing failed).
    #[error("cannot build relay auth: {0}")]
    Auth(String),
}

/// What the core needs from a relay connection (design section 6.1).
///
/// The production implementation is the per-bot connection handle plus
/// [`rest::RestClient`]; tests use a fake. Every method is a boxed future so
/// implementers can hold state across awaits.
pub trait RelayPort: Send + Sync {
    /// Publishes a signed event, resolving when the relay acknowledges it.
    fn publish(
        &self,
        event: nostr::Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), RelayError>> + Send + '_>>;

    /// Queries historical events matching any of the filters.
    fn query(
        &self,
        filters: Vec<nostr::Filter>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<nostr::Event>, RelayError>> + Send + '_>>;
}

/// Applies ±20% jitter to a retry or reconnect delay, exactly as `buzz-acp`
/// does: the factor is drawn from the current sub-second nanos over
/// `u32::MAX`, so it lands in [0.8, 0.9) in practice and never exceeds the
/// base.
pub(crate) fn jittered(base: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or_default();
    // factor in [0.8, 1.2).
    let factor = 0.8 + (f64::from(nanos) / f64::from(u32::MAX)) * 0.4;
    base.mul_f64(factor)
}
