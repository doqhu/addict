//! A MyThes thesaurus, the `.dat` file of a Hunspell thesaurus, as the
//! contents of an index of synonyms.
//!
//! The first line names the encoding, and only UTF-8 is read. Then each
//! entry is a line `word|n` followed by its `n` meaning lines, each
//! `(part of speech)|item|item…`. It is read the way the selection
//! experiment read it (`experiments/exp9/sources.py`, `mythes`): a line that
//! is not `word|n` where an entry should start is skipped.
//!
//! Each meaning line retains its full label as optional sense metadata,
//! with explicit grammatical class and description where understood. Its
//! definition stays empty; its
//! members are its items, trimmed, each once, without the word itself, kept
//! as written: annotations such as `lot (m)` and items of several words
//! stay. A meaning line with no item left gives no record, and a word with
//! no record is no headword. The entries of one word are joined in file
//! order. The headwords have no translations, and there is no form table.

use std::{
    collections::BTreeMap,
    fmt,
    io::{self, BufRead},
};

use crate::format::{Contents, SenseMetadata, SenseRecord};

/// Why a thesaurus gave no contents.
#[derive(Debug)]
pub enum ParseError {
    /// The stream could not be read.
    Io(io::Error),
    /// The file is empty, or its first line names no encoding.
    Empty,
    /// The encoding the first line names is not UTF-8.
    Encoding(String),
    /// No entry has a synonym.
    NoEntry,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cannot read the file: {error}"),
            Self::Empty => write!(f, "the thesaurus file is empty or names no encoding"),
            Self::Encoding(encoding) => {
                write!(f, "the thesaurus encoding {encoding:?} is not UTF-8")
            }
            Self::NoEntry => write!(f, "the thesaurus has no entry with a synonym"),
        }
    }
}

impl std::error::Error for ParseError {}

/// The contents of the index of the thesaurus `reader` holds; `source` is
/// the id written into the index.
pub fn parse(mut reader: impl BufRead, source: &str) -> Result<Contents, ParseError> {
    let mut text = Vec::new();
    reader.read_to_end(&mut text).map_err(ParseError::Io)?;
    let text = String::from_utf8_lossy(&text);
    let mut lines = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line));
    let encoding = lines
        .next()
        .filter(|line| !line.is_empty())
        .ok_or(ParseError::Empty)?;
    let encoding = encoding.trim();
    if !encoding.eq_ignore_ascii_case("UTF-8") {
        return Err(ParseError::Encoding(encoding.to_owned()));
    }

    let mut headwords: BTreeMap<String, Vec<SenseRecord>> = BTreeMap::new();
    while let Some(line) = lines.next() {
        let Some((word, count)) = entry_line(line) else {
            continue;
        };
        let mut senses = Vec::new();
        for meaning in lines.by_ref().take(count) {
            let mut fields = meaning.split('|');
            let metadata = SenseMetadata::from_label(fields.next().unwrap_or_default());
            let mut members: Vec<String> = Vec::new();
            for item in fields.map(str::trim) {
                if !item.is_empty() && item != word && !members.iter().any(|known| known == item) {
                    members.push(item.to_owned());
                }
            }
            if !members.is_empty() {
                let part_of_speech = metadata.as_ref().and_then(|metadata| {
                    let tag = metadata
                        .parts()
                        .map_or(metadata.label(), |(tag, _)| tag.trim());
                    let mut pos = addict_core::dictionary::PartOfSpeech::from_label(tag)?;
                    pos.label = metadata.label().to_owned();
                    Some(pos)
                });
                senses.push(SenseRecord {
                    part_of_speech,
                    definition: String::new(),
                    members,
                    metadata,
                });
            }
        }
        if !senses.is_empty() {
            headwords.entry(word.to_owned()).or_default().extend(senses);
        }
    }
    if headwords.is_empty() {
        return Err(ParseError::NoEntry);
    }

    Ok(Contents::of_senses(source, headwords))
}

