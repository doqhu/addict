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
//! A Russian word is looked up in the one index of Russian headwords
//! translated into the chosen language ([`Articles::look_up_russian`]), and
//! in nothing else: no other language is asked for it, and no other index
//! for a word read in a language. The Russian indexes are opened when they
//! are first asked, and are not among [`Indexes`]: the English line and the
//! states of Spanish and Catalan do not change with them.
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
    /// The translations the Russian index of the chosen language gives for
    /// a Russian word, each once; never empty.
    FromRussian(Vec<String>),
    /// An index is open and none has the word or a dictionary form of it.
    NoEntry,
    /// Neither dictionary is imported: there is nothing to look the word up
    /// in.
    NotImported,
    /// An index could not be read and no article is given: an open one
    /// failed, or none is open and this one is no index.
    Unreadable(IndexFile),
}

impl Answer {
    /// Whether the answer has lexical entries or sense metadata and nothing
    /// the window shows: no definition, synonym, translation, gloss or verb
    /// form that is not blank. Such an answer does not stop
    /// [`Articles::look_up_first`]; an answer with no metadata at all is
    /// never metadata only.
    fn metadata_only(&self) -> bool {
        let text = |value: &String| !value.trim().is_empty();
        let sense = |value: &addict_core::dictionary::Sense| {
            text(&value.definition) || value.synonyms.iter().any(text)
        };
        match self {
            Self::Article(article) => {
                (!article.entries.is_empty() || !article.sense_metadata.is_empty())
                    && !article.senses.iter().any(sense)
                    && !article.translations.iter().any(text)
            }
            Self::Grouped(grouped) => {
                (!grouped.entries.is_empty() || !grouped.sense_metadata.is_empty())
                    && grouped.verb.is_none()
                    && !grouped.definitions.iter().flatten().any(sense)
                    && !grouped.russian.iter().flatten().any(text)
                    && !grouped.english.iter().flatten().any(text)
                    && !grouped
                        .synonyms
                        .iter()
                        .flatten()
                        .any(|group| text(&group.definition) || group.synonyms.iter().any(text))
            }
            Self::FromRussian(_) | Self::NoEntry | Self::NotImported | Self::Unreadable(_) => false,
        }
    }
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

    /// What the source says about the Russian `word` translated into
    /// `into`, from the Russian index of that language alone: no other
    /// index and no other language is asked. [`Answer::NotImported`] when
    /// that index is not there, [`Answer::Unreadable`] naming it when it
    /// cannot be read. A source without Russian indexes has none imported.
    fn look_up_russian(&self, into: Language, word: &str) -> Looked {
        let _ = (into, word);
        Looked {
            answer: Answer::NotImported,
            indexes: self.indexes(),
        }
    }

