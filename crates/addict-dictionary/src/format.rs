//! The layout of an index file, its writer and the decoders of its parts.
//!
//! All integers are little-endian.
//!
//! ```text
//! header   magic "ADDICTIX" | version u32 | section count u32
//!          per section: offset u64 | length u64     (from the file start)
//! sections source        UTF-8 id of the imported source
//!          senses        sense records, each: length u32 | definition text
//!                        | member count u32 | member texts
//!          word table    one entry per headword, sorted by the bytes of it
//!          word keys     the headwords, back to back
//!          word records  sense count u32 | sense offsets u64 (into senses)
//!                        | translation count u32 | translation texts
//!          form table    one entry per exception form, sorted
//!          form keys     the forms, back to back
//!          form records  lemma count u32 | lemma texts
//! ```
//!
//! A table entry is `key offset u64 | key length u32 | record offset u64 |
//! record length u32`, the offsets into the keys and the records section of
//! the same table. A text is `length u32 | UTF-8 bytes`.
//!
//! A sense record is stored once and shared by every headword that has the
//! sense: its members are the lemmas of the whole synset, and the reader
//! drops the looked-up word from them to get the synonyms. A source without
//! synsets writes one record per sense, with the synonyms as members.
//!
//! This module does no I/O: [`encode`] gives the bytes of a file and the
//! decoders take bytes the reader has read. A decoder never trusts a length
//! or a count: what does not fit the bytes it was given is a [`Damage`].

use std::{collections::BTreeMap, fmt};

/// The first bytes of every index file.
pub const MAGIC: [u8; 8] = *b"ADDICTIX";

/// The version of the layout this module writes and reads.
pub const VERSION: u32 = 1;

/// The sections of a file, in the order of the header.
pub const SECTIONS: usize = 8;

/// The length of the header: magic, version, section count, section table.
pub const HEADER_LEN: usize = 16 + SECTIONS * 16;

/// The length of one table entry.
pub const ENTRY_LEN: u64 = 24;

/// The longest key and the longest record, in bytes. The writer refuses
/// more and the reader treats more as damage, so one read stays bounded.
pub const MAX_RECORD: u32 = 1 << 20;

/// The longest source id, in bytes.
pub const MAX_SOURCE: u64 = 255;

/// A section of the file, by its place in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionId {
    Source = 0,
    Senses = 1,
    WordTable = 2,
    WordKeys = 3,
    WordRecords = 4,
    FormTable = 5,
    FormKeys = 6,
    FormRecords = 7,
}

/// Everything an index file holds: the input of [`encode`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Contents {
    /// The id of the imported source, such as `oewn-2025`.
    pub source: String,
    /// The shared sense records; headwords refer to them by position.
    pub senses: Vec<SenseRecord>,
    /// The headwords, sorted by their bytes, each once.
    pub headwords: Vec<Headword>,
    /// The exception list, sorted by the bytes of the form, each form once.
    pub forms: Vec<ExceptionForm>,
}

impl Contents {
    /// The contents of `source` whose headwords are the words of `headwords`,
    /// in their order, each with its sense records in order; no translations
    /// and no forms. The builder of the readers of sources without synsets.
    pub(crate) fn of_senses(source: &str, headwords: BTreeMap<String, Vec<SenseRecord>>) -> Self {
        let mut contents = Contents {
            source: source.to_owned(),
            ..Contents::default()
        };
        for (word, senses) in headwords {
            let first = contents.senses.len() as u32;
            contents.senses.extend(senses);
            contents.headwords.push(Headword {
                word,
                senses: (first..contents.senses.len() as u32).collect(),
                translations: Vec::new(),
            });
        }
        contents
    }
}

/// One sense, shared by the headwords that have it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SenseRecord {
    pub definition: String,
    /// The words of the sense, the headword itself included.
    pub members: Vec<String>,
}

/// One headword as written in the source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Headword {
    pub word: String,
    /// Positions in [`Contents::senses`], in the order of the article.
    pub senses: Vec<u32>,
    /// Translations for the article as a whole.
    pub translations: Vec<String>,
}

/// An irregular form and the lemmas it is a form of.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExceptionForm {
    pub form: String,
    pub lemmas: Vec<String>,
}

