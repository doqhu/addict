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
//! headwords of its language, and [`grouped`] answers from every index of
//! one language at once, what each gives under its [`Kind`].

use addict_core::{
    dictionary::{Article, Sense},
    reduction,
    translation::Source as Language,
};

use crate::{
    combined::{Found, merged},
    import::Kind,
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
    Ok(merged(found(index, language, word)?))
}

/// The headwords of `index` that [`headwords`] gives for `word` read in
/// `language`, with their articles, in that order.
fn found(index: &Index, language: Language, word: &str) -> Result<Vec<Found>, Error> {
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
    Ok(found)
}

/// What the indexes of one language give for a word, by [`Kind`]. Each
/// field is `None` when no index of its kind was asked, and empty when the
/// indexes of its kind have nothing for the word.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grouped {
    /// [`Kind::Definitions`]: the senses, with their own synonyms, merged in
    /// the order of the indexes, a definition already there skipped.
    pub definitions: Option<Vec<Sense>>,
    /// [`Kind::Russian`]: the translations of the word, each once.
    pub russian: Option<Vec<String>>,
    /// [`Kind::Synonyms`]: the members of the senses as one list for the
    /// word, each once, without the word itself.
    pub synonyms: Option<Vec<String>>,
    /// [`Kind::English`]: the definitions of the senses as English glosses,
    /// each once, none blank.
    pub english: Option<Vec<String>>,
}

/// What `indexes`, each with its kind, give for `word` read in `language`,
/// grouped by kind in the order of the list. Each index is asked for the
/// headwords [`headwords`] gives, as [`article`] asks, and every sense of
/// every headword found is kept; only the definitions are merged, a
/// definition already there skipped.
/// `None` when no index has a headword for the word. An error of an index
/// is the error of the lookup; it carries the path of that index.
pub fn grouped(
    indexes: &[(Kind, &Index)],
    language: Language,
    word: &str,
) -> Result<Option<Grouped>, Error> {
    let mut grouped = Grouped::default();
    let mut found_any = false;
    let mut definitions = Vec::new();
    // The word as it may stand among the members of a sense.
    let itself = headwords(language, word, &[]);
    for (kind, index) in indexes {
        let found = found(index, language, word)?;
        found_any |= !found.is_empty();
        // Every sense of every headword: the senses of a synonyms index may
        // all have the same definition, or none.
        let senses = found.iter().flat_map(|found| &found.article.senses);
        match kind {
            Kind::Definitions => {
                grouped.definitions.get_or_insert_with(Vec::new);
                definitions.extend(found.into_iter().map(|found| Found {
                    article: Article {
                        translations: Vec::new(),
                        ..found.article
                    },
                    ..found
                }));
            }
            Kind::Russian => {
                let russian = grouped.russian.get_or_insert_with(Vec::new);
                let translations = found.iter().flat_map(|found| &found.article.translations);
                push_new(russian, translations.cloned());
            }
            Kind::Synonyms => {
                let synonyms = grouped.synonyms.get_or_insert_with(Vec::new);
                let members = senses.flat_map(|sense| &sense.synonyms);
                push_new(
                    synonyms,
                    members.filter(|member| !itself.contains(member)).cloned(),
                );
            }
            Kind::English => {
                let english = grouped.english.get_or_insert_with(Vec::new);
                let glosses = senses.filter(|sense| !sense.definition.trim().is_empty());
                push_new(english, glosses.map(|sense| sense.definition.clone()));
            }
        }
    }
    if !found_any {
        return Ok(None);
    }
    if let Some(senses) = &mut grouped.definitions {
        *senses = merged(definitions).map_or_else(Vec::new, |article| article.senses);
    }
    Ok(Some(grouped))
}

