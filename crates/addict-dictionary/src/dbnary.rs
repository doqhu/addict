//! A DBnary Ontolex file, Wiktionary as Turtle, as the contents of an index
//! of definitions.
//!
//! The file is pretty-printed Turtle: one block per subject, the blocks
//! separated by an empty line, one predicate per line. It is read the way
//! the selection experiment read it (`experiments/exp9/sources.py`,
//! `dbnary_entries`), for its definitions only:
//!
//! - an entry block is a block with `ontolex:LexicalEntry` and the
//!   language's `lexvo:` code: its `rdfs:label "…"` is the headword as
//!   written, and its `ontolex:sense a , b` line names its senses in order;
//! - a sense block is the block of such a sense: each of its
//!   `skos:definition [ rdf:value "…" ]` is one definition.
//!
//! Synonyms, translations, pages and morphology are not read. Each
//! definition is one sense record with no members, in the order of the
//! entry's `ontolex:sense` line; the entries of one label are joined in file
//! order into one headword. A label whose entries give no definition is no
//! headword. The headwords have no translations, and there is no form
//! table. The escapes of a literal (`\"`, `\\`, `\n`, `\uXXXX`, …) are
//! resolved.
//!
//! The file is read as a stream; the entries and the definitions are held
//! until its end, because a sense block may come before or after its entry.

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    io::{self, BufRead},
};

use crate::format::{Contents, SenseRecord};

/// Why a DBnary file gave no contents.
#[derive(Debug)]
pub enum ParseError {
    /// The stream could not be read.
    Io(io::Error),
    /// No block is a lexical entry of the language.
    NoEntry,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cannot read the file: {error}"),
            Self::NoEntry => write!(f, "the file has no lexical entry of the language"),
        }
    }
}

impl std::error::Error for ParseError {}

/// One entry block: its label and the ids of its senses.
struct Entry {
    label: String,
    senses: Vec<String>,
}

/// The contents of the index of the DBnary Turtle file `reader` holds, of
/// the entries whose language is the `lexvo:` code `language` (such as
/// `cat`); `source` is the id written into the index.
pub fn parse(reader: impl BufRead, language: &str, source: &str) -> Result<Contents, ParseError> {
    let code = format!("lexvo:{language}");
    let mut entries: Vec<Entry> = Vec::new();
    let mut definitions: HashMap<String, Vec<String>> = HashMap::new();
    let mut block = String::new();
    let mut take = |block: &str| {
        if block.contains("ontolex:LexicalEntry") && has_token(block, &code) {
            if let Some(label) = literal_after(block, "rdfs:label") {
                entries.push(Entry {
                    label,
                    senses: objects(block, "ontolex:sense"),
                });
            }
        } else if block.contains("skos:definition") {
            let found = definitions_of(block);
            if !found.is_empty() {
                definitions
                    .entry(subject(block).to_owned())
                    .or_default()
                    .extend(found);
            }
        }
    };
    for line in lines(reader) {
        let line = line.map_err(ParseError::Io)?;
        if line.trim().is_empty() {
            if !block.is_empty() {
                take(&block);
                block.clear();
            }
        } else {
            block.push_str(&line);
            block.push('\n');
        }
    }
    if !block.is_empty() {
        take(&block);
    }
    if entries.is_empty() {
        return Err(ParseError::NoEntry);
    }

    let mut headwords: BTreeMap<String, Vec<SenseRecord>> = BTreeMap::new();
    for entry in entries {
        let senses: Vec<SenseRecord> = entry
            .senses
            .iter()
            .filter_map(|sense| definitions.get(sense))
            .flatten()
            .map(|definition| SenseRecord {
                definition: definition.clone(),
                members: Vec::new(),
                metadata: None,
            })
            .collect();
        if !senses.is_empty() && !entry.label.is_empty() {
            headwords.entry(entry.label).or_default().extend(senses);
        }
    }
    Ok(Contents::of_senses(source, headwords))
}