/// Why [`encode`] refused its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// The source id is empty or longer than [`MAX_SOURCE`].
    Source,
    /// A headword or a form is empty.
    EmptyKey,
    /// The keys of a table are not strictly ascending; this is the later one.
    Unsorted(String),
    /// A headword refers to a sense that is not there.
    UnknownSense { word: String, sense: u32 },
    /// A key or a record is longer than [`MAX_RECORD`]; this is its key or
    /// the start of its definition.
    TooLarge(String),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source => write!(f, "the source id is empty or too long"),
            Self::EmptyKey => write!(f, "an empty headword or form"),
            Self::Unsorted(key) => write!(f, "{key:?} is out of order or repeated"),
            Self::UnknownSense { word, sense } => {
                write!(f, "{word:?} refers to the missing sense {sense}")
            }
            Self::TooLarge(what) => write!(f, "the record of {what:?} is too large"),
        }
    }
}

impl std::error::Error for WriteError {}

/// The bytes of the index file of `contents`. The same contents give the
/// same bytes.
pub fn encode(contents: &Contents) -> Result<Vec<u8>, WriteError> {
    let source = contents.source.as_bytes();
    if source.is_empty() || source.len() as u64 > MAX_SOURCE {
        return Err(WriteError::Source);
    }

    let mut senses = Vec::new();
    let mut sense_offsets = Vec::with_capacity(contents.senses.len());
    for sense in &contents.senses {
        let mut body = Vec::new();
        put_text(&mut body, &sense.definition)?;
        put_texts(&mut body, &sense.members)?;
        let len = bounded(body.len(), &sense.definition)?;
        sense_offsets.push(senses.len() as u64);
        senses.extend_from_slice(&len.to_le_bytes());
        senses.extend_from_slice(&body);
    }

    let mut words = Table::default();
    for headword in &contents.headwords {
        let mut record = Vec::new();
        put_count(&mut record, headword.senses.len(), &headword.word)?;
        for &sense in &headword.senses {
            let offset =
                sense_offsets
                    .get(sense as usize)
                    .ok_or_else(|| WriteError::UnknownSense {
                        word: headword.word.clone(),
                        sense,
                    })?;
            record.extend_from_slice(&offset.to_le_bytes());
        }
        put_texts(&mut record, &headword.translations)?;
        words.push(&headword.word, &record)?;
    }

    let mut forms = Table::default();
    for form in &contents.forms {
        let mut record = Vec::new();
        put_texts(&mut record, &form.lemmas)?;
        forms.push(&form.form, &record)?;
    }

    let sections: [&[u8]; SECTIONS] = [
        source,
        &senses,
        &words.entries,
        &words.keys,
        &words.records,
        &forms.entries,
        &forms.keys,
        &forms.records,
    ];
    let total = HEADER_LEN + sections.iter().map(|section| section.len()).sum::<usize>();
    let mut file = Vec::with_capacity(total);
    file.extend_from_slice(&MAGIC);
    file.extend_from_slice(&VERSION.to_le_bytes());
    file.extend_from_slice(&(SECTIONS as u32).to_le_bytes());
    let mut offset = HEADER_LEN as u64;
    for section in sections {
        file.extend_from_slice(&offset.to_le_bytes());
        file.extend_from_slice(&(section.len() as u64).to_le_bytes());
        offset += section.len() as u64;
    }
    for section in sections {
        file.extend_from_slice(section);
    }
    Ok(file)
}

/// A table under construction: its entries, its keys and its records.
#[derive(Default)]
struct Table {
    entries: Vec<u8>,
    keys: Vec<u8>,
    records: Vec<u8>,
    last: Option<Vec<u8>>,
}

impl Table {
    fn push(&mut self, key: &str, record: &[u8]) -> Result<(), WriteError> {
        if key.is_empty() {
            return Err(WriteError::EmptyKey);
        }
        if self
            .last
            .as_deref()
            .is_some_and(|last| last >= key.as_bytes())
        {
            return Err(WriteError::Unsorted(key.to_owned()));
        }
        let key_len = bounded(key.len(), key)?;
        let record_len = bounded(record.len(), key)?;
        self.entries
            .extend_from_slice(&(self.keys.len() as u64).to_le_bytes());
        self.entries.extend_from_slice(&key_len.to_le_bytes());
        self.entries
            .extend_from_slice(&(self.records.len() as u64).to_le_bytes());
        self.entries.extend_from_slice(&record_len.to_le_bytes());
        self.keys.extend_from_slice(key.as_bytes());
        self.records.extend_from_slice(record);
        self.last = Some(key.as_bytes().to_vec());
        Ok(())
    }
}

