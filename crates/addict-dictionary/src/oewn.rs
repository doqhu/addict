//! Open English WordNet in WN-LMF XML as the contents of an index.
//!
//! A `LexicalEntry` has a `Lemma`, the headword as written, any number of
//! `Form`s and its `Sense`s, each pointing at a `Synset`; a `Synset` has a
//! `Definition` and lists its members. One written form has an entry per
//! part of speech: their senses are joined in file order. A sense record is a
//! synset, stored once with the lemmas of its members, so the synonyms of a
//! sense are the other members.
//!
//! A member is the id of an entry, as in Open English WordNet, or the id of
//! a `Sense`, as in the Open Multilingual Wordnet 2.0 files: an entry id is
//! looked up first, and a sense id gives the lemma of the entry the sense is
//! in.
//!
//! The exception list is the `Form`s: form to the lemmas it is a form of,
//! without the forms equal to their lemma.
//!
//! The file is read as a stream; the entries and the synsets are held until
//! its end, because each refers to the other by id.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::BufRead,
};

use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};

pub use crate::xml::ParseError;
use crate::{
    format::{Contents, ExceptionForm, Headword, SenseRecord},
    xml,
};

/// What is said about an entity that XML does not predefine.
const UNKNOWN_ENTITY: &str = "an unknown entity in a definition";

struct Entry {
    id: String,
    lemma: Option<String>,
    forms: Vec<String>,
    synsets: Vec<String>,
    /// The ids of the senses, for the members that name a sense.
    senses: Vec<String>,
}

struct Synset {
    id: String,
    members: Vec<String>,
    definition: String,
}

/// What the stream gave, before the ids are resolved.
#[derive(Default)]
struct Parsed {
    /// Lemma and synset ids of each entry, in file order.
    entries: Vec<(String, Vec<String>)>,
    lemma_of: HashMap<String, String>,
    /// The lemma of the entry each sense id is in.
    lemma_of_sense: HashMap<String, String>,
    /// Definition and member entry ids of each synset, in file order.
    synsets: Vec<(String, Vec<String>)>,
    synset_at: HashMap<String, u32>,
    forms: BTreeMap<String, BTreeSet<String>>,
}

impl Parsed {
    fn entry(&mut self, entry: Entry) -> Result<(), ParseError> {
        let Some(lemma) = entry.lemma else {
            return Err(malformed(format!("the entry {} has no Lemma", entry.id)));
        };
        for form in entry.forms {
            if !form.is_empty() && form != lemma {
                self.forms.entry(form).or_default().insert(lemma.clone());
            }
        }
        if self
            .lemma_of
            .insert(entry.id.clone(), lemma.clone())
            .is_some()
        {
            return Err(malformed(format!("the entry id {} is repeated", entry.id)));
        }
        for sense in entry.senses {
            self.lemma_of_sense
                .entry(sense)
                .or_insert_with(|| lemma.clone());
        }
        self.entries.push((lemma, entry.synsets));
        Ok(())
    }

    fn synset(&mut self, synset: Synset) -> Result<(), ParseError> {
        let at = u32::try_from(self.synsets.len())
            .map_err(|_| malformed("too many synsets".to_owned()))?;
        if self.synset_at.insert(synset.id.clone(), at).is_some() {
            return Err(malformed(format!(
                "the synset id {} is repeated",
                synset.id
            )));
        }
        self.synsets.push((synset.definition, synset.members));
        Ok(())
    }