/// The lines of `reader` without their line ends; bytes that are not UTF-8
/// are replaced, as the experiment read them.
fn lines(mut reader: impl BufRead) -> impl Iterator<Item = io::Result<String>> {
    let mut buffer = Vec::new();
    std::iter::from_fn(move || {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) => None,
            Ok(_) => {
                while matches!(buffer.last(), Some(b'\n' | b'\r')) {
                    buffer.pop();
                }
                Some(Ok(String::from_utf8_lossy(&buffer).into_owned()))
            }
            Err(error) => Some(Err(error)),
        }
    })
}

/// The subject of a block: its first word.
fn subject(block: &str) -> &str {
    block.split_whitespace().next().unwrap_or("")
}

/// Whether `token` stands in `text` as a whole name, not as the start of a
/// longer one.
fn has_token(text: &str, token: &str) -> bool {
    text.match_indices(token).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + token.len()..].chars().next();
        !before.is_some_and(is_name) && !after.is_some_and(is_name)
    })
}

fn is_name(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-' | ':')
}

/// The objects of the line of `predicate` in `block`, as written: the rest
/// of the line without its final `;` or `.`, split at the commas.
fn objects(block: &str, predicate: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in block.lines() {
        let mut words = line.trim_start().splitn(2, char::is_whitespace);
        let mut first = words.next().unwrap_or("");
        let mut rest = words.next().unwrap_or("");
        // The subject may stand on the line of the first predicate.
        if first != predicate {
            let mut more = rest.trim_start().splitn(2, char::is_whitespace);
            first = more.next().unwrap_or("");
            rest = more.next().unwrap_or("");
            if first != predicate {
                continue;
            }
        }
        let rest = rest.trim();
        let rest = rest.strip_suffix(['.', ';']).map_or(rest, str::trim_end);
        found.extend(
            split_objects(rest)
                .into_iter()
                .map(str::trim)
                .filter(|object| !object.is_empty())
                .map(str::to_owned),
        );
    }
    found
}

/// The objects of a list `a , b`: split at each comma with a space on both
/// sides, since a name may hold a comma of its own (`cat:tal_pare,_tal_fill`).
fn split_objects(list: &str) -> Vec<&str> {
    let bytes = list.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    for (at, byte) in bytes.iter().enumerate() {
        if *byte == b','
            && at > 0
            && bytes[at - 1].is_ascii_whitespace()
            && bytes.get(at + 1).is_some_and(u8::is_ascii_whitespace)
        {
            parts.push(&list[start..at]);
            start = at + 1;
        }
    }
    parts.push(&list[start..]);
    parts
}

/// The first literal right after `predicate` in `block`, its escapes
/// resolved.
fn literal_after(block: &str, predicate: &str) -> Option<String> {
    block.match_indices(predicate).find_map(|(at, _)| {
        let rest = &block[at + predicate.len()..];
        let trimmed = rest.trim_start();
        // A space at least, and a literal next.
        (trimmed.len() < rest.len()).then_some(())?;
        literal(trimmed)
    })
}

/// Each `skos:definition [ rdf:value "…" ]` of a block, in order.
fn definitions_of(block: &str) -> Vec<String> {
    block
        .match_indices("skos:definition")
        .filter_map(|(at, predicate)| {
            let rest = block[at + predicate.len()..].trim_start();
            let rest = rest.strip_prefix('[')?.trim_start();
            let rest = rest.strip_prefix("rdf:value")?;
            let text = literal(rest.trim_start())?;
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        })
        .collect()
}