/// Appends each of `items` that `list` does not have yet.
fn push_new(list: &mut Vec<String>, items: impl IntoIterator<Item = String>) {
    for item in items {
        if !list.contains(&item) {
            list.push(item);
        }
    }
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

    /// One headword of a canned index: the word, its senses as a
    /// definition and its members, and its translations.
    type Entry<'a> = (&'a str, &'a [(&'a str, &'a [&'a str])], &'a [&'a str]);

    /// Writes an index of `entries` named `name` into `dir` and opens it.
    fn canned(dir: &TempDir, name: &str, entries: &[Entry<'_>]) -> Index {
        let mut entries = entries.to_vec();
        entries.sort_by_key(|(word, _, _)| *word);
        let mut contents = Contents {
            source: name.to_owned(),
            senses: Vec::new(),
            headwords: Vec::new(),
            forms: Vec::new(),
        };
        for (word, senses, translations) in entries {
            let mut numbers = Vec::new();
            for (definition, members) in senses {
                numbers.push(contents.senses.len() as u32);
                contents.senses.push(SenseRecord {
                    definition: (*definition).to_owned(),
                    members: texts(members),
                });
            }
            contents.headwords.push(Headword {
                word: word.to_owned(),
                senses: numbers,
                translations: texts(translations),
            });
        }
        let path = dir.path().join(name);
        std::fs::write(&path, encode(&contents).expect("encode")).expect("write");
        Index::open(&path).expect("open")
    }

    /// The four kinds of Spanish indexes, each having `banco`.
    struct Spanish {
        russian: Index,
        definitions: Index,
        synonyms: Index,
        english: Index,
    }

    fn spanish_set(dir: &TempDir) -> Spanish {
        Spanish {
            russian: canned(
                dir,
                "es-ru.index",
                &[
                    ("banco", &[], &["банк", "скамейка"]),
                    ("mesa", &[], &["стол"]),
                ],
            ),
            definitions: canned(
                dir,
                "es-def.index",
                &[
                    (
                        "banco",
                        &[
                            ("Empresa que ofrece servicios monetarios.", &[]),
                            ("Mueble alargado para sentarse.", &["banco", "banca"]),
                        ],
                        &[],
                    ),
                    ("cosa", &[("Lo que existe.", &[])], &[]),
                ],
            ),
            synonyms: canned(
                dir,
                "es-syn.index",
                &[(
                    "banco",
                    &[
                        ("", &["banco", "entidad", "banca"]),
                        ("", &["asiento", "banca"]),
                    ],
                    &[],
                )],
            ),
            english: canned(
                dir,
                "es-en.index",
                &[(
                    "banco",
                    &[
                        ("bank (financial institution)", &[]),
                        ("bench", &[]),
                        ("bench", &[]),
                    ],
                    &[],
                )],
            ),
        }
    }

    #[test]
    fn every_kind_gives_its_part_of_the_article() {
        let dir = TempDir::new("grouped-all");
        let set = spanish_set(&dir);
        let indexes = [
            (Kind::Russian, &set.russian),
            (Kind::Definitions, &set.definitions),
            (Kind::Synonyms, &set.synonyms),
            (Kind::English, &set.english),
        ];
        let banco = grouped(&indexes, Language::Es, "«Banco»,")
            .expect("look up")
            .expect("banco");
        assert_eq!(banco.russian, Some(texts(&["банк", "скамейка"])));
        assert_eq!(
            banco.definitions,
            Some(vec![
                Sense {
                    definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                    synonyms: Vec::new(),
                },
                Sense {
                    definition: "Mueble alargado para sentarse.".to_owned(),
                    synonyms: texts(&["banca"]),
                },
            ])
        );
        // One list for the word, each once and without the word.
        assert_eq!(
            banco.synonyms,
            Some(texts(&["entidad", "banca", "asiento"]))
        );
        assert_eq!(
            banco.english,
            Some(texts(&["bank (financial institution)", "bench"]))
        );
    }

    #[test]
    fn a_catalan_definitions_index_alone_gives_definitions_and_no_other_kind() {
        let dir = TempDir::new("grouped-ca");
        let definitions = canned(
            &dir,
            "ca-def.index",
            &[("col·lecció", &[("Conjunt de coses.", &[])], &[])],
        );
        let found = grouped(
            &[(Kind::Definitions, &definitions)],
            Language::Ca,
            "col·lecció",
        )
        .expect("look up")
        .expect("col·lecció");
        assert_eq!(
            found,
            Grouped {
                definitions: Some(vec![Sense {
                    definition: "Conjunt de coses.".to_owned(),
                    synonyms: Vec::new(),
                }]),
                russian: None,
                synonyms: None,
                english: None,
            }
        );
    }

    #[test]
    fn a_kind_whose_index_lacks_the_word_is_empty_and_not_absent() {
        let dir = TempDir::new("grouped-lacks");
        let set = spanish_set(&dir);
        let indexes = [
            (Kind::Russian, &set.russian),
            (Kind::Definitions, &set.definitions),
        ];
        let cosa = grouped(&indexes, Language::Es, "cosa")
            .expect("look up")
            .expect("cosa");
        assert_eq!(cosa.russian, Some(Vec::new()));
        assert_eq!(cosa.definitions.expect("definitions").len(), 1);
        assert_eq!(cosa.synonyms, None);
        assert_eq!(cosa.english, None);
        // Russian alone: no definitions asked, so none are said to be empty.
        let mesa = grouped(&indexes, Language::Es, "mesa")
            .expect("look up")
            .expect("mesa");
        assert_eq!(mesa.russian, Some(texts(&["стол"])));
        assert_eq!(mesa.definitions, Some(Vec::new()));
    }

    #[test]
    fn a_word_with_no_synonyms_anywhere_has_empty_lists() {
        let dir = TempDir::new("grouped-no-synonyms");
        let set = spanish_set(&dir);
        let indexes = [
            (Kind::Definitions, &set.definitions),
            (Kind::Synonyms, &set.synonyms),
        ];
        let cosa = grouped(&indexes, Language::Es, "cosa")
            .expect("look up")
            .expect("cosa");
        assert_eq!(cosa.synonyms, Some(Vec::new()));
        let senses = cosa.definitions.expect("definitions");
        assert!(senses.iter().all(|sense| sense.synonyms.is_empty()));
    }

    #[test]
    fn a_word_in_no_index_has_no_article() {
        let dir = TempDir::new("grouped-none");
        let set = spanish_set(&dir);
        let indexes = [
            (Kind::Russian, &set.russian),
            (Kind::Definitions, &set.definitions),
            (Kind::Synonyms, &set.synonyms),
            (Kind::English, &set.english),
        ];
        assert_eq!(
            grouped(&indexes, Language::Es, "zzz").expect("look up"),
            None
        );
        assert_eq!(grouped(&indexes, Language::Es, "…").expect("look up"), None);
        assert_eq!(grouped(&[], Language::Es, "banco").expect("look up"), None);
    }

    #[test]
    fn an_index_that_cannot_be_read_is_the_error_with_its_path() {
        let dir = TempDir::new("grouped-error");
        let set = spanish_set(&dir);
        let broken = dir.path().join("es-syn.index");
        std::fs::write(&broken, b"").expect("truncate");
        let indexes = [
            (Kind::Definitions, &set.definitions),
            (Kind::Synonyms, &set.synonyms),
        ];
        let error = grouped(&indexes, Language::Es, "banco").expect_err("unreadable");
        let (Error::NotImported(path) | Error::Io { path, .. } | Error::Damaged { path, .. }) =
            &error;
        assert_eq!(path, &broken, "{error}");
    }
}
