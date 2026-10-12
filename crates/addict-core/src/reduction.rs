//! Reducing a word to the forms a dictionary may list it under.
//!
//! There is no model and no part of speech: a word gives every form its
//! rules allow, and the caller keeps those that are headwords. The steps, in
//! the order a lookup takes them:
//!
//! 1. [`stripped`]: the word without the punctuation around it;
//! 2. [`lowercase`]: the forms are taken from the lowercase word;
//! 3. [`forms`]: the lemmas of the exception list, which the caller reads,
//!    sorted, then the [`suffix_forms`].
//!
//! Nothing here reads a file or keeps anything between calls.

/// A suffix and what replaces it, tried in this order; every rule that
/// matches a word longer than its suffix gives a form.
const SUFFIX_RULES: [(&str, &str); 23] = [
    // plural and third person
    ("ies", "y"),
    ("ches", "ch"),
    ("shes", "sh"),
    ("sses", "ss"),
    ("xes", "x"),
    ("zes", "z"),
    ("oes", "o"),
    ("es", "e"),
    ("es", ""),
    ("s", ""),
    ("men", "man"),
    // past
    ("ied", "y"),
    ("ed", "e"),
    ("ed", ""),
    // -ing
    ("ying", "ie"),
    ("ing", "e"),
    ("ing", ""),
    // comparison
    ("ier", "y"),
    ("iest", "y"),
    ("er", "e"),
    ("er", ""),
    ("est", "e"),
    ("est", ""),
];

/// Without one of these suffixes a doubled final consonant gives one more
/// form with a single consonant: `stopped`, `stopp`, `stop`.
const DOUBLING_SUFFIXES: [&str; 4] = ["ed", "ing", "er", "est"];

/// The fewest letters of a form the rules give.
const MIN_FORM: usize = 2;

const VOWELS: [char; 5] = ['a', 'e', 'i', 'o', 'u'];

/// `word` without the characters around it that are neither letters nor
/// digits; what is inside, such as `'` and `-`, is kept. Empty when the word
/// has no letter or digit.
pub fn stripped(word: &str) -> &str {
    word.trim_matches(|character: char| !character.is_alphanumeric())
}

/// `word` in lowercase.
pub fn lowercase(word: &str) -> String {
    word.to_lowercase()
}

/// The forms the suffix rules and the doubling rule give for `word`, in the
/// order of the rules: each at least two letters long, none equal to `word`,
/// none twice.
pub fn suffix_forms(word: &str) -> Vec<String> {
    let mut forms = Vec::new();
    for (suffix, replacement) in SUFFIX_RULES {
        if let Some(stem) = word.strip_suffix(suffix)
            && !stem.is_empty()
        {
            add(&mut forms, word, format!("{stem}{replacement}"));
        }
    }
    for suffix in DOUBLING_SUFFIXES {
        let Some(stem) = word.strip_suffix(suffix) else {
            continue;
        };
        let mut letters = stem.chars().rev();
        let (Some(last), Some(before)) = (letters.next(), letters.next()) else {
            continue;
        };
        // At least three letters: the two compared and one more.
        if letters.next().is_some() && last == before && !VOWELS.contains(&last) {
            add(
                &mut forms,
                word,
                stem[..stem.len() - last.len_utf8()].to_owned(),
            );
        }
    }
    forms
}

/// The dictionary forms to try for `word`: `lemmas`, the lemmas the
/// exception list gives for it, sorted, then the [`suffix_forms`]. Never
/// `word` itself and no form twice.
pub fn forms(word: &str, lemmas: &[String]) -> Vec<String> {
    let mut sorted: Vec<&String> = lemmas.iter().collect();
    sorted.sort();
    let mut forms: Vec<String> = Vec::new();
    for form in sorted.into_iter().cloned().chain(suffix_forms(word)) {
        if form != word && !forms.contains(&form) {
            forms.push(form);
        }
    }
    forms
}

