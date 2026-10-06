//! Logging (design section 14).
//!
//! For now this only sets up the human-readable stderr output. The JSON log file in the data
//! directory comes with the daemon (task 3.10).

use tracing_subscriber::EnvFilter;

/// The environment variable that holds the log filter, in `EnvFilter` syntax.
const FILTER_VAR: &str = "BUZZ_ROUTER_LOG";

/// The filter used when [`FILTER_VAR`] is unset or does not parse.
const DEFAULT_FILTER: &str = "info";

/// Installs the stderr subscriber. A subscriber that is already installed stays in place.
pub fn init() {
    let filter =
        EnvFilter::try_from_env(FILTER_VAR).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    // The only way this fails is that a global subscriber exists already, which is fine.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
