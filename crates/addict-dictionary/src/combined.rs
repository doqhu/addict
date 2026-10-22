//! One article from two indexes: the definitions of one, the translations of
//! the other.
//!
//! [`Combined`] holds two open [`Index`] values and nothing of their
//! contents, so its memory is theirs. Either may be absent: an index that was
//! not opened answers as one that has no word and no exception form. A lookup
//! asks both:
//!
//! - a word the definitions index has gets the senses of that index, and of
//!   no other;
//! - a word only the translations index has gets the senses of that index;
//! - the translations are always those of the translations index, and the
//!   list is empty when it lacks the word;
//! - a word neither has is `None`.
//!
//! That is the exact lookup, [`Combined::find`]. The reduced lookup,
//! [`Combined::find_reduced`], is what the dictionary contract answers with:
//! it tries the word as written, its lowercase and then the dictionary forms
//! of `addict_core::reduction`, as the English arm of
//! [`lookup::headwords`] gives them, each through the exact lookup, and keeps
//! every one that is a headword of either index. The exception list of the
//! definitions index serves both. The contract merges what was found into
//! one article, and [`Combined::article`] is that article with the error of
//! an index as a value. Nothing is kept between lookups.
//!
//! Opening the two files, and what to say when one was never imported, is the
//! caller's.

use addict_core::{
    dictionary::{Article, Dictionary},
    reduction,
    translation::Source as Language,
};

use crate::{
    index::{Error, Index},
    lookup,
};

/// One headword the reduced lookup found, and its article.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The headword as the indexes have it: the word as written, its
    /// lowercase or one of its dictionary forms.
    pub headword: String,
    /// What [`Combined::find`] gives for the headword.
    pub article: Article,
}

/// Two indexes answering as one dictionary; either may be absent.
#[derive(Debug, Default)]
pub struct Combined {
    definitions: Option<Index>,
    translations: Option<Index>,
}

impl Combined {
    /// `definitions` gives the senses of the words it has (Open English
    /// WordNet); `translations` gives the translations, and the senses of the
    /// words `definitions` lacks (FreeDict).
    pub fn new(definitions: Index, translations: Index) -> Self {
        Self {
            definitions: Some(definitions),
            translations: Some(translations),
        }
    }

    /// The index the senses come from first, when there is one.
    pub fn definitions(&self) -> Option<&Index> {
        self.definitions.as_ref()
    }

    /// The index the translations come from, when there is one.
    pub fn translations(&self) -> Option<&Index> {
        self.translations.as_ref()
    }

    /// `index` is the definitions index from now on.
    pub fn set_definitions(&mut self, index: Index) {
        self.definitions = Some(index);
    }

    /// `index` is the translations index from now on.
    pub fn set_translations(&mut self, index: Index) {
        self.translations = Some(index);
    }

    /// The article of `word`, which must equal a headword of either index
    /// byte for byte, or `None`. Every index there is is read: an error of
    /// either is the error of the lookup.
    pub fn find(&self, word: &str) -> Result<Option<Article>, Error> {
        let find = |index: &Option<Index>| match index {
            Some(index) => index.find(word),
            None => Ok(None),
        };
        let defined = find(&self.definitions)?;
        let translated = find(&self.translations)?;
        Ok(match (defined, translated) {
            (None, None) => None,
            (None, Some(translated)) => Some(translated),
            (Some(defined), translated) => Some(Article {
                senses: defined.senses,
                translations: translated.map_or_else(Vec::new, |article| article.translations),
            }),
        })
    }

    /// The headwords `word` leads to and their articles, in the order tried:
    /// the word without the punctuation around it, its lowercase, then the
    /// dictionary forms of the lowercase word, the lemmas of the exception
    /// list of the definitions index first; without that index there are no
    /// such lemmas. Every one that is a headword of either index is there,
    /// once; empty when none is, or when nothing of the word is left. An
    /// error of either index is the error of the lookup.
    pub fn find_reduced(&self, word: &str) -> Result<Vec<Found>, Error> {
        let written = reduction::stripped(word);
        if written.is_empty() {
            return Ok(Vec::new());
        }
        let lowercase = reduction::lowercase(written);
        let lemmas = match &self.definitions {
            Some(definitions) => definitions.exception_lemmas(&lowercase)?,
            None => Vec::new(),
        };
        let candidates = lookup::headwords(
            Language::En,
            word,
            &lookup::Lemmas::from([(lowercase, lemmas)]),
        );
        let mut found = Vec::new();
        for headword in candidates {
            if let Some(article) = self.find(&headword)? {
                found.push(Found { headword, article });
            }
        }
        Ok(found)
    }

