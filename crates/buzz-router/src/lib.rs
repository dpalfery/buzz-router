//! The buzz-router daemon and CLI.
//!
//! The binary in `src/main.rs` only calls [`cli::main`]. Every module lives in this library
//! target so that the integration tests under `tests/` can reach them.

pub mod cli;
mod logging;
pub mod paths;
