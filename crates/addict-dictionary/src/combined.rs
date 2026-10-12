//! One article from two indexes: the definitions of one, the translations of
//! the other.
//!
//! [`Combined`] holds two open [`Index`] values and nothing of their
//! contents, so its memory is theirs. A lookup asks both:
//!
//! - a word the definitions index has gets the senses of that index, and of
//!   no other;
//! - a word only the translations index has gets the senses of that index;
//! - the translations are always those of the translations index, and the
//!   list is empty when it lacks the word;
//! - a word neither has is `None`.
//!
//! Opening the two files, and what to do when one was never imported, is the
//! caller's.

use addict_core::dictionary::{Article, Dictionary};

use crate::index::{Error, Index};

/// Two open indexes answering as one dictionary.
#[derive(Debug)]
pub struct Combined {
    definitions: Index,
    translations: Index,
}

impl Combined {
    /// `definitions` gives the senses of the words it has (Open English
    /// WordNet); `translations` gives the translations, and the senses of the
    /// words `definitions` lacks (FreeDict).
    pub fn new(definitions: Index, translations: Index) -> Self {
        Self {
            definitions,
            translations,
        }
    }

    /// The index the senses come from first.
    pub fn definitions(&self) -> &Index {
        &self.definitions
    }

    /// The index the translations come from.
    pub fn translations(&self) -> &Index {
        &self.translations
    }

    /// The article of `word`, which must equal a headword of either index
    /// byte for byte, or `None`. Both indexes are read: an error of either
    /// is the error of the lookup.
    pub fn find(&self, word: &str) -> Result<Option<Article>, Error> {
        let defined = self.definitions.find(word)?;
        let translated = self.translations.find(word)?;
        Ok(match (defined, translated) {
            (None, None) => None,
            (None, Some(translated)) => Some(translated),
            (Some(defined), translated) => Some(Article {
                senses: defined.senses,
                translations: translated.map_or_else(Vec::new, |article| article.translations),
            }),
        })
    }
}

