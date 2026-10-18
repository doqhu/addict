//! Splitting a sentence into the words the user can pick, and the other text
//! rules of the input: the word as the dictionary writes it, a pasted text as
//! one line, a recognized text as one line and the sentence of a word in it.
//!
//! A word is a maximal run of alphabetic characters. Three things continue a
//! word instead of ending it: an apostrophe, U+0027 or U+2019, that stands
//! between two letters, so `don't` is one word and the quotation marks of
//! `'bank'` or the mark of `boys'` stay outside; the Catalan middle dot,
//! U+00B7, between two letters in the same way, so `col·lecció` is one word
//! and a dot with no letter on one side separates; and a combining mark of
//! U+0300–U+036F after a letter, so a letter typed with its accent apart is
//! not cut. Everything else only separates words, a hyphen included:
//! `well-known` is two words. The ranges are byte ranges of the given text,
//! so the text itself is kept as it was, punctuation included.
//!
//! The text read from an editor over the accessibility bus is cut by
//! [`of_selection`]: the sentence around a selection given in character
//! offsets, a line break ending a sentence. Only the area confirmed for the
//! editor gives one: a whole-line `//` comment, the sentence not the first in
//! its line and ending in it with a stop.
//!
//! A text recognized from a screen capture ([`recognized`]) has its lines
//! joined with one space, an empty line among them being only a line break;
//! a line that ends in `-` right after a letter, followed by a line that
//! starts with a lowercase letter, is joined without the hyphen, so `num-`
//! and `ber` give `number`. Such a text often holds several sentences, and
//! [`around`] finds the one that holds a word: a sentence ends after a run
//! of `.`, `!`, `?` or `…`, with any closing `"`, `”`, `’`, `)` or `]` right
//! after it, when whitespace and then a character that is not a lowercase
//! letter follow, or the end of the text. A lowercase letter after the
//! period keeps `e.g. the` in one sentence; there is no list of
//! abbreviations, so `Mr. Smith` is split. A line without closing
//! punctuation, such as a heading, belongs to the sentence after it.

use std::ops::Range;

/// The apostrophe the dictionaries write their headwords with.
const APOSTROPHE: char = '\'';

/// The apostrophe of typeset text.
const TYPOGRAPHIC_APOSTROPHE: char = '\u{2019}';

/// The middle dot of the Catalan `l·l`.
const MIDDLE_DOT: char = '\u{b7}';

/// Whether `character` is a combining diacritical mark, U+0300–U+036F.
fn is_combining_mark(character: char) -> bool {
    ('\u{300}'..='\u{36f}').contains(&character)
}

/// Whether `character` is a letter. A combining mark never is, though Unicode
/// calls some of them alphabetic (U+0345, U+0363–U+036F).
fn is_letter(character: char) -> bool {
    character.is_alphabetic() && !is_combining_mark(character)
}

/// The byte ranges of the words of `sentence`, in order. Each range lies on
/// character boundaries and is never empty.
pub fn words(sentence: &str) -> Vec<Range<usize>> {
    let mut words = Vec::new();
    let mut start = None;
    let mut characters = sentence.char_indices().peekable();
    while let Some((at, character)) = characters.next() {
        let continues = if is_letter(character) {
            true
        } else if start.is_none() {
            // Outside a word only a letter starts one.
            false
        } else if matches!(character, APOSTROPHE | TYPOGRAPHIC_APOSTROPHE | MIDDLE_DOT) {
            // Inside a word, so after a letter or its mark: the apostrophe
            // and the middle dot join when a letter follows as well.
            characters.peek().is_some_and(|&(_, next)| is_letter(next))
        } else {
            is_combining_mark(character)
        };
        match (continues, start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                words.push(from..at);
                start = None;
            }
            (true, Some(_)) | (false, None) => {}
        }
    }
    if let Some(from) = start {
        words.push(from..sentence.len());
    }
    words
}

/// The opening single quotation mark, which recognition gives for an
/// apostrophe at times.
const OPENING_QUOTE: char = '\u{2018}';

/// `word` as it is looked up: the typographic apostrophe and the opening
/// single quotation mark become U+0027, the one the headwords are written
/// with. Nothing else changes.
pub fn lookup_word(word: &str) -> String {
    word.replace([TYPOGRAPHIC_APOSTROPHE, OPENING_QUOTE], "'")
}

/// A pasted `text` as the one line the field holds: without the whitespace
/// around it and with each run of whitespace inside, line breaks included, as
/// one space. `None` when the text is empty or blank.
pub fn pasted(text: &str) -> Option<String> {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!line.is_empty()).then_some(line)
}

/// A text recognized from a screen capture, `raw`, as the one line the field
/// holds. Each line has its runs of whitespace as one space; the lines that
/// are not blank are joined with one space, or with nothing, the hyphen
/// dropped, when a line ends in `-` right after a letter and the next one
/// starts with a lowercase letter. `None` when the text is empty or blank.
pub fn recognized(raw: &str) -> Option<String> {
    let mut joined = String::new();
    for line in raw.lines().filter_map(pasted) {
        if joined.is_empty() {
            joined = line;
            continue;
        }
        let hyphenated = joined
            .strip_suffix('-')
            .and_then(|before| before.chars().next_back())
            .is_some_and(is_letter)
            && line.chars().next().is_some_and(char::is_lowercase);
        if hyphenated {
            joined.pop();
        } else {
            joined.push(' ');
        }
        joined.push_str(&line);
    }
    (!joined.is_empty()).then_some(joined)
}

