//! The imported indexes as the application's source of articles: the two
//! English index files and the Spanish one of the data directory, opened
//! when they are first asked and read on every lookup.
//!
//! [`Dictionaries`] opens what is not open on each call, so an index imported
//! while the application runs is found by the next lookup; an index imported
//! again over an open one is seen after a restart. An open index that could
//! not be read is closed, so that importing it again is found as well. One
//! index is enough: the
//! lookup answers from what there is. Each call also says where the
//! indexes stand, as [`Indexes`], for the line the window shows about the
//! English ones and for the choice of whether a Spanish word is looked up.
//!
//! A word is looked up in the indexes of its reading language
//! ([`Articles::look_up_in`]): an English word in the two English ones, as
//! [`Combined`] answers, a Spanish word in the Spanish one alone, through
//! the hook of `addict_dictionary::lookup`. No language falls back to
//! another.
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
    import::{self, Source},
    lookup,
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
    /// The index of the Spanish definitions and synonyms. It is not one of
    /// [`Indexes::files`], so the line of the English indexes does not
    /// change with it.
    pub spanish: State,
}

impl Indexes {
    /// Both English indexes are open, and no Spanish one is imported.
    pub const OPEN: Self = Self {
        definitions: State::Open,
        translations: State::Open,
        spanish: State::Missing,
    };

    /// Where the index of the offline dictionary of `language` stands, for
    /// the languages that have one beside English; `None` for English, whose
    /// two indexes are [`Indexes::files`], and for a language with no index
    /// of its own. No wildcard arm: a new language does not compile until it
    /// is given its index.
    pub fn of_language(self, language: Language) -> Option<State> {
        match language {
            Language::Es => Some(self.spanish),
            Language::En | Language::Ca => None,
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
    /// The article of the word, from the indexes that are open.
    Article(Article),
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
}

/// The indexes opened so far.
#[derive(Debug, Default)]
struct Opened {
    english: Combined,
    spanish: Option<Index>,
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
        let spanish = opened.spanish.is_some();
        let spanish = self.open(Source::KaikkiEs, spanish, |index| {
            opened.spanish = Some(index);
        });
        let combined = &mut opened.english;
        let definitions = combined.definitions().is_some();
        let definitions = self.open(Source::Oewn, definitions, |index| {
            combined.set_definitions(index);
        });
        let translations = combined.translations().is_some();
        let translations = self.open(Source::Freedict, translations, |index| {
            combined.set_translations(index);
        });
        Indexes {
            definitions,
            translations,
            spanish,
        }
    }

    /// The English source of the index `error` is about.
    fn source_of(&self, error: &Error) -> Source {
        let (Error::NotImported(path) | Error::Io { path, .. } | Error::Damaged { path, .. }) =
            error;
        let translations = self.path(Source::Freedict);
        if translations.as_deref() == Some(Path::new(path)) {
            Source::Freedict
        } else {
            Source::Oewn
        }
    }

    /// What the Spanish index says about `word`, read in Spanish: the
    /// article of the word as written, with no English index asked. An index
    /// that could not be read is closed and opened again by the next call,
    /// as an English one is.
    fn look_up_spanish(&self, word: &str) -> Looked {
        let mut opened = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let mut indexes = self.open_missing(&mut opened);
        let file = IndexFile::of(Source::KaikkiEs);
        let found = opened
            .spanish
            .as_ref()
            .map(|index| lookup::article(index, Language::Es, word));
        let answer = match (indexes.spanish, found) {
            (_, Some(Ok(Some(article)))) => Answer::Article(article),
            (_, Some(Ok(None))) => Answer::NoEntry,
            (_, Some(Err(_))) => {
                opened.spanish = None;
                indexes.spanish = State::Unreadable;
                Answer::Unreadable(file)
            }
            (State::Unreadable, None) => Answer::Unreadable(file),
            (State::Open | State::Missing, None) => Answer::NotImported,
        };
        Looked { answer, indexes }
    }
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
            Language::Es => self.look_up_spanish(word),
            Language::Ca => Looked {
                answer: Answer::NotImported,
                indexes: self.indexes(),
            },
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
                        Source::Oewn | Source::KaikkiEs => {
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

    use super::{Answer, Articles, Dictionaries, IndexFile, Indexes, Looked, State};

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
            };
            std::fs::write(self.index(source), encode(&contents).expect("encode"))
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

    fn spanish_banco() -> Article {
        Article {
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
        }
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
            assert_eq!(looked.answer, Answer::Article(spanish_banco()), "{word}");
            assert_eq!(looked.indexes, all, "{word}");
        }
        // As written: the form table is not read yet.
        assert_eq!(
            dictionaries.look_up_in(Language::Es, "bancos").answer,
            Answer::NoEntry
        );
        assert_eq!(
            dictionaries.look_up_in(Language::Es, "zzz").answer,
            Answer::NoEntry
        );
        // A word of both languages: the Spanish article in Spanish, the
        // English one in English, and no fallback either way.
        let Answer::Article(bank) = dictionaries.look_up_in(Language::Es, "bank").answer else {
            panic!("bank");
        };
        assert_eq!(bank.senses[0].definition, "Una palabra inventada.");
        assert!(bank.translations.is_empty());
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
        assert_eq!(
            dictionaries.look_up_in(Language::Ca, "banco").answer,
            Answer::NotImported
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
            Answer::Article(spanish_banco())
        );
        // Removed: the open file is still read until a restart, and a new
        // `Dictionaries`, as after a restart, finds none.
        std::fs::remove_file(dir.index(Source::KaikkiEs)).expect("remove");
        assert_eq!(
            dictionaries.look_up_in(Language::Es, "banco").answer,
            Answer::Article(spanish_banco())
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
            Answer::Article(spanish_banco())
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
        assert_eq!(looked.answer, Answer::Article(spanish_banco()));
        assert_eq!(looked.indexes.spanish, State::Open);
    }

    #[test]
    fn the_index_of_a_language_is_the_spanish_one_for_spanish_alone() {
        let indexes = Indexes {
            spanish: State::Unreadable,
            ..Indexes::OPEN
        };
        assert_eq!(indexes.of_language(Language::Es), Some(State::Unreadable));
        assert_eq!(indexes.of_language(Language::En), None);
        assert_eq!(indexes.of_language(Language::Ca), None);
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