/// Keeps `form` when the limits of the suffix rules allow it.
fn add(forms: &mut Vec<String>, word: &str, form: String) {
    if form.chars().count() >= MIN_FORM && form != word && !forms.contains(&form) {
        forms.push(form);
    }
}

#[cfg(test)]
mod tests {
    use super::{forms, lowercase, stripped, suffix_forms};

    fn texts(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn surrounding_punctuation_is_removed_and_inner_is_kept() {
        for (word, expected) in [
            ("bank", "bank"),
            ("bank.", "bank"),
            ("(bank)", "bank"),
            ("\"went,\"", "went"),
            ("«bank»…", "bank"),
            (" bank\n", "bank"),
            ("don't", "don't"),
            ("'don't!'", "don't"),
            ("well-known,", "well-known"),
            ("-ing", "ing"),
            ("Bank?", "Bank"),
            ("café.", "café"),
            ("(101b)", "101b"),
        ] {
            assert_eq!(stripped(word), expected, "{word:?}");
        }
    }

    #[test]
    fn nothing_is_left_of_a_word_without_letters() {
        for word in ["", " ", "...", "\u{a0}", "'", "-", "(?)", "… — !"] {
            assert_eq!(stripped(word), "", "{word:?}");
            assert!(suffix_forms(stripped(word)).is_empty(), "{word:?}");
            assert!(forms(stripped(word), &[]).is_empty(), "{word:?}");
        }
    }

    #[test]
    fn lowercase_changes_the_case_only() {
        for (word, expected) in [
            ("Bank", "bank"),
            ("BANKS", "banks"),
            ("The", "the"),
            ("English", "english"),
            ("bank", "bank"),
            ("don't", "don't"),
            ("Café", "café"),
        ] {
            assert_eq!(lowercase(word), expected, "{word:?}");
        }
    }

    #[test]
    fn every_suffix_rule_gives_its_form() {
        for (word, expected) in [
            // plural and third person
            ("flies", vec!["fly", "flie", "fli"]),
            ("churches", vec!["church", "churche"]),
            ("wishes", vec!["wish", "wishe"]),
            ("classes", vec!["class", "classe"]),
            ("boxes", vec!["box", "boxe"]),
            ("quizzes", vec!["quizz", "quizze"]),
            ("goes", vec!["go", "goe"]),
            ("makes", vec!["make", "mak"]),
            ("banks", vec!["bank"]),
            ("women", vec!["woman"]),
            // past
            ("tried", vec!["try", "trie", "tri"]),
            ("used", vec!["use", "us"]),
            ("walked", vec!["walke", "walk"]),
            // -ing
            ("lying", vec!["lie", "lye", "ly"]),
            ("making", vec!["make", "mak"]),
            ("walking", vec!["walke", "walk"]),
            // comparison
            ("happier", vec!["happy", "happie", "happi"]),
            ("happiest", vec!["happy", "happie", "happi"]),
            ("later", vec!["late", "lat"]),
            ("faster", vec!["faste", "fast"]),
            ("latest", vec!["late", "lat"]),
            ("fastest", vec!["faste", "fast"]),
        ] {
            assert_eq!(suffix_forms(word), expected, "{word}");
        }
    }

    #[test]
    fn a_doubled_consonant_gives_one_more_form_after_the_suffix_forms() {
        for (word, expected) in [
            ("stopped", vec!["stoppe", "stopp", "stop"]),
            ("running", vec!["runne", "runn", "run"]),
            ("bigger", vec!["bigge", "bigg", "big"]),
            ("biggest", vec!["bigge", "bigg", "big"]),
            ("better", vec!["bette", "bett", "bet"]),
            ("letter", vec!["lette", "lett", "let"]),
            ("added", vec!["adde", "add", "ad"]),
        ] {
            assert_eq!(suffix_forms(word), expected, "{word}");
        }
    }

    #[test]
    fn doubling_needs_three_letters_and_a_consonant() {
        // A doubled vowel is no doubling.
        assert_eq!(suffix_forms("seeing"), ["seee", "see"]);
        assert_eq!(suffix_forms("agreed"), ["agree", "agre"]);
        // Two letters before the suffix are too few.
        assert_eq!(suffix_forms("added"), ["adde", "add", "ad"]);
        assert_eq!(suffix_forms("ddd"), Vec::<String>::new());
        assert_eq!(suffix_forms("dded"), ["dde", "dd"]);
        // Only the last two letters are compared.
        assert_eq!(suffix_forms("walked"), ["walke", "walk"]);
    }

    #[test]
    fn a_word_no_longer_than_its_suffix_is_not_reduced_by_it() {
        for word in ["s", "es", "ed", "er", "ing", "est", "men"] {
            assert_eq!(suffix_forms(word), Vec::<String>::new(), "{word}");
        }
        // `ies` is not longer than `ies`, yet longer than `es` and `s`.
        assert_eq!(suffix_forms("ies"), ["ie"]);
    }

    #[test]
    fn a_form_shorter_than_two_letters_is_dropped() {
        for word in ["is", "us", "as"] {
            assert_eq!(suffix_forms(word), Vec::<String>::new(), "{word}");
        }
        // `re` stays, `r` does not.
        assert_eq!(suffix_forms("red"), ["re"]);
        assert_eq!(suffix_forms("ber"), ["be"]);
        // Letters are counted, not bytes.
        assert_eq!(suffix_forms("és"), Vec::<String>::new());
        assert_eq!(suffix_forms("éés"), ["éé"]);
    }

    #[test]
    fn a_word_without_a_matching_suffix_has_no_forms() {
        for word in [
            "bank", "go", "went", "the", "boolean", "xyzzy", "advice", "saw", "left", "",
        ] {
            assert_eq!(suffix_forms(word), Vec::<String>::new(), "{word:?}");
        }
    }

    #[test]
    fn a_form_is_never_the_word_and_is_given_once() {
        for word in [
            "flies", "churches", "boxes", "classes", "running", "stopped", "better", "happiest",
            "banks", "eses", "sss", "eded",
        ] {
            let found = suffix_forms(word);
            assert!(!found.iter().any(|form| form == word), "{word}: {found:?}");
            for (at, form) in found.iter().enumerate() {
                assert!(!found[..at].contains(form), "{word}: {found:?}");
                assert!(form.chars().count() >= 2, "{word}: {found:?}");
            }
        }
    }

    #[test]
    fn letters_outside_ascii_are_whole_letters() {
        assert_eq!(suffix_forms("naïves"), ["naïve", "naïv"]);
        // The doubled letter is one character of two bytes.
        assert_eq!(suffix_forms("añññed"), ["añññe", "añññ", "aññ"]);
        assert_eq!(suffix_forms("ééed"), ["éée", "éé"]);
    }

    #[test]
    fn exception_lemmas_come_first_and_sorted() {
        assert_eq!(forms("went", &texts(&["go"])), ["go"]);
        assert_eq!(
            forms("better", &texts(&["well", "good"])),
            ["good", "well", "bette", "bett", "bet"]
        );
        assert_eq!(forms("geese", &texts(&["goose"])), ["goose"]);
        assert_eq!(forms("is", &texts(&["be"])), ["be"]);
        assert_eq!(forms("banks", &[]), ["bank"]);
        assert_eq!(forms("bank", &[]), Vec::<String>::new());
    }

    #[test]
    fn the_forms_of_a_word_hold_neither_the_word_nor_a_repeat() {
        // The list names the word itself.
        assert_eq!(forms("lay", &texts(&["lay", "lie"])), ["lie"]);
        // A lemma the rules give as well, and a lemma listed twice.
        assert_eq!(forms("banks", &texts(&["bank"])), ["bank"]);
        assert_eq!(
            forms("flies", &texts(&["fly", "flie", "fly"])),
            ["flie", "fly", "fli"]
        );
    }
}
