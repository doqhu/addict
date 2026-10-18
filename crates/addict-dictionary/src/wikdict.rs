//! A WikDict translation file, an SQLite database, as the contents of an
//! index.
//!
//! Of the database only the `translation` table is read, its `written_rep`
//! and `trans_list` columns, the ones the selection experiment counted
//! (`experiments/exp9/sources.py`, `wikdict_pair`): `written_rep` is the
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
        forms: Vec::new(),
    })
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
}
