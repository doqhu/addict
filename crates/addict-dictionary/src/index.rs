//! An index file opened for lookups.
//!
//! [`Index`] holds the open file and where its sections are, and nothing of
//! their contents. A lookup is a binary search of a table by positioned
//! reads, about 17 steps for 128 thousand headwords, then one read of the
//! record and one or two per sense. Nothing is mapped, loaded or cached, so
//! the memory of a process with an open index does not depend on the size of
//! the file or on how much of it was looked up.
//!
//! The reads block. [`Index::find`] is the fallible lookup; the
//! [`Dictionary`] contract has no error, so its implementation panics on one.

use std::{
    cmp::Ordering,
    fmt,
    fs::File,
    io,
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
};

use addict_core::dictionary::{Article, Dictionary, Sense, SourcedSense};

use crate::{
    analysis::Analyses,
    format::{self, Damage, Entry, Header, Section, SectionId},
};

/// Why an index could not be opened or read.
#[derive(Debug)]
pub enum Error {
    /// There is no index file at the path: nothing was imported yet.
    NotImported(PathBuf),
    /// The file could not be opened or read.
    Io { path: PathBuf, source: io::Error },
    /// The file is not a whole index of the known format.
    Damaged { path: PathBuf, reason: &'static str },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImported(path) => write!(
                f,
                "the dictionary is not imported: there is no index at {}",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(f, "cannot read the index {}: {source}", path.display())
            }
            Self::Damaged { path, reason } => {
                write!(f, "the index {} is damaged: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::NotImported(_) | Self::Damaged { .. } => None,
        }
    }
}

/// The three sections of a table.
#[derive(Debug, Clone, Copy)]
struct Table {
    entries: SectionId,
    keys: SectionId,
    records: SectionId,
}

const WORDS: Table = Table {
    entries: SectionId::WordTable,
    keys: SectionId::WordKeys,
    records: SectionId::WordRecords,
};

const FORMS: Table = Table {
    entries: SectionId::FormTable,
    keys: SectionId::FormKeys,
    records: SectionId::FormRecords,
};

const ANALYSES: Table = Table {
    entries: SectionId::AnalysisTable,
    keys: SectionId::AnalysisKeys,
    records: SectionId::AnalysisRecords,
};

/// An open index file.
#[derive(Debug)]
pub struct Index {
    file: File,
    path: PathBuf,
    header: Header,
    source: String,
}

/// The `members` of a sense other than `word`, each once, in their order.
fn others(members: Vec<String>, word: &str) -> Vec<String> {
    let mut others: Vec<String> = Vec::new();
    for member in members {
        if member != word && !others.contains(&member) {
            others.push(member);
        }
    }
    others
}

/// One independent meaning from a synonym source, with its headword provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynonymGroup {
    pub headword: String,
    pub definition: String,
    pub metadata: Option<format::SenseMetadata>,
    pub synonyms: Vec<String>,
}

impl Index {
    /// Opens the index at `path` and checks its header. A missing file is
    /// [`Error::NotImported`].
    pub fn open(path: &Path) -> Result<Self, Error> {
        let io = |source| Error::Io {
            path: path.to_owned(),
            source,
        };
        let file = File::open(path).map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => Error::NotImported(path.to_owned()),
            _ => io(source),
        })?;
        let metadata = file.metadata().map_err(io)?;
        if !metadata.is_file() {
            return Err(io(io::Error::other("not a regular file")));
        }
        let damaged = |damage: Damage| Error::Damaged {
            path: path.to_owned(),
            reason: damage.0,
        };
        // The longer header; a file without analyses reads less of it.
        let mut bytes = [0; format::ANALYSED_HEADER_LEN];
        let read = read_some(&file, path, &mut bytes, 0)?;
        let header = Header::parse(&bytes[..read], metadata.len()).map_err(damaged)?;
        // The header bounds the length of the source id.
        let section = header.section(SectionId::Source);
        let mut source = vec![0; section.len as usize];
        if read_some(&file, path, &mut source, section.offset)? < source.len() {
            return Err(damaged(Damage("the file ends too early")));
        }
        let source =
            String::from_utf8(source).map_err(|_| damaged(Damage("its source id is not UTF-8")))?;
        Ok(Self {
            file,
            path: path.to_owned(),
            header,
            source,
        })
    }

    /// The path the index was opened at.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The id of the source the index was imported from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// How many headwords the index has.
    pub fn headwords(&self) -> u64 {
        self.header.section(SectionId::WordTable).len / format::ENTRY_LEN
    }

    /// How many exception forms the index has.
    pub fn exception_forms(&self) -> u64 {
        self.header.section(SectionId::FormTable).len / format::ENTRY_LEN
    }

    /// The article of `word`, which must equal a headword byte for byte, or
    /// `None`. The synonyms of a sense are its other words, each once.
    pub fn find(&self, word: &str) -> Result<Option<Article>, Error> {
        let Some(record) = self.search(WORDS, word)? else {
            return Ok(None);
        };
        let (offsets, translations, mut entries) =
            format::decode_word(&record).map_err(|damage| self.damaged(damage))?;
        let mut senses = Vec::new();
        for entry in &mut entries {
            entry.entry.identity.source = self.source().to_owned();
        }
        for (position, offset) in offsets.into_iter().enumerate() {
            let record = self.sense(offset)?;
            senses.push(Sense {
                part_of_speech: record.part_of_speech,
                entries: entries
                    .iter()
                    .filter(|entry| entry.senses.contains(&(position as u32)))
                    .map(|entry| entry.entry.identity.clone())
                    .collect(),
                synonyms: others(record.members, word),
                definition: record.definition,
            });
        }
        let sense_metadata = senses
            .iter()
            .filter(|sense| sense.part_of_speech.is_some() || !sense.entries.is_empty())
            .map(|sense| SourcedSense {
                source: self.source().to_owned(),
                headword: word.to_owned(),
                sense: sense.clone(),
            })
            .collect();
        Ok(Some(Article {
            sense_metadata,
            entries: entries.into_iter().map(|entry| entry.entry).collect(),
            senses,
            translations,
        }))
    }

    /// Reads independent synonym meanings without changing the core Article
    /// contract. Uses the same bounded, positioned reads as `find`.
    pub fn synonym_groups(&self, word: &str) -> Result<Option<Vec<SynonymGroup>>, Error> {
        let Some(record) = self.search(WORDS, word)? else {
            return Ok(None);
        };
        let (offsets, _, _) =
            format::decode_word(&record).map_err(|damage| self.damaged(damage))?;
        let mut groups = Vec::new();
        for offset in offsets {
            let sense = self.sense(offset)?;
            groups.push(SynonymGroup {
                headword: word.to_owned(),
                synonyms: others(sense.members, word),
                definition: sense.definition,
                metadata: sense.metadata,
            });
        }
        Ok(Some(groups))
    }

    /// The lemmas the exception list gives for `form`, in its order; empty
    /// when the form is not in the list.
    pub fn exception_lemmas(&self, form: &str) -> Result<Vec<String>, Error> {
        match self.search(FORMS, form)? {
            Some(record) => format::decode_form(&record).map_err(|damage| self.damaged(damage)),
            None => Ok(Vec::new()),
        }
    }

    /// The analyses of the verb form `form`, which must equal a stored form
    /// byte for byte, one per lemma in the order stored; empty when the
    /// form has none or the file has no analyses.
    pub fn analyses(&self, form: &str) -> Result<Vec<Analyses>, Error> {
        match self.search(ANALYSES, form)? {
            Some(record) => format::decode_analyses(&record).map_err(|damage| self.damaged(damage)),
            None => Ok(Vec::new()),
        }
    }

    /// The record of `key` in `table`, by binary search.
    fn search(&self, table: Table, key: &str) -> Result<Option<Vec<u8>>, Error> {
        let key = key.as_bytes();
        let entries = self.header.section(table.entries);
        let keys = self.header.section(table.keys);
        let records = self.header.section(table.records);
        let (mut low, mut high) = (0, entries.len / format::ENTRY_LEN);
        while low < high {
            let middle = low + (high - low) / 2;
            let bytes = self.read(
                entries,
                middle * format::ENTRY_LEN,
                format::ENTRY_LEN as usize,
            )?;
            let entry =
                Entry::parse(&bytes, keys, records).map_err(|damage| self.damaged(damage))?;
            // No more of the stored key than the looked-up one is long: the
            // lengths decide between a key and its prefix.
            let shared = (entry.key_len as usize).min(key.len());
            let stored = self.read(keys, entry.key_offset, shared)?;
            let order = stored[..]
                .cmp(&key[..shared])
                .then((entry.key_len as usize).cmp(&key.len()));
            match order {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => {
                    return self
                        .read(records, entry.record_offset, entry.record_len as usize)
                        .map(Some);
                }
            }
        }
        Ok(None)
    }

    /// The sense record at `offset` of the senses section.
    fn sense(&self, offset: u64) -> Result<format::SenseRecord, Error> {
        let senses = self.header.section(SectionId::Senses);
        if !format::fits(offset, 4, senses) {
            return Err(self.damaged(Damage("a sense lies outside its section")));
        }
        let len = self.read(senses, offset, 4)?;
        let len = u32::from_le_bytes([len[0], len[1], len[2], len[3]]);
        if !format::fits(offset + 4, len, senses) {
            return Err(self.damaged(Damage("a sense lies outside its section")));
        }
        let body = self.read(senses, offset + 4, len as usize)?;
        format::decode_sense(&body).map_err(|damage| self.damaged(damage))
    }

    /// `len` bytes at `offset` of `section`. The caller has checked that
    /// they lie inside the section; a file that ends before them is damaged.
    fn read(&self, section: Section, offset: u64, len: usize) -> Result<Vec<u8>, Error> {
        let inside = offset
            .checked_add(len as u64)
            .is_some_and(|end| end <= section.len);
        if !inside || len > format::MAX_RECORD as usize {
            return Err(self.damaged(Damage("a read lies outside its section")));
        }
        let mut bytes = vec![0; len];
        let read = read_some(&self.file, &self.path, &mut bytes, section.offset + offset)?;
        if read < len {
            return Err(self.damaged(Damage("the file ends too early")));
        }
        Ok(bytes)
    }

    fn damaged(&self, damage: Damage) -> Error {
        Error::Damaged {
            path: self.path.clone(),
            reason: damage.0,
        }
    }
}