    fn contents(self, source: &str) -> Result<Contents, ParseError> {
        if self.entries.is_empty() {
            return Err(malformed("no LexicalEntry element found".to_owned()));
        }
        let mut headwords: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for (lemma, synsets) in self.entries {
            let senses = headwords.entry(lemma).or_default();
            for synset in synsets {
                let at = self.synset_at.get(&synset).ok_or_else(|| {
                    malformed(format!("a sense refers to the missing synset {synset}"))
                })?;
                senses.push(*at);
            }
        }
        let senses = self
            .synsets
            .into_iter()
            .map(|(definition, member_ids)| {
                let mut members: Vec<String> = Vec::new();
                for id in member_ids {
                    // A member that is neither an entry nor a sense of the
                    // file has no lemma.
                    let lemma = self
                        .lemma_of
                        .get(&id)
                        .or_else(|| self.lemma_of_sense.get(&id));
                    if let Some(lemma) = lemma
                        && !members.contains(lemma)
                    {
                        members.push(lemma.clone());
                    }
                }
                SenseRecord {
                    definition,
                    members,
                }
            })
            .collect();
        Ok(Contents {
            source: source.to_owned(),
            senses,
            headwords: headwords
                .into_iter()
                .map(|(word, senses)| Headword {
                    word,
                    senses,
                    translations: Vec::new(),
                })
                .collect(),
            forms: self
                .forms
                .into_iter()
                .map(|(form, lemmas)| ExceptionForm {
                    form,
                    lemmas: lemmas.into_iter().collect(),
                })
                .collect(),
        })
    }
}

/// What is open at a place of the stream.
#[derive(Default)]
struct Open {
    entry: Option<Entry>,
    synset: Option<Synset>,
    /// The text of the open Definition element.
    definition: Option<String>,
    /// How many elements are open: a file cut between two entries is still
    /// cut.
    depth: u32,
}