/// The word and the meaning count of a line `word|n`.
fn entry_line(line: &str) -> Option<(&str, usize)> {
    let (word, count) = line.rsplit_once('|')?;
    if word.is_empty()
        || word.contains('|')
        || count.is_empty()
        || !count.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((word, count.parse().ok()?))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::format::{self, tests::texts};

    /// Lines in the shape of `sinonims-cat` (`th_ca_ES.dat`): `col·lecció`
    /// with two meanings and a second entry, items with annotations and of
    /// several words, the word among its own items, a repeated item, a stray
    /// line and an entry with no item.
    pub(crate) const FIXTURE: &str = "UTF-8
col·lecció|2
(n)|conjunt|recull|aplec|col·lecció|recull
(n)|sèrie|lot (m)|joc de peces
stray line
banc|1
(n)|seient | escó
buit|1
(adj)|buit|
col·lecció|1
(n)|antologia
";

    fn parsed() -> Contents {
        parse(FIXTURE.as_bytes(), "fixture-th").expect("parse")
    }

    fn members(contents: &Contents, word: &str) -> Vec<Vec<String>> {
        let headword = contents
            .headwords
            .iter()
            .find(|headword| headword.word == word)
            .unwrap_or_else(|| panic!("{word}"));
        headword
            .senses
            .iter()
            .map(|at| contents.senses[*at as usize].members.clone())
            .collect()
    }

    #[test]
    fn each_meaning_line_is_a_sense_with_its_items_as_members() {
        let contents = parsed();
        assert_eq!(contents.source, "fixture-th");
        let words: Vec<_> = contents.headwords.iter().map(|h| h.word.as_str()).collect();
        assert_eq!(words, ["banc", "col·lecció"]);
        assert_eq!(
            members(&contents, "col·lecció"),
            [
                texts(&["conjunt", "recull", "aplec"]),
                texts(&["sèrie", "lot (m)", "joc de peces"]),
                texts(&["antologia"]),
            ]
        );
        assert_eq!(members(&contents, "banc"), [texts(&["seient", "escó"])]);
        assert!(
            contents
                .senses
                .iter()
                .all(|sense| sense.definition.is_empty())
        );
        assert!(contents.headwords.iter().all(|h| h.translations.is_empty()));
        assert!(contents.forms.is_empty());
        let first = format::encode(&contents).expect("encode");
        assert_eq!(format::encode(&parsed()).expect("encode"), first);
    }

    #[test]
    fn labels_descriptions_annotations_and_duplicate_meanings_are_preserved() {
        use crate::format::GrammaticalClass;
        let contents = parse(&b"UTF-8\nfer|5\n(adj)|ferotge\n(v)|prendre\n(v; beure)|prendre|trincar (colloquial)\n(mystery)|other\n|unlabelled\n"[..], "labels").expect("parse");
        let labels: Vec<_> = contents
            .senses
            .iter()
            .map(|sense| sense.metadata.as_ref().map(|metadata| metadata.label()))
            .collect();
        assert_eq!(
            labels,
            [
                Some("(adj)"),
                Some("(v)"),
                Some("(v; beure)"),
                Some("(mystery)"),
                None
            ]
        );
        assert_eq!(
            contents.senses[0].metadata.as_ref().expect("label").class(),
            Some(GrammaticalClass::Adjective)
        );
        let beure = contents.senses[2].metadata.as_ref().expect("label");
        assert_eq!(beure.class(), Some(GrammaticalClass::Verb));
        assert_eq!(beure.description(), Some("beure"));
        assert_eq!(
            contents.senses[2].members,
            texts(&["prendre", "trincar (colloquial)"])
        );
        assert_eq!(
            contents.senses[3]
                .metadata
                .as_ref()
                .expect("unknown")
                .class(),
            None
        );
        // The grammar is read from the class tag; the label stays whole.
        use addict_core::dictionary::Pos;
        let grammar: Vec<_> = contents
            .senses
            .iter()
            .map(|sense| {
                sense
                    .part_of_speech
                    .as_ref()
                    .map(|pos| (pos.category, pos.label.as_str()))
            })
            .collect();
        assert_eq!(
            grammar,
            [
                Some((Some(Pos::Adjective), "(adj)")),
                Some((Some(Pos::Verb), "(v)")),
                Some((Some(Pos::Verb), "(v; beure)")),
                Some((None, "(mystery)")),
                None
            ]
        );
        // A thesaurus has no lexical entries of its own.
        assert!(
            contents
                .headwords
                .iter()
                .all(|headword| headword.entries.is_empty())
        );
    }

    #[test]
    fn an_empty_file_another_encoding_and_no_synonym_are_errors() {
        assert!(matches!(parse(&b""[..], "x"), Err(ParseError::Empty)));
        assert!(matches!(
            parse(&b"\nword|1\n(n)|other\n"[..], "x"),
            Err(ParseError::Empty)
        ));
        assert!(matches!(
            parse(&b"ISO8859-1\nword|1\n(n)|other\n"[..], "x"),
            Err(ParseError::Encoding(encoding)) if encoding == "ISO8859-1"
        ));
        assert!(matches!(
            parse(&b"UTF-8\n"[..], "x"),
            Err(ParseError::NoEntry)
        ));
        assert!(matches!(
            parse(&b"UTF-8\nword|1\n(n)|word\n"[..], "x"),
            Err(ParseError::NoEntry)
        ));
        // A file cut inside an entry keeps what it has.
        let cut = parse(&b"utf-8\nword|3\n(n)|other"[..], "x").expect("cut");
        assert_eq!(cut.headwords.len(), 1);
    }

    #[test]
    fn an_entry_line_is_a_word_and_a_count() {
        assert_eq!(entry_line("4x4|2"), Some(("4x4", 2)));
        assert_eq!(
            entry_line("a alta hora de la nit|12"),
            Some(("a alta hora de la nit", 12))
        );
        for line in ["(n)|a|b", "word|", "|2", "word|2a", "a|b|2", ""] {
            assert_eq!(entry_line(line), None, "{line}");
        }
    }
}
