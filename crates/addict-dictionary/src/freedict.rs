//! A FreeDict dictionary in TEI P5 XML as the contents of an index.
//!
//! An `entry` has its headword as written in `form/orth` and any number of
//! `sense`s; a sense has the Russian words in `cit type="trans"
//! xml:lang="ru"`, one per `quote`, and its English definitions in nested
//! `sense/def` elements, of which it may have none. One written form has an
//! entry per part of speech: they are joined in file order into one headword.
//!
//! A headword gets one sense record per definition, without members: the
//! format has no synonyms. Its translations are the Russian words of all its
//! senses in file order, each once at its first place; which sense a
//! translation belongs to is not kept. Texts are stored as written, stress
//! marks included.
//!
//! The pronunciation, the part of speech and everything outside the entries
//! are skipped. The file is read as a stream; the headwords are held until
//! its end, because the entries are not in the order of the index.

use std::{collections::BTreeMap, io::BufRead};

use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};

pub use crate::xml::ParseError;
use crate::{
    format::{Contents, Headword, SenseRecord},
    xml,
};

/// What is said about an entity that XML does not predefine.
const UNKNOWN_ENTITY: &str = "an unknown entity in an entry";

/// What an open `entry` has given so far.
#[derive(Default)]
struct Entry {
    orths: Vec<String>,
    definitions: Vec<String>,
    translations: Vec<String>,
}

/// The element whose text is being collected.
#[derive(Clone, Copy)]
enum Field {
    Orth,
    Definition,
    Translation,
}

struct Capture {
    field: Field,
    /// The depth of the element: it ends where the depth is this again.
    depth: u32,
    text: String,
}

/// The definitions and the translations of one headword, in file order.
#[derive(Default)]
struct Collected {
    definitions: Vec<String>,
    translations: Vec<String>,
}

#[derive(Default)]
struct Parsed {
    entries: usize,
    headwords: BTreeMap<String, Collected>,
}

impl Parsed {
    fn entry(&mut self, entry: Entry) -> Result<(), ParseError> {
        self.entries += 1;
        if entry.orths.is_empty() {
            return Err(malformed(format!("the entry {} has no orth", self.entries)));
        }
        for (at, orth) in entry.orths.iter().enumerate() {
            if entry.orths[..at].contains(orth) {
                continue;
            }
            let collected = self.headwords.entry(orth.clone()).or_default();
            collected
                .definitions
                .extend(entry.definitions.iter().cloned());
            for translation in &entry.translations {
                if !collected.translations.contains(translation) {
                    collected.translations.push(translation.clone());
                }
            }
        }
        Ok(())
    }

    fn contents(self, source: &str) -> Result<Contents, ParseError> {
        if self.entries == 0 {
            return Err(malformed("no entry element found".to_owned()));
        }
        let mut senses = Vec::new();
        let mut headwords = Vec::with_capacity(self.headwords.len());
        // The map gives the headwords in the order of their bytes.
        for (word, collected) in self.headwords {
            let mut places = Vec::with_capacity(collected.definitions.len());
            for definition in collected.definitions {
                places.push(
                    u32::try_from(senses.len())
                        .map_err(|_| malformed("too many definitions".to_owned()))?,
                );
                senses.push(SenseRecord {
                    definition,
                    members: Vec::new(),
                });
            }
            headwords.push(Headword {
                word,
                senses: places,
                translations: collected.translations,
            });
        }
        Ok(Contents {
            source: source.to_owned(),
            senses,
            headwords,
            forms: Vec::new(),
        })
    }
}

/// What is open at a place of the stream.
#[derive(Default)]
struct Open {
    /// The open entry and its depth.
    entry: Option<(u32, Entry)>,
    /// The depth of the open list of Russian translations.
    russian: Option<u32>,
    capture: Option<Capture>,
    /// How many elements are open: a file cut between two entries is still
    /// cut.
    depth: u32,
}

