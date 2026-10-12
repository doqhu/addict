//! Splitting a sentence into the words the user can pick, and the two other
//! text rules of the manual path: the word as the dictionary writes it and a
//! pasted text as one line.
//!
//! A word is a maximal run of alphabetic characters. Two things continue a
//! word instead of ending it: an apostrophe, U+0027 or U+2019, that stands
//! between two letters, so `don't` is one word and the quotation marks of
//! `'bank'` or the mark of `boys'` stay outside; and a combining mark of
//! U+0300–U+036F after a letter, so a letter typed with its accent apart is
//! not cut. Everything else only separates words, a hyphen included:
//! `well-known` is two words. The ranges are byte ranges of the given text,
//! so the text itself is kept as it was, punctuation included.

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

/// `word` as it is looked up: the typographic apostrophe becomes U+0027, the
/// one the headwords are written with. Nothing else changes.
pub fn lookup_word(word: &str) -> String {
    word.replace(TYPOGRAPHIC_APOSTROPHE, "'")
}

/// A pasted `text` as the one line the field holds: without the whitespace
/// around it and with each run of whitespace inside, line breaks included, as
/// one space. `None` when the text is empty or blank.
pub fn pasted(text: &str) -> Option<String> {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!line.is_empty()).then_some(line)
}

#[cfg(test)]
mod tests {
    use super::{lookup_word, pasted, words};

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
}