impl Open {
    /// The element called `name` ends: an entry or a synset that ends goes
    /// to `parsed`, a definition to its synset.
    fn end(&mut self, name: &[u8], parsed: &mut Parsed) -> Result<(), ParseError> {
        self.depth = self.depth.saturating_sub(1);
        match name {
            b"LexicalEntry" => {
                if let Some(entry) = self.entry.take() {
                    parsed.entry(entry)?;
                }
            }
            b"Synset" => {
                if let Some(synset) = self.synset.take() {
                    parsed.synset(synset)?;
                }
            }
            b"Definition" => {
                // The first definition with a text is the definition.
                if let (Some(text), Some(synset)) = (self.definition.take(), self.synset.as_mut())
                    && synset.definition.is_empty()
                {
                    synset.definition = text.trim().to_owned();
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Reads a WN-LMF file into the contents of an index with the source id
/// `source`. Headwords and exception forms come out sorted by their bytes,
/// the lemmas of a form alphabetically.
pub fn parse(input: impl BufRead, source: &str) -> Result<Contents, ParseError> {
    let mut reader = Reader::from_reader(input);
    let mut parsed = Parsed::default();
    let mut open = Open::default();
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let event = xml::read_event(&mut reader, &mut buffer)?;
        match event {
            Event::Start(element) => {
                open.depth += 1;
                match element.local_name().as_ref() {
                    b"LexicalEntry" => open.entry = Some(new_entry(&element)?),
                    b"Synset" => open.synset = Some(new_synset(&element)?),
                    b"Definition" if open.synset.is_some() => {
                        open.definition = Some(String::new());
                    }
                    _ => child(&element, open.entry.as_mut())?,
                }
            }
            Event::Empty(element) => match element.local_name().as_ref() {
                b"LexicalEntry" => parsed.entry(new_entry(&element)?)?,
                b"Synset" => parsed.synset(new_synset(&element)?)?,
                _ => child(&element, open.entry.as_mut())?,
            },
            Event::End(element) => open.end(element.local_name().as_ref(), &mut parsed)?,
            Event::Text(text) => {
                if let Some(definition) = open.definition.as_mut() {
                    xml::push_content(definition, text.xml10_content(), malformed)?;
                }
            }
            Event::CData(text) => {
                if let Some(definition) = open.definition.as_mut() {
                    xml::push_content(definition, text.xml10_content(), malformed)?;
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(definition) = open.definition.as_mut() {
                    xml::push_reference(definition, &reference, malformed, UNKNOWN_ENTITY)?;
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

fn new_entry(element: &BytesStart<'_>) -> Result<Entry, ParseError> {
    Ok(Entry {
        id: required(element, "id")?,
        lemma: None,
        forms: Vec::new(),
        synsets: Vec::new(),
        senses: Vec::new(),
    })
}

fn new_synset(element: &BytesStart<'_>) -> Result<Synset, ParseError> {
    Ok(Synset {
        id: required(element, "id")?,
        members: attribute(element, "members")?
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        definition: String::new(),
    })
}

/// A `Lemma`, a `Form` or a `Sense` of the open entry; anything else, and
/// anything outside an entry, is skipped.
fn child(element: &BytesStart<'_>, entry: Option<&mut Entry>) -> Result<(), ParseError> {
    let Some(entry) = entry else {
        return Ok(());
    };
    match element.local_name().as_ref() {
        b"Lemma" => {
            let lemma = required(element, "writtenForm")?;
            if lemma.is_empty() {
                return Err(malformed(format!(
                    "the entry {} has an empty Lemma",
                    entry.id
                )));
            }
            entry.lemma = Some(lemma);
        }
        b"Form" => entry.forms.extend(attribute(element, "writtenForm")?),
        b"Sense" => {
            entry.synsets.push(required(element, "synset")?);
            entry.senses.extend(attribute(element, "id")?);
        }
        _ => {}
    }
    Ok(())
}

/// The value of the attribute `name`, with its entities resolved.
fn attribute(element: &BytesStart<'_>, name: &str) -> Result<Option<String>, ParseError> {
    xml::attribute(element, name, malformed)
}

fn required(element: &BytesStart<'_>, name: &str) -> Result<String, ParseError> {
    attribute(element, name)?.ok_or_else(|| {
        malformed(format!(
            "{} without {name}",
            String::from_utf8_lossy(element.name().as_ref())
        ))
    })
}

fn malformed(what: String) -> ParseError {
    ParseError::new(format!("not a WordNet file: {what}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small lexicon: `bank` as a noun and as a verb, a synset shared by
    /// `bank` and `shore`, a lemma with an entity, forms, and words alone in
    /// their synsets. The entries are not in the order of their lemmas.
    pub(crate) const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE LexicalResource SYSTEM "http://globalwordnet.github.io/schemas/WN-LMF-1.3.dtd">
<LexicalResource xmlns:dc="https://globalwordnet.github.io/schemas/dc/">
  <Lexicon id="oewn" label="Test" language="en" email="a@b" license="l" version="1">
    <LexicalEntry id="oewn-shore-n">
      <Lemma writtenForm="shore" partOfSpeech="n"/>
      <Sense id="s1" synset="oewn-1-n"/>
    </LexicalEntry>
    <LexicalEntry id="oewn-bank-n">
      <Lemma writtenForm="bank" partOfSpeech="n">
        <Pronunciation>bæŋk</Pronunciation>
      </Lemma>
      <Sense id="s2" synset="oewn-1-n">
        <SenseRelation relType="derivation" target="s4"/>
      </Sense>
      <Sense id="s3" synset="oewn-2-n"/>
    </LexicalEntry>
    <LexicalEntry id="oewn-bank-v">
      <Lemma writtenForm="bank" partOfSpeech="v"/>
      <Form writtenForm="banked"/>
      <Form writtenForm="bank"/>
      <Sense id="s4" synset="oewn-3-v"/>
    </LexicalEntry>
    <LexicalEntry id="oewn--ap-hood-n">
      <Lemma writtenForm="&apos;hood" partOfSpeech="n"/>
      <Sense id="s5" synset="oewn-4-n"/>
    </LexicalEntry>
    <LexicalEntry id="oewn-Bank-n">
      <Lemma writtenForm="Bank" partOfSpeech="n"/>
      <Sense id="s6" synset="oewn-2-n"/>
    </LexicalEntry>
    <LexicalEntry id="oewn-lie-v">
      <Lemma writtenForm="lie" partOfSpeech="v"/>
      <Form writtenForm="lay"/>
      <Form writtenForm="lain"/>
      <Sense id="s7" synset="oewn-5-v"/>
    </LexicalEntry>
    <LexicalEntry id="oewn-lay-v">
      <Lemma writtenForm="lay" partOfSpeech="v"/>
      <Form writtenForm="laid"/>
      <Form writtenForm="lay"/>
      <Sense id="s8" synset="oewn-6-v"/>
    </LexicalEntry>
    <LexicalEntry id="oewn-lay-a">
      <Lemma writtenForm="lay" partOfSpeech="a"/>
      <Form writtenForm="lay"/>
      <Sense id="s9" synset="oewn-7-a"/>
    </LexicalEntry>
    <Synset id="oewn-1-n" ili="i1" members="oewn-bank-n oewn-shore-n" partOfSpeech="n">
      <Definition>sloping land beside a body of water</Definition>
      <SynsetRelation relType="hypernym" target="oewn-2-n"/>
      <Example>they pulled the canoe up on the bank</Example>
    </Synset>
    <Synset id="oewn-2-n" members="oewn-bank-n oewn-Bank-n oewn-missing-n" partOfSpeech="n">
      <Definition>
        a financial institution &amp; its building &#8212; &lt;money&gt;
      </Definition>
    </Synset>
    <Synset id="oewn-3-v" members="oewn-bank-v" partOfSpeech="v">
      <Definition>tip laterally</Definition>
    </Synset>
    <Synset id="oewn-4-n" members="oewn--ap-hood-n" partOfSpeech="n">
      <Definition>a neighborhood</Definition>
    </Synset>
    <Synset id="oewn-5-v" members="oewn-lie-v" partOfSpeech="v">
      <Definition>be lying</Definition>
    </Synset>
    <Synset id="oewn-6-v" members="oewn-lay-v" partOfSpeech="v">
      <Definition>put into a certain place</Definition>
    </Synset>
    <Synset id="oewn-7-a" members="oewn-lay-a" partOfSpeech="a"/>
  </Lexicon>
</LexicalResource>
"#;

    fn parsed() -> Contents {
        parse(FIXTURE.as_bytes(), "test").expect("parse")
    }

    /// The article the index would give for `word`: each sense with its
    /// definition and its members.
    fn senses<'a>(contents: &'a Contents, word: &str) -> Vec<(&'a str, Vec<&'a str>)> {
        let headword = contents
            .headwords
            .iter()
            .find(|headword| headword.word == word)
            .expect(word);
        headword
            .senses
            .iter()
            .map(|at| {
                let sense = &contents.senses[*at as usize];
                (
                    sense.definition.as_str(),
                    sense.members.iter().map(String::as_str).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn the_headwords_are_the_lemmas_as_written_sorted_each_once() {
        let contents = parsed();
        let words: Vec<_> = contents.headwords.iter().map(|h| h.word.as_str()).collect();
        assert_eq!(words, ["'hood", "Bank", "bank", "lay", "lie", "shore"]);
        assert_eq!(contents.source, "test");
        assert!(contents.headwords.iter().all(|h| h.translations.is_empty()));
    }

    #[test]
    fn the_senses_of_every_part_of_speech_come_in_file_order() {
        let contents = parsed();
        assert_eq!(
            senses(&contents, "bank"),
            [
                ("sloping land beside a body of water", vec!["bank", "shore"]),
                (
                    "a financial institution & its building \u{2014} <money>",
                    vec!["bank", "Bank"]
                ),
                ("tip laterally", vec!["bank"]),
            ]
        );
    }

    #[test]
    fn a_synset_is_stored_once_and_shared_by_its_members() {
        let contents = parsed();
        assert_eq!(contents.senses.len(), 7);
        let of = |word: &str| {
            let headword = contents.headwords.iter().find(|h| h.word == word);
            headword.expect(word).senses.clone()
        };
        assert_eq!(of("shore"), [0]);
        assert_eq!(of("bank"), [0, 1, 2]);
        assert_eq!(of("Bank"), [1]);
    }

    #[test]
    fn an_entity_in_a_lemma_is_resolved() {
        let contents = parsed();
        assert_eq!(
            senses(&contents, "'hood"),
            [("a neighborhood", vec!["'hood"])]
        );
    }

    #[test]
    fn a_word_alone_in_its_synset_is_its_only_member() {
        let contents = parsed();
        assert_eq!(senses(&contents, "lie"), [("be lying", vec!["lie"])]);
    }

    #[test]
    fn a_synset_without_a_definition_gives_an_empty_one() {
        let contents = parsed();
        assert_eq!(
            senses(&contents, "lay"),
            [("put into a certain place", vec!["lay"]), ("", vec!["lay"])]
        );
    }

    #[test]
    fn the_exception_list_is_the_forms_without_those_equal_to_the_lemma() {
        let contents = parsed();
        let forms: Vec<_> = contents
            .forms
            .iter()
            .map(|form| (form.form.as_str(), form.lemmas.clone()))
            .collect();
        assert_eq!(
            forms,
            [
                ("banked", vec!["bank".to_owned()]),
                ("laid", vec!["lay".to_owned()]),
                ("lain", vec!["lie".to_owned()]),
                // "lay" is a form of "lie"; as a form of "lay" it is dropped.
                ("lay", vec!["lie".to_owned()]),
            ]
        );
    }

    #[test]
    fn the_lemmas_of_a_form_are_sorted_and_not_repeated() {
        let xml = r#"<LexicalResource><Lexicon>
            <LexicalEntry id="b"><Lemma writtenForm="b"/><Form writtenForm="x"/></LexicalEntry>
            <LexicalEntry id="a"><Lemma writtenForm="a"/><Form writtenForm="x"/></LexicalEntry>
            <LexicalEntry id="a2"><Lemma writtenForm="a"/><Form writtenForm="x"/><Form/></LexicalEntry>
        </Lexicon></LexicalResource>"#;
        let contents = parse(xml.as_bytes(), "test").expect("parse");
        assert_eq!(contents.forms.len(), 1);
        assert_eq!(contents.forms[0].lemmas, ["a", "b"]);
        assert_eq!(contents.headwords.len(), 2);
    }

    /// A lexicon in the shape of Open Multilingual Wordnet 2.0: the members
    /// of a synset are sense ids, and an entry id among them is still read.
    pub(crate) const OMW: &str = r#"<LexicalResource><Lexicon id="omw-es">
        <LexicalEntry id="omw-es-banco-n">
          <Lemma writtenForm="banco" partOfSpeech="n"/>
          <Sense id="omw-es-banco-08420278-n" synset="omw-es-08420278-n"/>
          <Sense id="omw-es-banco-04177820-n" synset="omw-es-04177820-n"/>
        </LexicalEntry>
        <LexicalEntry id="omw-es-entidad_bancaria-n">
          <Lemma writtenForm="entidad bancaria" partOfSpeech="n"/>
          <Sense id="omw-es-entidad_bancaria-08420278-n" synset="omw-es-08420278-n"/>
        </LexicalEntry>
        <LexicalEntry id="omw-es-banca-n">
          <Lemma writtenForm="banca" partOfSpeech="n"/>
          <Sense id="omw-es-banca-08420278-n" synset="omw-es-08420278-n"/>
          <Sense synset="omw-es-04177820-n"/>
        </LexicalEntry>
        <LexicalEntry id="omw-es-escaño-n">
          <Lemma writtenForm="escaño" partOfSpeech="n"/>
          <Sense id="omw-es-escaño-04177820-n" synset="omw-es-04177820-n"/>
        </LexicalEntry>
        <Synset id="omw-es-08420278-n" ili="i1" members="omw-es-banco-08420278-n omw-es-entidad_bancaria-08420278-n omw-es-banca-08420278-n omw-es-gone-08420278-n">
          <Definition>entidad financiera</Definition>
        </Synset>
        <Synset id="omw-es-04177820-n" ili="i2" members="omw-es-escaño-04177820-n omw-es-banco-n omw-es-banco-04177820-n"/>
    </Lexicon></LexicalResource>"#;

    #[test]
    fn the_members_of_a_synset_may_name_senses() {
        let contents = parse(OMW.as_bytes(), "omw").expect("parse");
        assert_eq!(
            senses(&contents, "banco"),
            [
                (
                    "entidad financiera",
                    vec!["banco", "entidad bancaria", "banca"]
                ),
                // An entry id among sense ids, and the same lemma twice.
                ("", vec!["escaño", "banco"]),
            ]
        );
        assert_eq!(senses(&contents, "escaño"), [("", vec!["escaño", "banco"])]);
        // A sense without an id is a sense, and names no member.
        assert_eq!(senses(&contents, "banca").len(), 2);
        assert!(crate::format::encode(&contents).is_ok());
    }

    #[test]
    fn the_parsed_contents_can_be_written() {
        assert!(crate::format::encode(&parsed()).is_ok());
    }

    #[test]
    fn what_is_not_a_wordnet_file_is_an_error() {
        let entry =
            |body: &str| format!("<LexicalResource><Lexicon>{body}</Lexicon></LexicalResource>");
        let cases = [
            ("no entries", entry("")),
            ("not XML", "plain text".to_owned()),
            ("empty", String::new()),
            (
                "no lemma",
                entry(r#"<LexicalEntry id="a"><Sense synset="s"/></LexicalEntry>"#),
            ),
            ("an empty entry", entry(r#"<LexicalEntry id="a"/>"#)),
            (
                "an empty lemma",
                entry(r#"<LexicalEntry id="a"><Lemma writtenForm=""/></LexicalEntry>"#),
            ),
            (
                "no id",
                entry(r#"<LexicalEntry><Lemma writtenForm="a"/></LexicalEntry>"#),
            ),
            (
                "a missing synset",
                entry(
                    r#"<LexicalEntry id="a"><Lemma writtenForm="a"/><Sense synset="s"/></LexicalEntry>"#,
                ),
            ),
            (
                "a sense without a synset",
                entry(r#"<LexicalEntry id="a"><Lemma writtenForm="a"/><Sense/></LexicalEntry>"#),
            ),
            (
                "a repeated entry id",
                entry(
                    r#"<LexicalEntry id="a"><Lemma writtenForm="a"/></LexicalEntry><LexicalEntry id="a"><Lemma writtenForm="b"/></LexicalEntry>"#,
                ),
            ),
            (
                "a repeated synset id",
                entry(
                    r#"<LexicalEntry id="a"><Lemma writtenForm="a"/></LexicalEntry><Synset id="s"/><Synset id="s"/>"#,
                ),
            ),
            (
                "an unknown entity",
                entry(
                    r#"<LexicalEntry id="a"><Lemma writtenForm="a"/></LexicalEntry><Synset id="s"><Definition>&nbsp;</Definition></Synset>"#,
                ),
            ),
            (
                "mismatched tags",
                entry(r#"<LexicalEntry id="a"><Lemma writtenForm="a"/></Synset>"#),
            ),
            (
                "cut inside an entry",
                r#"<LexicalResource><Lexicon><LexicalEntry id="a"><Lemma writtenForm="a"/>"#
                    .to_owned(),
            ),
            ("cut inside a tag", FIXTURE[..FIXTURE.len() / 2].to_owned()),
            (
                "cut between two elements",
                FIXTURE
                    .replace("</Lexicon>", "")
                    .replace("</LexicalResource>", ""),
            ),
        ];
        for (name, xml) in cases {
            assert!(parse(xml.as_bytes(), "test").is_err(), "{name}");
        }
    }
}
