//! The pure text logic of the probe. Every offset here is a character
//! offset, as AT-SPI gives it; none is ever used as a byte index.

use serde::Serialize;

/// U+FFFC, the placeholder AT-SPI puts where a child object is embedded.
pub const EMBEDDED_OBJECT: char = '\u{FFFC}';

/// Length in characters.
pub fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// The characters `start..end` of `text`. Offsets beyond the text are
/// clamped; a reversed range is empty.
pub fn char_slice(text: &str, start: usize, end: usize) -> &str {
    if end <= start {
        return "";
    }
    let byte_at = |offset: usize| {
        text.char_indices()
            .nth(offset)
            .map_or(text.len(), |(byte, _)| byte)
    };
    let from = byte_at(start);
    let to = byte_at(end);
    &text[from..to]
}

/// The first `max` characters of `text` and whether anything was cut.
pub fn clip(text: &str, max: usize) -> (String, bool) {
    let clipped = char_slice(text, 0, max);
    (clipped.to_owned(), clipped.len() < text.len())
}

/// Runs of whitespace (including no-break spaces and line breaks) become
/// one space; leading and trailing whitespace goes.
pub fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Number of embedded-object characters in `text`.
pub fn embedded_object_count(text: &str) -> usize {
    text.chars().filter(|c| *c == EMBEDDED_OBJECT).count()
}

/// Whether `actual` is the expected text once whitespace is normalised.
/// Nothing else is folded: case, quotes and dashes must agree.
pub fn matches_expectation(expected: &str, actual: &str) -> bool {
    normalize_whitespace(expected) == normalize_whitespace(actual)
}

/// A sentence the probe cut out of wider text itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Derived {
    pub text: String,
    /// Character offsets of the sentence inside the wider text.
    pub start: usize,
    pub end: usize,
    /// No sentence boundary was found before the selection: the sentence
    /// starts where the wider text starts.
    pub open_start: bool,
    /// No sentence boundary was found after the selection.
    pub open_end: bool,
}

fn is_terminator(c: char) -> bool {
    matches!(
        c,
        '.' | '!' | '?' | '…' | '‼' | '⁇' | '⁈' | '⁉' | '。' | '！' | '？'
    )
}

fn is_closer(c: char) -> bool {
    matches!(c, '"' | '\'' | '”' | '’' | '»' | '›' | ')' | ']' | '}')
}

fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

/// Sentence breaks of `chars` as `(end of the sentence, start of the next)`.
///
/// The rule is deliberately small and is part of the evidence: a sentence
/// ends at a run of `. ! ? …` (plus closing quotes or brackets) followed by
/// whitespace, unless the next word starts with a lowercase letter; a line
/// break always ends a sentence.
fn breaks(chars: &[char]) -> Vec<(usize, usize)> {
    let n = chars.len();
    let mut found = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if is_line_break(c) {
            let mut next = i;
            while next < n && chars[next].is_whitespace() {
                next += 1;
            }
            found.push((i, next));
            i = next;
        } else if is_terminator(c) {
            let mut end = i + 1;
            while end < n && is_terminator(chars[end]) {
                end += 1;
            }
            while end < n && is_closer(chars[end]) {
                end += 1;
            }
            let mut next = end;
            let mut line_break = false;
            while next < n && chars[next].is_whitespace() {
                line_break |= is_line_break(chars[next]);
                next += 1;
            }
            if next > end && (next == n || line_break || !chars[next].is_lowercase()) {
                found.push((end, next));
            }
            i = next.max(i + 1);
        } else {
            i += 1;
        }
    }
    found
}

/// The sentence of `text` around the characters `sel_start..sel_end`, or
/// `None` when the range does not lie in the text or the result is empty.
pub fn derive_sentence(text: &str, sel_start: usize, sel_end: usize) -> Option<Derived> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n == 0 || sel_start > sel_end || sel_end > n {
        return None;
    }
    let breaks = breaks(&chars);
    let before = breaks
        .iter()
        .filter(|(_, next)| *next <= sel_start)
        .map(|(_, next)| *next)
        .max();
    let after = breaks
        .iter()
        .filter(|(end, _)| *end >= sel_end && *end >= before.unwrap_or(0))
        .map(|(end, _)| *end)
        .min();
    let mut start = before.unwrap_or(0);
    let mut end = after.unwrap_or(n);
    while start < end && chars[start].is_whitespace() {
        start += 1;
    }
    while end > start && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if start >= end {
        return None;
    }
    Some(Derived {
        text: chars[start..end].iter().collect(),
        start,
        end,
        open_start: before.is_none(),
        open_end: after.is_none(),
    })
}
