//! A WikDict translation file, an SQLite database, as the contents of an
//! index.
//!
//! Of the database only the `translation` table is read, its `written_rep`
//! and `trans_list` columns, the ones the selection experiment counted
//! (EXP-9 `sources.py`, `wikdict_pair`): `written_rep` is the
//! headword as written, and `trans_list` its translations, separated by `|`
//! and trimmed. A row without a headword is skipped, and so is an empty
//! translation. The rows of one headword, one per sense of an entry, are
//! joined: the headword gets each translation once, in row order. The
//! headwords have translations and no senses, and there is no form table.
//!
//! The database is read by [`sqlite`](crate::sqlite), from memory.

use std::{collections::BTreeMap, fmt};

use crate::{
    format::{Contents, Headword},
    sqlite::{Database, Value},
};

/// The table of the translations.
pub const TABLE: &str = "translation";

/// Why a WikDict file gave no contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// The contents of the index of the WikDict database `bytes`; `source` is
/// the id written into the index.
pub fn parse(bytes: &[u8], source: &str) -> Result<Contents, ParseError> {
    let sqlite = |error: crate::sqlite::Error| ParseError(error.to_string());
    let database = Database::open(bytes).map_err(sqlite)?;
    let rows = database
        .rows(TABLE, &["written_rep", "trans_list"])
        .map_err(sqlite)?;
    let mut headwords: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (at, row) in rows.into_iter().enumerate() {
        let mut row = row.into_iter();
        let (Some(written), Some(list)) = (row.next(), row.next()) else {
            continue;
        };
        let text = |value: Value| match value {
            Value::Null => Ok(String::new()),
            Value::Text(text) => Ok(text),
            _ => Err(ParseError(format!(
                "row {} of the {TABLE} table has a value that is not text",
                at + 1
            ))),
        };
        let (written, list) = (text(written)?, text(list)?);
        if written.is_empty() {
            continue;
        }
        let translations = headwords.entry(written).or_default();
        for translation in list.split('|').map(str::trim) {
            if !translation.is_empty() && !translations.iter().any(|known| known == translation) {
                translations.push(translation.to_owned());
            }
        }
    }
    let headwords: Vec<Headword> = headwords
        .into_iter()
        .filter(|(_, translations)| !translations.is_empty())
        .map(|(word, translations)| Headword {
            entries: Vec::new(),
            word,
            senses: Vec::new(),
            translations,
        })
        .collect();
    if headwords.is_empty() {
        return Err(ParseError(format!(
            "the {TABLE} table has no headword with a translation"
        )));
    }
    Ok(Contents {
        source: source.to_owned(),
        senses: Vec::new(),
        headwords,
        analyses: Vec::new(),
        forms: Vec::new(),
    })
}

/// The stress marks a Russian headword may be written with: the combining
/// acute (U+0301) and grave (U+0300) accents.
pub const STRESS_MARKS: [char; 2] = ['\u{0301}', '\u{0300}'];

/// `word` without its [`STRESS_MARKS`]; `кни́га` is `книга`. A grave
/// stress written precomposed, as in the NFC files, is dropped too: `ѐ`,
/// `ѝ`, `Ѐ` and `Ѝ` (U+0450, U+045D, U+0400, U+040D) are written `е`, `и`,
/// `Е` and `И`; `бѝотехнология` is `биотехнология`.
pub fn unstressed_word(word: &str) -> String {
    word.chars()
        .filter(|letter| !STRESS_MARKS.contains(letter))
        .map(|letter| match letter {
            '\u{0450}' => 'е',
            '\u{045D}' => 'и',
            '\u{0400}' => 'Е',
            '\u{040D}' => 'И',
            other => other,
        })
        .collect()
}