/// `len` as a stored length, refused when over [`MAX_RECORD`]; `what` names
/// the record in the error.
fn bounded(len: usize, what: &str) -> Result<u32, WriteError> {
    u32::try_from(len)
        .ok()
        .filter(|len| *len <= MAX_RECORD)
        .ok_or_else(|| WriteError::TooLarge(what.chars().take(40).collect()))
}

fn put_count(out: &mut Vec<u8>, count: usize, what: &str) -> Result<(), WriteError> {
    out.extend_from_slice(&bounded(count, what)?.to_le_bytes());
    Ok(())
}

fn put_text(out: &mut Vec<u8>, text: &str) -> Result<(), WriteError> {
    out.extend_from_slice(&bounded(text.len(), text)?.to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

fn put_texts(out: &mut Vec<u8>, texts: &[String]) -> Result<(), WriteError> {
    put_count(out, texts.len(), texts.first().map_or("", String::as_str))?;
    texts.iter().try_for_each(|text| put_text(out, text))
}

/// What is wrong with the bytes of an index; the reader adds the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Damage(pub &'static str);

/// Where a section lies in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub offset: u64,
    pub len: u64,
}

/// The header of a file: where its sections are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    sections: [Section; SECTIONS],
}

impl Header {
    /// Reads the header from the first [`HEADER_LEN`] bytes of a file that
    /// is `file_len` bytes long. Every section lies inside the file.
    pub fn parse(bytes: &[u8], file_len: u64) -> Result<Self, Damage> {
        let mut cursor = Cursor(bytes);
        if cursor.take(MAGIC.len())? != MAGIC {
            return Err(Damage("it does not start with the index magic"));
        }
        if cursor.u32()? != VERSION {
            return Err(Damage("its format version is unknown"));
        }
        if cursor.u32()? as usize != SECTIONS {
            return Err(Damage("its section count is wrong"));
        }
        let mut sections = [Section { offset: 0, len: 0 }; SECTIONS];
        for section in &mut sections {
            let offset = cursor.u64()?;
            let len = cursor.u64()?;
            let inside = offset >= HEADER_LEN as u64
                && offset.checked_add(len).is_some_and(|end| end <= file_len);
            if !inside {
                return Err(Damage("a section lies outside the file"));
            }
            *section = Section { offset, len };
        }
        let header = Self { sections };
        if header.section(SectionId::Source).len > MAX_SOURCE {
            return Err(Damage("its source id is too long"));
        }
        for table in [SectionId::WordTable, SectionId::FormTable] {
            if !header.section(table).len.is_multiple_of(ENTRY_LEN) {
                return Err(Damage("a table is not a whole number of entries"));
            }
        }
        Ok(header)
    }

    pub fn section(&self, id: SectionId) -> Section {
        self.sections[id as usize]
    }
}

/// One entry of a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub key_offset: u64,
    pub key_len: u32,
    pub record_offset: u64,
    pub record_len: u32,
}

impl Entry {
    /// Reads an entry from its [`ENTRY_LEN`] bytes. Its key and its record
    /// lie inside `keys` and `records` and are no longer than [`MAX_RECORD`].
    pub fn parse(bytes: &[u8], keys: Section, records: Section) -> Result<Self, Damage> {
        let mut cursor = Cursor(bytes);
        let entry = Self {
            key_offset: cursor.u64()?,
            key_len: cursor.u32()?,
            record_offset: cursor.u64()?,
            record_len: cursor.u32()?,
        };
        if !fits(entry.key_offset, entry.key_len, keys) {
            return Err(Damage("a key lies outside its section"));
        }
        if !fits(entry.record_offset, entry.record_len, records) {
            return Err(Damage("a record lies outside its section"));
        }
        Ok(entry)
    }
}

/// Whether `len` bytes at `offset` of `section` are inside it and bounded.
pub fn fits(offset: u64, len: u32, section: Section) -> bool {
    len <= MAX_RECORD
        && offset
            .checked_add(u64::from(len))
            .is_some_and(|end| end <= section.len)
}

