//! The buzz-router daemon and CLI.
//!
//! The binary in `src/main.rs` only calls [`cli::main`]. Every module lives in this library
//! target so that the integration tests under `tests/` can reach them.

pub mod adapter;
pub mod api;
pub mod cli;
pub mod clock;
pub mod core;
pub mod ingest;
pub mod keys;
mod logging;
pub mod paths;
pub mod publish;
pub mod relay;
pub mod service;
pub mod store;