/// The `contents` of a file of Russian headwords, each headword keyed
/// without its [`STRESS_MARKS`]. Headwords that become one key are one
/// headword: its translations are theirs, each once, in the order the
/// headwords had (byte order of the written headword), and within each in
/// its order. The headwords stay in byte order, each once. Nothing else of
/// the contents changes; a file of other headwords is not given here.
pub fn unstressed(contents: Contents) -> Contents {
    let mut merged: BTreeMap<String, Headword> = BTreeMap::new();
    for mut headword in contents.headwords {
        let key = unstressed_word(&headword.word);
        if key.is_empty() {
            continue;
        }
        // Preserve the original written headword in entry identities.
        for entry in &mut headword.entries {
            if entry.entry.identity.headword.is_empty() {
                entry.entry.identity.headword = headword.word.clone();
            }
        }
        let collected = merged.entry(key.clone()).or_insert_with(|| Headword {
            word: key,
            ..Default::default()
        });
        crate::combined::append_unique(&mut collected.translations, headword.translations);
        crate::combined::append_unique(&mut collected.senses, headword.senses);
        crate::combined::append_unique(&mut collected.entries, headword.entries);
    }
    Contents {
        headwords: merged.into_values().collect(),
        ..contents
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        format::{self, tests::texts},
        sqlite::tests::Builder,
    };

    const SQL: &str = "CREATE TABLE translation(\n  lexentry,\n  sense_num,\n  sense,\n  written_rep TEXT,\n  trans_list,\n  score,\n  is_good,\n  importance\n)";

    fn text(text: &str) -> Value {
        Value::Text(text.to_owned())
    }

    /// A row of the `translation` table.
    fn row(
        id: u64,
        entry: &str,
        sense: Option<&str>,
        written: Value,
        list: &str,
    ) -> (u64, Vec<Value>) {
        (
            id,
            vec![
                text(entry),
                sense.map_or(Value::Null, |_| text("01")),
                sense.map_or(Value::Null, text),
                written,
                text(list),
                Value::Real(22.5),
                Value::Integer(1),
                Value::Real(1.7),
            ],
        )
    }

    /// A database in the shape of `wikdict-es-ru` (or `wikdict-ca-ru` with
    /// the Catalan rows): `banco` twice, once per sense, rows that give
    /// nothing, and the rows over two leaf pages.
    pub(crate) fn database(catalan: bool) -> Vec<u8> {
        let mut builder = Builder::new(512);
        let rows = if catalan {
            vec![
                row(
                    1,
                    "cat/col%C2%B7lecció__Nom__1",
                    None,
                    text("col·lecció"),
                    "колле́кция | коллекция",
                ),
                row(
                    2,
                    "cat/banc__Nom__1",
                    Some("Seient llarg."),
                    text("banc"),
                    "скамья́",
                ),
                row(
                    3,
                    "cat/banc__Nom__2",
                    Some("Entitat."),
                    text("banc"),
                    "банк",
                ),
            ]
        } else {
            vec![
                row(
                    1,
                    "spa/banco__sustantivo_masculino__1",
                    None,
                    text("banco"),
                    "банк",
                ),
                row(
                    2,
                    "spa/banco__sustantivo_masculino__1",
                    Some("Empresa."),
                    text("banco"),
                    "банк",
                ),
                row(
                    3,
                    "spa/banco__sustantivo_masculino__2",
                    Some("Mueble."),
                    text("banco"),
                    " скамья |  | лавка ",
                ),
                row(
                    4,
                    "spa/mesa__sustantivo_femenino__1",
                    None,
                    text("mesa"),
                    "стол",
                ),
                row(5, "spa/x", None, text(""), "ничто"),
                row(6, "spa/y", None, Value::Null, "ничто"),
                row(7, "spa/vacío", None, text("vacío"), " | "),
            ]
        };
        let (left, right) = rows.split_at(rows.len() / 2);
        let left_page = builder.leaf(left);
        let right_page = builder.leaf(right);
        let root = builder.interior(
            &[(left_page, left.last().map_or(0, |(id, _)| *id))],
            right_page,
        );
        builder.finish(&[("table", TABLE, root, SQL)])
    }

    fn translations(contents: &Contents) -> Vec<(&str, Vec<String>)> {
        contents
            .headwords
            .iter()
            .map(|headword| (headword.word.as_str(), headword.translations.clone()))
            .collect()
    }

    #[test]
    fn the_spanish_rows_give_headwords_with_their_translations_each_once() {
        let contents = parse(&database(false), "wikdict-es").expect("parse");
        assert_eq!(contents.source, "wikdict-es");
        assert_eq!(
            translations(&contents),
            [
                ("banco", texts(&["банк", "скамья", "лавка"])),
                ("mesa", texts(&["стол"])),
            ]
        );
        assert!(contents.senses.is_empty());
        assert!(contents.headwords.iter().all(|h| h.senses.is_empty()));
        assert!(contents.forms.is_empty());
        let first = format::encode(&contents).expect("encode");
        assert_eq!(
            format::encode(&parse(&database(false), "wikdict-es").expect("parse")).expect("encode"),
            first
        );
    }

    #[test]
    fn the_catalan_rows_give_headwords_with_their_translations() {
        let contents = parse(&database(true), "wikdict-ca").expect("parse");
        assert_eq!(
            translations(&contents),
            [
                ("banc", texts(&["скамья́", "банк"])),
                ("col·lecció", texts(&["колле́кция", "коллекция"])),
            ]
        );
    }

    #[test]
    fn a_database_without_the_table_or_its_rows_is_an_error() {
        let error = |bytes: &[u8]| parse(bytes, "x").expect_err("error").to_string();
        assert_eq!(error(b"not a database"), "not an SQLite database");

        let mut builder = Builder::new(512);
        let other = builder.leaf(&[(1, vec![text("a")])]);
        let bytes = builder.finish(&[(
            "table",
            "simple_translation",
            other,
            "CREATE TABLE simple_translation(written_rep TEXT, trans_list)",
        )]);
        assert_eq!(error(&bytes), "the database has no table translation");

        let mut builder = Builder::new(512);
        let empty = builder.leaf(&[row(1, "spa/x", None, text(""), "банк")]);
        let bytes = builder.finish(&[("table", TABLE, empty, SQL)]);
        assert_eq!(
            error(&bytes),
            "the translation table has no headword with a translation"
        );

        let mut builder = Builder::new(512);
        let number = builder.leaf(&[row(1, "spa/x", None, Value::Integer(3), "банк")]);
        let bytes = builder.finish(&[("table", TABLE, number, SQL)]);
        assert!(error(&bytes).contains("not text"));
    }

    fn headword(word: &str, translations: &[&str]) -> Headword {
        Headword {
            entries: Vec::new(),
            word: word.to_owned(),
            senses: Vec::new(),
            translations: texts(translations),
        }
    }

    #[test]
    fn russian_headwords_are_keyed_without_stress_marks_and_merged() {
        let contents = Contents {
            source: "wikdict-ru-en".to_owned(),
            senses: Vec::new(),
            headwords: vec![
                headword("биотехнология", &["biotechnology"]),
                headword("б\u{045D}отехнология", &["biotech", "biotechnology"]),
                headword("за\u{0301}мок", &["castle", "palace"]),
                headword("замо\u{0301}к", &["lock", "castle"]),
                headword("замок", &["padlock"]),
                headword("зверь", &["beast"]),
                headword("кни\u{0301}га", &["book"]),
                headword("что\u{0300}-то", &["something"]),
                headword("ёж", &["hedgehog"]),
            ],
            analyses: Vec::new(),
            forms: Vec::new(),
        };
        let unstressed = unstressed(contents);
        assert_eq!(unstressed.source, "wikdict-ru-en");
        assert_eq!(
            translations(&unstressed),
            [
                ("биотехнология", texts(&["biotechnology", "biotech"])),
                ("замок", texts(&["castle", "palace", "lock", "padlock"])),
                ("зверь", texts(&["beast"])),
                ("книга", texts(&["book"])),
                ("что-то", texts(&["something"])),
                ("ёж", texts(&["hedgehog"])),
            ]
        );
        // Sorted in byte order, each once.
        let words: Vec<&str> = unstressed
            .headwords
            .iter()
            .map(|headword| headword.word.as_str())
            .collect();
        assert!(
            words
                .windows(2)
                .all(|pair| pair[0].as_bytes() < pair[1].as_bytes())
        );
        assert_eq!(unstressed_word("кни\u{0301}га"), "книга");
        assert_eq!(unstressed_word("bank"), "bank");
        assert_eq!(unstressed_word("\u{0400}\u{0450}\u{040D}\u{045D}"), "ЕеИи");
    }
    #[test]
    fn removing_stress_preserves_senses_entries_and_original_headword_provenance() {
        let json = r#"{"word":"ба́нк","lang_code":"ru","pos":"noun","sounds":[{"ipa":"stressed"}],"senses":[{"glosses":["one"]}]}
{"word":"банк","lang_code":"ru","pos":"verb","sounds":[{"ipa":"plain"}],"senses":[{"glosses":["two"]}]}"#;
        let contents = crate::kaikki::parse(json.as_bytes(), "ru", "provenance").unwrap();
        let merged = unstressed(contents);
        assert_eq!(merged.headwords.len(), 1);
        assert_eq!(merged.headwords[0].word, "банк");
        assert_eq!(merged.headwords[0].senses.len(), 2);
        assert_eq!(merged.headwords[0].entries.len(), 2);
        assert_eq!(
            merged.headwords[0].entries[0].entry.identity.headword,
            "ба́нк"
        );
        assert_eq!(
            merged.headwords[0].entries[1].entry.identity.headword,
            "банк"
        );
        let bytes = format::encode(&merged).unwrap();
        let dir = crate::testdir::TempDir::new("unstressed-metadata");
        let path = dir.path().join("word.index");
        std::fs::write(&path, bytes).unwrap();
        let article = crate::Index::open(&path)
            .unwrap()
            .find("банк")
            .unwrap()
            .unwrap();
        assert_eq!(article.entries.len(), 2);
        assert_eq!(article.senses[0].entries[0].headword, "ба́нк");
        assert_eq!(article.senses[1].entries[0].headword, "банк");
    }
}