/// The short literal `"…"` that `text` starts with, its escapes resolved;
/// `None` when `text` starts with none or it is not closed on its line.
fn literal(text: &str) -> Option<String> {
    let mut characters = text.strip_prefix('"')?.chars();
    let mut value = String::new();
    loop {
        match characters.next()? {
            '"' => return Some(value),
            '\n' => return None,
            '\\' => {
                let escaped = characters.next()?;
                match escaped {
                    'n' => value.push('\n'),
                    't' => value.push('\t'),
                    'r' => value.push('\r'),
                    'b' => value.push('\u{8}'),
                    'f' => value.push('\u{c}'),
                    'u' | 'U' => {
                        let digits = if escaped == 'u' { 4 } else { 8 };
                        // The digits are consumed only when they make a
                        // character, so a short escape cannot eat the quote.
                        let hex: String = characters.clone().take(digits).collect();
                        let code = u32::from_str_radix(&hex, 16)
                            .ok()
                            .filter(|_| hex.len() == digits);
                        match code.and_then(char::from_u32) {
                            Some(character) => {
                                value.push(character);
                                characters.nth(digits - 1);
                            }
                            None => {
                                value.push('\\');
                                value.push(escaped);
                            }
                        }
                    }
                    // `\"`, `\'`, `\\`, and any other escaped character as
                    // itself.
                    other => value.push(other),
                }
            }
            character => value.push(character),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::format;

    /// Blocks in the shape of `dbnary-ca-ontolex`: `col·lecció` as a noun
    /// with two senses, given in another order than its `ontolex:sense`
    /// line, a second entry of the same label, a sense block before its
    /// entry, an entry of another language, a page, a translation and
    /// literals with escapes.
    pub(crate) const FIXTURE: &str = r#"@prefix cat:      <http://kaiko.getalp.org/dbnary/cat/> .
@prefix lexvo:    <http://lexvo.org/id/iso639-3/> .

cat:__ws_1_banc__Nom__1
        rdf:type            ontolex:LexicalSense;
        dbnary:senseNumber  "1";
        skos:definition     [ rdf:value  "Seient llarg per a \"diverses\" persones."@ca ] .

cat:col%C2%B7lecció__Nom__1  rdf:type   ontolex:Word , ontolex:LexicalEntry;
        rdfs:label             "col·lecció"@ca;
        dbnary:partOfSpeech    "Nom";
        dbnary:synonym         cat:recull , cat:aplec;
        dcterms:language       lexvo:cat;
        lime:language          "ca";
        ontolex:sense          cat:__ws_1_col·lecció__Nom__1 , cat:__ws_2_col·lecció__Nom__1 , cat:__ws_9_col·lecció__Nom__1 .

cat:__ws_2_col·lecció__Nom__1
        rdf:type            ontolex:LexicalSense;
        dbnary:senseNumber  "2";
        skos:definition     [ rdf:value  "Conjunt d'obres publicades\\ amb un tret comú."@ca ];
        skos:example        [ rdf:value  "Una col·lecció de novel·les."@ca ] .

cat:__ws_1_col·lecció__Nom__1
        rdf:type            ontolex:LexicalSense;
        dbnary:senseNumber  "1";
        skos:definition     [ rdf:value  "Conjunt d'objectes\nreunits."@ca ] .

cat:col·lecció  rdf:type      dbnary:Page;
        dbnary:describes  cat:col%C2%B7lecció__Nom__1 .

cat:__tr_rus_1_col·lecció__Nom__1
        rdf:type                dbnary:Translation;
        dbnary:isTranslationOf  cat:col%C2%B7lecció__Nom__1;
        dbnary:targetLanguage   lexvo:rus;
        dbnary:writtenForm      "коллекция"@ru .

cat:banc__Nom__1
        rdf:type               ontolex:Word , ontolex:LexicalEntry;
        rdfs:label             "banc"@ca;
        dcterms:language       lexvo:cat;
        ontolex:sense          cat:__ws_1_banc__Nom__1 , cat:__ws_1_tal_pare,_tal_fill__Nom__1 .

cat:__ws_1_tal_pare,_tal_fill__Nom__1
        rdf:type            ontolex:LexicalSense;
        skos:definition     [ rdf:value  "Un nom amb una coma."@ca ] .

cat:bank__Nom__1
        rdf:type               ontolex:Word , ontolex:LexicalEntry;
        rdfs:label             "bank"@ca;
        dcterms:language       lexvo:eng;
        ontolex:sense          cat:__ws_1_bank__Nom__1 .

cat:__ws_1_bank__Nom__1
        rdf:type            ontolex:LexicalSense;
        skos:definition     [ rdf:value  "Banc anglès."@ca ] .

cat:col%C2%B7lecció__Verb__1
        rdf:type               ontolex:Word , ontolex:LexicalEntry;
        rdfs:label             "col·lecció"@ca;
        dcterms:language       lexvo:cat;
        ontolex:sense          cat:__ws_1_col·lecció__Verb__1 .

cat:__ws_1_col·lecció__Verb__1
        rdf:type            ontolex:LexicalSense;
        skos:definition     [ rdf:value  "Una forma inventada és."@ca ] .

cat:buit__Nom__1
        rdf:type               ontolex:Word , ontolex:LexicalEntry;
        rdfs:label             "buit"@ca;
        dcterms:language       lexvo:cat;
        ontolex:sense          cat:__ws_1_buit__Nom__1 .
"#;

    fn parsed() -> Contents {
        parse(FIXTURE.as_bytes(), "cat", "fixture-ca").expect("parse")
    }

    fn definitions(contents: &Contents, word: &str) -> Vec<String> {
        let headword = contents
            .headwords
            .iter()
            .find(|headword| headword.word == word)
            .unwrap_or_else(|| panic!("{word}"));
        headword
            .senses
            .iter()
            .map(|at| contents.senses[*at as usize].definition.clone())
            .collect()
    }

    #[test]
    fn the_definitions_come_in_the_order_of_the_senses_and_the_entries() {
        let contents = parsed();
        assert_eq!(contents.source, "fixture-ca");
        let words: Vec<_> = contents.headwords.iter().map(|h| h.word.as_str()).collect();
        // No English `bank`, no `buit` without a definition.
        assert_eq!(words, ["banc", "col·lecció"]);
        assert_eq!(
            definitions(&contents, "col·lecció"),
            [
                "Conjunt d'objectes\nreunits.",
                "Conjunt d'obres publicades\\ amb\u{a0}un tret comú.",
                "Una forma inventada és.",
            ]
        );
        // A sense whose name holds a comma.
        assert_eq!(
            definitions(&contents, "banc"),
            [
                "Seient llarg per a \"diverses\" persones.",
                "Un nom amb una coma."
            ]
        );
        assert!(contents.senses.iter().all(|sense| sense.members.is_empty()));
        assert!(contents.headwords.iter().all(|h| h.translations.is_empty()));
        assert!(contents.forms.is_empty());
        let first = format::encode(&contents).expect("encode");
        assert_eq!(format::encode(&parsed()).expect("encode"), first);
    }

    #[test]
    fn a_literal_has_its_escapes_resolved() {
        assert_eq!(
            literal(r#""a\"b\\c\ndé\U0001F600\'" rest"#).as_deref(),
            Some("a\"b\\c\nd\u{e9}\u{1F600}'")
        );
        assert_eq!(literal(r#""cut"#), None);
        assert_eq!(literal("no literal"), None);
        assert_eq!(literal(r#""bad \u00zz""#).as_deref(), Some("bad \\u00zz"));
        assert_eq!(
            literal(r#""short \u00" , "next""#).as_deref(),
            Some("short \\u00")
        );
    }

    #[test]
    fn a_file_without_an_entry_of_the_language_is_an_error() {
        assert!(matches!(
            parse(FIXTURE.as_bytes(), "spa", "x"),
            Err(ParseError::NoEntry)
        ));
        assert!(matches!(
            parse(&b""[..], "cat", "x"),
            Err(ParseError::NoEntry)
        ));
        assert!(matches!(
            parse(&b"plain text\n"[..], "cat", "x"),
            Err(ParseError::NoEntry)
        ));
        // `lexvo:catx` is not `lexvo:cat`.
        let other = FIXTURE.replace("lexvo:cat;", "lexvo:catx;");
        assert!(matches!(
            parse(other.as_bytes(), "cat", "x"),
            Err(ParseError::NoEntry)
        ));
    }
}
