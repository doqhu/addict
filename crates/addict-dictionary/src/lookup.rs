//! The dictionary-form hook: the headwords to try for a picked word, per
//! reading language, and the lookup of one index through them.
//!
//! [`headwords`] is the one place that says how a word reaches its
//! headwords in each language. Each gives the word without the punctuation
//! around it, then its lowercase, then the dictionary forms of the
//! lowercase word, each once:
//!
//! - English: the forms of `addict_core::reduction`, the lemmas of an
//!   exception list first. [`Combined`](crate::Combined) looks up through
//!   it.
//! - Spanish and Catalan: the forms of `addict_core::reduction_es_ca`, the
//!   port of EXP-9's `rules.py` (elision, enclitic pronouns, suffix rules,
//!   accents, the Catalan bare stem), with the exception lemmas of the word
//!   and of each of its bases.
//!
//! The exception lemmas come from the form tables of the indexes, through a
//! [`Lemmas`] map the caller reads for the forms [`asked_forms`] names. For
//! Spanish and Catalan it is the union of the form tables of every index of
//! the language that is asked, so an index without a form table (WikDict,
//! OMW) still answers for `fue` the `ser` and `ir` another index lists. The
//! tables are used whole, as imported: EXP-9 measured the same coverage
//! with the full tables as with their irregular-only part.
//!
//! [`article`] answers from one index of an ES or CA source with the
//! headwords of its language, and [`grouped`] answers from every index of
//! one language at once, what each gives under its [`Kind`].

use std::collections::BTreeMap;

use addict_core::{
    dictionary::{Article, Sense},
    reduction, reduction_es_ca,
    translation::Source as Language,
};

use crate::{
    combined::{Found, merged},
    import::Kind,
    index::{Error, Index},
};

/// The exception lemmas of a form, by form: sorted, each once. A form that
/// is not there has none.
pub type Lemmas = BTreeMap<String, Vec<String>>;

