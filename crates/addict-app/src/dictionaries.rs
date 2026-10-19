//! The imported indexes as the application's source of articles: the two
//! English index files and those of the other languages in the data
//! directory, opened when they are first asked and read on every lookup.
//!
//! [`Dictionaries`] opens what is not open on each call, so an index imported
//! while the application runs is found by the next lookup; an index imported
//! again over an open one is seen after a restart. An open index that could
//! not be read is closed, so that importing it again is found as well. One
//! index is enough: the
//! lookup answers from what there is. Each call also says where the
//! indexes stand, as [`Indexes`], for the line the window shows about the
//! English ones and for the choice of whether a Spanish or Catalan word is
//! looked up.
//!
//! A word is looked up in the indexes of its reading language
//! ([`Articles::look_up_in`]): an English word in the two English ones, as
//! [`Combined`] answers, a Spanish or Catalan word in every index of that
//! language among the sources of `import::KNOWN`, each under its kind,
//! through `addict_dictionary::lookup::grouped`. A word the chosen language
//! has no entry for is looked up in the other languages, in the order EN,
//! ES, CA, and the first that has it answers
//! ([`Articles::look_up_first`]); the chosen language stays the user's.
//!
//! Everything here opens or reads files, so it is called on the work runner
//! and never on the UI thread. Starting a lookup there is
//! [`manual`](crate::manual)'s; the texts are [`view`](crate::view)'s.

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use addict_core::{dictionary::Article, translation::Source as Language};
use addict_dictionary::{
    Combined, Error, Index,
    import::{self, Kind, Source},
    lookup::{self, Grouped},
};

/// One index of the data directory, by the names the user sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexFile {
    /// The name of the index file, such as `oewn.index`.
    pub index: &'static str,
    /// The name the dictionary file it is imported from is published under.
    pub published: &'static str,
}

impl IndexFile {
    pub(crate) fn of(source: Source) -> Self {
        Self {
            index: source.index_name(),
            published: import::KNOWN
                .iter()
                .find(|known| known.source == source)
                .map_or("", |known| known.name),
        }
    }
}

/// Where one index stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The index is open and lookups read it.
    Open,
    /// There is no index file: its dictionary was not imported.
    Missing,
    /// There is a file and it could not be opened as an index.
    Unreadable,
}

/// Where the indexes stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Indexes {
    /// The index of the English definitions and synonyms.
    pub definitions: State,
    /// The index of the Russian translations.
    pub translations: State,
    /// The Spanish indexes: open when any is open, else unreadable when any
    /// could not be opened, else missing. They are not among
    /// [`Indexes::files`], so the line of the English indexes does not
    /// change with them.
    pub spanish: State,
    /// The Catalan indexes, as the Spanish ones.
    pub catalan: State,
}

impl Indexes {
    /// Both English indexes are open, and no Spanish or Catalan one is
    /// imported.
    pub const OPEN: Self = Self {
        definitions: State::Open,
        translations: State::Open,
        spanish: State::Missing,
        catalan: State::Missing,
    };

    /// Where the indexes of the offline dictionary of `language` stand, for
    /// the languages beside English; `None` for English, whose two indexes
    /// are [`Indexes::files`]. No wildcard arm: a new language does not
    /// compile until it is given its indexes.
    pub fn of_language(self, language: Language) -> Option<State> {
        match language {
            Language::Es => Some(self.spanish),
            Language::Ca => Some(self.catalan),
            Language::En => None,
        }
    }

    /// The two English indexes with their names, the definitions first.
    pub fn files(self) -> [(IndexFile, State); 2] {
        [
            (IndexFile::of(Source::Oewn), self.definitions),
            (IndexFile::of(Source::Freedict), self.translations),
        ]
    }
}

/// What the indexes say about one word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The article of the word, from the English indexes that are open.
    Article(Article),
    /// What the open indexes of a language beside English give for the
    /// word, by kind; one of them at least has it.
    Grouped(Grouped),
    /// An index is open and none has the word or a dictionary form of it.
    NoEntry,
    /// Neither dictionary is imported: there is nothing to look the word up
    /// in.
    NotImported,
    /// An index could not be read and no article is given: an open one
    /// failed, or none is open and this one is no index.
    Unreadable(IndexFile),
}

/// The answer of one lookup and where the indexes stood when it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Looked {
    pub answer: Answer,
    pub indexes: Indexes,
}

/// A source of articles that can fail. It is shared with the thread that
/// runs the lookups, and every method may read files.
pub trait Articles: Send + Sync {
    /// Where the indexes stand now.
    fn indexes(&self) -> Indexes;

    /// What the source says about `word` read in English.
    fn look_up(&self, word: &str) -> Looked;

    /// What the source says about `word` read in `language`, from the
    /// indexes of that language only. English is [`Articles::look_up`]; a
    /// language with no index is [`Answer::NotImported`]. No wildcard arm:
    /// a new language does not compile until it is given its lookup.
    fn look_up_in(&self, language: Language, word: &str) -> Looked {
        match language {
            Language::En => self.look_up(word),
            Language::Es | Language::Ca => Looked {
                answer: Answer::NotImported,
                indexes: self.indexes(),
            },
        }
    }

    /// What the source says about `word`, read first in the `chosen`
    /// language, with the language that answered. The chosen language
    /// answers whenever it gives anything but [`Answer::NoEntry`]. When it
    /// has no entry, the other languages are asked in the order of
    /// [`FALLBACK`], and the first that gives an article answers; one with
    /// no index, no entry or an unreadable file is passed over. When none
    /// has the word, the chosen language's answer is given, with where the
    /// indexes stood at the last lookup.
    fn look_up_first(&self, chosen: Language, word: &str) -> (Language, Looked) {
        let mut first = self.look_up_in(chosen, word);
        if first.answer != Answer::NoEntry {
            return (chosen, first);
        }
        for language in FALLBACK.into_iter().filter(|other| *other != chosen) {
            let looked = self.look_up_in(language, word);
            match looked.answer {
                Answer::Article(_) | Answer::Grouped(_) => return (language, looked),
                Answer::NoEntry | Answer::NotImported | Answer::Unreadable(_) => {
                    first.indexes = looked.indexes;
                }
            }
        }
        (chosen, first)
    }
}

/// The order in which the languages are asked for a word the chosen one has
/// no entry for.
pub const FALLBACK: [Language; 3] = [Language::En, Language::Es, Language::Ca];