/// Whether `character` ends a sentence when the rest allows it.
fn is_terminator(character: char) -> bool {
    matches!(character, '.' | '!' | '?' | '\u{2026}')
}

/// Whether `character` may close a sentence right after its terminator.
fn is_closing(character: char) -> bool {
    matches!(character, '"' | '\u{201d}' | '\u{2019}' | ')' | ']')
}

/// `range` of `text` without the whitespace at either end; `None` when
/// nothing is left.
fn trimmed(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    let part = &text[range.clone()];
    let start = range.start + (part.len() - part.trim_start().len());
    let end = range.end - (part.len() - part.trim_end().len());
    (start < end).then_some(start..end)
}

/// The byte ranges of the sentences of `text`, in order and without the
/// whitespace around them. A text without a sentence end is one sentence;
/// a blank text has none.
pub fn sentences(text: &str) -> Vec<Range<usize>> {
    let characters: Vec<(usize, char)> = text.char_indices().collect();
    let at = |index: usize| characters.get(index).map_or(text.len(), |&(at, _)| at);
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < characters.len() {
        if !is_terminator(characters[index].1) {
            index += 1;
            continue;
        }
        let mut after = index;
        while characters
            .get(after)
            .is_some_and(|&(_, c)| is_terminator(c))
        {
            after += 1;
        }
        while characters.get(after).is_some_and(|&(_, c)| is_closing(c)) {
            after += 1;
        }
        let mut next = after;
        while characters
            .get(next)
            .is_some_and(|&(_, c)| c.is_whitespace())
        {
            next += 1;
        }
        let ends = match characters.get(next) {
            None => true,
            Some(&(_, following)) => next > after && !following.is_lowercase(),
        };
        if ends {
            let end = at(after);
            sentences.extend(trimmed(text, start..end));
            start = end;
        }
        index = after;
    }
    sentences.extend(trimmed(text, start..text.len()));
    sentences
}

/// The byte range of the sentence of `text` that holds `word`, a range of a
/// word of it, without the whitespace around it. The whole text, trimmed,
/// when no sentence holds the word.
pub fn around(text: &str, word: Range<usize>) -> Range<usize> {
    sentences(text)
        .into_iter()
        .find(|sentence| sentence.start <= word.start && word.end <= sentence.end)
        .or_else(|| trimmed(text, 0..text.len()))
        .unwrap_or(0..text.len())
}

/// The sentence a selection lies in, cut out of the text read around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    /// The sentence, without the whitespace around it; for the first sentence
    /// of a comment line also without the `//` marker and the whitespace
    /// after it.
    pub text: String,
    /// The selected characters, as a byte range of `text`.
    pub word: Range<usize>,
}

/// Whether `character` ends a sentence of a selection's text when the rest
/// allows it.
fn is_stop(character: char) -> bool {
    matches!(
        character,
        '.' | '!'
            | '?'
            | '\u{2026}'
            | '\u{203c}'
            | '\u{2047}'
            | '\u{2048}'
            | '\u{2049}'
            | '\u{3002}'
            | '\u{ff01}'
            | '\u{ff1f}'
    )
}

/// Whether `character` may close a sentence right after its stop.
fn is_closer(character: char) -> bool {
    matches!(
        character,
        '"' | '\'' | '\u{201d}' | '\u{2019}' | '\u{bb}' | '\u{203a}' | ')' | ']' | '}'
    )
}

