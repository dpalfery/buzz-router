//! `@everyone` detection (design section 5.4, requirement 7.1).

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

use super::text::blank;

/// `@everyone` at the start of the text or after whitespace, as a whole word (requirement 7.1).
/// Compiled once. See [`blank`] for why the static holds an `Option`.
static EVERYONE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)(^|\s)@everyone\b").ok());

/// Whether `text` contains `@everyone`.
///
/// `text` is expected to be the output of [`mention_text`](super::mention_text), so that code
/// regions and quoted lines are already gone. The caller counts the result only when the author
/// is the owner (requirement 7.2).
pub fn contains_everyone(text: &str) -> bool {
    EVERYONE
        .as_ref()
        .is_some_and(|pattern| pattern.is_match(text))
}

/// `text` with every `@everyone` match replaced by a space (control parsing, requirement 29.2).
pub(super) fn blank_everyone(text: &str) -> Cow<'_, str> {
    blank(EVERYONE.as_ref(), text)
}