/// The headwords to try for `word` read in `language`, in order, each once;
/// empty when nothing of the word is left: the word without the punctuation
/// around it, its lowercase, then the dictionary forms of the lowercase
/// word. `lemmas` holds the exception lemmas of the forms [`asked_forms`]
/// names. No wildcard arm: a new language does not compile until it is
/// given its hook.
pub fn headwords(language: Language, word: &str, lemmas: &Lemmas) -> Vec<String> {
    let written = reduction::stripped(word);
    if written.is_empty() {
        return Vec::new();
    }
    let lowercase = reduction::lowercase(written);
    let of = |form: &str| lemmas.get(form).cloned().unwrap_or_default();
    let forms = match language {
        Language::En => reduction::forms(&lowercase, &of(&lowercase)),
        Language::Es => reduction_es_ca::forms(&lowercase, reduction_es_ca::Language::Es, of),
        Language::Ca => reduction_es_ca::forms(&lowercase, reduction_es_ca::Language::Ca, of),
    };
    let mut candidates = vec![written.to_owned()];
    for candidate in std::iter::once(lowercase).chain(forms) {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

/// The forms whose exception lemmas [`headwords`] reads for `word` read in
/// `language`: the lowercase word, and in Spanish and Catalan its bases as
/// well. Empty when nothing of the word is left. No wildcard arm, as in
/// [`headwords`].
pub fn asked_forms(language: Language, word: &str) -> Vec<String> {
    let written = reduction::stripped(word);
    if written.is_empty() {
        return Vec::new();
    }
    let lowercase = reduction::lowercase(written);
    let bases = match language {
        Language::En => Vec::new(),
        Language::Es => reduction_es_ca::bases(&lowercase, reduction_es_ca::Language::Es),
        Language::Ca => reduction_es_ca::bases(&lowercase, reduction_es_ca::Language::Ca),
    };
    std::iter::once(lowercase).chain(bases).collect()
}

/// The exception lemmas `indexes` give for each of the [`asked_forms`] of
/// `word` read in `language`: their union per form, sorted, each once. An
/// error of an index is the error of the lookup.
fn lemmas_of<'a>(
    indexes: impl IntoIterator<Item = &'a Index> + Clone,
    language: Language,
    word: &str,
) -> Result<Lemmas, Error> {
    let mut lemmas = Lemmas::new();
    for form in asked_forms(language, word) {
        let mut union = Vec::new();
        for index in indexes.clone() {
            union.extend(index.exception_lemmas(&form)?);
        }
        union.sort();
        union.dedup();
        if !union.is_empty() {
            lemmas.insert(form, union);
        }
    }
    Ok(lemmas)
}

/// The article of `word` read in `language` from `index` alone: the senses
/// and translations of every headword [`headwords`] gives that the index
/// has, merged in that order, a sense whose definition is already there
/// skipped; the exception lemmas are those of this index's form table.
/// `None` when the index has none of them. An error of the index is the
/// error of the lookup.
pub fn article(index: &Index, language: Language, word: &str) -> Result<Option<Article>, Error> {
    let lemmas = lemmas_of([index], language, word)?;
    Ok(merged(found(index, language, word, &lemmas)?))
}

/// The headwords of `index` that [`headwords`] gives for `word` read in
/// `language` with `lemmas`, with their articles, in that order.
fn found(
    index: &Index,
    language: Language,
    word: &str,
    lemmas: &Lemmas,
) -> Result<Vec<Found>, Error> {
    let mut found = Vec::new();
    for headword in headwords(language, word, lemmas) {
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
/// grouped by kind in the order of the list. Every index is asked for the
/// same headwords: those [`headwords`] gives with the exception lemmas of
/// all the indexes together, read once. Every sense of every headword found
/// is kept; only the definitions are merged, a definition already there
/// skipped.
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
    let lemmas = lemmas_of(indexes.iter().map(|(_, index)| *index), language, word)?;
    // The word as it may stand among the members of a sense: as written and
    // in lowercase, never its forms, so the members of a form found keep
    // the synonyms of the word.
    let written = reduction::stripped(word);
    let itself = [written.to_owned(), reduction::lowercase(written)];
    for (kind, index) in indexes {
        let found = found(index, language, word, &lemmas)?;
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
                // `Index::find` already leaves each headword out of its own
                // members, so a lemma reached through a form is not listed.
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

    fn lemmas(entries: &[(&str, &[&str])]) -> Lemmas {
        entries
            .iter()
            .map(|(form, lemmas)| ((*form).to_owned(), texts(lemmas)))
            .collect()
    }

    #[test]
    fn english_headwords_are_what_they_were() {
        let went = lemmas(&[("went", &["go"])]);
        let english = headwords(Language::En, "“Went,”", &went);
        assert_eq!(english[..3], texts(&["Went", "went", "go"]));
        assert_eq!(
            headwords(Language::En, "banks", &Lemmas::new()),
            [vec!["banks".to_owned()], reduction::forms("banks", &[])].concat()
        );
        // Only the entry of the lowercase word is read.
        let other = lemmas(&[("Went", &["walk"]), ("wen", &["x"])]);
        assert_eq!(
            headwords(Language::En, "Went", &other),
            [texts(&["Went", "went"]), reduction::forms("went", &[])].concat()
        );
        assert_eq!(asked_forms(Language::En, "“Went,”"), texts(&["went"]));
        assert!(headwords(Language::En, "…", &went).is_empty());
    }

    #[test]
    fn spanish_and_catalan_words_reach_their_forms_after_the_word_and_its_lowercase() {
        let none = Lemmas::new();
        let cerró = headwords(Language::Es, "«Cerró»,", &none);
        assert_eq!(cerró[..2], texts(&["Cerró", "cerró"]));
        assert_eq!(
            cerró[2..],
            reduction_es_ca::forms("cerró", reduction_es_ca::Language::Es, |_| Vec::new())
        );
        assert!(cerró.contains(&"cerrar".to_owned()));
        let dámelo = headwords(Language::Es, "dámelo", &none);
        assert!(dámelo.contains(&"dar".to_owned()), "{dámelo:?}");
        assert_eq!(
            headwords(Language::Ca, "col·leccions", &none)[1],
            "col·lecció"
        );
        assert_eq!(headwords(Language::Ca, "d'art", &none)[1], "art");
        // Typeset text: the word as the app looks it up.
        let typeset = addict_core::sentence::lookup_word("d\u{2019}art");
        assert_eq!(headwords(Language::Ca, &typeset, &none)[1], "art");
        // The exception lemmas of the word come first among the forms.
        let fue = lemmas(&[("fue", &["ir", "ser"])]);
        assert_eq!(
            headwords(Language::Es, "Fue", &fue)[..4],
            texts(&["Fue", "fue", "ir", "ser"])
        );
        // A base's own lemmas follow the base.
        let havia = lemmas(&[("havia", &["haver"])]);
        assert_eq!(
            headwords(Language::Ca, "n'havia", &havia)[..3],
            texts(&["n'havia", "havia", "haver"])
        );
        for language in [Language::Es, Language::Ca] {
            assert!(headwords(language, "…", &none).is_empty());
            assert!(asked_forms(language, "…").is_empty());
        }
        assert_eq!(
            asked_forms(Language::Ca, "D'aigua'l"),
            texts(&["d'aigua'l", "d'aigua", "aigua'l", "aigua"])
        );
        assert_eq!(asked_forms(Language::Es, "bancos"), texts(&["bancos"]));
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
    fn a_spanish_word_reaches_its_lemma_through_its_forms() {
        let dir = TempDir::new("lookup-es");
        let path = dir.path().join("es.index");
        std::fs::write(&path, encode(&spanish()).expect("encode")).expect("write");
        let index = Index::open(&path).expect("open");
        let banco_senses = vec![
            Sense {
                definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                synonyms: Vec::new(),
            },
            Sense {
                definition: "Mueble alargado para sentarse.".to_owned(),
                synonyms: texts(&["banca"]),
            },
        ];
        let banco = article(&index, Language::Es, "Banco").expect("find");
        assert_eq!(
            banco,
            Some(Article {
                senses: banco_senses.clone(),
                translations: Vec::new(),
            })
        );
        // `bancos` itself first, then `banco`, which the form table and the
        // `-s` rule both give.
        let bancos = article(&index, Language::Es, "bancos")
            .expect("find")
            .expect("bancos");
        assert_eq!(bancos.senses[0].definition, "Plural de banco.");
        assert_eq!(bancos.senses[1..], banco_senses[..]);
        assert_eq!(article(&index, Language::Es, "zzz").expect("find"), None);
    }

    /// One headword of a canned index: the word, its senses as a
    /// definition and its members, and its translations.
    type Entry<'a> = (&'a str, &'a [(&'a str, &'a [&'a str])], &'a [&'a str]);

    /// Writes an index of `entries` named `name` into `dir` and opens it.
    fn canned(dir: &TempDir, name: &str, entries: &[Entry<'_>]) -> Index {
        canned_with_forms(dir, name, entries, &[])
    }

    /// [`canned`] with a form table of `forms`, each a form and its lemmas.
    fn canned_with_forms(
        dir: &TempDir,
        name: &str,
        entries: &[Entry<'_>],
        forms: &[(&str, &[&str])],
    ) -> Index {
        let mut forms = forms.to_vec();
        forms.sort_by_key(|(form, _)| *form);
        let forms = forms
            .into_iter()
            .map(|(form, lemmas)| ExceptionForm {
                form: form.to_owned(),
                lemmas: texts(lemmas),
            })
            .collect();
        let mut entries = entries.to_vec();
        entries.sort_by_key(|(word, _, _)| *word);
        let mut contents = Contents {
            source: name.to_owned(),
            senses: Vec::new(),
            headwords: Vec::new(),
            forms,
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
    fn a_spanish_verb_form_reaches_the_russian_of_its_infinitive() {
        let dir = TempDir::new("grouped-cerro");
        let russian = canned(&dir, "es-ru.index", &[("cerrar", &[], &["закрыть"])]);
        let cerró = grouped(&[(Kind::Russian, &russian)], Language::Es, "cerró")
            .expect("look up")
            .expect("cerró");
        assert_eq!(cerró.russian, Some(texts(&["закрыть"])));
    }

    #[test]
    fn the_form_table_of_one_index_serves_every_index_of_the_language() {
        let dir = TempDir::new("grouped-fue");
        // WikDict has no form table; the Kaikki index lists `fue`.
        let russian = canned(
            &dir,
            "es-ru.index",
            &[("ir", &[], &["идти"]), ("ser", &[], &["быть"])],
        );
        let definitions = canned_with_forms(
            &dir,
            "es-def.index",
            &[("ser", &[("Existir.", &[])], &[])],
            &[("fue", &["ser", "ir"])],
        );
        let indexes = [(Kind::Russian, &russian), (Kind::Definitions, &definitions)];
        let fue = grouped(&indexes, Language::Es, "fue")
            .expect("look up")
            .expect("fue");
        // The lemmas sorted: `ir`, then `ser`.
        assert_eq!(fue.russian, Some(texts(&["идти", "быть"])));
        assert_eq!(
            fue.definitions.expect("definitions")[0].definition,
            "Existir."
        );
        // One index alone reads only its own form table.
        assert_eq!(article(&russian, Language::Es, "fue").expect("find"), None);
    }

    #[test]
    fn a_capitalised_word_is_tried_as_written_then_in_lowercase_then_by_its_forms() {
        let dir = TempDir::new("grouped-order");
        let definitions = canned(
            &dir,
            "es-def.index",
            &[
                ("Cerró", &[("As written.", &[])], &[]),
                ("cerrar", &[("The infinitive.", &[])], &[]),
                ("cerró", &[("In lowercase.", &[])], &[]),
            ],
        );
        let found = grouped(&[(Kind::Definitions, &definitions)], Language::Es, "Cerró")
            .expect("look up")
            .expect("Cerró");
        let order: Vec<_> = found
            .definitions
            .expect("definitions")
            .into_iter()
            .map(|sense| sense.definition)
            .collect();
        assert_eq!(
            order,
            texts(&["As written.", "In lowercase.", "The infinitive."])
        );
    }

    #[test]
    fn a_synonym_that_is_also_a_form_of_the_word_is_kept() {
        let dir = TempDir::new("grouped-own-synonyms");
        // `banco` is a rule form of `banca` (a -> o), and its synonym too.
        let synonyms = canned(
            &dir,
            "es-syn.index",
            &[("banca", &[("", &["banca", "banco", "asiento"])], &[])],
        );
        let banca = grouped(&[(Kind::Synonyms, &synonyms)], Language::Es, "Banca")
            .expect("look up")
            .expect("banca");
        assert_eq!(banca.synonyms, Some(texts(&["banco", "asiento"])));
    }

    #[test]
    fn a_lemma_reached_through_a_form_is_not_its_own_synonym() {
        let dir = TempDir::new("grouped-lemma-synonym");
        let synonyms = canned(
            &dir,
            "es-syn.index",
            &[("banco", &[("", &["banco", "entidad", "asiento"])], &[])],
        );
        let bancos = grouped(&[(Kind::Synonyms, &synonyms)], Language::Es, "bancos")
            .expect("look up")
            .expect("bancos");
        let list = bancos.synonyms.expect("synonyms");
        assert!(!list.contains(&"banco".to_owned()), "{list:?}");
        assert_eq!(list, texts(&["entidad", "asiento"]));
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
