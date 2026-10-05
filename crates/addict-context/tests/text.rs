//! The pure logic of the matrix: character offsets, whitespace, sentence
//! derivation, embedded objects, expectations.

use addict_context::text::{
    EMBEDDED_OBJECT, char_len, char_slice, clip, derive_sentence, embedded_object_count,
    matches_expectation, normalize_whitespace,
};

/// Character range of the first `needle` in `text`.
fn range(text: &str, needle: &str) -> (usize, usize) {
    let byte = text.find(needle).expect("needle is in the text");
    let start = text[..byte].chars().count();
    (start, start + needle.chars().count())
}

fn sentence(text: &str, needle: &str) -> String {
    let (start, end) = range(text, needle);
    derive_sentence(text, start, end).expect("a sentence").text
}

#[test]
fn slices_by_characters_not_bytes() {
    let text = "Он — «bank» 🙂 here";
    assert_eq!(char_len(text), 18);
    assert_eq!(char_slice(text, 6, 10), "bank");
    assert_eq!(char_slice(text, 12, 13), "🙂");
    assert_eq!(char_slice(text, 0, 2), "Он");
}

#[test]
fn slice_clamps_and_never_panics() {
    let text = "да 🙂";
    assert_eq!(char_slice(text, 3, 99), "🙂");
    assert_eq!(char_slice(text, 99, 120), "");
    assert_eq!(char_slice(text, 3, 1), "");
    assert_eq!(char_slice("", 0, 5), "");
    for start in 0..8 {
        for end in 0..8 {
            let _ = char_slice(text, start, end);
        }
    }
}

#[test]
fn clip_reports_truncation() {
    assert_eq!(clip("привет", 3), ("при".to_owned(), true));
    assert_eq!(clip("привет", 6), ("привет".to_owned(), false));
    assert_eq!(clip("привет", 60), ("привет".to_owned(), false));
}

#[test]
fn whitespace_is_normalised() {
    assert_eq!(normalize_whitespace("  a\u{00A0}b \n\t c  "), "a b c");
    assert_eq!(normalize_whitespace(""), "");
}

#[test]
fn expectation_ignores_only_whitespace() {
    assert!(matches_expectation(
        "The bank  was closed.",
        " The bank\nwas closed. "
    ));
    assert!(!matches_expectation(
        "the bank was closed.",
        "The bank was closed."
    ));
    assert!(!matches_expectation("It's", "It’s"));
}

#[test]
fn embedded_objects_are_counted() {
    let text = format!("a {EMBEDDED_OBJECT} b {EMBEDDED_OBJECT}");
    assert_eq!(embedded_object_count(&text), 2);
    assert_eq!(embedded_object_count("plain"), 0);
}

#[test]
fn sentence_in_the_middle() {
    let text = "First one. The bank was closed. Third one.";
    let (start, end) = range(text, "bank");
    let derived = derive_sentence(text, start, end).unwrap();
    assert_eq!(derived.text, "The bank was closed.");
    assert_eq!(char_slice(text, derived.start, derived.end), derived.text);
    assert!(!derived.open_start && !derived.open_end);
}

#[test]
fn sentence_at_the_edges_is_open() {
    let text = "The bank was closed. Second";
    let first = derive_sentence(text, 4, 8).unwrap();
    assert_eq!(first.text, "The bank was closed.");
    assert!(first.open_start && !first.open_end);
    let (start, end) = range(text, "Second");
    let last = derive_sentence(text, start, end).unwrap();
    assert_eq!(last.text, "Second");
    assert!(!last.open_start && last.open_end);
}

#[test]
fn question_exclamation_and_ellipsis_end_a_sentence() {
    let text = "Really?! The charge was high… Next issue! Done.";
    assert_eq!(sentence(text, "charge"), "The charge was high…");
    assert_eq!(sentence(text, "issue"), "Next issue!");
    assert_eq!(sentence(text, "Really"), "Really?!");
}

#[test]
fn closing_quotes_stay_with_their_sentence() {
    let text = "He said, “Go to the bank.” Then he left. «Что?» Да.";
    assert_eq!(sentence(text, "bank"), "He said, “Go to the bank.”");
    assert_eq!(sentence(text, "left"), "Then he left.");
    assert_eq!(sentence(text, "Что"), "«Что?»");
}

#[test]
fn lowercase_continuation_and_inner_dots_do_not_split() {
    let text = "Costs, e.g. the charge, rose by 3.5 percent. Next.";
    assert_eq!(
        sentence(text, "charge"),
        "Costs, e.g. the charge, rose by 3.5 percent."
    );
}

#[test]
fn line_break_ends_a_sentence() {
    let text = "A heading\nThe bank was closed\nAnother line";
    assert_eq!(sentence(text, "bank"), "The bank was closed");
    let text = "End of one.\n\n  The mean value. Tail";
    assert_eq!(sentence(text, "mean"), "The mean value.");
}

#[test]
fn non_ascii_around_the_selection() {
    let text = "Она сказала: «Иди». It’s a “bank” — not a bench 🙂 at all! Потом ушла.";
    let (start, end) = range(text, "bank");
    let derived = derive_sentence(text, start, end).unwrap();
    assert_eq!(derived.text, "It’s a “bank” — not a bench 🙂 at all!");
    assert_eq!(char_slice(text, derived.start, derived.end), derived.text);
    assert_eq!(sentence(text, "ушла"), "Потом ушла.");
}

#[test]
fn every_range_of_a_non_ascii_text_is_safe() {
    let text = "Я — «тут» 🙂. And “you”… нет?\n¡Sí! 👩‍👩‍👧 ok.";
    let n = char_len(text);
    for start in 0..=n + 2 {
        for end in 0..=n + 2 {
            if let Some(derived) = derive_sentence(text, start, end) {
                assert!(derived.start < derived.end && derived.end <= n);
                assert_eq!(char_slice(text, derived.start, derived.end), derived.text);
                assert!(!derived.text.starts_with(char::is_whitespace));
                assert!(!derived.text.ends_with(char::is_whitespace));
            }
        }
    }
}

#[test]
fn out_of_range_or_empty_gives_none() {
    assert!(derive_sentence("", 0, 0).is_none());
    assert!(derive_sentence("abc", 2, 9).is_none());
    assert!(derive_sentence("abc", 2, 1).is_none());
    assert!(derive_sentence("   \n  ", 1, 2).is_none());
}

#[test]
fn embedded_object_stays_in_the_sentence() {
    let text = format!("Before. A {EMBEDDED_OBJECT} word here. After.");
    let (start, end) = range(&text, "word");
    let derived = derive_sentence(&text, start, end).unwrap();
    assert_eq!(embedded_object_count(&derived.text), 1);
}