/// The sense offsets and the translations of a word record.
pub fn decode_word(record: &[u8]) -> Result<(Vec<u64>, Vec<String>), Damage> {
    let mut cursor = Cursor(record);
    let mut senses = Vec::new();
    for _ in 0..cursor.u32()? {
        senses.push(cursor.u64()?);
    }
    let translations = cursor.texts()?;
    cursor.end()?;
    Ok((senses, translations))
}

/// A sense record from its body, the bytes after its length.
pub fn decode_sense(body: &[u8]) -> Result<SenseRecord, Damage> {
    let mut cursor = Cursor(body);
    let definition = cursor.text()?;
    let members = cursor.texts()?;
    cursor.end()?;
    Ok(SenseRecord {
        definition,
        members,
    })
}

/// The lemmas of a form record.
pub fn decode_form(record: &[u8]) -> Result<Vec<String>, Damage> {
    let mut cursor = Cursor(record);
    let lemmas = cursor.texts()?;
    cursor.end()?;
    Ok(lemmas)
}

/// The bytes still to decode. Every step checks what is left.
struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], Damage> {
        let (head, tail) = self
            .0
            .split_at_checked(len)
            .ok_or(Damage("a record ends too early"))?;
        self.0 = tail;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, Damage> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| SHORT)?))
    }

    fn u64(&mut self) -> Result<u64, Damage> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| SHORT)?))
    }

    fn text(&mut self) -> Result<String, Damage> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| Damage("a text is not UTF-8"))
    }

    /// A count and that many texts. The count reserves nothing: a false one
    /// runs out of bytes instead.
    fn texts(&mut self) -> Result<Vec<String>, Damage> {
        let mut texts = Vec::new();
        for _ in 0..self.u32()? {
            texts.push(self.text()?);
        }
        Ok(texts)
    }

    fn end(&self) -> Result<(), Damage> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Damage("a record is longer than its contents"))
        }
    }
}