/// The indexes opened so far.
#[derive(Debug, Default)]
struct Opened {
    english: Combined,
    /// The open indexes of the languages beside English, by source.
    others: Vec<(Source, Index)>,
    /// The sources beside English whose file is there and could not be
    /// opened by the last check, or failed a lookup since.
    unreadable: Vec<Source>,
}

impl Opened {
    /// Where the indexes of `language`, not English, stand: open when any is
    /// open, else unreadable when any is, else missing.
    fn state_of(&self, language: Language) -> State {
        if self
            .others
            .iter()
            .any(|(source, _)| source.language() == language)
        {
            State::Open
        } else if self
            .unreadable
            .iter()
            .any(|source| source.language() == language)
        {
            State::Unreadable
        } else {
            State::Missing
        }
    }

    /// `indexes` with the states of the languages beside English as they
    /// stand now.
    fn languages(&self, indexes: Indexes) -> Indexes {
        Indexes {
            spanish: self.state_of(Language::Es),
            catalan: self.state_of(Language::Ca),
            ..indexes
        }
    }
}

/// The known sources of the languages beside English, each once, in the
/// order of `import::KNOWN`.
fn other_sources() -> Vec<Source> {
    let mut sources = Vec::new();
    for known in import::KNOWN {
        if known.source.language() != Language::En && !sources.contains(&known.source) {
            sources.push(known.source);
        }
    }
    sources
}

/// The path an index error is about.
fn error_path(error: &Error) -> &Path {
    let (Error::NotImported(path) | Error::Io { path, .. } | Error::Damaged { path, .. }) = error;
    path
}

/// The indexes of one data directory.
#[derive(Debug)]
pub struct Dictionaries {
    /// The data directory; `None` when it cannot be found, and then nothing
    /// is imported.
    dir: Option<PathBuf>,
    /// The indexes opened so far. Only the runner's thread takes the lock.
    open: Mutex<Opened>,
}

impl Dictionaries {
    /// The indexes of the product's data directory. Nothing is opened yet.
    pub fn in_data_dir() -> Self {
        Self::new(import::data_dir())
    }

    /// The indexes of the data directory `dir`. Nothing is opened yet.
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            open: Mutex::default(),
        }
    }

    fn path(&self, source: Source) -> Option<PathBuf> {
        self.dir
            .as_deref()
            .map(|dir| import::index_path(dir, source))
    }

    /// Opens the index of `source` when `opened` says it is not open yet, and
    /// says where it stands.
    fn open(&self, source: Source, opened: bool, set: impl FnOnce(Index)) -> State {
        if opened {
            return State::Open;
        }
        let Some(path) = self.path(source) else {
            return State::Missing;
        };
        match Index::open(&path) {
            Ok(index) => {
                set(index);
                State::Open
            }
            Err(Error::NotImported(_)) => State::Missing,
            Err(Error::Io { .. } | Error::Damaged { .. }) => State::Unreadable,
        }
    }

    /// Opens what is not open in `opened` and says where all stand.
    fn open_missing(&self, opened: &mut Opened) -> Indexes {
        opened.unreadable.clear();
        for source in other_sources() {
            let open = opened.others.iter().any(|(kept, _)| *kept == source);
            let mut index = None;
            if self.open(source, open, |opened| index = Some(opened)) == State::Unreadable {
                opened.unreadable.push(source);
            }
            if let Some(index) = index {
                opened.others.push((source, index));
            }
        }
        let combined = &mut opened.english;
        let definitions = combined.definitions().is_some();
        let definitions = self.open(Source::Oewn, definitions, |index| {
            combined.set_definitions(index);
        });
        let translations = combined.translations().is_some();
        let translations = self.open(Source::Freedict, translations, |index| {
            combined.set_translations(index);
        });
        opened.languages(Indexes {
            definitions,
            translations,
            ..Indexes::OPEN
        })
    }

    /// The English source of the index `error` is about.
    fn source_of(&self, error: &Error) -> Source {
        let translations = self.path(Source::Freedict);
        if translations.as_deref() == Some(error_path(error)) {
            Source::Freedict
        } else {
            Source::Oewn
        }
    }

    /// What the indexes of `language`, not English, say about `word`: what
    /// each open one gives, by its kind, with no English index asked. A file
    /// of the language that is there and is no index is named, and no
    /// article is given, so that it is imported again. An index that could
    /// not be read is closed and opened again by the next call, as an
    /// English one is.
    fn look_up_language(&self, language: Language, word: &str) -> Looked {
        let mut opened = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let indexes = self.open_missing(&mut opened);
        if let Some(source) = opened
            .unreadable
            .iter()
            .find(|source| source.language() == language)
        {
            return Looked {
                answer: Answer::Unreadable(IndexFile::of(*source)),
                indexes,
            };
        }
        // In the order of `import::KNOWN`, whatever order they were opened in.
        let asked: Vec<(Kind, &Index)> = other_sources()
            .into_iter()
            .filter(|source| source.language() == language)
            .filter_map(|source| {
                let (_, index) = opened.others.iter().find(|(kept, _)| *kept == source)?;
                Some((source.kind(), index))
            })
            .collect();
        if asked.is_empty() {
            return Looked {
                answer: Answer::NotImported,
                indexes,
            };
        }
        let answer = match lookup::grouped(&asked, language, word) {
            Ok(Some(grouped)) => Answer::Grouped(grouped),
            Ok(None) => Answer::NoEntry,
            Err(error) => {
                let failed = other_sources().into_iter().find(|source| {
                    source.language() == language
                        && self.path(*source).as_deref() == Some(error_path(&error))
                });
                // Every index has the path of its source, so one is found.
                let source = failed.unwrap_or(asked_first(&opened, language));
                opened.others.retain(|(kept, _)| *kept != source);
                opened.unreadable.push(source);
                Answer::Unreadable(IndexFile::of(source))
            }
        };
        Looked {
            answer,
            indexes: opened.languages(indexes),
        }
    }
}

/// The first open source of `language`, the one to name when an error
/// cannot be told apart.
fn asked_first(opened: &Opened, language: Language) -> Source {
    opened
        .others
        .iter()
        .map(|(source, _)| *source)
        .find(|source| source.language() == language)
        .unwrap_or(Source::KaikkiEs)
}