    /// What the source says about `word`, read first in the `chosen`
    /// language, with the language that answered. The chosen language's
    /// errors and missing indexes come first. A miss or an answer that is
    /// [`Answer::metadata_only`] asks the other languages in [`FALLBACK`]
    /// order for one with content; when none has it, the first metadata-only
    /// answer is given, or the chosen miss, with the latest index states.
    fn look_up_first(&self, chosen: Language, word: &str) -> (Language, Looked) {
        let mut first = self.look_up_in(chosen, word);
        let mut answered = chosen;
        if first.answer != Answer::NoEntry && !first.answer.metadata_only() {
            return (chosen, first);
        }
        for language in FALLBACK.into_iter().filter(|other| *other != chosen) {
            let looked = self.look_up_in(language, word);
            if looked.answer.metadata_only() {
                if first.answer == Answer::NoEntry {
                    first = looked;
                    answered = language;
                } else {
                    first.indexes = looked.indexes;
                }
                continue;
            }
            match looked.answer {
                // A lookup in a language never gives a Russian answer.
                Answer::Article(_) | Answer::Grouped(_) | Answer::FromRussian(_) => {
                    return (language, looked);
                }
                Answer::NoEntry | Answer::NotImported | Answer::Unreadable(_) => {
                    first.indexes = looked.indexes;
                }
            }
        }
        (answered, first)
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
    /// The open indexes of Russian headwords, by source: each is opened by
    /// the first lookup that asks it, and closed when it fails one.
    russian: Vec<(Source, Index)>,
}

impl Opened {
    /// Where the indexes of `language`, not English, stand: open when any is
    /// open, else unreadable when any is, else missing.
    fn state_of(&self, language: Language) -> State {
        if self
            .others
            .iter()
            .any(|(source, _)| source.language() == Some(language))
        {
            State::Open
        } else if self
            .unreadable
            .iter()
            .any(|source| source.language() == Some(language))
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
/// order of `import::KNOWN`. A source of Russian headwords reads no
/// language and is not one of them.
fn other_sources() -> Vec<Source> {
    let mut sources = Vec::new();
    for known in import::KNOWN {
        let other = matches!(known.source.language(), Some(Language::Es | Language::Ca));
        if other && !sources.contains(&known.source) {
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
            .find(|source| source.language() == Some(language))
        {
            return Looked {
                answer: Answer::Unreadable(IndexFile::of(*source)),
                indexes,
            };
        }
        // In the order of `import::KNOWN`, whatever order they were opened in.
        let asked: Vec<(Kind, &Index)> = other_sources()
            .into_iter()
            .filter(|source| source.language() == Some(language))
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
                    source.language() == Some(language)
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

    /// What the Russian index translated into `into` says about the Russian
    /// `word`; see [`Articles::look_up_russian`]. It is opened when it is
    /// not open yet; one that could not be read is closed, and opened again
    /// by the next call. Where the other indexes stand is checked as by
    /// every call, and the Russian index changes none of it.
    fn look_up_russian_in(&self, into: Language, word: &str) -> Looked {
        let mut opened = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let indexes = self.open_missing(&mut opened);
        let source = Source::from_russian(into);
        let looked = |answer| Looked { answer, indexes };
        let open = opened.russian.iter().any(|(kept, _)| *kept == source);
        let mut index = None;
        match self.open(source, open, |opened| index = Some(opened)) {
            State::Open => {}
            State::Missing => return looked(Answer::NotImported),
            State::Unreadable => return looked(Answer::Unreadable(IndexFile::of(source))),
        }
        if let Some(index) = index {
            opened.russian.push((source, index));
        }
        let Some((_, index)) = opened.russian.iter().find(|(kept, _)| *kept == source) else {
            return looked(Answer::NotImported);
        };
        looked(match lookup::russian(index, word) {
            Ok(Some(translations)) => Answer::FromRussian(translations),
            Ok(None) => Answer::NoEntry,
            Err(_) => {
                opened.russian.retain(|(kept, _)| *kept != source);
                Answer::Unreadable(IndexFile::of(source))
            }
        })
    }
}

/// The first open source of `language`, the one to name when an error
/// cannot be told apart.
fn asked_first(opened: &Opened, language: Language) -> Source {
    opened
        .others
        .iter()
        .map(|(source, _)| *source)
        .find(|source| source.language() == Some(language))
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

    fn look_up_russian(&self, into: Language, word: &str) -> Looked {
        self.look_up_russian_in(into, word)
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
                        | Source::KaikkiEnCatalan
                        | Source::WikdictRuEn
                        | Source::WikdictRuEs
                        | Source::WikdictRuCa => {
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
                Source::WikdictRuEn => headwords(
                    "ru-en",
                    &[
                        ("Москва", &[], &["Moscow", "Moskva"]),
                        ("банк", &[], &["bank"]),
                    ],
                ),
                Source::WikdictRuEs => headwords(
                    "ru-es",
                    &[
                        ("собака", &[], &["perro", "cachorro", "arroba"]),
                        ("дом", &[], &["casa", "hogar", "domicilio"]),
                        // A Spanish word as a Russian headword: never read
                        // for a word read in Spanish.
                        ("banco", &[], &["no"]),
                    ],
                ),
                Source::WikdictRuCa => headwords("ru-ca", &[("ёж", &[], &["eriçó"])]),
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
                    part_of_speech: None,
                    definition: "sloping land beside a body of water".to_owned(),
                    members: texts(&["bank", "shore"]),
                    metadata: None,
                },
                SenseRecord {
                    part_of_speech: None,
                    definition: "change location".to_owned(),
                    members: texts(&["go"]),
                    metadata: None,
                },
            ],
            headwords: vec![
                Headword {
                    entries: Vec::new(),
                    word: "bank".to_owned(),
                    senses: vec![0],
                    translations: Vec::new(),
                },
                Headword {
                    entries: Vec::new(),
                    word: "go".to_owned(),
                    senses: vec![1],
                    translations: Vec::new(),
                },
            ],
            analyses: Vec::new(),
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
                part_of_speech: None,
                definition: "An edge of river, lake, or other watercourse.".to_owned(),
                members: Vec::new(),
                metadata: None,
            }],
            headwords: vec![
                Headword {
                    entries: Vec::new(),
                    word: "bank".to_owned(),
                    senses: vec![0],
                    translations: texts(&["берег", "банк"]),
                },
                Headword {
                    entries: Vec::new(),
                    word: "go".to_owned(),
                    senses: Vec::new(),
                    translations: texts(&["идти"]),
                },
            ],
            analyses: Vec::new(),
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
                    part_of_speech: None,
                    definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                    members: Vec::new(),
                    metadata: None,
                },
                SenseRecord {
                    part_of_speech: None,
                    definition: "Mueble alargado para sentarse.".to_owned(),
                    members: texts(&["banca"]),
                    metadata: None,
                },
                SenseRecord {
                    part_of_speech: None,
                    definition: "Una palabra inventada.".to_owned(),
                    members: Vec::new(),
                    metadata: None,
                },
            ],
            headwords: vec![
                Headword {
                    entries: Vec::new(),
                    word: "banco".to_owned(),
                    senses: vec![0, 1],
                    translations: Vec::new(),
                },
                Headword {
                    entries: Vec::new(),
                    word: "bank".to_owned(),
                    senses: vec![2],
                    translations: Vec::new(),
                },
            ],
            analyses: Vec::new(),
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
            entries: Vec::new(),
            sense_metadata: Vec::new(),
            definitions: Some(vec![
                Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                    synonyms: Vec::new(),
                },
                Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "Mueble alargado para sentarse.".to_owned(),
                    synonyms: texts(&["banca"]),
                },
            ]),
            russian: None,
            synonyms: None,
            verb: None,
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
                .any(|source| source.language() == Some(Language::Ca))
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
            part_of_speech: None,
            entries: Vec::new(),
            definition: "sloping land beside a body of water".to_owned(),
            synonyms: texts(&["shore"]),
        }]
    }

    fn russian_bank() -> Article {
        Article {
            sense_metadata: Vec::new(),
            entries: Vec::new(),
            senses: vec![Sense {
                part_of_speech: None,
                entries: Vec::new(),
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
            sense_metadata: Vec::new(),
            entries: Vec::new(),
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
                sense_metadata: Vec::new(),
                entries: Vec::new(),
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
                sense_metadata: Vec::new(),
                entries: Vec::new(),
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
                sense_metadata: Vec::new(),
                entries: Vec::new(),
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
            analyses: Vec::new(),
            forms: Vec::new(),
        };
        let mut sorted = headwords.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        for (word, senses, translations) in sorted {
            let mut numbers = Vec::new();
            for (definition, members) in senses {
                numbers.push(contents.senses.len() as u32);
                contents.senses.push(SenseRecord {
                    part_of_speech: None,
                    definition: (*definition).to_owned(),
                    members: texts(members),
                    metadata: None,
                });
            }
            contents.headwords.push(Headword {
                entries: Vec::new(),
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
            analyses: Vec::new(),
            forms: Vec::new(),
        };
        for (word, senses, translations) in headwords {
            let mut numbers = Vec::new();
            for (definition, members) in *senses {
                numbers.push(contents.senses.len() as u32);
                contents.senses.push(SenseRecord {
                    part_of_speech: None,
                    definition: (*definition).to_owned(),
                    members: texts(members),
                    metadata: None,
                });
            }
            contents.headwords.push(Headword {
                entries: Vec::new(),
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
        let line = |listed: Option<&crate::view::Listed<'_>>| match listed {
            Some(crate::view::Listed::Items { label, items }) => {
                format!("{label} {}", items.join(", "))
            }
            Some(crate::view::Listed::Missing(line)) => (*line).to_owned(),
            None => String::new(),
        };
        (
            texts.senses_label,
            vec![
                line(texts.translations.as_ref()),
                format!("{} senses", texts.senses.len()),
                texts
                    .word_synonyms
                    .as_ref()
                    .map_or_else(String::new, |synonyms| synonyms.line()),
                line(texts.glosses.as_ref()),
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
                    "More Spanish synonyms entidad",
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
            if matches!(source.language(), Some(Language::Es | Language::Ca)) {
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
            .filter(|source| source.language() == Some(Language::Ca))
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
                entries: Vec::new(),
                sense_metadata: Vec::new(),
                definitions: Some(vec![Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "Conjunt d'objectes reunits.".to_owned(),
                    synonyms: Vec::new(),
                }]),
                russian: Some(texts(&["коллекция"])),
                synonyms: Some(vec![
                    addict_dictionary::index::SynonymGroup {
                        headword: "col·lecció".to_owned(),
                        definition: String::new(),
                        metadata: None,
                        synonyms: texts(&["recull", "aplec"])
                    },
                    addict_dictionary::index::SynonymGroup {
                        headword: "col·lecció".to_owned(),
                        definition: String::new(),
                        metadata: None,
                        synonyms: texts(&["sèrie", "recull"])
                    },
                ]),
                verb: None,
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
        assert_eq!(
            banco.synonyms.expect("groups")[0].synonyms,
            texts(&["banca"])
        );
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

    /// A real Kaikki import with retained metadata and blank visible content.
    fn metadata_only_contents(code: &str, word: &str) -> Contents {
        let json = serde_json::json!({
            "word": word, "lang_code": code, "pos": "noun",
            "sounds": [{"ipa": "source sound", "mp3_url": "https://example.org/sound.mp3"}],
            "senses": [{"glosses": ["", "  "]}]
        })
        .to_string();
        addict_dictionary::kaikki::parse(json.as_bytes(), code, &format!("metadata-{code}"))
            .unwrap()
    }

    #[test]
    fn metadata_only_chosen_languages_allow_english_content_and_direct_metadata_lookup() {
        let dir = DataDir::new("metadata-chosen-fallback");
        dir.import(Source::Oewn);
        for (source, code) in [(Source::KaikkiEs, "es"), (Source::DbnaryCa, "ca")] {
            dir.write(source, &metadata_only_contents(code, "bank"));
        }
        let dictionaries = dir.dictionaries();
        for chosen in [Language::Es, Language::Ca] {
            let direct = dictionaries.look_up_in(chosen, "bank");
            let Answer::Grouped(grouped) = &direct.answer else {
                panic!("{direct:?}")
            };
            assert_eq!(grouped.entries.len(), 1);
            assert_eq!(
                grouped.entries[0].pronunciations[0].text.as_deref(),
                Some("source sound")
            );
            assert_eq!(grouped.definitions, Some(Vec::new()));
            assert!(direct.answer.metadata_only());
            let (language, looked) = dictionaries.look_up_first(chosen, "bank");
            assert_eq!(language, Language::En);
            assert_eq!(looked.answer, dictionaries.look_up("bank").answer);
            assert!(
                matches!(&looked.answer, Answer::Article(article) if !article.senses.is_empty())
            );
            assert_eq!(looked.indexes, dictionaries.indexes());
        }
    }

    #[test]
    fn metadata_only_english_is_skipped_for_a_later_translation_only_fallback() {
        let dir = DataDir::new("metadata-english-fallback");
        dir.write(Source::Oewn, &metadata_only_contents("en", "bank"));
        dir.write(Source::KaikkiEs, &metadata_only_contents("es", "bank"));
        dir.write(
            Source::WikdictEsRu,
            &headwords("es-ru", &[("bank", &[], &["банк"])]),
        );
        dir.write(Source::DbnaryCa, &metadata_only_contents("ca", "another"));
        let dictionaries = dir.dictionaries();
        assert!(dictionaries.look_up("bank").answer.metadata_only());
        for chosen in [Language::Ca, Language::En, Language::Es] {
            let (language, looked) = dictionaries.look_up_first(chosen, "bank");
            assert_eq!(language, Language::Es);
            assert_eq!(russian_of(&looked.answer), Some(texts(&["банк"])));
            assert!(!looked.answer.metadata_only());
            assert_eq!(looked.indexes, dictionaries.indexes());
        }
    }

    #[test]
    fn without_visible_content_fallback_keeps_the_first_metadata_candidate_and_language() {
        let dir = DataDir::new("metadata-no-content");
        for (source, code) in [
            (Source::Oewn, "en"),
            (Source::KaikkiEs, "es"),
            (Source::DbnaryCa, "ca"),
        ] {
            dir.write(source, &metadata_only_contents(code, "bank"));
        }
        let dictionaries = dir.dictionaries();
        for chosen in Language::ALL {
            let (language, looked) = dictionaries.look_up_first(chosen, "bank");
            assert_eq!(language, chosen);
            assert_eq!(
                looked.answer,
                dictionaries.look_up_in(chosen, "bank").answer
            );
            assert!(looked.answer.metadata_only());
            assert_eq!(looked.indexes, dictionaries.indexes());
        }
        // A miss keeps the first fallback's metadata even if later lookups fail.
        dir.write(Source::DbnaryCa, &metadata_only_contents("ca", "another"));
        std::fs::write(dir.index(Source::KaikkiEs), b"damaged index").unwrap();
        let dictionaries = dir.dictionaries();
        let (language, looked) = dictionaries.look_up_first(Language::Ca, "bank");
        assert_eq!(language, Language::En);
        assert_eq!(looked.answer, dictionaries.look_up("bank").answer);
        assert!(looked.answer.metadata_only());
        assert_eq!(looked.indexes.spanish, State::Unreadable);
        assert_eq!(looked.indexes, dictionaries.indexes());
    }

    #[test]
    fn chosen_missing_or_unreadable_indexes_keep_precedence_with_metadata_elsewhere() {
        let dir = DataDir::new("metadata-errors");
        dir.write(Source::Oewn, &metadata_only_contents("en", "bank"));
        dir.write(
            Source::WikdictCaRu,
            &headwords("ca-ru", &[("bank", &[], &["банк"])]),
        );
        let dictionaries = dir.dictionaries();
        assert_eq!(
            dictionaries.look_up_first(Language::Es, "bank").1.answer,
            Answer::NotImported
        );
        std::fs::write(dir.index(Source::KaikkiEs), b"damaged index").unwrap();
        let (language, looked) = dictionaries.look_up_first(Language::Es, "bank");
        assert_eq!(language, Language::Es);
        assert_eq!(looked.answer, Answer::Unreadable(SPANISH));
    }

    #[test]
    fn fallback_content_detection_ignores_blank_text_and_preserves_legacy_empty_answers() {
        use addict_core::dictionary::LexicalEntry;
        let blank_sense = Sense {
            part_of_speech: None,
            entries: Vec::new(),
            definition: "  ".to_owned(),
            synonyms: texts(&["", "\t"]),
        };
        let article = Article {
            entries: vec![LexicalEntry::default()],
            senses: vec![blank_sense.clone()],
            translations: texts(&["", " "]),
            ..Default::default()
        };
        assert!(Answer::Article(article.clone()).metadata_only());
        let mut useful = article.clone();
        useful.senses[0].definition = "definition".to_owned();
        assert!(!Answer::Article(useful).metadata_only());
        let mut useful = article.clone();
        useful.senses[0].synonyms.push("synonym".to_owned());
        assert!(!Answer::Article(useful).metadata_only());
        let mut useful = article;
        useful.translations.push("translation".to_owned());
        assert!(!Answer::Article(useful).metadata_only());
        let grouped = Grouped {
            entries: vec![LexicalEntry::default()],
            definitions: Some(vec![blank_sense]),
            russian: Some(texts(&["", " "])),
            english: Some(texts(&["", "\n"])),
            synonyms: Some(vec![addict_dictionary::index::SynonymGroup {
                headword: "word".to_owned(),
                definition: " ".to_owned(),
                metadata: None,
                synonyms: texts(&["", " "]),
            }]),
            ..Default::default()
        };
        assert!(Answer::Grouped(grouped.clone()).metadata_only());
        for kind in [
            Kind::Definitions,
            Kind::Russian,
            Kind::Synonyms,
            Kind::English,
        ] {
            let mut useful = grouped.clone();
            match kind {
                Kind::Definitions => {
                    useful.definitions.as_mut().unwrap()[0].definition = "definition".to_owned()
                }
                Kind::Russian => useful
                    .russian
                    .as_mut()
                    .unwrap()
                    .push("translation".to_owned()),
                Kind::Synonyms => useful.synonyms.as_mut().unwrap()[0]
                    .synonyms
                    .push("synonym".to_owned()),
                Kind::English => useful.english.as_mut().unwrap().push("gloss".to_owned()),
                Kind::FromRussian => unreachable!("not a reading-language content kind"),
            }
            assert!(!Answer::Grouped(useful).metadata_only(), "{kind:?}");
        }
        // The line of a verb form is shown, so it is content too.
        let verb_form = Grouped {
            verb: Some(Box::new(addict_dictionary::lookup::VerbForm {
                analyses: vec![addict_dictionary::analysis::Analyses {
                    lemma: "beure".to_owned(),
                    shapes: Vec::new(),
                }],
                periphrastic: None,
            })),
            ..grouped
        };
        assert!(!Answer::Grouped(verb_form).metadata_only());
        assert!(!Answer::Article(Article::default()).metadata_only());
        assert!(!Answer::Grouped(Grouped::default()).metadata_only());
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

    /// A data directory with every index of the three reading languages
    /// and the Russian indexes of `russian`.
    fn with_russian(tag: &str, russian: &[Source]) -> DataDir {
        let dir = DataDir::new(tag);
        for known in import::KNOWN {
            if known.source.language().is_some() {
                dir.import(known.source);
            }
        }
        for source in russian {
            dir.import(*source);
        }
        dir
    }

    const RUSSIAN: [Source; 3] = [
        Source::WikdictRuEn,
        Source::WikdictRuEs,
        Source::WikdictRuCa,
    ];

    #[test]
    fn a_russian_word_is_answered_by_the_russian_index_of_the_chosen_language() {
        let without = with_russian("russian-without", &[])
            .dictionaries()
            .indexes();
        let dir = with_russian("russian", &RUSSIAN);
        let dictionaries = dir.dictionaries();
        let indexes = dictionaries.indexes();
        // The Russian indexes change none of the states.
        assert_eq!(indexes, without);
        let russian = |into, word| dictionaries.look_up_russian(into, word);
        for (into, word, translations) in [
            (Language::Es, "собака", &["perro", "cachorro", "arroba"][..]),
            (Language::En, "Москва", &["Moscow", "Moskva"]),
            (Language::Ca, "еж", &["eriçó"]),
            (Language::Es, "«Дом»,", &["casa", "hogar", "domicilio"]),
        ] {
            let looked = russian(into, word);
            assert_eq!(
                looked.answer,
                Answer::FromRussian(texts(translations)),
                "{word}"
            );
            assert_eq!(looked.indexes, indexes, "{word}");
        }
        // Only the chosen language answers: no fallback to the others.
        assert_eq!(russian(Language::En, "собака").answer, Answer::NoEntry);
        assert_eq!(russian(Language::Ca, "Москва").answer, Answer::NoEntry);
        assert_eq!(russian(Language::Es, "собаки").answer, Answer::NoEntry);
        // A word read in a language never reaches a Russian index.
        assert_eq!(
            dictionaries.look_up_in(Language::Es, "собака").answer,
            Answer::NoEntry
        );
        let Answer::Grouped(banco) = dictionaries.look_up_in(Language::Es, "banco").answer else {
            panic!("banco");
        };
        assert_eq!(banco.russian, Some(texts(&["банк"])));
        assert!(
            !other_sources()
                .iter()
                .any(|source| RUSSIAN.contains(source))
        );
    }

    #[test]
    fn a_russian_word_without_the_index_of_the_chosen_language_has_nothing_to_be_looked_up_in() {
        let dir = with_russian(
            "russian-missing",
            &[Source::WikdictRuEs, Source::WikdictRuEn],
        );
        let dictionaries = dir.dictionaries();
        let looked = dictionaries.look_up_russian(Language::Ca, "собака");
        assert_eq!(looked.answer, Answer::NotImported);
        assert_eq!(looked.indexes, dictionaries.indexes());
        assert_eq!(
            Dictionaries::new(None)
                .look_up_russian(Language::Es, "собака")
                .answer,
            Answer::NotImported
        );
        // Imported while resident: found by the next call.
        dir.import(Source::WikdictRuCa);
        assert_eq!(
            dictionaries.look_up_russian(Language::Ca, "ёж").answer,
            Answer::FromRussian(texts(&["eriçó"]))
        );
    }

    #[test]
    fn a_damaged_russian_index_is_named_closed_and_opened_again() {
        let file = IndexFile {
            index: "wikdict-ru-en.index",
            published: "ru-en.sqlite3",
        };
        assert_eq!(IndexFile::of(Source::WikdictRuEn), file);
        let dir = with_russian("russian-damaged", &[]);
        std::fs::write(dir.index(Source::WikdictRuEn), b"not an index").expect("write");
        let dictionaries = dir.dictionaries();
        let states = dictionaries.indexes();
        let looked = dictionaries.look_up_russian(Language::En, "Москва");
        assert_eq!(looked.answer, Answer::Unreadable(file));
        assert_eq!(looked.indexes, states);
        dir.import(Source::WikdictRuEn);
        assert!(matches!(
            dictionaries.look_up_russian(Language::En, "Москва").answer,
            Answer::FromRussian(_)
        ));
        // Broken while open: named, closed, and read again once imported.
        std::fs::write(dir.index(Source::WikdictRuEn), b"").expect("truncate");
        assert_eq!(
            dictionaries.look_up_russian(Language::En, "Москва").answer,
            Answer::Unreadable(file)
        );
        let old = dir.path().join("old.index");
        std::fs::rename(dir.index(Source::WikdictRuEn), &old).expect("rename");
        dir.import(Source::WikdictRuEn);
        assert_eq!(
            dictionaries.look_up_russian(Language::En, "Москва").answer,
            Answer::FromRussian(texts(&["Moscow", "Moskva"]))
        );
        // The English line is not about it.
        assert!(
            looked
                .indexes
                .files()
                .iter()
                .all(|(named, _)| *named != file)
        );
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
    #[test]
    fn english_source_sense_associations_survive_answer_lookup_and_shown() {
        use crate::{
            manual::result_in,
            view::{Lookup, Shown},
        };
        use addict_core::dictionary::Pos;
        use addict_platform::work::Outcome;

        let freedict = addict_dictionary::freedict::parse(
            br#"<TEI><entry>
            <form><orth>bank</orth><pron notation="IPA">bank transcription</pron></form>
            <sense><gramGrp><pos>noun</pos></gramGrp><sense><def>FreeDict noun</def></sense>
            <sense><gramGrp><pos>verb</pos></gramGrp><def>FreeDict verb</def></sense>
            </sense></entry></TEI>"#
                .as_slice(),
            "freedict-source",
        )
        .unwrap();
        let wordnet = addict_dictionary::oewn::parse(
            br#"<LexicalResource><Lexicon>
            <LexicalEntry id="wordnet-bank-n"><Lemma writtenForm="bank" partOfSpeech="n"/>
            <Sense id="bank-sense" synset="bank-synset"/></LexicalEntry>
            <Synset id="bank-synset" members="wordnet-bank-n" partOfSpeech="n">
            <Definition>WordNet noun</Definition></Synset></Lexicon></LexicalResource>"#
                .as_slice(),
            "wordnet-source",
        )
        .unwrap();
        for legacy in [false, true] {
            let dir = DataDir::new("source-sense-transport");
            dir.write(Source::Freedict, &freedict);
            let mut definitions = wordnet.clone();
            if legacy {
                definitions
                    .headwords
                    .iter_mut()
                    .for_each(|word| word.entries.clear());
                definitions
                    .senses
                    .iter_mut()
                    .for_each(|sense| sense.part_of_speech = None);
            }
            let mut bytes = encode(&definitions).unwrap();
            if legacy {
                bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
            }
            std::fs::write(dir.index(Source::Oewn), bytes).unwrap();
            let dictionaries = dir.dictionaries();
            for word in ["bank", "Banks"] {
                let looked = dictionaries.look_up_in(Language::En, word);
                let Answer::Article(article) = &looked.answer else {
                    panic!("{looked:?}")
                };
                assert_eq!(article.senses.len(), 1);
                assert_eq!(article.senses[0].definition, "WordNet noun");
                let metadata: Vec<_> = article
                    .sense_metadata
                    .iter()
                    .filter(|sense| sense.source == "freedict-source")
                    .collect();
                assert_eq!(metadata.len(), 2);
                let entry = article
                    .entries
                    .iter()
                    .find(|entry| entry.identity.source == "freedict-source")
                    .unwrap();
                assert_eq!(entry.identity.id, "entry-1");
                assert_eq!(entry.identity.headword, "bank");
                assert_eq!(
                    entry.pronunciations[0].text.as_deref(),
                    Some("bank transcription")
                );
                for (record, pos) in metadata.iter().zip([Pos::Noun, Pos::Verb]) {
                    assert_eq!(record.headword, "bank");
                    assert_eq!(
                        record.sense.part_of_speech.as_ref().unwrap().category,
                        Some(pos)
                    );
                    assert!(record.sense.entries.contains(&entry.identity));
                    assert!(record.sense.entries.iter().all(|identity| {
                        article
                            .entries
                            .iter()
                            .any(|entry| &entry.identity == identity)
                    }));
                }
                let expected = article.clone();
                let lookup = result_in(Outcome::Finished(looked), word, Language::En).unwrap();
                assert_eq!(lookup, Lookup::Article(expected));
                let mut shown = Shown::default();
                shown.show(Some(lookup.clone()));
                assert_eq!(shown.lookup(), Some(&lookup));
            }
        }
    }

    #[test]
    fn grammar_pronunciation_and_audio_survive_application_answers_and_lookup_transport() {
        use crate::{
            manual::result_in,
            view::{Lookup, Shown},
        };
        use addict_core::dictionary::Pos;
        use addict_platform::work::Outcome;
        for (source, language, code, word) in [
            (Source::Oewn, Language::En, "en", "bank"),
            (Source::KaikkiEs, Language::Es, "es", "banco"),
            (Source::DbnaryCa, Language::Ca, "ca", "banc"),
        ] {
            let dir = DataDir::new("metadata-transport");
            let json = r#"{"word":"WORD","lang_code":"LANG","pos":"noun","sounds":[{"ipa":"source transcription","tags":["source region"],"mp3_url":"https://example.org/sound.mp3"}],"senses":[{"glosses":["source definition"]}]}"#.replace("WORD", word).replace("LANG", code);
            let contents =
                addict_dictionary::kaikki::parse(json.as_bytes(), code, "metadata-source").unwrap();
            dir.write(source, &contents);
            let looked = dir.dictionaries().look_up_in(language, word);
            let lookup = result_in(Outcome::Finished(looked), word, language).unwrap();
            let entries = match &lookup {
                Lookup::Article(article) => {
                    assert_eq!(language, Language::En);
                    assert_eq!(
                        article.senses[0].part_of_speech.as_ref().unwrap().category,
                        Some(Pos::Noun)
                    );
                    assert_eq!(article.senses[0].entries[0], article.entries[0].identity);
                    &article.entries
                }
                Lookup::Grouped {
                    language: actual,
                    grouped,
                } => {
                    assert_eq!(*actual, language);
                    assert_eq!(
                        grouped.definitions.as_ref().unwrap()[0]
                            .part_of_speech
                            .as_ref()
                            .unwrap()
                            .category,
                        Some(Pos::Noun)
                    );
                    &grouped.entries
                }
                other => panic!("{other:?}"),
            };
            assert_eq!(entries[0].identity.source, "metadata-source");
            assert_eq!(entries[0].identity.headword, word);
            assert_eq!(entries[0].part_of_speech.as_ref().unwrap().label, "noun");
            let sound = &entries[0].pronunciations[0];
            assert_eq!(sound.text.as_deref(), Some("source transcription"));
            assert_eq!(sound.notation.as_deref(), Some("IPA"));
            assert_eq!(sound.labels, ["source region"]);
            assert_eq!(sound.audio[0].url, "https://example.org/sound.mp3");
            let mut shown = Shown::default();
            shown.show(Some(lookup.clone()));
            assert_eq!(shown.lookup(), Some(&lookup));
        }
    }
}
