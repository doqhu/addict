//! Splitting a sentence into the words the user can pick.
//!
//! A word is a maximal run of alphabetic characters; everything else only
//! separates words. The ranges are byte ranges of the given text, so the text
//! itself is kept as it was, punctuation included.

use std::ops::Range;

/// The byte ranges of the words of `sentence`, in order. Each range lies on
/// character boundaries and is never empty.
pub fn words(sentence: &str) -> Vec<Range<usize>> {
    let mut words = Vec::new();
    let mut start = None;
    for (at, character) in sentence.char_indices() {
        match (character.is_alphabetic(), start) {
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

#[cfg(test)]
mod tests {
    use super::words;

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
}
