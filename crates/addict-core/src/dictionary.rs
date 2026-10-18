//! The dictionary contract: a word in, its [`Article`] out.
//!
//! [`Dictionary::lookup`] is synchronous: core has no runtime, and keeping a
//! lookup off the UI thread is the caller's duty. `Sample` is a dictionary of
//! a few words for tests, present only with the `sample` feature; its texts
//! were written for this project.

/// What a dictionary says about one word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Article {
    /// The senses of the word, in the order of the dictionary.
    pub senses: Vec<Sense>,
    /// Translations of the word into Russian, for the article as a whole.
    pub translations: Vec<String>,
}

/// One sense of a word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sense {
    /// The definition, in the language of the dictionary it comes from:
    /// English for an English index, Spanish for a Spanish one.
    pub definition: String,
    /// Words of that language close to this sense; may be empty.
    pub synonyms: Vec<String>,
}

/// A source of articles. It is shared with the thread that runs the lookups.
pub trait Dictionary: Send + Sync {
    /// The article of `word`, or `None` when the dictionary has no entry.
    fn lookup(&self, word: &str) -> Option<Article>;
}

/// One sense of the sample: its definition and its synonyms.
#[cfg(any(test, feature = "sample"))]
type SampleSense = (&'static str, &'static [&'static str]);

/// One entry of the sample: the word, its senses and its translations.
#[cfg(any(test, feature = "sample"))]
type SampleEntry = (
    &'static str,
    &'static [SampleSense],
    &'static [&'static str],
);

/// The entries of [`Sample`]; the words are lowercase.
#[cfg(any(test, feature = "sample"))]
const ENTRIES: &[SampleEntry] = &[
    (
        "bank",
        &[
            (
                "the ground along the edge of a river or a lake",
                &["shore", "riverside"],
            ),
            (
                "a company that keeps people's money and lends money",
                &["lender"],
            ),
            (
                "to expect something and make plans that depend on it",
                &["rely", "count"],
            ),
        ],
        &["берег", "банк", "рассчитывать"],
    ),
    (
        "charge",
        &[
            ("the money asked for a service", &["fee", "price"]),
            (
                "an official statement that a person has broken the law",
                &["accusation"],
            ),
            ("the amount of electricity held by an object", &[]),
            ("to run at somebody in order to attack", &["rush", "storm"]),
        ],
        &["плата", "обвинение", "заряд", "атаковать"],
    ),
    (
        "issue",
        &[
            (
                "a subject that people talk or disagree about",
                &["matter", "question"],
            ),
            (
                "a difficulty that has to be dealt with",
                &["problem", "trouble"],
            ),
            (
                "one of the regular numbers of a magazine or a newspaper",
                &["edition"],
            ),
            (
                "to give something out in an official way",
                &["release", "publish"],
            ),
        ],
        &["вопрос", "проблема", "выпуск", "выдавать"],
    ),
    (
        "mean",
        &[
            (
                "to have a certain sense; to stand for something",
                &["signify", "denote"],
            ),
            ("to want and plan to do something", &["intend"]),
            ("not kind to other people", &["unkind", "nasty"]),
            (
                "the sum of several numbers divided by how many they are",
                &["average"],
            ),
        ],
        &["значить", "намереваться", "злой", "среднее"],
    ),
];

/// The dictionary of a few words, for tests. A word matches its entry when it
/// is the entry's word after trimming and lowercasing, and in no other form.
#[cfg(any(test, feature = "sample"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sample;

#[cfg(any(test, feature = "sample"))]
impl Dictionary for Sample {
    fn lookup(&self, word: &str) -> Option<Article> {
        let key = word.trim().to_lowercase();
        let (_, senses, translations) = ENTRIES.iter().find(|(entry, _, _)| *entry == key)?;
        Some(Article {
            senses: senses
                .iter()
                .map(|(definition, synonyms)| Sense {
                    definition: (*definition).to_owned(),
                    synonyms: owned(synonyms),
                })
                .collect(),
            translations: owned(translations),
        })
    }
}

#[cfg(any(test, feature = "sample"))]
fn owned(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|text| (*text).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Dictionary, ENTRIES, Sample};

    const WORDS: [&str; 4] = ["bank", "charge", "issue", "mean"];

    #[test]
    fn the_sample_has_exactly_its_words() {
        let entries: Vec<_> = ENTRIES.iter().map(|(word, _, _)| *word).collect();
        assert_eq!(entries, WORDS);
    }

    #[test]
    fn every_word_has_a_full_article() {
        for word in WORDS {
            let article = Sample.lookup(word).expect(word);
            assert!(article.senses.len() >= 2, "{word}");
            assert!(!article.translations.is_empty(), "{word}");
            assert!(
                article
                    .senses
                    .iter()
                    .any(|sense| !sense.synonyms.is_empty()),
                "{word}"
            );
            for sense in &article.senses {
                assert!(!sense.definition.trim().is_empty(), "{word}");
                assert!(sense.definition.is_ascii(), "{word}: {}", sense.definition);
                for synonym in &sense.synonyms {
                    assert!(!synonym.trim().is_empty(), "{word}");
                }
            }
            for translation in &article.translations {
                assert!(
                    translation.chars().all(|c| ('а'..='я').contains(&c)),
                    "{word}: {translation}"
                );
                assert!(!translation.is_empty(), "{word}");
            }
        }
    }

    #[test]
    fn the_article_of_bank_has_both_of_its_common_senses() {
        let article = Sample.lookup("bank").expect("bank");
        assert!(article.senses[0].definition.contains("river"));
        assert!(article.senses[1].definition.contains("money"));
        assert!(article.translations.contains(&"берег".to_owned()));
        assert!(article.translations.contains(&"банк".to_owned()));
    }

    #[test]
    fn case_and_surrounding_whitespace_do_not_matter() {
        for word in WORDS {
            let expected = Sample.lookup(word);
            assert!(expected.is_some(), "{word}");
            let capitalised = format!("{}{}", word[..1].to_uppercase(), &word[1..]);
            for form in [
                word.to_uppercase(),
                capitalised,
                format!(" {word}\n"),
                format!("\u{a0}{word}\t"),
            ] {
                assert_eq!(Sample.lookup(&form), expected, "{form:?}");
            }
        }
    }

    #[test]
    fn a_word_outside_the_sample_has_no_entry() {
        // No lemmatisation: other forms of a sample word are other words.
        for word in [
            "sat", "banks", "banking", "meant", "issued", "", " ", "ban", "bank.", "банк",
        ] {
            assert_eq!(Sample.lookup(word), None, "{word:?}");
        }
    }

    #[test]
    fn the_sample_is_usable_through_the_contract_from_another_thread() {
        let dictionary: Arc<dyn Dictionary> = Arc::new(Sample);
        let shared = dictionary.clone();
        let found = std::thread::spawn(move || shared.lookup("Bank"))
            .join()
            .expect("thread");
        assert_eq!(found, dictionary.lookup("bank"));
        assert!(found.is_some());
    }
}