    /// The reduced lookup as one article: the senses of the word as written,
    /// then of each dictionary form in the order tried, and the translations
    /// joined in the same order. `None` when no form is a headword. An error
    /// of either index is the error of the lookup, and no article is given.
    pub fn article(&self, word: &str) -> Result<Option<Article>, Error> {
        Ok(merged(self.find_reduced(word)?))
    }
}

/// One article of what the reduced lookup found: the senses in the order
/// found, a sense whose definition is already there skipped, and the translations joined in the same
/// order, each once. `None` when nothing was found.
pub(crate) fn merged(found: Vec<Found>) -> Option<Article> {
    if found.is_empty() {
        return None;
    }
    let mut article = Article {
        senses: Vec::new(),
        translations: Vec::new(),
    };
    for Found { article: next, .. } in found {
        for sense in next.senses {
            // By the definition: the synonyms of one sense differ with the
            // headword it was reached through, and the first found stays.
            if !article
                .senses
                .iter()
                .any(|kept| kept.definition == sense.definition)
            {
                article.senses.push(sense);
            }
        }
        for translation in next.translations {
            if !article.translations.contains(&translation) {
                article.translations.push(translation);
            }
        }
    }
    Some(article)
}

impl Dictionary for Combined {
    /// [`Combined::article`]. Panics when an index cannot be read, as
    /// [`Index`] does.
    fn lookup(&self, word: &str) -> Option<Article> {
        self.article(word).unwrap_or_else(|error| panic!("{error}"))
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use addict_core::dictionary::Sense;

    use super::*;
    use crate::{
        format::{
            Contents, ExceptionForm, HEADER_LEN, Headword, SenseRecord, encode, tests::sample,
            tests::texts,
        },
        testdir::TempDir,
    };

    /// The sample index with what an Open English WordNet import adds for
    /// the reduced lookup: words alone in their senses, a capitalised
    /// headword and exception forms, one with its lemmas out of order.
    fn english() -> Contents {
        let mut contents = sample();
        for (word, definition) in [
            ("English", "the language of England"),
            ("be", "have the quality of being"),
            ("bet", "stake on the outcome of an issue"),
            ("go", "change location"),
            ("good", "having desirable qualities"),
            ("leave", "go away from a place"),
            ("left", "on the side to the north when facing east"),
            ("let", "make it possible for something to happen"),
            ("letter", "a written message addressed to a person"),
            ("run", "move fast by using one's feet"),
            ("saw", "a hand tool having a toothed blade"),
            ("see", "perceive by sight"),
            ("stop", "come to a halt"),
            ("well", "in a good or proper manner"),
        ] {
            contents.headwords.push(Headword {
                word: word.to_owned(),
                senses: vec![contents.senses.len() as u32],
                translations: Vec::new(),
            });
            contents.senses.push(SenseRecord {
                definition: definition.to_owned(),
                members: texts(&[word]),
                metadata: None,
            });
        }
        // One sense the word and its dictionary form share.
        let shared = contents.senses.len() as u32;
        contents.senses.push(SenseRecord {
            definition: "the trees in a large wooded area".to_owned(),
            members: texts(&["forest", "wood", "woods"]),
            metadata: None,
        });
        contents.senses.push(SenseRecord {
            definition: "the hard fibrous substance under the bark of trees".to_owned(),
            members: texts(&["wood"]),
            metadata: None,
        });
        for (word, senses) in [("wood", vec![shared + 1, shared]), ("woods", vec![shared])] {
            contents.headwords.push(Headword {
                word: word.to_owned(),
                senses,
                translations: Vec::new(),
            });
        }
        contents.headwords.sort_by(|a, b| a.word.cmp(&b.word));
        for (form, lemmas) in [
            ("better", &["well", "good"][..]),
            ("is", &["be"]),
            ("left", &["leave"]),
            ("saw", &["see"]),
            ("went", &["go"]),
        ] {
            contents.forms.push(ExceptionForm {
                form: form.to_owned(),
                lemmas: texts(lemmas),
            });
        }
        contents.forms.sort_by(|a, b| a.form.cmp(&b.form));
        contents
    }

    /// What a FreeDict import writes: definitions without members.
    fn russian() -> Contents {
        let definition = |text: &str| SenseRecord {
            definition: text.to_owned(),
            members: Vec::new(),
            metadata: None,
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
                    word: "go".to_owned(),
                    senses: vec![],
                    translations: texts(&["идти́"]),
                },
                Headword {
                    word: "leave".to_owned(),
                    senses: vec![],
                    translations: texts(&["оставля́ть", "оста́вил"]),
                },
                Headword {
                    word: "left".to_owned(),
                    senses: vec![],
                    translations: texts(&["ле́вый", "оста́вил"]),
                },
                Headword {
                    word: "saw".to_owned(),
                    senses: vec![],
                    translations: texts(&["пила́"]),
                },
                Headword {
                    word: "see".to_owned(),
                    senses: vec![],
                    translations: texts(&["ви́деть"]),
                },
                Headword {
                    word: "the".to_owned(),
                    senses: vec![],
                    translations: texts(&["э́тот"]),
                },
                Headword {
                    word: "webhook".to_owned(),
                    senses: vec![1, 2],
                    translations: texts(&["вебхук"]),
                },
                Headword {
                    word: "webhooks".to_owned(),
                    senses: vec![2],
                    translations: texts(&["вебхуки", "вебхук"]),
                },
                Headword {
                    word: "well-known".to_owned(),
                    senses: vec![],
                    translations: texts(&["изве́стный"]),
                },
            ],
            analyses: Vec::new(),
            forms: Vec::new(),
        }
    }

    /// The headwords the reduced lookup finds for `word`, in order.
    fn headwords(combined: &Combined, word: &str) -> Vec<String> {
        combined
            .find_reduced(word)
            .expect("find")
            .into_iter()
            .map(|found| found.headword)
            .collect()
    }

    fn definitions(article: &Article) -> Vec<&str> {
        article
            .senses
            .iter()
            .map(|sense| sense.definition.as_str())
            .collect()
    }

    fn paths(dir: &TempDir) -> (PathBuf, PathBuf) {
        let definitions = dir.path().join("oewn.index");
        let translations = dir.path().join("freedict.index");
        std::fs::write(&definitions, encode(&english()).expect("encode")).expect("write");
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
                .expect("definitions")
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
                // The reduced lookup reads both for a word, whatever its form.
                for word in ["bank", "banks", "went", "Bank", "(bank)", "boolean"] {
                    let error = combined.find_reduced(word).expect_err("truncated");
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

    #[test]
    #[should_panic(expected = "is damaged")]
    fn the_contract_panics_on_a_definitions_index_it_cannot_read() {
        let dir = TempDir::new("combined-panic-definitions");
        let (definitions, translations) = paths(&dir);
        let combined = Combined::new(
            Index::open(&definitions).expect("open"),
            Index::open(&translations).expect("open"),
        );
        std::fs::write(&definitions, b"").expect("truncate");
        let _ = combined.lookup("banks");
    }

    #[test]
    fn a_regular_form_has_the_article_of_its_dictionary_form() {
        let dir = TempDir::new("combined-regular");
        let combined = opened(&dir);
        let bank = combined.find("bank").expect("find").expect("bank");
        assert_eq!(combined.find("banks").expect("find"), None);
        assert_eq!(
            combined.find_reduced("banks").expect("find"),
            vec![Found {
                headword: "bank".to_owned(),
                article: bank.clone(),
            }]
        );
        assert_eq!(combined.lookup("banks"), Some(bank));
    }

    #[test]
    fn a_doubled_consonant_leads_to_the_dictionary_form() {
        let dir = TempDir::new("combined-doubling");
        let combined = opened(&dir);
        for (word, form) in [("running", "run"), ("stopped", "stop")] {
            assert_eq!(headwords(&combined, word), [form], "{word}");
            assert_eq!(
                combined.lookup(word),
                combined.find(form).expect("find"),
                "{word}"
            );
            assert!(combined.lookup(word).is_some(), "{word}");
        }
    }

    #[test]
    fn exception_lemmas_are_tried_first_and_sorted_then_the_suffix_forms() {
        let dir = TempDir::new("combined-exceptions");
        let combined = opened(&dir);
        assert_eq!(headwords(&combined, "went"), ["go"]);
        let went = combined.lookup("went").expect("went");
        assert_eq!(definitions(&went), ["change location"]);
        // The exception list of the definitions index serves the other too.
        assert_eq!(went.translations, texts(&["идти́"]));

        // The list has `well` before `good`; `bette` and `bett` are no words.
        assert_eq!(headwords(&combined, "better"), ["good", "well", "bet"]);
        assert_eq!(
            definitions(&combined.lookup("better").expect("better")),
            [
                "having desirable qualities",
                "in a good or proper manner",
                "stake on the outcome of an issue",
            ]
        );
        assert_eq!(headwords(&combined, "is"), ["be"]);
    }

    #[test]
    fn a_dictionary_form_has_its_own_article() {
        let dir = TempDir::new("combined-own");
        let combined = opened(&dir);
        for word in ["bank", "go", "advice", "webhook", "despite"] {
            assert_eq!(headwords(&combined, word), [word], "{word}");
            assert_eq!(
                combined.lookup(word),
                combined.find(word).expect("find"),
                "{word}"
            );
        }
    }

    #[test]
    fn a_word_and_its_form_give_one_article_the_word_first() {
        let dir = TempDir::new("combined-merged");
        let combined = opened(&dir);
        assert_eq!(headwords(&combined, "saw"), ["saw", "see"]);
        let saw = combined.lookup("saw").expect("saw");
        assert_eq!(
            definitions(&saw),
            ["a hand tool having a toothed blade", "perceive by sight"]
        );
        assert_eq!(saw.translations, texts(&["пила́", "ви́деть"]));

        assert_eq!(headwords(&combined, "left"), ["left", "leave"]);
        let left = combined.lookup("left").expect("left");
        assert_eq!(
            definitions(&left),
            [
                "on the side to the north when facing east",
                "go away from a place",
            ]
        );
        // Each translation once, in the order found.
        assert_eq!(left.translations, texts(&["ле́вый", "оста́вил", "оставля́ть"]));

        // A doubtful form is not sifted out.
        assert_eq!(headwords(&combined, "letter"), ["letter", "let"]);
        assert_eq!(
            definitions(&combined.lookup("letter").expect("letter")),
            [
                "a written message addressed to a person",
                "make it possible for something to happen",
            ]
        );
    }

    #[test]
    fn a_sense_the_word_and_its_form_share_is_given_once() {
        let dir = TempDir::new("combined-repeated");
        let combined = opened(&dir);
        assert_eq!(headwords(&combined, "webhooks"), ["webhooks", "webhook"]);
        let found = combined.find_reduced("webhooks").expect("find");
        assert_eq!(definitions(&found[0].article), ["A web hook."]);
        assert_eq!(
            definitions(&found[1].article),
            ["A unit of work for a web server.", "A web hook."]
        );
        let article = combined.lookup("webhooks").expect("webhooks");
        assert_eq!(
            definitions(&article),
            ["A web hook.", "A unit of work for a web server."]
        );
        assert_eq!(article.translations, texts(&["вебхуки", "вебхук"]));
    }

    #[test]
    fn a_synset_of_the_word_and_its_form_is_given_once_as_first_found() {
        let dir = TempDir::new("combined-synset");
        let combined = opened(&dir);
        assert_eq!(headwords(&combined, "woods"), ["woods", "wood"]);
        let found = combined.find_reduced("woods").expect("find");
        // The same synset, with other synonyms through the other headword.
        assert_eq!(
            found[0].article.senses[0].synonyms,
            texts(&["forest", "wood"])
        );
        assert_eq!(
            found[1].article.senses[1].synonyms,
            texts(&["forest", "woods"])
        );
        assert_eq!(
            combined.lookup("woods").expect("woods").senses,
            vec![
                Sense {
                    definition: "the trees in a large wooded area".to_owned(),
                    synonyms: texts(&["forest", "wood"]),
                },
                Sense {
                    definition: "the hard fibrous substance under the bark of trees".to_owned(),
                    synonyms: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn the_word_as_written_is_tried_before_its_lowercase() {
        let dir = TempDir::new("combined-case");
        let combined = opened(&dir);
        for (word, expected) in [
            ("Bank", "bank"),
            ("BANK", "bank"),
            ("The", "the"),
            ("BANKS", "bank"),
            ("Banks", "bank"),
            ("Went", "go"),
            ("English", "English"),
        ] {
            assert_eq!(headwords(&combined, word), [expected], "{word}");
            assert_eq!(
                combined.lookup(word),
                combined.find(expected).expect("find"),
                "{word}"
            );
        }
        // The forms are those of the lowercase word, not a second spelling.
        assert_eq!(headwords(&combined, "english"), Vec::<String>::new());
        assert_eq!(headwords(&combined, "Bánk"), ["Bánk"]);
    }

    #[test]
    fn surrounding_punctuation_is_not_part_of_the_word() {
        let dir = TempDir::new("combined-punctuation");
        let combined = opened(&dir);
        for (word, expected) in [
            ("bank.", "bank"),
            ("(bank)", "bank"),
            ("\"went,\"", "go"),
            (" Banks! ", "bank"),
            ("«well-known»", "well-known"),
        ] {
            assert_eq!(headwords(&combined, word), [expected], "{word:?}");
            assert_eq!(
                combined.lookup(word),
                combined.find(expected).expect("find"),
                "{word:?}"
            );
        }
        // Nothing is split: the parts of a hyphenated word are not looked up.
        assert_eq!(headwords(&combined, "bank-well"), Vec::<String>::new());
    }

    #[test]
    fn a_word_with_nothing_left_or_without_a_headword_form_is_none() {
        let dir = TempDir::new("combined-none");
        let combined = opened(&dir);
        for word in ["", " ", "...", "boolean", "xyzzy", "us", "mutexes", "(?)"] {
            assert_eq!(headwords(&combined, word), Vec::<String>::new(), "{word:?}");
            assert_eq!(combined.lookup(word), None, "{word:?}");
        }
    }

    #[test]
    fn a_word_with_nothing_left_reads_no_index() {
        let dir = TempDir::new("combined-unread");
        let (definitions, translations) = paths(&dir);
        let combined = Combined::new(
            Index::open(&definitions).expect("open"),
            Index::open(&translations).expect("open"),
        );
        std::fs::write(&definitions, b"").expect("truncate");
        std::fs::write(&translations, b"").expect("truncate");
        for word in ["", " ", "..."] {
            assert_eq!(combined.find_reduced(word).expect("find"), Vec::new());
            assert_eq!(combined.lookup(word), None);
        }
    }

    #[test]
    fn the_article_is_the_merged_one_and_an_unreadable_index_is_its_error() {
        let dir = TempDir::new("combined-article");
        let (definitions, translations) = paths(&dir);
        let combined = opened(&dir);
        assert_eq!(
            combined.article("banks").expect("article"),
            combined.lookup("bank")
        );
        assert_eq!(combined.article("xyzzy").expect("article"), None);
        // Each index in turn: the error names the file, and there is no
        // article of what the other index still has.
        for damaged in [&translations, &definitions] {
            let whole = std::fs::read(damaged).expect("read");
            std::fs::write(damaged, b"").expect("truncate");
            let error = combined.article("bank").expect_err("unreadable");
            assert!(
                matches!(&error, Error::Damaged { path, .. } if path == damaged),
                "{error}"
            );
            std::fs::write(damaged, whole).expect("restore");
        }
        assert!(combined.article("bank").expect("article").is_some());
    }

    #[test]
    fn the_definitions_index_alone_gives_senses_and_no_translation() {
        let dir = TempDir::new("combined-definitions-alone");
        let (definitions, _) = paths(&dir);
        let mut alone = Combined::default();
        alone.set_definitions(Index::open(&definitions).expect("open"));
        assert!(alone.translations().is_none());
        let both = opened(&dir);
        for word in ["bank", "banks", "went", "Bank"] {
            let article = alone.article(word).expect("article").expect(word);
            let whole = both.article(word).expect("article").expect(word);
            assert_eq!(article.senses, whole.senses, "{word}");
            assert!(!article.senses.is_empty(), "{word}");
            assert_eq!(article.translations, Vec::<String>::new(), "{word}");
        }
        // Words only the other index has.
        for word in ["despite", "webhook", "xyzzy"] {
            assert_eq!(alone.article(word).expect("article"), None, "{word}");
        }
    }

    #[test]
    fn the_translations_index_alone_gives_its_definitions_and_translations() {
        let dir = TempDir::new("combined-translations-alone");
        let (_, translations) = paths(&dir);
        let mut alone = Combined::default();
        alone.set_translations(Index::open(&translations).expect("open"));
        assert!(alone.definitions().is_none());
        let bank = alone.article("bank").expect("article").expect("bank");
        assert_eq!(
            bank.senses,
            vec![Sense {
                definition: "An edge of river, lake, or other watercourse.".to_owned(),
                synonyms: Vec::new(),
            }]
        );
        assert_eq!(bank.translations, texts(&["бе́рег", "брег", "банк"]));
        // The suffix forms still lead to the dictionary form.
        assert_eq!(alone.article("banks").expect("article"), Some(bank));
        assert_eq!(headwords(&alone, "webhooks"), ["webhooks", "webhook"]);
        // No exception lemmas without the definitions index: `went` is `go`
        // only through them.
        assert_eq!(alone.article("went").expect("article"), None);
        assert_eq!(alone.article("xyzzy").expect("article"), None);
    }

    #[test]
    fn without_an_index_there_is_no_article_and_no_error() {
        let neither = Combined::default();
        for word in ["bank", "banks", ""] {
            assert_eq!(neither.article(word).expect("article"), None, "{word:?}");
        }
    }
}
