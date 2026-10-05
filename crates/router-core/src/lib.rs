//! Pure routing logic for buzz-router.
//!
//! This crate does no I/O, uses no async runtime and reads no clock except through the
//! `now` argument of the functions that need one. It holds [`ids`] and [`config`] so far. Later
//! tasks add the other modules listed in the design: classify, parse, thread, route, quiet,
//! prompt, payload and replay.

pub mod config;
pub mod ids;