impl Open {
    /// An element with contents starts.
    fn start(&mut self, element: &BytesStart<'_>) -> Result<(), ParseError> {
        if self.depth == 0 {
            root(element)?;
        }
        self.depth += 1;
        match (element.local_name().as_ref(), self.entry.is_some()) {
            (b"entry", false) => self.entry = Some((self.depth, Entry::default())),
            (b"entry", true) => return Err(nested()),
            (name, true) if self.capture.is_none() => {
                let field = match name {
                    b"orth" => Some(Field::Orth),
                    b"def" => Some(Field::Definition),
                    b"quote" if self.russian.is_some() => Some(Field::Translation),
                    b"cit" if self.russian.is_none() && is_russian(element)? => {
                        self.russian = Some(self.depth);
                        None
                    }
                    _ => None,
                };
                let depth = self.depth;
                self.capture = field.map(|field| Capture {
                    field,
                    depth,
                    text: String::new(),
                });
            }
            _ => {}
        }
        Ok(())
    }

    /// An element ends: the text collected in it goes to the open entry, and
    /// an entry that ends goes to `parsed`.
    fn end(&mut self, parsed: &mut Parsed) -> Result<(), ParseError> {
        let depth = self.depth;
        if let Some(done) = self.capture.take_if(|capture| capture.depth == depth)
            && let Some((_, entry)) = self.entry.as_mut()
        {
            let text = done.text.trim();
            if !text.is_empty() {
                match done.field {
                    Field::Orth => entry.orths.push(text.to_owned()),
                    Field::Definition => entry.definitions.push(text.to_owned()),
                    Field::Translation => entry.translations.push(text.to_owned()),
                }
            }
        }
        if self.russian == Some(depth) {
            self.russian = None;
        }
        if let Some((_, entry)) = self.entry.take_if(|(at, _)| *at == depth) {
            parsed.entry(entry)?;
        }
        self.depth = depth.saturating_sub(1);
        Ok(())
    }
}

