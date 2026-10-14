//! Splitting a sentence into the words the user can pick, and the other text
//! rules of the input: the word as the dictionary writes it, a pasted text as
//! one line, a recognized text as one line and the sentence of a word in it.
//!
//! A word is a maximal run of alphabetic characters. Two things continue a
//! word instead of ending it: an apostrophe, U+0027 or U+2019, that stands
//! between two letters, so `don't` is one word and the quotation marks of
//! `'bank'` or the mark of `boys'` stay outside; and a combining mark of
//! U+0300–U+036F after a letter, so a letter typed with its accent apart is
//! not cut. Everything else only separates words, a hyphen included:
//! `well-known` is two words. The ranges are byte ranges of the given text,
//! so the text itself is kept as it was, punctuation included.
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
        } else if matches!(character, APOSTROPHE | TYPOGRAPHIC_APOSTROPHE) {
            // Inside a word, so after a letter or its mark: the apostrophe
            // joins when a letter follows as well.
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

#[cfg(test)]
mod tests {
    use super::{around, lookup_word, pasted, recognized, sentences, words};

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
}