/// Fills `bytes` from `offset` of `file`; less only at the end of the file.
fn read_some(file: &File, path: &Path, bytes: &mut [u8], offset: u64) -> Result<usize, Error> {
    let mut filled = 0;
    while filled < bytes.len() {
        match file.read_at(&mut bytes[filled..], offset + filled as u64) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(source) => {
                return Err(Error::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        }
    }
    Ok(filled)
}

impl Dictionary for Index {
    /// Panics when the index cannot be read: the contract has no error, and
    /// the caller that runs lookups reports a panic as its own outcome.
    fn lookup(&self, word: &str) -> Option<Article> {
        self.find(word).unwrap_or_else(|error| panic!("{error}"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        format::{HEADER_LEN, encode, tests::header, tests::sample, tests::texts},
        testdir::TempDir,
    };

    fn written(dir: &TempDir, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join("test.index");
        std::fs::write(&path, bytes).expect("write index");
        path
    }

    fn opened(dir: &TempDir) -> Index {
        Index::open(&written(dir, &encode(&sample()).expect("encode"))).expect("open")
    }

    #[test]
    fn open_reads_the_header_only() {
        let dir = TempDir::new("open");
        let index = opened(&dir);
        assert_eq!(index.source(), "test-1");
        assert_eq!(index.headwords(), 4);
        assert_eq!(index.exception_forms(), 2);
        assert_eq!(index.path(), dir.path().join("test.index"));
    }

    #[test]
    fn a_word_has_its_senses_in_order_with_the_other_words_as_synonyms() {
        let dir = TempDir::new("senses");
        let article = opened(&dir).find("bank").expect("find").expect("bank");
        assert_eq!(
            article.senses,
            vec![
                Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "sloping land beside water".to_owned(),
                    synonyms: texts(&["shore"]),
                },
                Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "a financial institution".to_owned(),
                    synonyms: texts(&["depository"]),
                },
            ]
        );
        assert_eq!(article.translations, texts(&["берег", "банк"]));

        let shore = opened(&dir).find("shore").expect("find").expect("shore");
        assert_eq!(shore.senses[0].synonyms, texts(&["bank"]));
        assert_eq!(shore.translations, Vec::<String>::new());
    }

    #[test]
    fn a_word_alone_in_its_sense_has_no_synonyms() {
        let dir = TempDir::new("alone");
        let article = opened(&dir).find("advice").expect("find").expect("advice");
        assert_eq!(article.senses.len(), 1);
        assert!(!article.senses[0].definition.is_empty());
        assert!(article.senses[0].synonyms.is_empty());
    }

    #[test]
    fn a_word_with_translations_only_has_no_senses() {
        let dir = TempDir::new("translations");
        let article = opened(&dir).find("Bánk").expect("find").expect("Bánk");
        assert!(article.senses.is_empty());
        assert_eq!(article.translations, texts(&["Банк"]));
    }

    #[test]
    fn only_a_headword_as_written_is_found() {
        let dir = TempDir::new("absent");
        let index = opened(&dir);
        for word in [
            "boolean", "Bank", "BANK", "banks", "ban", "bank ", " bank", "bank.", "", "a", "zzz",
            "Bánk ", "Bank\0", "geese",
        ] {
            assert_eq!(index.find(word).expect("find"), None, "{word:?}");
        }
    }

    #[test]
    fn every_headword_of_a_larger_index_is_found_and_nothing_between() {
        let mut contents = sample();
        contents.headwords = (0..1000)
            .map(|number| format::Headword {
                entries: Vec::new(),
                word: format!("w{number:04}"),
                senses: vec![number % 3],
                translations: vec![format!("t{number}")],
            })
            .collect();
        let dir = TempDir::new("larger");
        let index = Index::open(&written(&dir, &encode(&contents).expect("encode"))).expect("open");
        for number in 0..1000 {
            let article = index.find(&format!("w{number:04}")).expect("find");
            assert_eq!(
                article.expect("headword").translations,
                vec![format!("t{number}")]
            );
            assert_eq!(index.find(&format!("w{number:04}x")).expect("find"), None);
            assert_eq!(index.find(&format!("w{number:03}")).expect("find"), None);
        }
    }

    #[test]
    fn the_exception_list_gives_the_lemmas_of_a_form() {
        let dir = TempDir::new("forms");
        let index = opened(&dir);
        assert_eq!(index.exception_lemmas("geese").expect("form"), ["goose"]);
        assert_eq!(index.exception_lemmas("lay").expect("form"), ["lay", "lie"]);
        for form in ["goose", "Geese", "bank", ""] {
            assert!(index.exception_lemmas(form).expect("form").is_empty());
        }
    }

    #[test]
    fn a_verb_form_has_its_analyses_and_a_file_without_them_has_none() {
        let dir = TempDir::new("analyses");
        let contents = format::tests::analysed();
        let index = Index::open(&written(&dir, &encode(&contents).expect("encode"))).expect("open");
        assert_eq!(
            index.analyses("fue").expect("fue"),
            contents.analyses[0].lemmas
        );
        assert_eq!(
            index.analyses("hable").expect("hable"),
            contents.analyses[1].lemmas
        );
        for form in ["Fue", "fu", "fues", "hablar", ""] {
            assert!(index.analyses(form).expect("form").is_empty(), "{form:?}");
        }
        // The rest of the file reads as without them.
        let old_dir = TempDir::new("analyses-old");
        let old = opened(&old_dir);
        assert_eq!(
            index.find("bank").expect("find"),
            old.find("bank").expect("find")
        );
        assert_eq!(index.exception_lemmas("lay").expect("form"), ["lay", "lie"]);
        assert!(old.analyses("fue").expect("old file").is_empty());
    }

    #[test]
    fn a_damaged_analysis_is_an_error_of_the_index() {
        let dir = TempDir::new("analyses-damaged");
        let mut file = encode(&format::tests::analysed()).expect("encode");
        // The last byte of the file is in the last shape of `hable`.
        let last = file.len() - crate::analysis::SHAPE_LEN;
        file[last] = 200;
        let index = Index::open(&written(&dir, &file)).expect("open");
        assert!(index.analyses("fue").is_ok());
        assert!(matches!(
            index.analyses("hable"),
            Err(Error::Damaged { .. })
        ));
    }

    #[test]
    fn the_index_answers_the_contract_from_another_thread() {
        let dir = TempDir::new("thread");
        let dictionary: Arc<dyn Dictionary> = Arc::new(opened(&dir));
        let shared = dictionary.clone();
        let found = std::thread::spawn(move || shared.lookup("bank"))
            .join()
            .expect("thread");
        assert!(found.is_some());
        assert_eq!(found, dictionary.lookup("bank"));
        assert_eq!(dictionary.lookup("Bank"), None);
    }

    #[test]
    fn a_missing_index_is_not_imported() {
        let dir = TempDir::new("missing");
        let path = dir.path().join("oewn.index");
        let error = Index::open(&path).expect_err("no index");
        assert!(matches!(&error, Error::NotImported(at) if *at == path));
        assert!(error.to_string().contains("not imported"), "{error}");
        assert!(error.to_string().contains("oewn.index"), "{error}");
    }

    #[test]
    fn a_directory_is_an_error_and_not_a_missing_index() {
        let dir = TempDir::new("directory");
        assert!(matches!(
            Index::open(dir.path()),
            Err(Error::Io { .. } | Error::Damaged { .. })
        ));
    }

    #[test]
    fn a_wrong_magic_and_an_unknown_version_fail_the_open() {
        let dir = TempDir::new("magic");
        let good = encode(&sample()).expect("encode");

        let mut magic = good.clone();
        magic[3] = b'?';
        let error = Index::open(&written(&dir, &magic)).expect_err("magic");
        assert!(matches!(error, Error::Damaged { .. }), "{error}");
        assert!(error.to_string().contains("test.index"), "{error}");

        let mut version = good.clone();
        version[8] = 99;
        let error = Index::open(&written(&dir, &version)).expect_err("version");
        assert!(error.to_string().contains("version"), "{error}");

        let error = Index::open(&written(&dir, b"<html>not an index</html>")).expect_err("text");
        assert!(matches!(error, Error::Damaged { .. }), "{error}");
        let error = Index::open(&written(&dir, b"")).expect_err("empty");
        assert!(matches!(error, Error::Damaged { .. }), "{error}");
    }

    #[test]
    fn a_file_truncated_anywhere_fails_the_open() {
        let dir = TempDir::new("truncated");
        let good = encode(&sample()).expect("encode");
        for len in 0..good.len() {
            let error = Index::open(&written(&dir, &good[..len])).expect_err("truncated");
            assert!(matches!(error, Error::Damaged { .. }), "{len}: {error}");
        }
    }

    #[test]
    fn a_file_truncated_after_the_open_fails_the_lookup() {
        let dir = TempDir::new("shrunk");
        let good = encode(&sample()).expect("encode");
        let path = written(&dir, &good);
        let index = Index::open(&path).expect("open");
        // Without its last byte the file has lost the end of the last form
        // record only.
        std::fs::write(&path, &good[..good.len() - 1]).expect("truncate");
        let error = index.exception_lemmas("lay").expect_err("truncated");
        assert!(matches!(error, Error::Damaged { .. }), "{error}");
        assert!(index.find("bank").expect("find").is_some());
        for len in [HEADER_LEN + 10, HEADER_LEN, 3, 0] {
            std::fs::write(&path, &good[..len]).expect("truncate");
            for word in ["bank", "advice", "shore"] {
                let error = index.find(word).expect_err("truncated");
                assert!(
                    matches!(error, Error::Damaged { .. }),
                    "{len} {word}: {error}"
                );
            }
        }
    }

    /// The index with the eight bytes at `at` of `section` replaced.
    fn patched(dir: &TempDir, section: SectionId, at: u64, value: u64) -> Index {
        let mut file = encode(&sample()).expect("encode");
        let start = (header(&file).section(section).offset + at) as usize;
        file[start..start + 8].copy_from_slice(&value.to_le_bytes());
        Index::open(&written(dir, &file)).expect("the header is whole")
    }

    #[test]
    fn an_offset_past_the_end_fails_the_lookup_that_reads_it() {
        let dir = TempDir::new("offsets");
        // "bank" is the third of four headwords: the search reads it first.
        let bank = 2 * format::ENTRY_LEN;
        for value in [1 << 20, u64::MAX, u64::MAX - 2] {
            // Its key offset.
            let index = patched(&dir, SectionId::WordTable, bank, value);
            let error = index.find("bank").expect_err("key offset");
            assert!(matches!(error, Error::Damaged { .. }), "{error}");

            // Its record offset.
            let index = patched(&dir, SectionId::WordTable, bank + 12, value);
            let error = index.find("bank").expect_err("record offset");
            assert!(matches!(error, Error::Damaged { .. }), "{error}");

            // The first sense offset of its record: the record of "bank"
            // follows those of "Bánk" (4 + 4 + 4 + 8) and "advice" (4 + 8 + 4).
            let index = patched(&dir, SectionId::WordRecords, 20 + 16 + 4, value);
            let error = index.find("bank").expect_err("sense offset");
            assert!(matches!(error, Error::Damaged { .. }), "{error}");
            // A word whose record is whole is still found.
            assert!(index.find("advice").expect("find").is_some());

            // The entry of the form "lay", the second of two.
            let index = patched(&dir, SectionId::FormTable, format::ENTRY_LEN, value);
            let error = index.exception_lemmas("lay").expect_err("form key offset");
            assert!(matches!(error, Error::Damaged { .. }), "{error}");
        }
    }

    #[test]
    fn a_length_past_the_end_fails_the_lookup_that_reads_it() {
        let dir = TempDir::new("lengths");
        // The length of the first sense record, which "bank" and "shore" share.
        let index = patched(&dir, SectionId::Senses, 0, u64::from(u32::MAX));
        for word in ["bank", "shore"] {
            let error = index.find(word).expect_err("sense length");
            assert!(matches!(error, Error::Damaged { .. }), "{error}");
        }
    }

    #[test]
    fn no_damage_of_one_byte_makes_a_lookup_panic() {
        let dir = TempDir::new("bytes");
        let good = encode(&sample()).expect("encode");
        for at in 0..good.len() {
            for flip in [0x01, 0x80, 0xff] {
                let mut file = good.clone();
                file[at] ^= flip;
                let Ok(index) = Index::open(&written(&dir, &file)) else {
                    continue;
                };
                for word in ["Bánk", "advice", "bank", "shore", "zzz", ""] {
                    let _ = index.find(word);
                }
                for form in ["geese", "lay", "zzz"] {
                    let _ = index.exception_lemmas(form);
                }
            }
        }
    }

    #[test]
    #[should_panic(expected = "is damaged")]
    fn the_contract_panics_on_an_index_it_cannot_read() {
        let dir = TempDir::new("panic");
        let index = patched(&dir, SectionId::WordTable, 2 * format::ENTRY_LEN, u64::MAX);
        let _ = index.lookup("bank");
    }
    fn metadata_index(dir: &TempDir, contents: &format::Contents) -> Index {
        let bytes = encode(contents).expect("encode metadata");
        assert_eq!(bytes, encode(contents).expect("deterministic bytes"));
        Index::open(&written(dir, &bytes)).expect("metadata index")
    }

    #[test]
    fn lexical_homographs_and_shared_synsets_keep_their_own_pronunciation() {
        use addict_core::dictionary::Pos;
        let xml = r#"<Lexicon>
          <LexicalEntry id="bank-n"><Lemma writtenForm="bank" partOfSpeech="n">
            <Pronunciation variety="GB" notation="IPA">bæŋk</Pronunciation>
            <Pronunciation variety="US" notation="IPA">bænk</Pronunciation>
          </Lemma><Sense id="bank-s" synset="noun"/></LexicalEntry>
          <LexicalEntry id="shore-n"><Lemma writtenForm="shore" partOfSpeech="n"/><Sense id="shore-s" synset="noun"/></LexicalEntry>
          <LexicalEntry id="bank-v"><Lemma writtenForm="bank" partOfSpeech="v"><Pronunciation notation="IPA">verb-sound</Pronunciation></Lemma><Sense synset="verb"/></LexicalEntry>
          <Synset id="noun" partOfSpeech="n" members="bank-s shore-s"><Definition>same definition</Definition></Synset>
          <Synset id="verb" partOfSpeech="v" members="bank-v"><Definition>same definition</Definition></Synset>
          <LexicalEntry id="satellite"><Lemma writtenForm="ready" partOfSpeech="a"/><Sense synset="sat"/></LexicalEntry>
          <Synset id="sat" partOfSpeech="s" members="satellite"><Definition>ready</Definition></Synset>
        </Lexicon>"#;
        let contents = crate::oewn::parse(xml.as_bytes(), "oewn-test").unwrap();
        let dir = TempDir::new("metadata-homographs");
        let index = metadata_index(&dir, &contents);
        let bank = crate::lookup::article(&index, addict_core::translation::Source::En, "bank")
            .unwrap()
            .unwrap();
        assert_eq!(
            bank.senses.len(),
            2,
            "differing POS prevents definition deduplication"
        );
        assert_eq!(
            bank.senses[0].part_of_speech.as_ref().unwrap().category,
            Some(Pos::Noun)
        );
        assert_eq!(
            bank.senses[1].part_of_speech.as_ref().unwrap().category,
            Some(Pos::Verb)
        );
        assert_eq!(bank.entries[0].pronunciations.len(), 2);
        assert_eq!(bank.entries[0].pronunciations[0].labels, ["GB"]);
        assert_eq!(bank.senses[0].entries[0].id, "bank-n");
        assert_eq!(bank.senses[1].entries[0].id, "bank-v");
        assert_eq!(bank.entries[0].identity.source, "oewn-test");
        let shore = index.find("shore").unwrap().unwrap();
        assert_eq!(
            shore.senses[0].part_of_speech,
            bank.senses[0].part_of_speech
        );
        assert!(
            shore
                .entries
                .iter()
                .all(|entry| entry.pronunciations.is_empty())
        );
        assert_eq!(shore.senses[0].entries[0].headword, "shore");
        assert_eq!(
            index.find("ready").unwrap().unwrap().senses[0]
                .part_of_speech
                .as_ref()
                .unwrap()
                .category,
            Some(Pos::AdjectiveSatellite)
        );
    }

    const SOUNDS: &str = r#"{"word":"banc","lang_code":"ca","pos":"noun","sounds":[{"ipa":"baŋk","tags":["Central Catalan"],"raw_tags":["source label"]},{"mp3_url":"https://example.org/one.mp3","tags":["Valencian"]},{"ipa":"bank","ogg_url":"https://example.org/two.ogg","mp3_url":"https://example.org/two.mp3","tags":["Balearic"]},{"ipa":"bank","ogg_url":"https://example.org/two.ogg","mp3_url":"https://example.org/two.mp3","tags":["Balearic"]},{"ipa":" "},{"tags":["irrelevant"]}],"senses":[{"glosses":["a seat"]}]}
{"word":"banc","lang_code":"ca","pos":"verb","sounds":[{"ipa":"verb"}],"senses":[{"glosses":["a seat"]}]}
{"word":"sense-definició","lang_code":"ca","pos":"unfamiliar-class","sounds":[{"ogg_url":"https://example.org/only.ogg"}]}
"#;

    #[test]
    fn sound_records_keep_independent_audio_labels_unknown_pos_and_definition_free_entries() {
        let contents = crate::kaikki::parse(SOUNDS.as_bytes(), "ca", "sounds").unwrap();
        let dir = TempDir::new("metadata-sounds");
        let index = metadata_index(&dir, &contents);
        let article = index.find("banc").unwrap().unwrap();
        let sounds = &article.entries[0].pronunciations;
        assert_eq!(
            sounds.len(),
            3,
            "only exact duplicates and empty sounds are removed"
        );
        assert_eq!(sounds[0].text.as_deref(), Some("baŋk"));
        assert_eq!(sounds[0].notation.as_deref(), Some("IPA"));
        assert_eq!(sounds[0].raw_labels, ["source label"]);
        assert!(sounds[0].audio.is_empty());
        assert!(sounds[1].text.is_none());
        assert_eq!(sounds[1].audio[0].url, "https://example.org/one.mp3");
        assert_eq!(sounds[2].audio.len(), 2);
        let unknown = index.find("sense-definició").unwrap().unwrap();
        assert!(unknown.senses.is_empty());
        assert_eq!(
            unknown.entries[0].part_of_speech.as_ref().unwrap().category,
            None
        );
        assert_eq!(
            unknown.entries[0].part_of_speech.as_ref().unwrap().label,
            "unfamiliar-class"
        );
        assert_eq!(article.senses[1].entries[0], article.entries[1].identity);
    }

    #[test]
    fn freedict_keeps_form_scoped_pronunciation_and_nested_grammar_before_flattening() {
        use addict_core::dictionary::Pos;
        let xml = r#"<TEI><text><body><entry>
          <form><orth>bank</orth><pron notation="IPA">bæŋk</pron></form>
          <form><orth>bank-alt</orth><pron notation="source">alternate</pron></form>
          <gramGrp><pos>noun</pos></gramGrp>
          <sense><def>outer noun</def><sense><def>nested verb</def><gramGrp><pos>verb</pos></gramGrp></sense></sense>
        </entry><entry><form><orth>bank</orth><pron>another entry</pron></form><sense><gramGrp><pos>unlisted</pos></gramGrp><cit type="trans" xml:lang="ru"><quote>банк</quote></cit></sense></entry>
        </body></text></TEI>"#;
        let contents = crate::freedict::parse(xml.as_bytes(), "tei").unwrap();
        let dir = TempDir::new("metadata-freedict");
        let article = metadata_index(&dir, &contents)
            .find("bank")
            .unwrap()
            .unwrap();
        assert_eq!(
            article.senses[0].part_of_speech.as_ref().unwrap().category,
            Some(Pos::Noun)
        );
        assert_eq!(
            article.senses[1].part_of_speech.as_ref().unwrap().category,
            Some(Pos::Verb)
        );
        assert_eq!(
            article.entries[0].pronunciations[0].text.as_deref(),
            Some("bæŋk")
        );
        assert!(
            article
                .entries
                .iter()
                .flat_map(|entry| &entry.pronunciations)
                .all(|sound| sound.text.as_deref() != Some("alternate"))
        );
        assert!(article.entries.iter().any(|entry| {
            entry
                .part_of_speech
                .as_ref()
                .is_some_and(|pos| pos.label == "unlisted")
        }));
        assert_eq!(article.translations, ["банк"]);
        assert!(
            article.senses[1].entries.len() >= 2,
            "nested grammar scope and parent entry are explicit"
        );
    }

    #[test]
    fn freedict_repeated_spellings_collect_every_form_sound_once() {
        let xml = r#"<TEI><entry>
          <form><orth>lead</orth><orth>lead</orth><pron notation="IPA">liːd</pron><pron notation="IPA">liːd</pron></form>
          <form><orth>lead</orth><pron notation="IPA">lɛd</pron></form>
          <form><orth>lead</orth><pron notation="IPA">liːd</pron></form>
          <form><orth>other</orth><pron>unrelated</pron></form>
          <sense><def>shared definition</def><cit type="trans" xml:lang="ru"><quote>свинец</quote><quote>свинец</quote></cit></sense>
        </entry></TEI>"#;
        let contents = crate::freedict::parse(xml.as_bytes(), "tei").unwrap();
        assert_eq!(contents.headwords.len(), 2);
        let lead = &contents.headwords[0];
        assert_eq!(lead.word, "lead");
        assert_eq!(lead.senses.len(), 1);
        assert_eq!(lead.entries.len(), 1);
        assert_eq!(lead.entries[0].entry.identity.id, "entry-1");
        assert_eq!(
            lead.entries[0]
                .entry
                .pronunciations
                .iter()
                .map(|sound| sound.text.as_deref())
                .collect::<Vec<_>>(),
            [Some("liːd"), Some("lɛd")]
        );
        assert_eq!(
            encode(&contents).unwrap(),
            encode(&crate::freedict::parse(xml.as_bytes(), "tei").unwrap()).unwrap()
        );
        let dir = TempDir::new("metadata-freedict-repeated");
        let index = metadata_index(&dir, &contents);
        let article = crate::lookup::article(&index, addict_core::translation::Source::En, "lead")
            .unwrap()
            .unwrap();
        assert_eq!(article.entries[0], lead.entries[0].entry);
        assert_eq!(article.senses[0].definition, "shared definition");
        assert_eq!(
            article.senses[0].entries,
            [article.entries[0].identity.clone()]
        );
        assert_eq!(article.translations, ["свинец"]);
    }

    #[test]
    fn freedict_form_grammar_stays_with_its_spelling_and_falls_back_in_scope_order() {
        use addict_core::dictionary::Pos;
        let xml = r#"<TEI><entry>
          <form><orth>nominal</orth><gramGrp><pos>noun</pos></gramGrp><pron>noun-sound</pron></form>
          <form><orth>verbal</orth><pron>verb-sound</pron><gramGrp><pos>verb</pos></gramGrp></form>
          <form><orth>fallback</orth></form>
          <sense><def>shared</def><sense><def>scoped</def><gramGrp><pos>adverb</pos></gramGrp><sense><def>inherited</def></sense><sense><pos>unknown-scope</pos><def>nearest</def></sense></sense></sense>
          <gramGrp><pos>adjective</pos></gramGrp>
        </entry><entry><form><orth>translation-only</orth><pos>noun</pos><pron>only-sound</pron></form><cit type="trans" xml:lang="ru"><quote>только</quote></cit></entry></TEI>"#;
        let contents = crate::freedict::parse(xml.as_bytes(), "tei").unwrap();
        for (word, category) in [
            ("nominal", Pos::Noun),
            ("verbal", Pos::Verb),
            ("fallback", Pos::Adjective),
        ] {
            let headword = contents
                .headwords
                .iter()
                .find(|headword| headword.word == word)
                .unwrap();
            assert_eq!(
                headword.entries[0]
                    .entry
                    .part_of_speech
                    .as_ref()
                    .unwrap()
                    .category,
                Some(category)
            );
            assert_eq!(
                contents.senses[headword.senses[0] as usize]
                    .part_of_speech
                    .as_ref()
                    .unwrap()
                    .category,
                Some(category)
            );
        }
        let dir = TempDir::new("metadata-freedict-form-grammar");
        let index = metadata_index(&dir, &contents);
        for (word, category, sound) in [
            ("nominal", Pos::Noun, "noun-sound"),
            ("verbal", Pos::Verb, "verb-sound"),
        ] {
            let article = index.find(word).unwrap().unwrap();
            assert_eq!(
                article.senses[0].part_of_speech.as_ref().unwrap().category,
                Some(category)
            );
            assert_eq!(
                article.senses[1].part_of_speech.as_ref().unwrap().category,
                Some(Pos::Adverb)
            );
            assert_eq!(
                article.senses[2].part_of_speech.as_ref().unwrap().category,
                Some(Pos::Adverb)
            );
            assert_eq!(
                article.senses[3].part_of_speech.as_ref().unwrap().label,
                "unknown-scope"
            );
            assert_eq!(
                article.entries[0].pronunciations[0].text.as_deref(),
                Some(sound)
            );
            assert!(
                article
                    .senses
                    .iter()
                    .all(|sense| sense.entries.contains(&article.entries[0].identity))
            );
        }
        let only = index.find("translation-only").unwrap().unwrap();
        assert!(only.senses.is_empty());
        assert_eq!(only.translations, ["только"]);
        assert_eq!(
            only.entries[0].part_of_speech.as_ref().unwrap().category,
            Some(Pos::Noun)
        );
        assert_eq!(
            only.entries[0].pronunciations[0].text.as_deref(),
            Some("only-sound")
        );
    }

    #[test]
    fn freedict_same_spelling_with_different_form_pos_keeps_distinct_sense_associations() {
        use addict_core::dictionary::Pos;
        let xml = r#"<TEI><entry>
          <form><orth>lead</orth><pos>noun</pos><pron notation="IPA">lɛd</pron></form>
          <form><orth>lead</orth><pos>verb</pos><pron notation="IPA">liːd</pron></form>
          <form><orth>lead</orth><pos>noun</pos><pron notation="IPA">lɛd</pron></form>
          <gramGrp><pos>adjective</pos></gramGrp>
          <sense><def>shared definition</def><cit type="trans" xml:lang="ru"><quote>вести</quote></cit></sense>
          <sense><pos>adverb</pos><def>explicit sense</def></sense>
        </entry></TEI>"#;
        let contents = crate::freedict::parse(xml.as_bytes(), "tei").unwrap();
        let headword = &contents.headwords[0];
        assert_eq!(
            headword.entries.len(),
            3,
            "two form grammars and one explicit sense grammar"
        );
        assert_eq!(
            headword.senses.len(),
            3,
            "shared explicit sense grammar is not duplicated"
        );
        assert_eq!(headword.entries[0].entry.identity.id, "entry-1");
        assert_eq!(headword.entries[1].entry.identity.id, "entry-1-form-1");
        assert_eq!(headword.entries[0].senses[1], headword.entries[1].senses[1]);
        let dir = TempDir::new("metadata-freedict-form-homographs");
        let index = metadata_index(&dir, &contents);
        let article = crate::lookup::article(&index, addict_core::translation::Source::En, "lead")
            .unwrap()
            .unwrap();
        for (at, category, sound) in [(0, Pos::Noun, "lɛd"), (1, Pos::Verb, "liːd")] {
            let entry = &article.entries[at];
            assert_eq!(
                entry.part_of_speech.as_ref().unwrap().category,
                Some(category)
            );
            assert_eq!(entry.pronunciations.len(), 1);
            assert_eq!(entry.pronunciations[0].text.as_deref(), Some(sound));
            assert_eq!(entry.identity.source, "tei");
            assert_eq!(entry.identity.headword, "lead");
            let sense = article
                .senses
                .iter()
                .find(|sense| {
                    sense.definition == "shared definition"
                        && sense.part_of_speech.as_ref().unwrap().category == Some(category)
                })
                .unwrap();
            assert_eq!(sense.entries, [entry.identity.clone()]);
        }
        let explicit = article
            .senses
            .iter()
            .find(|sense| sense.definition == "explicit sense")
            .unwrap();
        assert_eq!(
            explicit.part_of_speech.as_ref().unwrap().category,
            Some(Pos::Adverb)
        );
        assert_eq!(explicit.entries.len(), 3);
        assert_eq!(article.translations, ["вести"]);
        assert_eq!(
            encode(&contents).unwrap(),
            encode(&crate::freedict::parse(xml.as_bytes(), "tei").unwrap()).unwrap()
        );
    }

    #[test]
    fn freedict_translation_only_scopes_keep_their_entry_and_spelling_sounds() {
        use addict_core::{dictionary::Pos, translation::Source as Language};
        let xml = r#"<TEI><entry>
          <form><orth>lead</orth><pron notation="IPA">lɛd</pron><pron notation="IPA">lɛd</pron></form>
          <form><orth>lead</orth><pron notation="IPA">lɛːd</pron></form>
          <form><orth>metal</orth><pron notation="IPA">metal-sound</pron></form>
          <sense><pos>noun</pos><cit type="trans" xml:lang="ru"><quote>свинец</quote></cit></sense>
        </entry><entry>
          <form><orth>lead</orth><pron notation="IPA">liːd</pron></form>
          <form><orth>guide</orth><pron notation="IPA">guide-sound</pron></form>
          <sense><pos>verb</pos><cit type="trans" xml:lang="ru"><quote>вести</quote></cit></sense>
        </entry></TEI>"#;
        let contents = crate::freedict::parse(xml.as_bytes(), "tei-scopes").unwrap();
        assert!(contents.senses.is_empty());
        assert!(contents.headwords.iter().all(|word| word.senses.is_empty()));
        assert_eq!(
            encode(&contents).unwrap(),
            encode(&crate::freedict::parse(xml.as_bytes(), "tei-scopes").unwrap()).unwrap()
        );
        let dir = TempDir::new("metadata-freedict-translation-scopes");
        let index = metadata_index(&dir, &contents);
        for (word, expected) in [
            (
                "lead",
                vec![
                    (Pos::Noun, "entry-1", vec!["lɛd", "lɛːd"]),
                    (Pos::Verb, "entry-2", vec!["liːd"]),
                ],
            ),
            ("metal", vec![(Pos::Noun, "entry-1", vec!["metal-sound"])]),
            ("guide", vec![(Pos::Verb, "entry-2", vec!["guide-sound"])]),
        ] {
            let article = crate::lookup::article(&index, Language::En, word)
                .unwrap()
                .unwrap();
            assert!(article.senses.is_empty());
            assert!(article.sense_metadata.is_empty());
            let scoped: Vec<_> = article
                .entries
                .iter()
                .filter(|entry| entry.part_of_speech.is_some())
                .collect();
            assert_eq!(scoped.len(), expected.len());
            for (scoped, (pos, parent, sounds)) in scoped.iter().zip(expected) {
                assert_eq!(scoped.part_of_speech.as_ref().unwrap().category, Some(pos));
                assert_eq!(scoped.identity.source, "tei-scopes");
                assert_eq!(scoped.identity.headword, word);
                assert_eq!(scoped.identity.id, format!("{parent}-scope-0"));
                assert!(
                    article
                        .entries
                        .iter()
                        .any(|entry| entry.identity.id == parent)
                );
                assert_eq!(
                    scoped
                        .pronunciations
                        .iter()
                        .map(|sound| sound.text.as_deref().unwrap())
                        .collect::<Vec<_>>(),
                    sounds
                );
                assert!(
                    scoped
                        .pronunciations
                        .iter()
                        .all(|sound| sound.notation.as_deref() == Some("IPA"))
                );
            }
            let translations: &[&str] = match word {
                "lead" => &["свинец", "вести"],
                "metal" => &["свинец"],
                "guide" => &["вести"],
                _ => unreachable!(),
            };
            assert_eq!(article.translations, translations);
            let grouped = crate::lookup::grouped(
                &[(crate::import::Kind::Russian, &index)],
                Language::En,
                word,
            )
            .unwrap()
            .unwrap();
            assert!(grouped.definitions.is_none());
            assert!(grouped.sense_metadata.is_empty());
            assert_eq!(grouped.entries, article.entries);
            assert_eq!(grouped.russian.as_ref().unwrap(), &article.translations);
        }
    }

    #[test]
    fn dbnary_resolves_all_canonical_phonetics_in_either_block_order() {
        use addict_core::dictionary::Pos;
        let entry = "cat:e a ontolex:LexicalEntry;\n dcterms:language lexvo:cat;\n rdfs:label \"banc\";\n lexinfo:partOfSpeech lexinfo:noun;\n dbnary:partOfSpeech \"Nom\";\n ontolex:canonicalForm cat:form;\n ontolex:sense cat:s .\n\n";
        let form = "cat:form a ontolex:Form;\n ontolex:phoneticRep \"bank\"@ca-fonipa , \"baŋk\"@ca-fonipa;\n ontolex:phoneticRep \"banc\"@ca-fonipa .\n\n";
        let sense = "cat:s skos:definition [ rdf:value \"a seat\"@ca ] .\n\n";
        for turtle in [
            format!("{entry}{form}{sense}"),
            format!("{form}{sense}{entry}"),
        ] {
            let contents = crate::dbnary::parse(turtle.as_bytes(), "cat", "dbnary").unwrap();
            let dir = TempDir::new("metadata-dbnary");
            let article = metadata_index(&dir, &contents)
                .find("banc")
                .unwrap()
                .unwrap();
            assert_eq!(article.entries[0].pronunciations.len(), 3);
            assert_eq!(
                article.entries[0].pronunciations[0].notation.as_deref(),
                Some("IPA")
            );
            assert_eq!(
                article.entries[0].part_of_speech.as_ref().unwrap().category,
                Some(Pos::Noun)
            );
            assert_eq!(
                article.entries[0].part_of_speech.as_ref().unwrap().label,
                "Nom"
            );
            assert_eq!(
                article.senses[0].part_of_speech,
                article.entries[0].part_of_speech
            );
            assert_eq!(article.senses[0].entries[0].id, "cat:e");
        }
    }

    #[test]
    fn dbnary_notation_is_named_by_the_phonetic_subtag_alone() {
        let entry = "cat:e a ontolex:LexicalEntry;\n dcterms:language lexvo:cat;\n rdfs:label \"banc\";\n lexinfo:partOfSpeech lexinfo:noun;\n ontolex:canonicalForm cat:form .\n\n";
        let form = "cat:form a ontolex:Form;\n ontolex:phoneticRep \"ipa\"@CA-FONIPA , \"typed\"^^xsd:string , \"tagged\"@ca , \"bare\" .\n\n";
        let contents =
            crate::dbnary::parse(format!("{entry}{form}").as_bytes(), "cat", "dbnary").unwrap();
        let dir = TempDir::new("metadata-dbnary-notation");
        let article = metadata_index(&dir, &contents)
            .find("banc")
            .unwrap()
            .unwrap();
        let notations: Vec<_> = article.entries[0]
            .pronunciations
            .iter()
            .map(|sound| (sound.text.as_deref().unwrap(), sound.notation.as_deref()))
            .collect();
        assert_eq!(
            notations,
            [
                ("ipa", Some("IPA")),
                ("typed", None),
                ("tagged", None),
                ("bare", None)
            ]
        );
    }

    #[test]
    fn metadata_merges_from_every_source_with_headword_provenance() {
        use crate::{import::Kind, lookup};
        use addict_core::translation::Source as Language;
        let dir = TempDir::new("metadata-merge");
        let mut indexes = Vec::new();
        for (at, kind) in [
            Kind::Definitions,
            Kind::Russian,
            Kind::English,
            Kind::Synonyms,
        ]
        .into_iter()
        .enumerate()
        {
            let mut contents =
                crate::kaikki::parse(SOUNDS.as_bytes(), "ca", &format!("source-{at}")).unwrap();
            contents.forms.push(format::ExceptionForm {
                form: "bancs".to_owned(),
                lemmas: vec!["banc".to_owned()],
            });
            let path = dir.path().join(format!("{at}.index"));
            std::fs::write(&path, encode(&contents).unwrap()).unwrap();
            indexes.push((kind, Index::open(&path).unwrap()));
        }
        let borrowed: Vec<_> = indexes.iter().map(|(kind, index)| (*kind, index)).collect();
        let grouped = lookup::grouped(&borrowed, Language::Ca, "bancs")
            .unwrap()
            .unwrap();
        assert_eq!(grouped.entries.len(), 8);
        assert_eq!(grouped.sense_metadata.len(), 8);
        assert_eq!(grouped.sense_metadata[4].source, "source-2");
        assert_eq!(grouped.sense_metadata[4].headword, "banc");
        assert!(grouped.sense_metadata[4].sense.part_of_speech.is_some());
        assert!(
            grouped
                .entries
                .iter()
                .all(|entry| entry.identity.headword == "banc")
        );
        assert_eq!(grouped.entries[0].identity.source, "source-0");
        assert_eq!(grouped.entries[6].identity.source, "source-3");
        assert_eq!(grouped.definitions.as_ref().unwrap().len(), 2);
        let mut combined = crate::Combined::default();
        combined.set_definitions(Index::open(&dir.path().join("0.index")).unwrap());
        combined.set_translations(Index::open(&dir.path().join("1.index")).unwrap());
        assert_eq!(combined.article("bancs").unwrap().unwrap().entries.len(), 4);
    }

    #[test]
    fn legacy_v1_articles_remain_usable_without_lexical_metadata() {
        let dir = TempDir::new("metadata-v1");
        let mut bytes = encode(&sample()).unwrap();
        // Metadata-free record layout is identical to v1; only the writer version changed.
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        let index = Index::open(&written(&dir, &bytes)).unwrap();
        let article = index.find("bank").unwrap().unwrap();
        assert_eq!(article.senses.len(), 2);
        assert!(article.entries.is_empty());
        assert!(article.sense_metadata.is_empty());
        assert!(
            article
                .senses
                .iter()
                .all(|sense| sense.part_of_speech.is_none() && sense.entries.is_empty())
        );
        assert_eq!(index.exception_lemmas("lay").unwrap(), ["lay", "lie"]);
    }

    #[test]
    fn corrupted_metadata_never_panics_and_invalid_new_records_are_rejected() {
        let contents = crate::kaikki::parse(SOUNDS.as_bytes(), "ca", "sound-damage").unwrap();
        let good = encode(&contents).unwrap();
        let dir = TempDir::new("metadata-damage");
        let word_section = header(&good).section(SectionId::WordRecords);
        for at in word_section.offset as usize..(word_section.offset + word_section.len) as usize {
            let mut bytes = good.clone();
            bytes[at] ^= 0xff;
            let index = Index::open(&written(&dir, &bytes)).unwrap();
            // The fallible lookup may succeed if a changed byte is still a valid string.
            let _ = index.find("banc");
            let _ = index.find("sense-definició");
        }
        let index = metadata_index(&dir, &contents);
        let mut record = index.search(WORDS, "banc").unwrap().unwrap();
        let at = record
            .windows(4)
            .position(|bytes| bytes == b"ENT2")
            .unwrap();
        record[at + 4..at + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(format::decode_word(&record).is_err());
        record[at] = b'?';
        assert!(format::decode_word(&record).is_err());
        let mut invalid = contents.clone();
        invalid.headwords[0].entries[0].entry.pronunciations[0].text = Some(" ".to_owned());
        assert!(matches!(
            encode(&invalid),
            Err(format::WriteError::Metadata(_))
        ));
        let mut invalid = contents;
        invalid.headwords[0].entries[0].senses.push(u32::MAX);
        assert!(matches!(
            encode(&invalid),
            Err(format::WriteError::UnknownSense { .. })
        ));
    }

    #[test]
    #[ignore = "needs cached sources: set ADDICT_SOURCE_ROOT to repository root"]
    fn real_sources_rebuild_with_metadata_and_legacy_indexes_still_open() {
        use std::io::{BufRead, BufReader};
        let root =
            PathBuf::from(std::env::var_os("ADDICT_SOURCE_ROOT").expect("ADDICT_SOURCE_ROOT"));
        let dir = TempDir::new("real-metadata");
        for (file, word, sounds_required) in [
            (
                "experiments/exp7/data/english-wordnet-2025.xml.gz",
                "lead",
                true,
            ),
            (
                "experiments/exp7/data/freedict-eng-rus-2025.11.23.src.tar.xz",
                "bank",
                true,
            ),
            ("experiments/exp9/data/omw-es-2.0.tar.xz", "banco", false),
            (
                "experiments/exp9/data/dbnary-ca_dbnary_ontolex_2026-10-01.ttl.bz2",
                "parlament",
                true,
            ),
            (
                "experiments/exp9/data/sinonims-cat-2.3.2-th_ca_ES.dat",
                "banc",
                false,
            ),
            (
                "experiments/exp9/data/wikdict-2026-06-ca-ru.sqlite3",
                "banc",
                false,
            ),
        ] {
            let imported = crate::import::import(&root.join(file), dir.path()).expect(file);
            let first = std::fs::read(&imported.path).unwrap();
            crate::import::import(&root.join(file), dir.path()).expect("deterministic reimport");
            assert_eq!(first, std::fs::read(&imported.path).unwrap(), "{file}");
            let index = Index::open(&imported.path).unwrap();
            let article = index.find(word).unwrap().expect(word);
            let sounds: usize = article
                .entries
                .iter()
                .map(|entry| entry.pronunciations.len())
                .sum();
            if sounds_required {
                assert!(sounds > 0, "{file}: {word} pronunciation");
            }
            if file.contains("wikdict") {
                assert!(article.entries.is_empty());
            } else {
                assert!(
                    article
                        .entries
                        .iter()
                        .any(|entry| entry.part_of_speech.is_some()),
                    "{file}: grammar"
                );
            }
            eprintln!(
                "{file}: {} headwords, {word}: {} senses, {} entries, {sounds} sounds; deterministic",
                imported.headwords,
                article.senses.len(),
                article.entries.len()
            );
        }
        for (file, language) in [
            ("experiments/exp9/data/kaikki-es-Espanol.jsonl.gz", "es"),
            ("experiments/exp9/data/kaikki-en-Spanish.jsonl.gz", "es"),
            ("experiments/exp9/data/kaikki-en-Catalan.jsonl.gz", "ca"),
        ] {
            let decoder =
                flate2::read::GzDecoder::new(std::fs::File::open(root.join(file)).unwrap());
            let sample = BufReader::new(decoder)
                .lines()
                .take(1000)
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join("\n");
            let contents = crate::kaikki::parse(sample.as_bytes(), language, file).expect(file);
            let sound_count: usize = contents
                .headwords
                .iter()
                .flat_map(|word| &word.entries)
                .map(|entry| entry.entry.pronunciations.len())
                .sum();
            let audio_count: usize = contents
                .headwords
                .iter()
                .flat_map(|word| &word.entries)
                .flat_map(|entry| &entry.entry.pronunciations)
                .map(|sound| sound.audio.len())
                .sum();
            assert!(sound_count > 0);
            let index = metadata_index(&dir, &contents);
            for word in &contents.headwords {
                let article = index.find(&word.word).unwrap().unwrap();
                assert_eq!(article.entries.len(), word.entries.len());
            }
            eprintln!(
                "{file}: first 1000 source entries -> {} headwords, {sound_count} sounds, {audio_count} audio URLs; deterministic",
                contents.headwords.len()
            );
        }
        if let Some(legacy) = std::env::var_os("ADDICT_LEGACY_INDEX_DIR") {
            for entry in std::fs::read_dir(legacy).unwrap() {
                let path = entry.unwrap().path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "index")
                {
                    let index = Index::open(&path).expect("open real legacy index");
                    for word in ["bank", "banco", "banc", "банк"] {
                        let _ = index.find(word).unwrap();
                    }
                    eprintln!("legacy {}: open and bounded lookup pass", path.display());
                }
            }
        }
    }
}
