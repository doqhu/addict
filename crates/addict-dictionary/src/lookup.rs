//! The dictionary-form hook: the headwords to try for a picked word, per
//! reading language, and the lookup of one index through them.
//!
//! [`headwords`] is the one place that says how a word reaches its
//! headwords in each language:
//!
//! - English: the word without the punctuation around it, its lowercase,
//!   then the dictionary forms of `addict_core::reduction`, the lemmas of an
//!   exception list first. [`Combined`](crate::Combined) looks up through
//!   it.
//! - Spanish and Catalan: the word as written, without the punctuation
//!   around it, and its lowercase, with no dictionary-form rule and no
//!   exception list. The rules and the lists of a later entry replace this
//!   arm; the form tables of the ES and CA indexes are written already.
//!
//! [`article`] answers from one index of an ES or CA source with the
//! headwords of its language.

use addict_core::{dictionary::Article, reduction, translation::Source as Language};

use crate::{
    combined::{Found, merged},
    index::{Error, Index},
};

/// The headwords to try for `word` read in `language`, in order, each once;
/// empty when nothing of the word is left. `lemmas` are what an exception
/// list gives for the lowercase word; only English uses them. No wildcard
/// arm: a new language does not compile until it is given its hook.
pub fn headwords(language: Language, word: &str, lemmas: &[String]) -> Vec<String> {
    let written = reduction::stripped(word);
    if written.is_empty() {
        return Vec::new();
    }
    let lowercase = reduction::lowercase(written);
    let forms = match language {
        Language::En => reduction::forms(&lowercase, lemmas),
        Language::Es | Language::Ca => Vec::new(),
    };
    let mut candidates = vec![written.to_owned()];
    for candidate in std::iter::once(lowercase).chain(forms) {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

/// Whether `language` reads the exception list of an index for its
/// headwords. No wildcard arm, as in [`headwords`].
pub fn uses_exceptions(language: Language) -> bool {
    match language {
        Language::En => true,
        Language::Es | Language::Ca => false,
    }
}

/// The article of `word` read in `language` from `index` alone: the senses
/// and translations of every headword [`headwords`] gives that the index
/// has, merged in that order, a sense whose definition is already there
/// skipped. `None` when the index has none of them. An error of the index
/// is the error of the lookup.
pub fn article(index: &Index, language: Language, word: &str) -> Result<Option<Article>, Error> {
    let lemmas = if uses_exceptions(language) {
        let lowercase = reduction::lowercase(reduction::stripped(word));
        index.exception_lemmas(&lowercase)?
    } else {
        Vec::new()
    };
    let mut found = Vec::new();
    for headword in headwords(language, word, &lemmas) {
        if let Some(article) = index.find(&headword)? {
            found.push(Found { headword, article });
        }
    }
    Ok(merged(found))
}

#[cfg(test)]
mod tests {
    use addict_core::dictionary::Sense;

    use super::*;
    use crate::{
        format::{Contents, ExceptionForm, Headword, SenseRecord, encode, tests::texts},
        testdir::TempDir,
    };

    #[test]
    fn english_keeps_the_reduction_and_spanish_and_catalan_take_the_word_as_written() {
        let lemmas = texts(&["go"]);
        let english = headwords(Language::En, "“Went,”", &lemmas);
        assert_eq!(english[..3], texts(&["Went", "went", "go"]));
        assert_eq!(
            headwords(Language::En, "banks", &[]),
            [vec!["banks".to_owned()], reduction::forms("banks", &[])].concat()
        );
        for language in [Language::Es, Language::Ca] {
            assert_eq!(
                headwords(language, "«Bancos»,", &lemmas),
                texts(&["Bancos", "bancos"])
            );
            assert_eq!(headwords(language, "cerró", &lemmas), texts(&["cerró"]));
            assert_eq!(
                headwords(language, "col·leccions", &[]),
                texts(&["col·leccions"])
            );
            assert!(headwords(language, "…", &[]).is_empty());
            assert!(!uses_exceptions(language));
        }
        assert!(uses_exceptions(Language::En));
    }

    fn spanish() -> Contents {
        Contents {
            source: "spanish-1".to_owned(),
            senses: vec![
                SenseRecord {
                    definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                    members: Vec::new(),
                },
                SenseRecord {
                    definition: "Mueble alargado para sentarse.".to_owned(),
                    members: texts(&["banca"]),
                },
                SenseRecord {
                    definition: "Plural de banco.".to_owned(),
                    members: Vec::new(),
                },
            ],
            headwords: vec![
                Headword {
                    word: "banco".to_owned(),
                    senses: vec![0, 1],
                    translations: Vec::new(),
                },
                Headword {
                    word: "bancos".to_owned(),
                    senses: vec![2],
                    translations: Vec::new(),
                },
            ],
            forms: vec![ExceptionForm {
                form: "bancos".to_owned(),
                lemmas: texts(&["banco"]),
            }],
        }
    }

    #[test]
    fn a_spanish_word_is_found_as_written_and_not_through_its_forms() {
        let dir = TempDir::new("lookup-es");
        let path = dir.path().join("es.index");
        std::fs::write(&path, encode(&spanish()).expect("encode")).expect("write");
        let index = Index::open(&path).expect("open");
        let banco = article(&index, Language::Es, "Banco").expect("find");
        assert_eq!(
            banco,
            Some(Article {
                senses: vec![
                    Sense {
                        definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                        synonyms: Vec::new(),
                    },
                    Sense {
                        definition: "Mueble alargado para sentarse.".to_owned(),
                        synonyms: texts(&["banca"]),
                    },
                ],
                translations: Vec::new(),
            })
        );
        // The form table names `banco`, and is not read: only `bancos`.
        let bancos = article(&index, Language::Es, "bancos")
            .expect("find")
            .expect("bancos");
        assert_eq!(bancos.senses.len(), 1);
        assert_eq!(bancos.senses[0].definition, "Plural de banco.");
        assert_eq!(article(&index, Language::Es, "zzz").expect("find"), None);
    }
}
