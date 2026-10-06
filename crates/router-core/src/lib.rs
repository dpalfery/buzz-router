//! Pure routing logic for buzz-router.
//!
//! This crate does no I/O, uses no async runtime and reads no clock except through the
//! `now` argument of the functions that need one. It holds [`ids`], [`config`], [`classify`],
//! [`thread`], [`parse`], [`quiet`], [`route`] and [`replay`] so far. Later tasks add the other
//! modules listed in the design: prompt and payload.

pub mod classify;
pub mod config;
pub mod ids;
pub mod parse;
pub mod quiet;
pub mod replay;
pub mod route;
pub mod thread;