impl Articles for Dictionaries {
    fn indexes(&self) -> Indexes {
        // A panic of an earlier lookup leaves the opened indexes as they were.
        let mut opened = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        self.open_missing(&mut opened)
    }

    fn look_up_in(&self, language: Language, word: &str) -> Looked {
        match language {
            Language::En => self.look_up(word),
            Language::Es | Language::Ca => self.look_up_language(language, word),
        }
    }

    fn look_up(&self, word: &str) -> Looked {
        let mut opened = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let mut indexes = self.open_missing(&mut opened);
        let combined = &mut opened.english;
        let [(definitions, _), (translations, _)] = indexes.files();
        let answer = match (indexes.definitions, indexes.translations) {
            (State::Open, _) | (_, State::Open) => match combined.article(word) {
                Ok(Some(article)) => Answer::Article(article),
                Ok(None) => Answer::NoEntry,
                Err(error) => {
                    // Both are closed and opened again by the next call: the
                    // file may be another one by then, imported again.
                    *combined = Combined::default();
                    match self.source_of(&error) {
                        // Only an English source is named here.
                        Source::Oewn
                        | Source::KaikkiEs
                        | Source::WikdictEsRu
                        | Source::OmwEs
                        | Source::KaikkiEnSpanish
                        | Source::WikdictCaRu
                        | Source::DbnaryCa
                        | Source::SinonimsCat
                        | Source::KaikkiEnCatalan => {
                            indexes.definitions = State::Unreadable;
                            Answer::Unreadable(definitions)
                        }
                        Source::Freedict => {
                            indexes.translations = State::Unreadable;
                            Answer::Unreadable(translations)
                        }
                    }
                }
            },
            // Nothing is open. A file that is there and is no index is the
            // cause to name, before one that was never imported.
            (State::Unreadable, _) => Answer::Unreadable(definitions),
            (_, State::Unreadable) => Answer::Unreadable(translations),
            (State::Missing, State::Missing) => Answer::NotImported,
        };
        Looked { answer, indexes }
    }
}