const SHORT: Damage = Damage("a record ends too early");

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn texts(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    /// A small index: a shared sense, a word alone in its sense, a word with
    /// translations only and two exception forms.
    pub(crate) fn sample() -> Contents {
        Contents {
            source: "test-1".to_owned(),
            senses: vec![
                SenseRecord {
                    definition: "sloping land beside water".to_owned(),
                    members: texts(&["bank", "shore"]),
                },
                SenseRecord {
                    definition: "a financial institution".to_owned(),
                    members: texts(&["bank", "depository", "bank"]),
                },
                SenseRecord {
                    definition: "a proposal for a course of action".to_owned(),
                    members: texts(&["advice"]),
                },
            ],
            headwords: vec![
                Headword {
                    word: "Bánk".to_owned(),
                    senses: vec![],
                    translations: texts(&["Банк"]),
                },
                Headword {
                    word: "advice".to_owned(),
                    senses: vec![2],
                    translations: vec![],
                },
                Headword {
                    word: "bank".to_owned(),
                    senses: vec![0, 1],
                    translations: texts(&["берег", "банк"]),
                },
                Headword {
                    word: "shore".to_owned(),
                    senses: vec![0],
                    translations: vec![],
                },
            ],
            forms: vec![
                ExceptionForm {
                    form: "geese".to_owned(),
                    lemmas: texts(&["goose"]),
                },
                ExceptionForm {
                    form: "lay".to_owned(),
                    lemmas: texts(&["lay", "lie"]),
                },
            ],
        }
    }

    pub(crate) fn header(file: &[u8]) -> Header {
        Header::parse(&file[..HEADER_LEN], file.len() as u64).expect("header")
    }

    fn slice(file: &[u8], section: Section) -> &[u8] {
        &file[section.offset as usize..(section.offset + section.len) as usize]
    }

    /// Reads a whole table back through the decoders.
    fn table(
        file: &[u8],
        table: SectionId,
        keys: SectionId,
        records: SectionId,
    ) -> Vec<(String, Vec<u8>)> {
        let header = header(file);
        let (keys, records) = (header.section(keys), header.section(records));
        slice(file, header.section(table))
            .chunks(ENTRY_LEN as usize)
            .map(|bytes| {
                let entry = Entry::parse(bytes, keys, records).expect("entry");
                let key = &slice(file, keys)[entry.key_offset as usize..][..entry.key_len as usize];
                let record = &slice(file, records)[entry.record_offset as usize..]
                    [..entry.record_len as usize];
                (
                    String::from_utf8(key.to_vec()).expect("key"),
                    record.to_vec(),
                )
            })
            .collect()
    }

    fn sense_at(file: &[u8], offset: u64) -> SenseRecord {
        let senses = slice(file, header(file).section(SectionId::Senses));
        let at = &senses[offset as usize..];
        let len = u32::from_le_bytes(at[..4].try_into().expect("length")) as usize;
        decode_sense(&at[4..4 + len]).expect("sense")
    }

    #[test]
    fn what_is_written_is_read_back() {
        let contents = sample();
        let file = encode(&contents).expect("encode");
        let header = header(&file);
        assert_eq!(
            slice(&file, header.section(SectionId::Source)),
            contents.source.as_bytes()
        );

        let words = table(
            &file,
            SectionId::WordTable,
            SectionId::WordKeys,
            SectionId::WordRecords,
        );
        assert_eq!(words.len(), contents.headwords.len());
        for ((key, record), headword) in words.iter().zip(&contents.headwords) {
            assert_eq!(key, &headword.word);
            let (offsets, translations) = decode_word(record).expect("word record");
            assert_eq!(translations, headword.translations);
            let senses: Vec<_> = offsets.iter().map(|at| sense_at(&file, *at)).collect();
            let expected: Vec<_> = headword
                .senses
                .iter()
                .map(|at| contents.senses[*at as usize].clone())
                .collect();
            assert_eq!(senses, expected, "{key}");
        }

        let forms = table(
            &file,
            SectionId::FormTable,
            SectionId::FormKeys,
            SectionId::FormRecords,
        );
        assert_eq!(forms.len(), contents.forms.len());
        for ((key, record), form) in forms.iter().zip(&contents.forms) {
            assert_eq!(key, &form.form);
            assert_eq!(decode_form(record).expect("form record"), form.lemmas);
        }
    }

    #[test]
    fn the_sections_fill_the_file_without_gaps() {
        let file = encode(&sample()).expect("encode");
        let header = header(&file);
        let mut expected = HEADER_LEN as u64;
        for id in 0..SECTIONS {
            assert_eq!(header.sections[id].offset, expected, "section {id}");
            expected += header.sections[id].len;
        }
        assert_eq!(expected, file.len() as u64);
    }

    #[test]
    fn the_same_contents_give_the_same_bytes() {
        assert_eq!(encode(&sample()), encode(&sample()));
        assert_eq!(encode(&sample().clone()), encode(&sample()));
    }

    #[test]
    fn an_empty_index_is_valid() {
        let file = encode(&Contents {
            source: "empty".to_owned(),
            ..Contents::default()
        })
        .expect("encode");
        assert_eq!(file.len(), HEADER_LEN + 5);
        assert_eq!(header(&file).section(SectionId::WordTable).len, 0);
    }

    #[test]
    fn the_writer_refuses_what_the_reader_could_not_search() {
        let mut unsorted = sample();
        unsorted.headwords.swap(1, 2);
        assert_eq!(
            encode(&unsorted),
            Err(WriteError::Unsorted("advice".to_owned()))
        );

        let mut repeated = sample();
        repeated.forms[1].form = "geese".to_owned();
        assert_eq!(
            encode(&repeated),
            Err(WriteError::Unsorted("geese".to_owned()))
        );

        let mut empty = sample();
        empty.headwords[0].word.clear();
        assert_eq!(encode(&empty), Err(WriteError::EmptyKey));

        let mut dangling = sample();
        dangling.headwords[2].senses.push(3);
        assert_eq!(
            encode(&dangling),
            Err(WriteError::UnknownSense {
                word: "bank".to_owned(),
                sense: 3
            })
        );

        let mut nameless = sample();
        nameless.source.clear();
        assert_eq!(encode(&nameless), Err(WriteError::Source));

        let mut huge = sample();
        huge.senses[0].definition = "x".repeat(MAX_RECORD as usize + 1);
        assert!(matches!(encode(&huge), Err(WriteError::TooLarge(_))));
    }

    #[test]
    fn a_wrong_magic_is_damage() {
        let mut file = encode(&sample()).expect("encode");
        file[0] ^= 1;
        assert_eq!(
            Header::parse(&file[..HEADER_LEN], file.len() as u64),
            Err(Damage("it does not start with the index magic"))
        );
    }

    #[test]
    fn an_unknown_version_is_damage() {
        let mut file = encode(&sample()).expect("encode");
        file[8..12].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert_eq!(
            Header::parse(&file[..HEADER_LEN], file.len() as u64),
            Err(Damage("its format version is unknown"))
        );
    }

    #[test]
    fn a_wrong_section_count_is_damage() {
        let mut file = encode(&sample()).expect("encode");
        file[12..16].copy_from_slice(&9u32.to_le_bytes());
        assert!(Header::parse(&file[..HEADER_LEN], file.len() as u64).is_err());
    }

    #[test]
    fn a_truncated_file_is_damage_at_every_length() {
        let file = encode(&sample()).expect("encode");
        for len in 0..file.len() {
            let head = &file[..len.min(HEADER_LEN)];
            assert!(Header::parse(head, len as u64).is_err(), "length {len}");
        }
        assert!(Header::parse(&file[..HEADER_LEN], file.len() as u64).is_ok());
    }

    #[test]
    fn a_section_past_the_end_is_damage() {
        let file = encode(&sample()).expect("encode");
        // Each field of the section table in turn, pushed past the end.
        for field in 0..SECTIONS * 2 {
            for value in [file.len() as u64 + 1, u64::MAX, u64::MAX - 3] {
                let mut damaged = file.clone();
                let at = 16 + field * 8;
                damaged[at..at + 8].copy_from_slice(&value.to_le_bytes());
                assert!(
                    Header::parse(&damaged[..HEADER_LEN], damaged.len() as u64).is_err(),
                    "field {field} = {value}"
                );
            }
        }
    }

    #[test]
    fn a_table_of_a_broken_length_is_damage() {
        let mut file = encode(&sample()).expect("encode");
        let at = 16 + SectionId::WordTable as usize * 16 + 8;
        let len = u64::from_le_bytes(file[at..at + 8].try_into().expect("length"));
        file[at..at + 8].copy_from_slice(&(len - 1).to_le_bytes());
        assert_eq!(
            Header::parse(&file[..HEADER_LEN], file.len() as u64),
            Err(Damage("a table is not a whole number of entries"))
        );
    }

    #[test]
    fn an_entry_past_its_sections_is_damage() {
        let keys = Section { offset: 0, len: 10 };
        let records = Section { offset: 0, len: 20 };
        let entry = |key_offset: u64, key_len: u32, record_offset: u64, record_len: u32| {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&key_offset.to_le_bytes());
            bytes.extend_from_slice(&key_len.to_le_bytes());
            bytes.extend_from_slice(&record_offset.to_le_bytes());
            bytes.extend_from_slice(&record_len.to_le_bytes());
            Entry::parse(&bytes, keys, records)
        };
        assert!(entry(6, 4, 0, 20).is_ok());
        assert!(entry(7, 4, 0, 20).is_err());
        assert!(entry(u64::MAX, 4, 0, 20).is_err());
        assert!(entry(0, u32::MAX, 0, 20).is_err());
        assert!(entry(6, 4, 1, 20).is_err());
        assert!(entry(6, 4, u64::MAX - 1, 2).is_err());
        assert!(Entry::parse(&[0; 23], keys, records).is_err());
    }

    #[test]
    fn a_record_that_lies_about_its_contents_is_damage() {
        // A count far beyond the bytes: an error, and no allocation for it.
        let lying = u32::MAX.to_le_bytes();
        assert!(decode_word(&lying).is_err());
        assert!(decode_form(&lying).is_err());
        assert!(decode_sense(&lying).is_err());
        assert!(decode_word(&[]).is_err());

        let mut record = Vec::new();
        put_texts(&mut record, &texts(&["goose"])).expect("texts");
        assert_eq!(decode_form(&record), Ok(texts(&["goose"])));
        for len in 0..record.len() {
            assert!(decode_form(&record[..len]).is_err(), "length {len}");
        }
        record.push(0);
        assert_eq!(
            decode_form(&record),
            Err(Damage("a record is longer than its contents"))
        );

        let mut not_text = Vec::new();
        not_text.extend_from_slice(&1u32.to_le_bytes());
        not_text.extend_from_slice(&2u32.to_le_bytes());
        not_text.extend_from_slice(&[0xff, 0xfe]);
        assert_eq!(decode_form(&not_text), Err(Damage("a text is not UTF-8")));
    }
}