/// Whether `character` is a line break.
fn is_line_break(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// The breaks of `characters` as `(end of a sentence, start of the next)`,
/// both character indexes. A line break always ends a sentence; a run of
/// stops with the closers after it ends one when whitespace follows, and then
/// the end or a character that is not a lowercase letter, or a line break.
fn breaks(characters: &[char]) -> Vec<(usize, usize)> {
    let count = characters.len();
    let mut found = Vec::new();
    let mut index = 0;
    while index < count {
        let character = characters[index];
        if is_line_break(character) {
            let mut next = index;
            while next < count && characters[next].is_whitespace() {
                next += 1;
            }
            found.push((index, next));
            index = next;
        } else if is_stop(character) {
            let mut end = index + 1;
            while end < count && is_stop(characters[end]) {
                end += 1;
            }
            while end < count && is_closer(characters[end]) {
                end += 1;
            }
            let mut next = end;
            let mut line_break = false;
            while next < count && characters[next].is_whitespace() {
                line_break |= is_line_break(characters[next]);
                next += 1;
            }
            if next > end && (next == count || line_break || !characters[next].is_lowercase()) {
                found.push((end, next));
            }
            index = next.max(index + 1);
        } else {
            index += 1;
        }
    }
    found
}

/// The index of the first non-whitespace character of `start..end`, or `end`.
fn content_start(characters: &[char], start: usize, end: usize) -> usize {
    characters[start..end]
        .iter()
        .position(|character| !character.is_whitespace())
        .map_or(end, |index| start + index)
}

/// Whether `start..end` holds a `//` anywhere: a whole-line comment, or one
/// trailing after code. A `//` inside a string counts too, which only ever
/// keeps a sentence out.
fn has_comment(characters: &[char], start: usize, end: usize) -> bool {
    characters[start..end]
        .windows(2)
        .any(|pair| pair == ['/', '/'])
}

/// Whether `start..end`, trailing whitespace and closers aside, ends with a
/// stop.
fn ends_with_stop(characters: &[char], start: usize, end: usize) -> bool {
    let mut last = end;
    while last > start && characters[last - 1].is_whitespace() {
        last -= 1;
    }
    while last > start && is_closer(characters[last - 1]) {
        last -= 1;
    }
    last > start && is_stop(characters[last - 1])
}

/// Whether a sentence starts with the comment text at `text` of the line that
/// starts at `line_start`. The line before it must lie whole in
/// `characters`, a line break before it too: a line the text cuts, or none
/// at all, could be a comment and so does not count. Then either it holds
/// no `//`, so no comment there can go on into this line; or it is a
/// whole-line `//` comment that ends with a stop and the text here does not
/// start with a lowercase letter, so the sentence above has ended.
fn sentence_starts_here(characters: &[char], line_start: usize, text: usize) -> bool {
    let Some(mut previous_end) = line_start.checked_sub(1) else {
        return false;
    };
    // `\r\n` is one break, not an empty line between two.
    if characters[previous_end] == '\n' && previous_end > 0 && characters[previous_end - 1] == '\r'
    {
        previous_end -= 1;
    }
    let Some(previous_start) = characters[..previous_end]
        .iter()
        .rposition(|&character| is_line_break(character))
        .map(|index| index + 1)
    else {
        return false;
    };
    if !has_comment(characters, previous_start, previous_end) {
        return true;
    }
    is_comment_line(characters, previous_start, previous_end)
        && ends_with_stop(characters, previous_start, previous_end)
        && characters
            .get(text)
            .is_some_and(|first| !first.is_lowercase())
}

/// Whether the content of `start..end` begins with the `//` of a comment.
fn is_comment_line(characters: &[char], start: usize, end: usize) -> bool {
    characters[content_start(characters, start, end)..end].starts_with(&['/', '/'])
}

/// Where the text of the trimmed sentence `from..to` of `characters` starts
/// when it lies in the area where the sentence of an editor is known to be
/// right, `None` otherwise. The area is a sentence of a whole-line `//`
/// comment that ends in its line with a stop: a later sentence of the line,
/// or the first one when a sentence starts on this line
/// ([`sentence_starts_here`]). The first sentence's text starts after the
/// `/` marker and the whitespace that follows it.
fn in_area(characters: &[char], from: usize, to: usize) -> Option<usize> {
    let line_start = characters[..from]
        .iter()
        .rposition(|&character| is_line_break(character))
        .map_or(0, |index| index + 1);
    let line_end = characters[from..]
        .iter()
        .position(|&character| is_line_break(character))
        .map_or(characters.len(), |index| from + index);
    let content = content_start(characters, line_start, line_end);
    let is_comment = characters[content..line_end].starts_with(&['/', '/']);
    if !(is_comment && to <= line_end && ends_with_stop(characters, from, to)) {
        return None;
    }
    if from > content {
        return Some(from);
    }
    let mut text = from;
    while text < to && characters[text] == '/' {
        text += 1;
    }
    let text = content_start(characters, text, to);
    (text < to && sentence_starts_here(characters, line_start, text)).then_some(text)
}

/// The sentence of `wider` around the selected characters `start..end`,
/// which are character offsets of `wider`, as an accessibility bus gives
/// them and never byte indexes; `None` when the range is empty or lies
/// outside the text, or when no sentence is left, as for a blank text.
///
/// A sentence ends as in [`sentences`] and also at every line break: the
/// text of an editor is read line by line, so a sentence never runs over one.
/// The word is the selection as far as it lies in the sentence, in bytes of
/// the sentence; a selection that has only whitespace there gives `None`.
///
/// Only a sentence of the confirmed area is given, anything else is `None`:
/// its line, the run of `wider` between line breaks, starts with `//` after
/// its leading whitespace; the sentence starts after a sentence break inside
/// that line, or it is the first sentence of the line and a sentence starts
/// on this line: the line before it is whole in `wider` and holds no `//`,
/// or is a whole-line `//` comment that ends with a stop while this sentence
/// does not start with a lowercase letter; and it ends in the line with a
/// stop, closers after it allowed. The first sentence's text leaves out the
/// `//` marker and the whitespace after it. A string literal, a trailing
/// comment after code and a sentence wrapped by hand after a line without a
/// stop, or onto a line that starts lowercase, are thus outside the area. A
/// wrap right after a stop, as in `Mr.` then `Smith`, is not told apart from
/// two sentences.
pub fn of_selection(wider: &str, start: usize, end: usize) -> Option<Selected> {
    let characters: Vec<char> = wider.chars().collect();
    let count = characters.len();
    if start >= end || end > count {
        return None;
    }
    let breaks = breaks(&characters);
    let before = breaks
        .iter()
        .filter(|&&(_, next)| next <= start)
        .map(|&(_, next)| next)
        .max();
    let after = breaks
        .iter()
        .filter(|&&(stop, _)| stop >= end)
        .map(|&(stop, _)| stop)
        .min();
    let mut from = before.unwrap_or(0);
    let mut to = after.unwrap_or(count);
    while from < to && characters[from].is_whitespace() {
        from += 1;
    }
    while to > from && characters[to - 1].is_whitespace() {
        to -= 1;
    }
    let from = in_area(&characters, from, to)?;
    let (word_from, word_to) = (start.max(from), end.min(to));
    if word_from >= word_to {
        return None;
    }
    let bytes = |upto: usize| -> usize {
        characters[from..upto]
            .iter()
            .map(|character| character.len_utf8())
            .sum()
    };
    Some(Selected {
        text: characters[from..to].iter().collect(),
        word: bytes(word_from)..bytes(word_to),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Selected, around, lookup_word, of_selection, pasted, recognized, sentences, words,
    };

    /// The words of `sentence` as texts.
    fn texts(sentence: &str) -> Vec<&str> {
        words(sentence)
            .into_iter()
            .map(|range| &sentence[range])
            .collect()
    }

    #[test]
    fn a_sentence_gives_its_words_without_the_full_stop() {
        assert_eq!(
            texts("I sat on the bank."),
            ["I", "sat", "on", "the", "bank"]
        );
    }

    #[test]
    fn ranges_are_in_order_and_point_into_the_sentence() {
        let sentence = "I sat on the bank.";
        assert_eq!(words(sentence), [0..1, 2..5, 6..8, 9..12, 13..17]);
    }

    #[test]
    fn empty_or_blank_text_has_no_words() {
        // The last two are Unicode whitespace: no-break and ideographic space.
        for sentence in ["", " ", "\t\n", "\u{a0}", "\u{3000}"] {
            assert!(words(sentence).is_empty(), "{sentence:?}");
        }
    }

    #[test]
    fn text_without_letters_has_no_words() {
        for sentence in [".", "… — !", "12 345", "(?)"] {
            assert!(words(sentence).is_empty(), "{sentence:?}");
        }
    }

    #[test]
    fn non_ascii_letters_are_letters() {
        assert_eq!(texts("Café über naïve."), ["Café", "über", "naïve"]);
        assert_eq!(texts("Я сидел на берегу."), ["Я", "сидел", "на", "берегу"]);
    }

    #[test]
    fn adjacent_punctuation_is_not_part_of_a_word() {
        for (sentence, expected) in [
            ("(bank)", vec!["bank"]),
            ("«bank», she said", vec!["bank", "she", "said"]),
            ("bank,river;lake", vec!["bank", "river", "lake"]),
            ("...bank", vec!["bank"]),
            ("bank?!", vec!["bank"]),
            ("room 101b", vec!["room", "b"]),
        ] {
            assert_eq!(texts(sentence), expected, "{sentence:?}");
        }
    }

    #[test]
    fn a_word_at_either_end_is_kept() {
        assert_eq!(texts("bank"), ["bank"]);
        assert_eq!(texts("  the bank"), ["the", "bank"]);
    }

    #[test]
    fn ranges_lie_on_character_boundaries() {
        let sentence = "«Café» — über…";
        for range in words(sentence) {
            assert!(sentence.is_char_boundary(range.start), "{range:?}");
            assert!(sentence.is_char_boundary(range.end), "{range:?}");
            assert!(!range.is_empty(), "{range:?}");
        }
    }

    #[test]
    fn an_apostrophe_between_two_letters_is_part_of_the_word() {
        assert_eq!(texts("I don't know."), ["I", "don't", "know"]);
        assert_eq!(texts("I don\u{2019}t know."), ["I", "don\u{2019}t", "know"]);
        assert_eq!(texts("o'clock, rock'n'roll"), ["o'clock", "rock'n'roll"]);
    }

    #[test]
    fn quotation_marks_are_not_apostrophes() {
        assert_eq!(
            texts("'bank' rock 'n' roll, the boys' bank"),
            ["bank", "rock", "n", "roll", "the", "boys", "bank"]
        );
        assert_eq!(
            texts("\u{2018}bank\u{2019} the boys\u{2019} bank\u{2019}"),
            ["bank", "the", "boys", "bank"]
        );
    }

    #[test]
    fn an_apostrophe_joins_only_letters() {
        for (sentence, expected) in [
            ("don''t", vec!["don", "t"]),
            ("don'", vec!["don"]),
            // A combining mark is not a letter, alphabetic or not.
            ("a'\u{308}", vec!["a"]),
            ("a'\u{345}", vec!["a"]),
            ("a'\u{363}", vec!["a"]),
            ("a'\u{363}b", vec!["a", "b"]),
            ("'t", vec!["t"]),
            ("'", vec![]),
            ("the '90s", vec!["the", "s"]),
            ("a'1", vec!["a"]),
            ("1'a", vec!["a"]),
            ("it's-a", vec!["it's", "a"]),
        ] {
            assert_eq!(texts(sentence), expected, "{sentence:?}");
        }
    }

    #[test]
    fn a_middle_dot_between_two_letters_is_part_of_the_word() {
        assert_eq!(texts("La col·lecció d'art"), ["La", "col·lecció", "d'art"]);
        assert_eq!(
            texts("intel·ligent, paral·lel."),
            ["intel·ligent", "paral·lel"]
        );
    }

    #[test]
    fn a_middle_dot_not_between_letters_separates() {
        for (sentence, expected) in [
            ("col··lecció", vec!["col", "lecció"]),
            ("col·", vec!["col"]),
            ("·lecció", vec!["lecció"]),
            ("a · b", vec!["a", "b"]),
            ("a·1", vec!["a"]),
            ("1·a", vec!["a"]),
            ("a·\u{308}", vec!["a"]),
            ("·", vec![]),
        ] {
            assert_eq!(texts(sentence), expected, "{sentence:?}");
        }
    }

    #[test]
    fn ranges_of_words_with_a_middle_dot_lie_on_character_boundaries() {
        let sentence = "La col·lecció·";
        assert_eq!(words(sentence), [0..2, 3..15]);
        for range in words(sentence) {
            assert!(sentence.is_char_boundary(range.start), "{range:?}");
            assert!(sentence.is_char_boundary(range.end), "{range:?}");
        }
    }

    #[test]
    fn a_combining_mark_after_a_letter_stays_in_the_word() {
        assert_eq!(texts("nai\u{308}ve"), ["nai\u{308}ve"]);
        // At the end of a word, twice on one letter, and before an apostrophe.
        assert_eq!(texts("cafe\u{301}."), ["cafe\u{301}"]);
        assert_eq!(texts("a\u{300}\u{36f}b c"), ["a\u{300}\u{36f}b", "c"]);
        assert_eq!(texts("e\u{301}'a"), ["e\u{301}'a"]);
    }

    #[test]
    fn a_combining_mark_without_a_letter_is_not_a_word() {
        assert!(words("\u{308}").is_empty());
        assert!(words("1\u{308} \u{301}").is_empty());
        // Marks that Unicode calls alphabetic are marks all the same.
        for text in [
            "\u{345}",
            "\u{363}",
            "\u{36f}",
            "1\u{363}",
            "12 \u{345}\u{363}",
        ] {
            assert!(words(text).is_empty(), "{text:?}");
        }
        assert_eq!(texts("\u{345}ve"), ["ve"]);
        assert_eq!(
            texts("1\u{363}ve a\u{345}\u{363}b"),
            ["ve", "a\u{345}\u{363}b"]
        );
        assert_eq!(texts("\u{308}ve"), ["ve"]);
        assert_eq!(texts("1\u{308}ve"), ["ve"]);
    }

    #[test]
    fn a_hyphen_separates_words() {
        assert_eq!(
            texts("a well-known bank - here"),
            ["a", "well", "known", "bank", "here"]
        );
        assert_eq!(
            texts("well\u{2010}known \u{2014}so"),
            ["well", "known", "so"]
        );
    }

    #[test]
    fn ranges_of_joined_words_lie_on_character_boundaries() {
        for sentence in [
            "I don\u{2019}t know.",
            "\u{2018}nai\u{308}ve\u{2019} \u{2014} cafe\u{301}",
            "\u{2019}\u{308}é\u{2019}",
            "é\u{2019}",
        ] {
            let mut end = 0;
            for range in words(sentence) {
                assert!(sentence.is_char_boundary(range.start), "{sentence:?}");
                assert!(sentence.is_char_boundary(range.end), "{sentence:?}");
                assert!(!range.is_empty(), "{sentence:?}");
                assert!(range.start >= end, "{sentence:?}");
                end = range.end;
            }
        }
    }

    #[test]
    fn the_looked_up_word_has_the_plain_apostrophe() {
        assert_eq!(lookup_word("don\u{2019}t"), "don't");
        assert_eq!(lookup_word("rock\u{2019}n\u{2019}roll"), "rock'n'roll");
        assert_eq!(lookup_word("don\u{2018}t"), "don't");
        // Everything else is kept: case, marks and the plain apostrophe.
        for word in ["don't", "Bank", "nai\u{308}ve", "Café"] {
            assert_eq!(lookup_word(word), word);
        }
    }

    #[test]
    fn a_pasted_text_is_one_line_with_single_spaces() {
        for (text, expected) in [
            ("The charge\n was  dropped.", "The charge was dropped."),
            ("  bank \n", "bank"),
            ("a\r\nb\tc\u{a0}d\u{3000}e", "a b c d e"),
            ("I don\u{2019}t know.", "I don\u{2019}t know."),
            ("12 \u{2026}", "12 \u{2026}"),
        ] {
            assert_eq!(pasted(text).as_deref(), Some(expected), "{text:?}");
        }
    }

    #[test]
    fn an_empty_or_blank_paste_is_nothing() {
        for text in ["", " ", "\n", "\t\r\n ", "\u{a0}", "\u{3000}"] {
            assert_eq!(pasted(text), None, "{text:?}");
        }
    }

    /// The sentences of `text` as texts.
    fn sentence_texts(text: &str) -> Vec<&str> {
        sentences(text)
            .into_iter()
            .map(|range| &text[range])
            .collect()
    }

    /// The sentence of `text` around the first word that reads `word`.
    fn sentence_of<'a>(text: &'a str, word: &str) -> &'a str {
        let range = words(text)
            .into_iter()
            .find(|range| &text[range.clone()] == word)
            .expect("word");
        &text[around(text, range)]
    }

    #[test]
    fn recognized_lines_are_joined_with_one_space() {
        for (raw, expected) in [
            ("the cat\nsat  down\n", "the cat sat down"),
            ("  one line  ", "one line"),
            ("a\r\nb", "a b"),
            ("last line\n\u{c}", "last line"),
        ] {
            assert_eq!(recognized(raw).as_deref(), Some(expected), "{raw:?}");
        }
    }

    #[test]
    fn a_hyphenated_line_break_is_joined_without_the_hyphen() {
        assert_eq!(
            recognized("the num-\nber of").as_deref(),
            Some("the number of")
        );
        assert_eq!(
            recognized("always propor-\ntional to n").as_deref(),
            Some("always proportional to n")
        );
    }

    #[test]
    fn a_hyphen_before_an_uppercase_letter_or_after_a_digit_stays() {
        for (raw, expected) in [
            ("GTK-\nRust", "GTK- Rust"),
            ("x28-\n43", "x28- 43"),
            ("a -\nb", "a - b"),
            ("well-\n", "well-"),
        ] {
            assert_eq!(recognized(raw).as_deref(), Some(expected), "{raw:?}");
        }
    }

    #[test]
    fn an_empty_line_inside_is_a_line_break() {
        assert_eq!(
            recognized("Firebase\n\nStorage.").as_deref(),
            Some("Firebase Storage.")
        );
        assert_eq!(recognized("num-\n \nber").as_deref(), Some("number"));
    }

    #[test]
    fn an_empty_or_blank_recognized_text_is_nothing() {
        for raw in ["", "\n", " \n\t\n", "\u{c}"] {
            assert_eq!(recognized(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn sentences_end_after_their_punctuation_and_a_capital() {
        assert_eq!(
            sentence_texts("I sat. You stood! Did he? Then\u{2026} It ended."),
            [
                "I sat.",
                "You stood!",
                "Did he?",
                "Then\u{2026}",
                "It ended."
            ]
        );
        assert_eq!(sentence_texts("  No end at all  "), ["No end at all"]);
        assert!(sentences("").is_empty());
        assert!(sentences(" \n").is_empty());
    }

    #[test]
    fn a_closing_quote_stays_with_its_sentence() {
        assert_eq!(
            sentence_texts("\"Find your voice.\u{201d} Smaller line"),
            ["\"Find your voice.\u{201d}", "Smaller line"]
        );
        assert_eq!(
            sentence_texts("(It ended.) Then \"no!\" She left."),
            ["(It ended.)", "Then \"no!\"", "She left."]
        );
    }

    #[test]
    fn a_lowercase_letter_after_a_period_does_not_end_the_sentence() {
        for text in [
            "Open Claude.ai Click here",
            "\"A little practice.\" followed by more",
            "Use e.g. the bank and so on.",
            "Pi is 3.14 or so.",
        ] {
            assert_eq!(sentence_texts(text), [text], "{text:?}");
        }
        // No list of abbreviations.
        assert_eq!(sentence_texts("Mr. Smith"), ["Mr.", "Smith"]);
    }

    #[test]
    fn the_sentence_around_a_word_is_the_one_that_holds_it() {
        let text = "I sat. You stood by the bank.";
        assert_eq!(sentence_of(text, "sat"), "I sat.");
        assert_eq!(sentence_of(text, "bank"), "You stood by the bank.");
        assert_eq!(sentence_of("  just words  ", "words"), "just words");
    }

    /// The text of 21-27-22 of EXP-8 at 2×, psm 6, as Tesseract gave it.
    const PITFALL: &str = "When we talk about problem size, we have to be careful about which size, or sizes, we\nare talking about. This example demonstrates a pitfall of algorithm analysis: the\ntempting shortcut of counting loops. If there is one loop, the algorithm is often linear.\nIf there are two loops (one nested inside the other), the algorithm is often quadratic.\nBut be careful! You have to think about how many times each loop runs. If the num-\nber of iterations is proportional to n for all loops, you can get away with just counting\nthe loops. But if, as in this example, the number of iterations is not always propor-\ntional to n, you have to give it more thought.";

    /// The text of 21-27-02 of EXP-8 at 2×, psm 6, as Tesseract gave it.
    const LINEAR: &str = "If we get lucky, we might find the target object right away and return after testing\nonly one element. If we are unlucky, we might have to test all of the elements. On\naverage, we expect to test half of the elements, so this method is considered linear\n(except in the unlikely case that we know the target element is at the beginning of the\narray).";

    #[test]
    fn a_recognized_text_of_exp_8_gives_the_sentence_of_its_word() {
        let text = recognized(PITFALL).expect("text");
        assert!(text.contains("the number of iterations"));
        assert!(text.contains("always proportional to n"));
        assert_eq!(
            sentence_of(&text, "iterations"),
            "If the number of iterations is proportional to n for all loops, you can get away with just counting the loops."
        );
        let text = recognized(LINEAR).expect("text");
        assert_eq!(
            sentence_of(&text, "unlikely"),
            "On average, we expect to test half of the elements, so this method is considered linear (except in the unlikely case that we know the target element is at the beginning of the array)."
        );
    }

    /// Character offsets of the `nth` occurrence of `needle` in `text`.
    fn chars_of(text: &str, needle: &str, nth: usize) -> (usize, usize) {
        let byte = text.match_indices(needle).nth(nth).expect("needle").0;
        let start = text[..byte].chars().count();
        (start, start + needle.chars().count())
    }

    fn selected(text: &str, needle: &str, nth: usize) -> Option<Selected> {
        let (start, end) = chars_of(text, needle, nth);
        of_selection(text, start, end)
    }

    #[test]
    fn a_selection_gives_its_sentence_and_the_place_of_the_word_in_it() {
        let text = "// The bank was closed. We sat on the bank of the river.";
        let found = selected(text, "bank", 1).expect("sentence");
        assert_eq!(found.text, "We sat on the bank of the river.");
        assert_eq!(&found.text[found.word.clone()], "bank");
        // The second occurrence, not the first.
        assert_eq!(found.word, 14..18);
        // The first sentence of the line, with no line before it to show
        // that the comment starts here.
        assert_eq!(selected(text, "bank", 0), None);
    }

    #[test]
    fn a_line_break_ends_a_sentence() {
        let text = "a.\n// Lead. We sat on the bank.\nnext.";
        let found = selected(text, "bank", 0).expect("sentence");
        assert_eq!(found.text, "We sat on the bank.");
        assert_eq!(&found.text[found.word], "bank");
        // Without a stop before the break the sentence is not known to end.
        let text = "// Lead. We sat on the bank\n// next one.";
        assert_eq!(selected(text, "bank", 0), None);
        let text = "one two\nthree four\nfive";
        assert_eq!(selected(text, "four", 0), None);
    }

    #[test]
    fn offsets_are_characters_and_not_bytes() {
        let text = "// \u{1f600} Sit. \u{1f600}\u{1f600} We sat on the b\u{e4}nk, \u{1f600} here.";
        let found = selected(text, "b\u{e4}nk", 0).expect("sentence");
        assert_eq!(
            found.text,
            "\u{1f600}\u{1f600} We sat on the b\u{e4}nk, \u{1f600} here."
        );
        assert_eq!(&found.text[found.word], "b\u{e4}nk");
        let found = selected(text, "here", 0).expect("sentence");
        assert_eq!(&found.text[found.word], "here");
    }

    #[test]
    fn a_range_outside_the_text_or_empty_is_nothing() {
        let text = "// Lead. We sat on the bank.";
        let count = text.chars().count();
        let (start, end) = chars_of(text, "bank", 0);
        assert!(of_selection(text, start, end).is_some());
        assert_eq!(of_selection(text, start, count + 1), None);
        assert_eq!(of_selection(text, count + 2, count + 4), None);
        assert_eq!(of_selection(text, start, start), None);
        assert_eq!(of_selection(text, end, start), None);
        assert_eq!(of_selection("", 0, 0), None);
    }

    #[test]
    fn a_blank_text_or_selection_has_no_sentence() {
        assert_eq!(of_selection("  \n ", 0, 2), None);
        // Whitespace only of the sentence is selected.
        let text = "// Lead. We sat.   ";
        let (start, _) = chars_of(text, "   ", 0);
        assert_eq!(of_selection(text, start, start + 3), None);
    }

    #[test]
    fn a_selection_with_whitespace_at_its_ends_keeps_the_word_inside() {
        let text = "// Lead. We sat on the bank.  ";
        let (start, end) = chars_of(text, "bank.  ", 0);
        let found = of_selection(text, start, end).expect("sentence");
        assert_eq!(found.text, "We sat on the bank.");
        assert_eq!(&found.text[found.word], "bank.");
    }

    #[test]
    fn a_text_without_a_break_is_one_sentence_outside_the_area() {
        // Not a comment and without a stop; then a comment with no line
        // before it to show that it starts there.
        let text = "  the bank of the river  ";
        assert_eq!(selected(text, "bank", 0), None);
        let text = "  // the bank of the river.  ";
        assert_eq!(selected(text, "bank", 0), None);
    }

    #[test]
    fn a_closer_after_the_stop_ends_the_sentence() {
        let text = "// Lead. He said \"we sat on the bank.\"";
        let found = selected(text, "bank", 0).expect("sentence");
        assert_eq!(found.text, "He said \"we sat on the bank.\"");
    }

    #[test]
    fn a_lowercase_letter_after_a_stop_keeps_the_sentence() {
        let text = "// Lead. Use e.g. the bank here. Next";
        let found = selected(text, "bank", 0).expect("sentence");
        assert_eq!(found.text, "Use e.g. the bank here.");
    }

    /// `experiment-exp-2/context-fixture.kt`, copied verbatim.
    const FIXTURE: &str = "// The bank was closed for the day. We could not charge the card.
// Check the value. It\u{2019}s a \u{201c}mean\u{201d} value \u{2014} not the median \u{1f642} \u{2014} and that is the issue.
val message = \"The charge will appear on your next statement. Keep the receipt.\"
// This sentence is long enough that it has been wrapped by hand onto
// a second line of the comment before it ends. Next one.
";

    fn fixture_sentence(needle: &str, nth: usize) -> Option<String> {
        selected(FIXTURE, needle, nth).map(|found| {
            assert_eq!(&found.text[found.word.clone()], needle);
            found.text
        })
    }

    #[test]
    fn the_fixture_gives_a_sentence_only_in_the_confirmed_area() {
        // J2: the second sentence of a comment line.
        assert_eq!(
            fixture_sentence("charge", 0).as_deref(),
            Some("We could not charge the card.")
        );
        // The first sentence of the first line, with no line before it.
        assert_eq!(fixture_sentence("bank", 0), None);
        // J3: after and before a character outside the BMP.
        let j3 = "It\u{2019}s a \u{201c}mean\u{201d} value \u{2014} not the median \u{1f642} \u{2014} and that is the issue.";
        assert_eq!(fixture_sentence("issue", 0).as_deref(), Some(j3));
        assert_eq!(fixture_sentence("mean", 0).as_deref(), Some(j3));
        // J4: a string literal.
        assert_eq!(fixture_sentence("charge", 1), None);
        assert_eq!(fixture_sentence("receipt", 0), None);
        // J5: a comment wrapped by hand.
        assert_eq!(fixture_sentence("wrapped", 0), None);
        assert_eq!(fixture_sentence("ends", 0), None);
        // A sentence that ends in its line after the wrap.
        assert_eq!(fixture_sentence("Next", 0).as_deref(), Some("Next one."));
    }

    #[test]
    fn the_first_sentence_of_a_comment_that_starts_on_its_line_is_given() {
        // Read from Android Studio on 2026-10-15.
        let text = "    val sync: SettingsSyncRepository = koinInject()\n    val source: TwisterAudioSource = koinInject()\n    // Created in this dialog's composition, never shared with an exercise player's listeners.\n    val player = koinInject()";
        let found = selected(text, "dialog", 0).expect("sentence");
        assert_eq!(
            found.text,
            "Created in this dialog's composition, never shared with an exercise player's listeners."
        );
        assert_eq!(&found.text[found.word], "dialog");
        // The first of two sentences, and the second as before.
        let text = "}\nx\n// First one. Second one.";
        let found = selected(text, "First", 0).expect("sentence");
        assert_eq!(found.text, "First one.");
        assert_eq!(found.word, 0..5);
        assert_eq!(
            selected(text, "Second", 0).expect("sentence").text,
            "Second one."
        );
        // A blank line before, and `\r\n` breaks.
        let found = selected("a\n\n// Blank above.\n", "above", 0).expect("sentence");
        assert_eq!(found.text, "Blank above.");
        let found = selected("}\r\nx\r\n// One here.\r\nnext", "here", 0).expect("sentence");
        assert_eq!(found.text, "One here.");
        // Any run of `/`, with or without a space after it.
        let found = selected("}\nx\n/// Docs here.", "Docs", 0).expect("sentence");
        assert_eq!(found.text, "Docs here.");
        let found = selected("}\nx\n//No space here.", "No", 0).expect("sentence");
        assert_eq!(found.text, "No space here.");
    }

    #[test]
    fn the_first_sentence_is_not_given_when_the_comment_may_start_earlier() {
        // Wrapped by hand: the comment line before ends without a stop, and
        // this line starts lowercase.
        let text = "}\nx\n// The player is released when the dialog\n// leaves the composition.\n";
        assert_eq!(selected(text, "leaves", 0), None);
        // Its first line does not end with a stop.
        assert_eq!(selected(text, "dialog", 0), None);
        let text = "}\r\nx\r\n// A wrapped\r\n// line.";
        assert_eq!(selected(text, "line", 0), None);
        // No line before, or one the window cuts, may be a comment.
        assert_eq!(selected("// One sentence here.\n", "One", 0), None);
        assert_eq!(selected("ment tail\n// One here.", "One", 0), None);
        // Only the marker is selected.
        assert_eq!(selected("}\nx\n// One here.", "//", 0), None);
        // A trailing comment wrapped by hand onto the next line, and a `//`
        // in a string above.
        let text = "}\nval x = 1 // a trailing note that\n// continues here.";
        assert_eq!(selected(text, "continues", 0), None);
        let text = "}\nval u = \"http://x\"\n// One here.";
        assert_eq!(selected(text, "One", 0), None);
        // A trailing comment after code is not a whole-line comment.
        assert_eq!(selected("}\nx\nf() // One here.", "One", 0), None);
    }

    #[test]
    fn a_later_comment_line_after_a_finished_sentence_gives_its_first_sentence() {
        // One sentence per line, from the file read on 2026-10-15.
        let text = "}\nx\n// The row carries the click and the role.\n// The button only draws.\n";
        let found = selected(text, "button", 0).expect("sentence");
        assert_eq!(found.text, "The button only draws.");
        assert_eq!(&found.text[found.word], "button");
        // The first line as before, and a third line after the second.
        assert_eq!(
            selected(text, "row", 0).expect("sentence").text,
            "The row carries the click and the role."
        );
        let text = "}\nx\n// One.\n// Two here.\n// Three here.";
        assert_eq!(
            selected(text, "Three", 0).expect("sentence").text,
            "Three here."
        );
        // A closer after the stop, whitespace after it, and `\r\n` breaks.
        let text = "}\nx\n    // He said \"done.\"  \n    // Then we left.";
        assert_eq!(
            selected(text, "left", 0).expect("sentence").text,
            "Then we left."
        );
        let text = "}\r\nx\r\n// One.\r\n// Two here.";
        assert_eq!(
            selected(text, "Two", 0).expect("sentence").text,
            "Two here."
        );
    }

    #[test]
    fn a_later_comment_line_continuing_a_sentence_gives_nothing() {
        // The line above ends without a stop: wrapped by hand.
        let text = "}\nx\n// A published twister recorded in every voice of its language; no\n// separate preview object exists.";
        assert_eq!(selected(text, "separate", 0), None);
        // A stop above, but this line starts lowercase: as after `e.g.`.
        let text = "}\nx\n// See e.g.\n// the docs here.";
        assert_eq!(selected(text, "docs", 0), None);
        // A trailing comment above, even one that ends with a stop.
        let text = "}\nval x = 1 // a note.\n// Next one.";
        assert_eq!(selected(text, "Next", 0), None);
        // An empty comment line above has no stop.
        let text = "}\nx\n// One.\n//\n// Two here.";
        assert_eq!(selected(text, "Two", 0), None);
        // A comment line above that the text cuts.
        assert_eq!(selected("tail.\n// Two here.", "Two", 0), None);
    }

    #[test]
    fn the_line_must_be_a_whole_line_comment_with_a_stop() {
        assert_eq!(selected("// A. We sat on the bank", "bank", 0), None);
        let found = selected("    // A. We sat on the bank.", "bank", 0).expect("sentence");
        assert_eq!(found.text, "We sat on the bank.");
        assert_eq!(selected("f() // A. We sat on the bank.", "bank", 0), None);
    }
}