#[cfg(test)]
pub mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicU32, Ordering},
    };

    use addict_core::{
        dictionary::{Article, Dictionary, Sample, Sense},
        translation::Source as Language,
    };
    use addict_dictionary::{
        format::{Contents, ExceptionForm, Headword, SenseRecord, encode},
        import::{self, Source},
    };

    use super::{
        Answer, Articles, Dictionaries, Grouped, IndexFile, Indexes, Kind, Looked, State,
        other_sources,
    };

    /// The sample dictionary as a source of articles with both indexes open.
    pub struct Sampled;

    impl Articles for Sampled {
        fn indexes(&self) -> Indexes {
            Indexes::OPEN
        }

        fn look_up(&self, word: &str) -> Looked {
            Looked {
                answer: Sample.lookup(word).map_or(Answer::NoEntry, Answer::Article),
                indexes: Indexes::OPEN,
            }
        }
    }

    pub const OEWN: IndexFile = IndexFile {
        index: "oewn.index",
        published: "english-wordnet-2025.xml.gz",
    };

    pub const FREEDICT: IndexFile = IndexFile {
        index: "freedict.index",
        published: "freedict-eng-rus-2025.11.23.src.tar.xz",
    };

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A data directory of one test, removed when the test ends.
    struct DataDir(PathBuf);

    impl DataDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "addict-dictionaries-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir(&path).expect("create the data directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn index(&self, source: Source) -> PathBuf {
            import::index_path(&self.0, source)
        }

        /// Writes the index of `source`, as an import does.
        fn import(&self, source: Source) {
            let contents = match source {
                Source::Oewn => english(),
                Source::Freedict => russian(),
                Source::KaikkiEs => spanish(),
                Source::WikdictEsRu => headwords("es-ru", &[("banco", &[], &["банк"])]),
                Source::OmwEs => {
                    headwords("es-syn", &[("banco", &[("", &["banco", "banca"])], &[])])
                }
                Source::KaikkiEnSpanish => headwords("es-en", &[("banco", &[("bank", &[])], &[])]),
                Source::WikdictCaRu => headwords(
                    "ca-ru",
                    &[
                        ("col·lecció", &[], &["коллекция"]),
                        ("banc", &[], &["скамья"]),
                    ],
                ),
                Source::DbnaryCa => headwords(
                    "ca-def",
                    &[("col·lecció", &[("Conjunt d'objectes reunits.", &[])], &[])],
                ),
                Source::SinonimsCat => headwords(
                    "ca-syn",
                    &[(
                        "col·lecció",
                        &[("", &["recull", "aplec"]), ("", &["sèrie", "recull"])],
                        &[],
                    )],
                ),
                Source::KaikkiEnCatalan => headwords(
                    "ca-en",
                    &[("col·lecció", &[("collection", &[]), ("set", &[])], &[])],
                ),
            };
            std::fs::write(self.index(source), encode(&contents).expect("encode"))
                .expect("write the index");
        }

        /// Writes `contents` as the index of `source`.
        fn write(&self, source: Source, contents: &Contents) {
            std::fs::write(self.index(source), encode(contents).expect("encode"))
                .expect("write the index");
        }

        fn dictionaries(&self) -> Dictionaries {
            Dictionaries::new(Some(self.0.clone()))
        }
    }

    impl Drop for DataDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn texts(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    /// What an Open English WordNet import writes, in small.
    fn english() -> Contents {
        Contents {
            source: "english-1".to_owned(),
            senses: vec![
                SenseRecord {
                    definition: "sloping land beside a body of water".to_owned(),
                    members: texts(&["bank", "shore"]),
                },
                SenseRecord {
                    definition: "change location".to_owned(),
                    members: texts(&["go"]),
                },
            ],
            headwords: vec![
                Headword {
                    word: "bank".to_owned(),
                    senses: vec![0],
                    translations: Vec::new(),
                },
                Headword {
                    word: "go".to_owned(),
                    senses: vec![1],
                    translations: Vec::new(),
                },
            ],
            forms: vec![ExceptionForm {
                form: "went".to_owned(),
                lemmas: texts(&["go"]),
            }],
        }
    }

    /// What a FreeDict import writes, in small.
    fn russian() -> Contents {
        Contents {
            source: "russian-1".to_owned(),
            senses: vec![SenseRecord {
                definition: "An edge of river, lake, or other watercourse.".to_owned(),
                members: Vec::new(),
            }],
            headwords: vec![
                Headword {
                    word: "bank".to_owned(),
                    senses: vec![0],
                    translations: texts(&["берег", "банк"]),
                },
                Headword {
                    word: "go".to_owned(),
                    senses: Vec::new(),
                    translations: texts(&["идти"]),
                },
            ],
            forms: Vec::new(),
        }
    }

    /// What a `kaikki-es-Espanol` import writes, in small: `bank` is a
    /// Spanish word too here, to show that no English article is read.
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
                    definition: "Una palabra inventada.".to_owned(),
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
                    word: "bank".to_owned(),
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

    /// What the Spanish index alone gives for `banco`: its definitions, and
    /// no index of another kind asked.
    fn spanish_banco() -> Answer {
        Answer::Grouped(Grouped {
            definitions: Some(vec![
                Sense {
                    definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                    synonyms: Vec::new(),
                },
                Sense {
                    definition: "Mueble alargado para sentarse.".to_owned(),
                    synonyms: texts(&["banca"]),
                },
            ]),
            russian: None,
            synonyms: None,
            english: None,
        })
    }

    const SPANISH: IndexFile = IndexFile {
        index: "kaikki-es-Espanol.index",
        published: "kaikki.org-dictionary-Español.jsonl.gz",
    };

    #[test]
    fn a_spanish_word_is_read_from_the_spanish_index_alone() {
        let dir = DataDir::new("spanish");
        dir.import(Source::Oewn);
        dir.import(Source::Freedict);
        dir.import(Source::KaikkiEs);
        let dictionaries = dir.dictionaries();
        let all = Indexes {
            spanish: State::Open,
            ..Indexes::OPEN
        };
        assert_eq!(dictionaries.indexes(), all);
        // The English line names the same files as before.
        assert_eq!(all.files(), Indexes::OPEN.files());
        for word in ["banco", "Banco", "«banco»,"] {
            let looked = dictionaries.look_up_in(Language::Es, word);
            assert_eq!(looked.answer, spanish_banco(), "{word}");
            assert_eq!(looked.indexes, all, "{word}");
        }
        // A form reaches its headword: `bancos` gives the article of `banco`.
        assert_eq!(
            dictionaries.look_up_in(Language::Es, "bancos").answer,
            spanish_banco()
        );
        assert_eq!(
            dictionaries.look_up_in(Language::Es, "zzz").answer,
            Answer::NoEntry
        );
        // A word of both languages: the Spanish article in Spanish, the
        // English one in English, and no fallback either way.
        let Answer::Grouped(bank) = dictionaries.look_up_in(Language::Es, "bank").answer else {
            panic!("bank");
        };
        assert_eq!(
            bank.definitions.expect("definitions")[0].definition,
            "Una palabra inventada."
        );
        assert_eq!(bank.russian, None);
        let english = dictionaries.look_up_in(Language::En, "bank");
        assert_eq!(english.answer, dictionaries.look_up("bank").answer);
        let Answer::Article(english) = english.answer else {
            panic!("bank");
        };
        assert_eq!(english.senses, english_bank());
        assert_eq!(
            dictionaries.look_up_in(Language::En, "banco").answer,
            Answer::NoEntry
        );
        // Catalan has no index.
        let catalan = dictionaries.look_up_in(Language::Ca, "banco");
        assert_eq!(catalan.answer, Answer::NotImported);
        assert_eq!(catalan.indexes.catalan, State::Missing);
    }

    #[test]
    fn without_a_catalan_index_a_catalan_word_has_nothing_to_be_looked_up_in() {
        let dir = DataDir::new("no-catalan");
        dir.import(Source::Oewn);
        dir.import(Source::Freedict);
        dir.import(Source::KaikkiEs);
        let dictionaries = dir.dictionaries();
        assert_eq!(dictionaries.indexes().catalan, State::Missing);
        assert_eq!(
            dictionaries.indexes().of_language(Language::Ca),
            Some(State::Missing)
        );
        // `banco` is in the Spanish index, and is not read for Catalan.
        for word in ["banco", "bank", "col·lecció"] {
            let looked = dictionaries.look_up_in(Language::Ca, word);
            assert_eq!(looked.answer, Answer::NotImported, "{word}");
            assert_eq!(looked.indexes.catalan, State::Missing, "{word}");
            assert_eq!(looked.indexes.spanish, State::Open, "{word}");
        }
        // The Catalan sources are known, and none is imported.
        assert!(
            other_sources()
                .iter()
                .any(|source| source.language() == Language::Ca)
        );
    }

    #[test]
    fn without_the_spanish_index_a_spanish_word_has_nothing_to_be_looked_up_in() {
        let dir = DataDir::new("no-spanish");
        dir.import(Source::Oewn);
        dir.import(Source::Freedict);
        let dictionaries = dir.dictionaries();
        assert_eq!(dictionaries.indexes(), Indexes::OPEN);
        let looked = dictionaries.look_up_in(Language::Es, "bank");
        assert_eq!(looked.answer, Answer::NotImported);
        assert_eq!(looked.indexes, Indexes::OPEN);
        // Imported while resident: found by the next call.
        dir.import(Source::KaikkiEs);
        assert_eq!(dictionaries.indexes().spanish, State::Open);
        assert_eq!(
            dictionaries.look_up_in(Language::Es, "banco").answer,
            spanish_banco()
        );
        // Removed: the open file is still read until a restart, and a new
        // `Dictionaries`, as after a restart, finds none.
        std::fs::remove_file(dir.index(Source::KaikkiEs)).expect("remove");
        assert_eq!(
            dictionaries.look_up_in(Language::Es, "banco").answer,
            spanish_banco()
        );
        assert_eq!(dir.dictionaries().indexes().spanish, State::Missing);
        assert_eq!(
            dir.dictionaries().look_up_in(Language::Es, "banco").answer,
            Answer::NotImported
        );
    }

    #[test]
    fn a_damaged_spanish_index_is_named_closed_and_opened_again() {
        let dir = DataDir::new("spanish-damaged");
        std::fs::write(dir.index(Source::KaikkiEs), b"not an index").expect("write");
        let dictionaries = dir.dictionaries();
        assert_eq!(dictionaries.indexes().spanish, State::Unreadable);
        let looked = dictionaries.look_up_in(Language::Es, "banco");
        assert_eq!(looked.answer, Answer::Unreadable(SPANISH));
        assert_eq!(looked.indexes.spanish, State::Unreadable);
        // The English line is not about it.
        assert!(
            looked
                .indexes
                .files()
                .iter()
                .all(|(file, _)| *file != SPANISH)
        );

        dir.import(Source::KaikkiEs);
        assert_eq!(
            dictionaries.look_up_in(Language::Es, "banco").answer,
            spanish_banco()
        );
        // Broken while open: named, closed, and read again once imported.
        std::fs::write(dir.index(Source::KaikkiEs), b"").expect("truncate");
        let looked = dictionaries.look_up_in(Language::Es, "banco");
        assert_eq!(looked.answer, Answer::Unreadable(SPANISH));
        assert_eq!(looked.indexes.spanish, State::Unreadable);
        let old = dir.path().join("old.index");
        std::fs::rename(dir.index(Source::KaikkiEs), &old).expect("rename");
        dir.import(Source::KaikkiEs);
        let looked = dictionaries.look_up_in(Language::Es, "banco");
        assert_eq!(looked.answer, spanish_banco());
        assert_eq!(looked.indexes.spanish, State::Open);
    }

    #[test]
    fn the_indexes_of_a_language_are_its_own() {
        let indexes = Indexes {
            spanish: State::Unreadable,
            catalan: State::Open,
            ..Indexes::OPEN
        };
        assert_eq!(indexes.of_language(Language::Es), Some(State::Unreadable));
        assert_eq!(indexes.of_language(Language::Ca), Some(State::Open));
        assert_eq!(indexes.of_language(Language::En), None);
        assert_eq!(indexes.files(), Indexes::OPEN.files());
        assert_eq!(IndexFile::of(Source::KaikkiEs), SPANISH);
    }

    fn english_bank() -> Vec<Sense> {
        vec![Sense {
            definition: "sloping land beside a body of water".to_owned(),
            synonyms: texts(&["shore"]),
        }]
    }

    fn russian_bank() -> Article {
        Article {
            senses: vec![Sense {
                definition: "An edge of river, lake, or other watercourse.".to_owned(),
                synonyms: Vec::new(),
            }],
            translations: texts(&["берег", "банк"]),
        }
    }

    const NEITHER: Indexes = Indexes {
        definitions: State::Missing,
        translations: State::Missing,
        spanish: State::Missing,
        catalan: State::Missing,
    };

    #[test]
    fn the_files_have_the_names_the_user_sees() {
        let [definitions, translations] = Indexes::OPEN.files();
        assert_eq!(definitions, (OEWN, State::Open));
        assert_eq!(translations, (FREEDICT, State::Open));
    }

    #[test]
    fn both_indexes_give_one_article_also_for_an_inflected_word() {
        let dir = DataDir::new("both");
        dir.import(Source::Oewn);
        dir.import(Source::Freedict);
        let dictionaries = dir.dictionaries();
        assert_eq!(dictionaries.indexes(), Indexes::OPEN);
        let bank = Article {
            senses: english_bank(),
            translations: texts(&["берег", "банк"]),
        };
        for word in ["bank", "banks", "Bank"] {
            let looked = dictionaries.look_up(word);
            assert_eq!(looked.answer, Answer::Article(bank.clone()), "{word}");
            assert_eq!(looked.indexes, Indexes::OPEN, "{word}");
        }
        let Answer::Article(went) = dictionaries.look_up("went").answer else {
            panic!("went");
        };
        assert_eq!(went.senses[0].definition, "change location");
        assert_eq!(went.translations, texts(&["идти"]));
    }

    #[test]
    fn a_word_in_neither_index_has_no_entry() {
        let dir = DataDir::new("no-entry");
        dir.import(Source::Oewn);
        dir.import(Source::Freedict);
        let looked = dir.dictionaries().look_up("xyzzy");
        assert_eq!(looked.answer, Answer::NoEntry);
        assert_eq!(looked.indexes, Indexes::OPEN);
    }

    #[test]
    fn with_nothing_imported_there_is_nothing_to_look_a_word_up_in() {
        let dir = DataDir::new("nothing");
        // An empty data directory, one that does not exist and none at all.
        for dictionaries in [
            dir.dictionaries(),
            Dictionaries::new(Some(dir.path().join("absent"))),
            Dictionaries::new(None),
        ] {
            assert_eq!(dictionaries.indexes(), NEITHER);
            let looked = dictionaries.look_up("bank");
            assert_eq!(looked.answer, Answer::NotImported);
            assert_eq!(looked.indexes, NEITHER);
        }
        // Nothing was created by asking.
        assert!(!dir.path().join("absent").exists());
    }

    #[test]
    fn the_definitions_alone_give_senses_and_no_translation() {
        let dir = DataDir::new("oewn-only");
        dir.import(Source::Oewn);
        let looked = dir.dictionaries().look_up("bank");
        assert_eq!(
            looked.answer,
            Answer::Article(Article {
                senses: english_bank(),
                translations: Vec::new(),
            })
        );
        assert_eq!(
            looked.indexes,
            Indexes {
                definitions: State::Open,
                translations: State::Missing,
                spanish: State::Missing,
                catalan: State::Missing,
            }
        );
    }

    #[test]
    fn the_translations_alone_give_their_definitions_and_the_suffix_forms() {
        let dir = DataDir::new("freedict-only");
        dir.import(Source::Freedict);
        let dictionaries = dir.dictionaries();
        let alone = Indexes {
            definitions: State::Missing,
            translations: State::Open,
            spanish: State::Missing,
            catalan: State::Missing,
        };
        for word in ["bank", "banks"] {
            let looked = dictionaries.look_up(word);
            assert_eq!(looked.answer, Answer::Article(russian_bank()), "{word}");
            assert_eq!(looked.indexes, alone, "{word}");
        }
        // `went` is `go` only through the exception list of the other index.
        assert_eq!(dictionaries.look_up("went").answer, Answer::NoEntry);
    }

    #[test]
    fn an_index_imported_while_resident_is_found_by_the_next_lookup() {
        let dir = DataDir::new("imported-later");
        let dictionaries = dir.dictionaries();
        assert_eq!(dictionaries.look_up("bank").answer, Answer::NotImported);

        dir.import(Source::Oewn);
        let looked = dictionaries.look_up("bank");
        assert_eq!(
            looked.answer,
            Answer::Article(Article {
                senses: english_bank(),
                translations: Vec::new(),
            })
        );
        assert_eq!(looked.indexes.translations, State::Missing);

        dir.import(Source::Freedict);
        let looked = dictionaries.look_up("bank");
        assert_eq!(
            looked.answer,
            Answer::Article(Article {
                senses: english_bank(),
                translations: texts(&["берег", "банк"]),
            })
        );
        assert_eq!(looked.indexes, Indexes::OPEN);
        // The check alone opens too.
        let dictionaries = dir.dictionaries();
        assert_eq!(dictionaries.indexes(), Indexes::OPEN);
    }

    #[test]
    fn an_index_that_cannot_be_opened_leaves_the_other_one_to_answer() {
        let dir = DataDir::new("damaged-at-open");
        dir.import(Source::Freedict);
        let stands = Indexes {
            definitions: State::Unreadable,
            translations: State::Open,
            spanish: State::Missing,
            catalan: State::Missing,
        };
        // A truncated index, a foreign file and a directory in its place.
        let whole = encode(&english()).expect("encode");
        for damage in [&whole[..whole.len() / 2], b"not an index".as_slice()] {
            std::fs::write(dir.index(Source::Oewn), damage).expect("write");
            let dictionaries = dir.dictionaries();
            assert_eq!(dictionaries.indexes(), stands);
            let looked = dictionaries.look_up("bank");
            assert_eq!(looked.answer, Answer::Article(russian_bank()));
            assert_eq!(looked.indexes, stands);
        }
        std::fs::remove_file(dir.index(Source::Oewn)).expect("remove");
        std::fs::create_dir(dir.index(Source::Oewn)).expect("directory");
        assert_eq!(dir.dictionaries().indexes(), stands);
    }

    #[test]
    fn an_index_imported_again_after_it_could_not_be_opened_is_opened() {
        let dir = DataDir::new("repaired");
        std::fs::write(dir.index(Source::Oewn), b"not an index").expect("write");
        let dictionaries = dir.dictionaries();
        // Nothing is open, and the cause is the file that is no index.
        assert_eq!(
            dictionaries.look_up("bank").answer,
            Answer::Unreadable(OEWN)
        );
        assert_eq!(dictionaries.indexes().definitions, State::Unreadable);
        dir.import(Source::Oewn);
        assert_eq!(dictionaries.indexes().definitions, State::Open);
    }

    #[test]
    fn with_no_index_open_an_unreadable_one_is_the_cause_before_a_missing_one() {
        let dir = DataDir::new("unreadable-and-missing");
        std::fs::write(dir.index(Source::Freedict), b"not an index").expect("write");
        let looked = dir.dictionaries().look_up("bank");
        assert_eq!(looked.answer, Answer::Unreadable(FREEDICT));
        assert_eq!(
            looked.indexes,
            Indexes {
                definitions: State::Missing,
                translations: State::Unreadable,
                spanish: State::Missing,
                catalan: State::Missing,
            }
        );
        std::fs::write(dir.index(Source::Oewn), b"not an index").expect("write");
        assert_eq!(
            dir.dictionaries().look_up("bank").answer,
            Answer::Unreadable(OEWN)
        );
    }

    #[test]
    fn an_open_index_that_cannot_be_read_gives_no_article_and_names_its_file() {
        for (source, file) in [(Source::Oewn, OEWN), (Source::Freedict, FREEDICT)] {
            let dir = DataDir::new("damaged-deeper");
            dir.import(Source::Oewn);
            dir.import(Source::Freedict);
            let dictionaries = dir.dictionaries();
            assert!(matches!(
                dictionaries.look_up("bank").answer,
                Answer::Article(_)
            ));
            // The header was read at open; the records are gone now.
            std::fs::write(dir.index(source), b"").expect("truncate");
            let looked = dictionaries.look_up("bank");
            assert_eq!(looked.answer, Answer::Unreadable(file), "{file:?}");
            // The line of the indexes names it too.
            let [definitions, translations] = looked.indexes.files();
            for (named, state) in [definitions, translations] {
                assert_eq!(state == State::Unreadable, named == file, "{file:?}");
                assert_eq!(state == State::Open, named != file, "{file:?}");
            }
            // Imported again, as the line says, by a rename over the old
            // file: the next lookup reads the new one, with no restart.
            let old = dir.path().join("old.index");
            std::fs::rename(dir.index(source), &old).expect("rename");
            dir.import(source);
            let looked = dictionaries.look_up("bank");
            assert!(matches!(looked.answer, Answer::Article(_)), "{file:?}");
            assert_eq!(looked.indexes, Indexes::OPEN, "{file:?}");
        }
    }

    #[test]
    fn an_index_still_damaged_after_it_failed_leaves_the_other_one_to_answer() {
        let dir = DataDir::new("still-damaged");
        dir.import(Source::Oewn);
        dir.import(Source::Freedict);
        let dictionaries = dir.dictionaries();
        assert_eq!(dictionaries.indexes(), Indexes::OPEN);
        std::fs::write(dir.index(Source::Oewn), b"").expect("truncate");
        assert_eq!(
            dictionaries.look_up("bank").answer,
            Answer::Unreadable(OEWN)
        );
        // Opened again, the empty file is no index: the other one answers.
        let looked = dictionaries.look_up("bank");
        assert_eq!(looked.answer, Answer::Article(russian_bank()));
        assert_eq!(looked.indexes.definitions, State::Unreadable);
    }

    /// One headword of a canned index: the word, its senses as a
    /// definition and its members, and its translations.
    type Canned<'a> = (&'a str, &'a [(&'a str, &'a [&'a str])], &'a [&'a str]);

    /// The contents of an index with the source id `source` and `headwords`,
    /// sorted.
    fn headwords(source: &str, headwords: &[Canned<'_>]) -> Contents {
        let mut contents = Contents {
            source: source.to_owned(),
            senses: Vec::new(),
            headwords: Vec::new(),
            forms: Vec::new(),
        };
        let mut sorted = headwords.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        for (word, senses, translations) in sorted {
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
        contents
    }

    /// Writes `headwords`, each with its senses as a definition and its
    /// members and with its translations, as the index `name` in `dir`, and
    /// opens it.
    fn canned(dir: &DataDir, name: &str, headwords: &[Canned<'_>]) -> addict_dictionary::Index {
        let mut contents = Contents {
            source: name.to_owned(),
            senses: Vec::new(),
            headwords: Vec::new(),
            forms: Vec::new(),
        };
        for (word, senses, translations) in headwords {
            let mut numbers = Vec::new();
            for (definition, members) in *senses {
                numbers.push(contents.senses.len() as u32);
                contents.senses.push(SenseRecord {
                    definition: (*definition).to_owned(),
                    members: texts(members),
                });
            }
            contents.headwords.push(Headword {
                word: (*word).to_owned(),
                senses: numbers,
                translations: texts(translations),
            });
        }
        let path = dir.path().join(name);
        std::fs::write(&path, encode(&contents).expect("encode")).expect("write");
        addict_dictionary::Index::open(&path).expect("open")
    }

    /// The lines of the sections beside the senses of `grouped` read in
    /// `language`: the Russian, the synonyms of the word, the English.
    fn section_lines(language: Language, grouped: Grouped) -> (&'static str, Vec<String>) {
        let lookup = crate::view::Lookup::Grouped { language, grouped };
        let texts = crate::view::result_texts(&lookup, None, None, None);
        let line = |listed: Option<crate::view::Listed<'_>>| match listed {
            Some(crate::view::Listed::Items { label, items }) => {
                format!("{label} {}", items.join(", "))
            }
            Some(crate::view::Listed::Missing(line)) => line.to_owned(),
            None => String::new(),
        };
        (
            texts.senses_label,
            vec![
                line(texts.translations),
                format!("{} senses", texts.senses.len()),
                line(texts.word_synonyms),
                line(texts.glosses),
            ],
        )
    }

    #[test]
    fn canned_indexes_of_every_kind_give_a_labelled_section_each() {
        use addict_dictionary::{import::Kind, lookup::grouped};

        let dir = DataDir::new("kinds");
        let russian = canned(&dir, "es-ru.index", &[("banco", &[], &["банк"])]);
        let definitions = canned(
            &dir,
            "es-def.index",
            &[(
                "banco",
                &[("Mueble alargado para sentarse.", &["banco", "banca"])],
                &[],
            )],
        );
        let synonyms = canned(
            &dir,
            "es-syn.index",
            &[("banco", &[("", &["banco", "entidad"])], &[])],
        );
        let english = canned(&dir, "es-en.index", &[("banco", &[("bench", &[])], &[])]);
        let indexes = [
            (Kind::Russian, &russian),
            (Kind::Definitions, &definitions),
            (Kind::Synonyms, &synonyms),
            (Kind::English, &english),
        ];
        let banco = grouped(&indexes, Language::Es, "banco")
            .expect("look up")
            .expect("banco");
        // The definitions keep their own synonyms.
        let senses = banco.definitions.as_deref().expect("definitions");
        assert_eq!(senses[0].synonyms, texts(&["banca"]));
        assert_eq!(
            section_lines(Language::Es, banco),
            (
                "Spanish definitions",
                texts(&[
                    "Russian, for the word as a whole: банк",
                    "1 senses",
                    "Spanish synonyms of the word entidad",
                    "English glosses bench",
                ])
            )
        );

        // A Catalan definitions index alone: the other kinds are named.
        let catalan = canned(
            &dir,
            "ca-def.index",
            &[("banc", &[("Seient llarg.", &[])], &[])],
        );
        let banc = grouped(&[(Kind::Definitions, &catalan)], Language::Ca, "banc")
            .expect("look up")
            .expect("banc");
        assert_eq!(
            section_lines(Language::Ca, banc),
            (
                "Catalan definitions",
                texts(&[
                    "No Catalan-Russian dictionary is imported.",
                    "1 senses",
                    "No dictionary of Catalan synonyms is imported.",
                    "No Catalan-English dictionary is imported.",
                ])
            )
        );
        // A Russian index without the word says so.
        let mesa = canned(&dir, "es-ru-2.index", &[("mesa", &[], &["стол"])]);
        let found = grouped(
            &[(Kind::Russian, &mesa), (Kind::Definitions, &definitions)],
            Language::Es,
            "banco",
        )
        .expect("look up")
        .expect("banco");
        assert_eq!(
            section_lines(Language::Es, found).1[0],
            "The dictionary has no Russian translation of this word."
        );
    }

    #[test]
    fn a_catalan_word_with_the_four_catalan_indexes_gets_the_four_kinds() {
        let dir = DataDir::new("catalan-kinds");
        dir.import(Source::Oewn);
        dir.import(Source::Freedict);
        for source in import::KNOWN.iter().map(|known| known.source) {
            if source.language() != Language::En {
                dir.import(source);
            }
        }
        let dictionaries = dir.dictionaries();
        let indexes = dictionaries.indexes();
        assert_eq!(indexes.catalan, State::Open);
        assert_eq!(indexes.spanish, State::Open);
        assert_eq!(indexes.files(), Indexes::OPEN.files());
        let catalan: Vec<_> = other_sources()
            .into_iter()
            .filter(|source| source.language() == Language::Ca)
            .map(|source| source.kind())
            .collect();
        assert_eq!(
            catalan,
            [
                Kind::Russian,
                Kind::Definitions,
                Kind::Synonyms,
                Kind::English
            ]
        );

        let looked = dictionaries.look_up_in(Language::Ca, "col·lecció");
        assert_eq!(
            looked.answer,
            Answer::Grouped(Grouped {
                definitions: Some(vec![Sense {
                    definition: "Conjunt d'objectes reunits.".to_owned(),
                    synonyms: Vec::new(),
                }]),
                russian: Some(texts(&["коллекция"])),
                synonyms: Some(texts(&["recull", "aplec", "sèrie"])),
                english: Some(texts(&["collection", "set"])),
            })
        );
        // A word only some indexes have: the others are asked and say so.
        let Answer::Grouped(banc) = dictionaries.look_up_in(Language::Ca, "Banc").answer else {
            panic!("banc");
        };
        assert_eq!(banc.russian, Some(texts(&["скамья"])));
        assert_eq!(banc.definitions, Some(Vec::new()));
        assert_eq!(banc.synonyms, Some(Vec::new()));
        assert_eq!(banc.english, Some(Vec::new()));
        // Spanish reads its own four, and English is unchanged.
        let Answer::Grouped(banco) = dictionaries.look_up_in(Language::Es, "banco").answer else {
            panic!("banco");
        };
        assert_eq!(banco.russian, Some(texts(&["банк"])));
        assert_eq!(banco.synonyms, Some(texts(&["banca"])));
        assert_eq!(banco.english, Some(texts(&["bank"])));
        assert_eq!(
            dictionaries.look_up_in(Language::Ca, "banco").answer,
            Answer::NoEntry
        );
        let Answer::Article(bank) = dictionaries.look_up("bank").answer else {
            panic!("bank");
        };
        assert_eq!(bank.senses, english_bank());
    }

    /// A data directory with the English indexes, a Spanish Russian index
    /// with `cerrar` and `banco`, and, when `catalan`, a Catalan Russian
    /// index with `banco` and `banc`.
    fn three_languages(tag: &str, catalan: bool) -> DataDir {
        let dir = DataDir::new(tag);
        dir.import(Source::Oewn);
        dir.import(Source::Freedict);
        dir.write(
            Source::WikdictEsRu,
            &headwords(
                "es-ru",
                &[("banco", &[], &["банк"]), ("cerrar", &[], &["закрыть"])],
            ),
        );
        if catalan {
            dir.write(
                Source::WikdictCaRu,
                &headwords(
                    "ca-ru",
                    &[("banc", &[], &["скамья"]), ("banco", &[], &["лавка"])],
                ),
            );
        }
        dir
    }

    /// The Russian of a grouped answer.
    fn russian_of(answer: &Answer) -> Option<Vec<String>> {
        match answer {
            Answer::Grouped(grouped) => grouped.russian.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_word_the_chosen_language_lacks_is_answered_by_the_first_that_has_it() {
        let dir = three_languages("fallback", true);
        let dictionaries = dir.dictionaries();
        // `cerró` is Spanish only, through the Spanish dictionary-form rules.
        assert_eq!(
            dictionaries.look_up_in(Language::Ca, "cerró").answer,
            Answer::NoEntry
        );
        let (language, looked) = dictionaries.look_up_first(Language::Ca, "cerró");
        assert_eq!(language, Language::Es);
        assert_eq!(russian_of(&looked.answer), Some(texts(&["закрыть"])));
        assert_eq!(looked, dictionaries.look_up_in(Language::Es, "cerró"));
        // An English word read in Spanish or Catalan: English answers first.
        for chosen in [Language::Es, Language::Ca] {
            let (language, looked) = dictionaries.look_up_first(chosen, "went");
            assert_eq!(language, Language::En, "{chosen:?}");
            assert_eq!(looked, dictionaries.look_up("went"), "{chosen:?}");
        }
        // A Catalan word read in English.
        let (language, looked) = dictionaries.look_up_first(Language::En, "banc");
        assert_eq!(language, Language::Ca);
        assert_eq!(russian_of(&looked.answer), Some(texts(&["скамья"])));
    }

    #[test]
    fn the_chosen_language_answers_a_word_it_has() {
        let dir = three_languages("chosen-wins", true);
        let dictionaries = dir.dictionaries();
        let (language, looked) = dictionaries.look_up_first(Language::Es, "banco");
        assert_eq!(language, Language::Es);
        assert_eq!(russian_of(&looked.answer), Some(texts(&["банк"])));
        let (language, looked) = dictionaries.look_up_first(Language::Ca, "banco");
        assert_eq!(language, Language::Ca);
        assert_eq!(russian_of(&looked.answer), Some(texts(&["лавка"])));
        // English with the word: the same as the English lookup.
        for word in ["bank", "went", "Bank"] {
            assert_eq!(
                dictionaries.look_up_first(Language::En, word),
                (Language::En, dictionaries.look_up(word)),
                "{word}"
            );
        }
    }

    #[test]
    fn a_word_no_language_has_is_the_chosen_language_s_miss() {
        let dir = three_languages("nobody", true);
        let dictionaries = dir.dictionaries();
        for chosen in Language::ALL {
            let (language, looked) = dictionaries.look_up_first(chosen, "zzz");
            assert_eq!(language, chosen);
            assert_eq!(looked.answer, Answer::NoEntry);
            assert_eq!(looked.indexes, dictionaries.indexes());
        }
        // English with no Spanish or Catalan index: as before.
        let dir = DataDir::new("english-only");
        dir.import(Source::Oewn);
        dir.import(Source::Freedict);
        let dictionaries = dir.dictionaries();
        assert_eq!(
            dictionaries.look_up_first(Language::En, "zzz"),
            (Language::En, dictionaries.look_up("zzz"))
        );
    }

    #[test]
    fn a_language_with_no_index_is_passed_over() {
        let dir = DataDir::new("no-spanish-fallback");
        dir.import(Source::Oewn);
        dir.import(Source::Freedict);
        dir.write(
            Source::WikdictCaRu,
            &headwords("ca-ru", &[("banc", &[], &["скамья"])]),
        );
        let dictionaries = dir.dictionaries();
        let (language, looked) = dictionaries.look_up_first(Language::Ca, "cerró");
        assert_eq!(language, Language::Ca);
        assert_eq!(looked.answer, Answer::NoEntry);
        assert_eq!(looked.indexes.spanish, State::Missing);
        // A damaged file of another language is passed over too.
        std::fs::write(dir.index(Source::WikdictEsRu), b"no index").expect("write");
        let (language, looked) = dictionaries.look_up_first(Language::Ca, "cerró");
        assert_eq!(language, Language::Ca);
        assert_eq!(looked.answer, Answer::NoEntry);
        assert_eq!(looked.indexes.spanish, State::Unreadable);
    }

    #[test]
    fn an_unreadable_chosen_language_falls_back_to_none() {
        let dir = three_languages("chosen-unreadable", true);
        std::fs::write(dir.index(Source::KaikkiEs), b"no index").expect("write");
        let dictionaries = dir.dictionaries();
        let (language, looked) = dictionaries.look_up_first(Language::Es, "went");
        assert_eq!(language, Language::Es);
        assert_eq!(looked.answer, Answer::Unreadable(SPANISH));
    }

    #[test]
    fn the_sample_double_answers_as_the_sample_does() {
        assert_eq!(
            Sampled.look_up("bank").answer,
            Answer::Article(Sample.lookup("bank").expect("bank"))
        );
        assert_eq!(Sampled.look_up("sat").answer, Answer::NoEntry);
        assert_eq!(Sampled.indexes(), Indexes::OPEN);
    }
}
