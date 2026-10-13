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

use addict_core::dictionary::{Article, Dictionary, Sense};

use crate::format::{self, Damage, Entry, Header, Section, SectionId};

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

/// An open index file.
#[derive(Debug)]
pub struct Index {
    file: File,
    path: PathBuf,
    header: Header,
    source: String,
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
        let mut bytes = [0; format::HEADER_LEN];
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
        let (offsets, translations) =
            format::decode_word(&record).map_err(|damage| self.damaged(damage))?;
        let mut senses = Vec::new();
        for offset in offsets {
            let record = self.sense(offset)?;
            let mut synonyms: Vec<String> = Vec::new();
            for member in record.members {
                if member != word && !synonyms.contains(&member) {
                    synonyms.push(member);
                }
            }
            senses.push(Sense {
                definition: record.definition,
                synonyms,
            });
        }
        Ok(Some(Article {
            senses,
            translations,
        }))
    }

    /// The lemmas the exception list gives for `form`, in its order; empty
    /// when the form is not in the list.
    pub fn exception_lemmas(&self, form: &str) -> Result<Vec<String>, Error> {
        match self.search(FORMS, form)? {
            Some(record) => format::decode_form(&record).map_err(|damage| self.damaged(damage)),
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
                    definition: "sloping land beside water".to_owned(),
                    synonyms: texts(&["shore"]),
                },
                Sense {
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
        version[8] = 2;
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
}