/// Reads a FreeDict TEI file into the contents of an index with the source
/// id `source`. Headwords come out sorted by their bytes, each once; there
/// are no exception forms.
pub fn parse(input: impl BufRead, source: &str) -> Result<Contents, ParseError> {
    let mut reader = Reader::from_reader(input);
    let mut parsed = Parsed::default();
    let mut open = Open::default();
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let event = xml::read_event(&mut reader, &mut buffer)?;
        match event {
            Event::Start(element) => open.start(&element)?,
            Event::Empty(element) => {
                if open.depth == 0 {
                    root(&element)?;
                }
                if element.local_name().as_ref() == b"entry" {
                    if open.entry.is_some() {
                        return Err(nested());
                    }
                    parsed.entry(Entry::default())?;
                }
            }
            Event::End(_) => open.end(&mut parsed)?,
            Event::Text(text) => {
                if let Some(capture) = open.capture.as_mut() {
                    xml::push_content(&mut capture.text, text.xml10_content(), malformed)?;
                }
            }
            Event::CData(text) => {
                if let Some(capture) = open.capture.as_mut() {
                    xml::push_content(&mut capture.text, text.xml10_content(), malformed)?;
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(capture) = open.capture.as_mut() {
                    xml::push_reference(&mut capture.text, &reference, malformed, UNKNOWN_ENTITY)?;
                }
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
    if open.depth != 0 {
        return Err(malformed("the file ends inside an element".to_owned()));
    }
    parsed.contents(source)
}

/// The outermost element of the file must be `TEI`.
fn root(element: &BytesStart<'_>) -> Result<(), ParseError> {
    if element.local_name().as_ref() == b"TEI" {
        Ok(())
    } else {
        Err(malformed(format!(
            "its root element is {}",
            String::from_utf8_lossy(element.name().as_ref())
        )))
    }
}

/// Whether a `cit` is the list of Russian translations of a sense.
fn is_russian(element: &BytesStart<'_>) -> Result<bool, ParseError> {
    Ok(
        xml::attribute(element, "type", malformed)?.as_deref() == Some("trans")
            && xml::attribute(element, "xml:lang", malformed)?.as_deref() == Some("ru"),
    )
}

fn malformed(what: String) -> ParseError {
    ParseError::new(format!("not a FreeDict TEI file: {what}"))
}

fn nested() -> ParseError {
    malformed("an entry inside an entry".to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small dictionary: `bank` as a noun and as a verb, a sense with two
    /// definitions and one with none, translations repeated inside an entry
    /// and across the two, entities, a citation that is not a translation,
    /// and a headword without any definition. The entries are not in the
    /// order of their headwords.
    pub(crate) const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<?xml-stylesheet type="text/css" href="freedict-dictionary.css"?>
<!DOCTYPE TEI SYSTEM "freedict-P5.dtd">
<TEI xmlns="http://www.tei-c.org/ns/1.0" xmlns:wikdict="http://www.wikdict.com/ns/1.0">
  <teiHeader xml:lang="en">
    <fileDesc>
      <titleStmt><title>English-Русский test dictionary</title></titleStmt>
      <sourceDesc><p>An <ref target="http://example.org/">orth</ref> of no entry.</p></sourceDesc>
    </fileDesc>
  </teiHeader>
  <text>
    <body>
    <entry>
      <form>
        <orth>shore</orth>
      </form>
      <sense>
        <cit type="trans" xml:lang="ru">
          <quote>бе́рег</quote>
        </cit>
        <sense>
          <def>Land adjoining a large body of water.</def>
        </sense>
      </sense>
    </entry>
    <entry>
      <form>
        <orth>bank</orth>
        <pron>/bæŋk/</pron>
      </form>
      <gramGrp>
        <pos>n</pos>
      </gramGrp>
      <sense>
        <cit type="trans" xml:lang="ru">
          <quote>бе́рег</quote>
          <quote>брег</quote>
        </cit>
        <sense>
          <def>(hydrology) An edge of river &amp; lake.</def>
        </sense>
      </sense>
      <sense>
        <cit type="trans" xml:lang="ru">
          <quote>банк</quote>
          <quote>брег</quote>
        </cit>
        <cit type="example" xml:lang="en">
          <quote>not a translation</quote>
        </cit>
        <cit type="trans" xml:lang="de">
          <quote>Bank</quote>
        </cit>
        <sense>
          <def>A branch office of such an institution.</def>
        </sense>
        <sense>
          <def>The sum of money &lt;held&gt; by the dealer.</def>
        </sense>
      </sense>
      <sense>
        <cit type="trans" xml:lang="ru">
          <quote>ряд</quote>
        </cit>
      </sense>
    </entry>
    <entry>
      <form>
        <orth>R&amp;D</orth>
      </form>
      <sense>
        <cit type="trans" xml:lang="ru">
          <quote>НИОКР</quote>
        </cit>
      </sense>
    </entry>
    <entry>
      <form>
        <orth>bank</orth>
      </form>
      <gramGrp>
        <pos>v</pos>
      </gramGrp>
      <sense>
        <cit type="trans" xml:lang="ru">
          <quote>накреня́ться</quote>
          <quote>банк</quote>
        </cit>
        <sense>
          <def>To roll or incline laterally.</def>
        </sense>
      </sense>
    </entry>
    <entry>
      <form>
        <orth>'s-Hertogenbosch</orth>
      </form>
      <sense>
        <cit type="trans" xml:lang="ru">
          <quote>Хертогенбос</quote>
        </cit>
        <sense>
          <def>   </def>
        </sense>
      </sense>
    </entry>
    </body>
  </text>
</TEI>
"#;

    fn parsed() -> Contents {
        parse(FIXTURE.as_bytes(), "test").expect("parse")
    }

    fn headword<'a>(contents: &'a Contents, word: &str) -> &'a Headword {
        contents
            .headwords
            .iter()
            .find(|headword| headword.word == word)
            .expect(word)
    }

    fn definitions<'a>(contents: &'a Contents, word: &str) -> Vec<&'a str> {
        headword(contents, word)
            .senses
            .iter()
            .map(|at| contents.senses[*at as usize].definition.as_str())
            .collect()
    }

    #[test]
    fn the_headwords_are_the_orths_as_written_sorted_each_once() {
        let contents = parsed();
        let words: Vec<_> = contents.headwords.iter().map(|h| h.word.as_str()).collect();
        assert_eq!(words, ["'s-Hertogenbosch", "R&D", "bank", "shore"]);
        assert_eq!(contents.source, "test");
        assert!(contents.forms.is_empty());
    }

    #[test]
    fn the_entries_of_one_headword_are_joined_in_file_order() {
        let contents = parsed();
        assert_eq!(
            definitions(&contents, "bank"),
            [
                "(hydrology) An edge of river & lake.",
                "A branch office of such an institution.",
                "The sum of money <held> by the dealer.",
                "To roll or incline laterally.",
            ]
        );
    }

    #[test]
    fn a_translation_is_listed_once_at_its_first_place_with_its_stress_mark() {
        let contents = parsed();
        assert_eq!(
            headword(&contents, "bank").translations,
            ["бе́рег", "брег", "банк", "ряд", "накреня́ться"]
        );
        assert!(
            headword(&contents, "bank").translations[0].contains('\u{301}'),
            "the stress mark is kept"
        );
        assert_eq!(headword(&contents, "shore").translations, ["бе́рег"]);
    }

    #[test]
    fn a_sense_record_is_a_definition_without_members() {
        let contents = parsed();
        assert_eq!(contents.senses.len(), 5);
        assert!(contents.senses.iter().all(|sense| sense.members.is_empty()));
        assert!(
            contents
                .senses
                .iter()
                .all(|sense| !sense.definition.is_empty())
        );
        // Every record belongs to one headword.
        let mut used: Vec<u32> = contents
            .headwords
            .iter()
            .flat_map(|headword| headword.senses.iter().copied())
            .collect();
        used.sort_unstable();
        assert_eq!(used, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_headword_without_a_definition_has_its_translations_only() {
        let contents = parsed();
        for (word, translation) in [("R&D", "НИОКР"), ("'s-Hertogenbosch", "Хертогенбос")]
        {
            assert!(headword(&contents, word).senses.is_empty(), "{word}");
            assert_eq!(headword(&contents, word).translations, [translation]);
        }
    }

    #[test]
    fn the_parsed_contents_can_be_written() {
        assert!(crate::format::encode(&parsed()).is_ok());
    }

    #[test]
    fn an_orth_given_twice_in_an_entry_is_one_headword() {
        let xml = r#"<TEI><entry><form><orth>a</orth><orth>b</orth><orth>a</orth></form>
            <sense><cit type="trans" xml:lang="ru"><quote>а</quote></cit>
            <sense><def>first</def></sense></sense></entry></TEI>"#;
        let contents = parse(xml.as_bytes(), "test").expect("parse");
        assert_eq!(contents.headwords.len(), 2);
        assert_eq!(definitions(&contents, "a"), ["first"]);
        assert_eq!(definitions(&contents, "b"), ["first"]);
        assert_eq!(headword(&contents, "a").translations, ["а"]);
    }

    #[test]
    fn what_is_not_a_freedict_file_is_an_error() {
        let tei = |body: &str| format!("<TEI><text><body>{body}</body></text></TEI>");
        let cases = [
            ("no entries", tei("")),
            ("not XML", "plain text".to_owned()),
            ("empty", String::new()),
            (
                "another root",
                r#"<LexicalResource><entry><form><orth>a</orth></form></entry></LexicalResource>"#
                    .to_owned(),
            ),
            (
                "no orth",
                tei(r#"<entry><sense><def>a</def></sense></entry>"#),
            ),
            ("an empty entry", tei("<entry/>")),
            (
                "an empty orth",
                tei("<entry><form><orth> </orth></form></entry>"),
            ),
            (
                "an entry inside an entry",
                tei(
                    "<entry><form><orth>a</orth></form><entry><form><orth>b</orth></form></entry></entry>",
                ),
            ),
            (
                "an empty entry inside an entry",
                tei("<entry><form><orth>a</orth></form><entry/></entry>"),
            ),
            (
                "an unknown entity",
                tei("<entry><form><orth>a&nbsp;b</orth></form></entry>"),
            ),
            (
                "mismatched tags",
                tei("<entry><form><orth>a</orth></sense></entry>"),
            ),
            (
                "cut inside an entry",
                "<TEI><text><body><entry><form><orth>a</orth></form>".to_owned(),
            ),
            ("cut inside a tag", FIXTURE[..FIXTURE.len() / 2].to_owned()),
            (
                "cut between two elements",
                FIXTURE
                    .replace("</body>", "")
                    .replace("</text>", "")
                    .replace("</TEI>", ""),
            ),
        ];
        for (name, xml) in cases {
            let error = parse(xml.as_bytes(), "test").expect_err(name);
            assert!(!error.to_string().contains('\n'), "{name}: {error}");
        }
    }
}
