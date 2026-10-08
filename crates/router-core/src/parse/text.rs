//! Text preparation shared by the parsers (design section 5.4, requirement 17.1).

use std::borrow::Cow;

use buzz_sdk::mentions::strip_code_regions;
use regex::Regex;

/// The text in which mentions and `@everyone` are looked for.
///
/// It removes code regions with `buzz_sdk::mentions::strip_code_regions`, then replaces every
/// line whose first character is `>` with an empty line (requirement 17.1). The line structure
/// is kept, so a mention on the line after a quote still counts.
pub fn mention_text(content: &str) -> String {
    let stripped = strip_code_regions(content);
    let mut text = String::with_capacity(stripped.len());
    for (index, line) in stripped.split('\n').enumerate() {
        if index > 0 {
            text.push('\n');
        }
        if !line.starts_with('>') {
            text.push_str(line);
        }
    }
    text
}

/// Replaces every match of `pattern` in `text` with a single space.
///
/// The parsers keep their constant patterns in `LazyLock<Option<Regex>>` statics, so a pattern
/// that failed to compile is `None`. That cannot happen for the literals in this module, and the
/// tests would catch it; `None` replaces nothing.
pub(super) fn blank<'t>(pattern: Option<&Regex>, text: &'t str) -> Cow<'t, str> {
    match pattern {
        Some(pattern) => pattern.replace_all(text, " "),
        None => Cow::Borrowed(text),
    }
}