impl Dictionary for Combined {
    /// Panics when an index cannot be read, as [`Index`] does.
    fn lookup(&self, word: &str) -> Option<Article> {
        self.find(word).unwrap_or_else(|error| panic!("{error}"))
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use addict_core::dictionary::Sense;

    use super::*;
    use crate::{
        format::{Contents, HEADER_LEN, Headword, SenseRecord, encode, tests::sample},
        testdir::TempDir,
    };

    fn texts(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    /// What a FreeDict import writes: definitions without members.
    fn russian() -> Contents {
        let definition = |text: &str| SenseRecord {
            definition: text.to_owned(),
            members: Vec::new(),
        };
        Contents {
            source: "russian-1".to_owned(),
            senses: vec![
                definition("An edge of river, lake, or other watercourse."),
                definition("A unit of work for a web server."),
                definition("A web hook."),
            ],
            headwords: vec![
                Headword {
                    word: "bank".to_owned(),
                    senses: vec![0],
                    translations: texts(&["бе́рег", "брег", "банк"]),
                },
                Headword {
                    word: "despite".to_owned(),
                    senses: vec![],
                    translations: texts(&["несмотря на"]),
                },
                Headword {
                    word: "webhook".to_owned(),
                    senses: vec![1, 2],
                    translations: texts(&["вебхук"]),
                },
            ],
            forms: Vec::new(),
        }
    }

    fn paths(dir: &TempDir) -> (PathBuf, PathBuf) {
        let definitions = dir.path().join("oewn.index");
        let translations = dir.path().join("freedict.index");
        std::fs::write(&definitions, encode(&sample()).expect("encode")).expect("write");
        std::fs::write(&translations, encode(&russian()).expect("encode")).expect("write");
        (definitions, translations)
    }

    fn opened(dir: &TempDir) -> Combined {
        let (definitions, translations) = paths(dir);
        Combined::new(
            Index::open(&definitions).expect("open"),
            Index::open(&translations).expect("open"),
        )
    }

    #[test]
    fn a_word_in_both_has_the_senses_of_the_first_and_the_translations_of_the_second() {
        let dir = TempDir::new("combined-both");
        let combined = opened(&dir);
        let article = combined.find("bank").expect("find").expect("bank");
        assert_eq!(
            article.senses,
            combined
                .definitions()
                .find("bank")
                .expect("find")
                .expect("bank")
                .senses
        );
        assert_eq!(article.senses.len(), 2);
        assert!(
            article
                .senses
                .iter()
                .all(|sense| !sense.definition.contains("watercourse"))
        );
        // Not the translations the definitions index happens to hold.
        assert_eq!(article.translations, texts(&["бе́рег", "брег", "банк"]));
    }

    #[test]
    fn a_word_without_a_translation_has_an_empty_list() {
        let dir = TempDir::new("combined-untranslated");
        let combined = opened(&dir);
        for word in ["advice", "shore"] {
            let article = combined.find(word).expect("find").expect(word);
            assert_eq!(article.senses.len(), 1, "{word}");
            assert!(article.translations.is_empty(), "{word}");
        }
        // The translations of the definitions index are never used.
        let article = combined.find("Bánk").expect("find").expect("Bánk");
        assert!(article.senses.is_empty());
        assert!(article.translations.is_empty());
    }

    #[test]
    fn a_word_of_the_second_only_has_its_definitions_as_senses_without_synonyms() {
        let dir = TempDir::new("combined-second");
        let combined = opened(&dir);
        let article = combined.find("webhook").expect("find").expect("webhook");
        assert_eq!(
            article.senses,
            vec![
                Sense {
                    definition: "A unit of work for a web server.".to_owned(),
                    synonyms: Vec::new(),
                },
                Sense {
                    definition: "A web hook.".to_owned(),
                    synonyms: Vec::new(),
                },
            ]
        );
        assert_eq!(article.translations, texts(&["вебхук"]));

        let article = combined.find("despite").expect("find").expect("despite");
        assert!(article.senses.is_empty());
        assert_eq!(article.translations, texts(&["несмотря на"]));
    }

    #[test]
    fn a_word_in_neither_is_none() {
        let dir = TempDir::new("combined-neither");
        let combined = opened(&dir);
        for word in ["boolean", "mutex", "", "Bank", "banks", "bank "] {
            assert_eq!(combined.find(word).expect("find"), None, "{word:?}");
        }
    }

    #[test]
    fn the_pair_answers_the_contract_from_another_thread() {
        let dir = TempDir::new("combined-thread");
        let dictionary: Arc<dyn Dictionary> = Arc::new(opened(&dir));
        let shared = dictionary.clone();
        let found = std::thread::spawn(move || shared.lookup("bank"))
            .join()
            .expect("thread");
        assert_eq!(
            found.expect("bank").translations,
            texts(&["бе́рег", "брег", "банк"])
        );
        assert_eq!(dictionary.lookup("boolean"), None);
    }

    #[test]
    fn an_unreadable_index_of_either_side_fails_the_lookup() {
        for damaged_translations in [false, true] {
            let dir = TempDir::new("combined-damaged");
            let (definitions, translations) = paths(&dir);
            let combined = Combined::new(
                Index::open(&definitions).expect("open"),
                Index::open(&translations).expect("open"),
            );
            let damaged = if damaged_translations {
                &translations
            } else {
                &definitions
            };
            let whole = std::fs::read(damaged).expect("index");
            for len in [HEADER_LEN, 3, 0] {
                std::fs::write(damaged, &whole[..len]).expect("truncate");
                // Also a word the whole index alone would answer.
                for word in ["bank", "advice", "webhook", "boolean"] {
                    let error = combined.find(word).expect_err("truncated");
                    assert!(
                        matches!(error, Error::Damaged { .. }),
                        "{len} {word}: {error}"
                    );
                    assert!(
                        error.to_string().contains(&damaged.display().to_string()),
                        "{error}"
                    );
                }
            }
        }
    }

    #[test]
    #[should_panic(expected = "is damaged")]
    fn the_contract_panics_on_an_index_it_cannot_read() {
        let dir = TempDir::new("combined-panic");
        let (definitions, translations) = paths(&dir);
        let combined = Combined::new(
            Index::open(&definitions).expect("open"),
            Index::open(&translations).expect("open"),
        );
        std::fs::write(&translations, b"").expect("truncate");
        let _ = combined.lookup("bank");
    }
}
