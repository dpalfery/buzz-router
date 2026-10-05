//! Pure routing logic for buzz-router.
//!
//! This crate does no I/O, uses no async runtime and reads no clock except through the
//! `now` argument of the functions that need one. Later tasks add the modules listed in the
//! design: ids, config, classify, parse, thread, route, quiet, prompt, payload and replay.
