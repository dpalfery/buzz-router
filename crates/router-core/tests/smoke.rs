//! Smoke tests for task 1.1: prove that the two Buzz git dependencies link.
//!
//! `buzz-core` and `buzz-sdk` are pinned to a git `rev` of `https://github.com/block/buzz`.
//! Calling one public function from each crate proves both resolve, compile and link.
//!
//! Every test name carries the `smoke` substring because the task's run command,
//! `cargo test --workspace --locked smoke`, filters on test names, not file names.

use buzz_core::nip10::{parse_thread_markers_from_parts, ThreadMarkers};
use buzz_sdk::mentions::strip_code_regions;

#[test]
fn smoke_buzz_core_nip10_empty_tag_list_yields_default_markers() {
    let tags: Vec<Vec<String>> = Vec::new();

    let markers = parse_thread_markers_from_parts(tags.iter().map(Vec::as_slice));

    assert_eq!(markers, ThreadMarkers::default());
}

#[test]
fn smoke_buzz_sdk_mentions_inline_code_span_is_replaced_by_one_space() {
    let stripped = strip_code_regions("`x`");

    assert_eq!(stripped, " ");
}
