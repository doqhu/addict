//! A Kaikki file, Wiktionary as JSON Lines, as the contents of an index.
//!
//! Each line is one entry: one word of one language in one part of speech.
//! Only the entries of the language asked for are read, by their
//! `lang_code`; the others are skipped. Of an entry, these fields are read,
//! the same ones the selection experiment counted
//! (`experiments/exp9/sources.py`, `kaikki`):
//!
//! - `word`, the headword as written;
//! - `senses[].glosses`, the definition of each sense, its glosses joined
//!   with a space. A sense that points at another word (`form_of`,
//!   `alt_of`), such as «Plural de banco.», is a sense like the others. A
//!   sense without a gloss is skipped;
//! - `synonyms[]`, each with the `sense_index` of the sense it belongs to,
//!   1-based within the entry: it is matched with the `sense_index` of the
//!   senses, and with the place of the sense when no sense has one. A
//!   synonym without an index, or with one that names no sense of the entry,
//!   is dropped, and so is the word itself;
//! - the form→lemma pairs: the word an entry with a `form_of` or `alt_of`
//!   sense points at, and the `forms` list of an entry without the forms
//!   that are table tags, inflection templates, classes, romanizations or
//!   canonical spellings. Both sides of a pair are one word ([`is_word`]);
//! - of an entry whose `pos` is `verb`, its `forms` again with their
//!   `tags`: the conjugation table, each form with what
//!   [`analysis::from_tags`] makes of its tags, the word as its lemma. A
//!   form that is the word itself, or not one word, or whose tags give no
//!   shape, has no analysis.
//!
//! The entries of one word, one per part of speech, are joined in file order
//! into one headword. A headword gets one sense record per sense, with its
//! synonyms as members: the format's way for a source without synsets. A
//! word whose entries give no sense is no headword. The headwords have no
//! translations. The pairs go into the form table, a form with its lemmas in
//! the order first seen; the lookups do not read them yet. The analyses go
//! into the analysis table: per form its lemmas sorted, per lemma its
//! shapes sorted, each once, so two rows of one analysis are one.
//!
//! The file is read as a stream; the headwords are held until its end,
//! because the entries are not in the order of the index.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::{self, BufRead},
};

use serde::Deserialize;

use crate::{
    analysis::{self, Analyses, Shape},
    format::{AnalysedForm, Contents, ExceptionForm, SenseRecord},
};

/// The analyses being gathered: per form, per lemma, the shapes.
type Gathered = BTreeMap<String, BTreeMap<String, BTreeSet<Shape>>>;

/// The tags of a `forms` item that is no inflected form of the word.
const FORMS_SKIPPED: [&str; 5] = [
    "table-tags",
    "inflection-template",
    "class",
    "romanization",
    "canonical",
];

/// Why a Kaikki file gave no contents.
#[derive(Debug)]
pub enum ParseError {
    /// The stream could not be read.
    Io(io::Error),
    /// A line is not an entry; its number, from 1, and why.
    Line { number: usize, cause: String },
    /// No line is an entry of the language.
    NoEntry,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cannot read the file: {error}"),
            Self::Line { number, cause } => {
                write!(f, "line {number} is not a Kaikki entry: {cause}")
            }
            Self::NoEntry => write!(f, "the file has no entry of the language"),
        }
    }
}

impl std::error::Error for ParseError {}

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    word: Option<String>,
    #[serde(default)]
    lang_code: Option<String>,
    #[serde(default)]
    pos: Option<String>,
    #[serde(default)]
    senses: Vec<Sense>,
    #[serde(default)]
    synonyms: Vec<Synonym>,
    #[serde(default)]
    forms: Vec<Form>,
}

#[derive(Deserialize)]
struct Sense {
    #[serde(default)]
    glosses: Vec<String>,
    #[serde(default)]
    sense_index: Option<String>,
    #[serde(default)]
    form_of: Vec<Target>,
    #[serde(default)]
    alt_of: Vec<Target>,
}

#[derive(Deserialize)]
struct Synonym {
    #[serde(default)]
    word: Option<String>,
    #[serde(default)]
    sense_index: Option<String>,
}

#[derive(Deserialize)]
struct Target {
    #[serde(default)]
    word: Option<String>,
}

#[derive(Deserialize)]
struct Form {
    #[serde(default)]
    form: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Whether `sense` has a gloss that is not blank.
fn has_gloss(sense: &Sense) -> bool {
    sense.glosses.iter().any(|gloss| !gloss.trim().is_empty())
}

/// Whether `text` is one word: letters, with an inner `'` or `·` only
/// between two letters. The check of `rules.py`, `is_word`.
pub fn is_word(text: &str) -> bool {
    let characters: Vec<char> = text.chars().collect();
    let (Some(first), Some(last)) = (characters.first(), characters.last()) else {
        return false;
    };
    if !first.is_alphabetic() || !last.is_alphabetic() {
        return false;
    }
    characters.iter().enumerate().all(|(at, character)| {
        character.is_alphabetic()
            || (matches!(character, '\'' | '·')
                && characters[at - 1].is_alphabetic()
                && characters[at + 1].is_alphabetic())
    })
}

/// The contents of the index of the Kaikki file `reader` holds, of the
/// entries whose `lang_code` is `language`; `source` is the id written into
/// the index.
pub fn parse(reader: impl BufRead, language: &str, source: &str) -> Result<Contents, ParseError> {
    let mut entries = 0;
    let mut headwords: BTreeMap<String, Vec<SenseRecord>> = BTreeMap::new();
    let mut forms: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut analyses = Gathered::new();
    for (at, line) in reader.lines().enumerate() {
        let line = line.map_err(ParseError::Io)?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: Entry = serde_json::from_str(&line).map_err(|error| ParseError::Line {
            number: at + 1,
            cause: error.to_string(),
        })?;
        if entry.lang_code.as_deref() != Some(language) {
            continue;
        }
        let Some(word) = entry.word.filter(|word| !word.is_empty()) else {
            continue;
        };
        entries += 1;

        add_pairs(&mut forms, &word, &entry.senses, &entry.forms);
        if entry.pos.as_deref() == Some("verb") {
            add_analyses(&mut analyses, &word, &entry.forms);
        }
        let senses = sense_records(&word, &entry.senses, &entry.synonyms);
        if !senses.is_empty() {
            headwords.entry(word).or_default().extend(senses);
        }
    }
    if entries == 0 {
        return Err(ParseError::NoEntry);
    }

    let mut contents = Contents::of_senses(source, headwords);
    contents.forms = forms
        .into_iter()
        .map(|(form, lemmas)| ExceptionForm { form, lemmas })
        .collect();
    contents.analyses = analyses
        .into_iter()
        .map(|(form, lemmas)| AnalysedForm {
            form,
            lemmas: lemmas
                .into_iter()
                .map(|(lemma, shapes)| Analyses {
                    lemma,
                    shapes: shapes.into_iter().collect(),
                })
                .collect(),
        })
        .collect();
    Ok(contents)
}

/// Adds to `forms` the form→lemma pairs of the entry of `word` with
/// `senses` and `forms_of_entry`: the word its `form_of` and `alt_of`
/// senses point at, then its `forms` without the skipped tags.
fn add_pairs(
    forms: &mut BTreeMap<String, Vec<String>>,
    word: &str,
    senses: &[Sense],
    forms_of_entry: &[Form],
) {
    for sense in senses {
        for target in sense.form_of.iter().chain(&sense.alt_of) {
            if let Some(lemma) = &target.word {
                pair(forms, word, lemma);
            }
        }
    }
    for form in forms_of_entry {
        let skipped = form
            .tags
            .iter()
            .any(|tag| FORMS_SKIPPED.contains(&tag.as_str()));
        if let (Some(text), false) = (&form.form, skipped) {
            pair(forms, text, word);
        }
    }
}

/// Adds to `analyses` the shapes of the conjugation table `forms_of_entry`
/// of the verb `lemma`.
fn add_analyses(analyses: &mut Gathered, lemma: &str, forms_of_entry: &[Form]) {
    if !is_word(lemma) {
        return;
    }
    for form in forms_of_entry {
        let Some(text) = form.form.as_deref() else {
            continue;
        };
        if text == lemma || !is_word(text) {
            continue;
        }
        if let Some(shape) = analysis::from_tags(&form.tags) {
            analyses
                .entry(text.to_owned())
                .or_default()
                .entry(lemma.to_owned())
                .or_default()
                .insert(shape);
        }
    }
}

/// Adds `lemma` to the lemmas of `form` in `forms`, once, when the two
/// differ and both are one word.
fn pair(forms: &mut BTreeMap<String, Vec<String>>, form: &str, lemma: &str) {
    if form != lemma && is_word(form) && is_word(lemma) {
        let lemmas = forms.entry(form.to_owned()).or_default();
        if !lemmas.iter().any(|known| known == lemma) {
            lemmas.push(lemma.to_owned());
        }
    }
}

/// The sense records of the entry of `word` with `senses` and `synonyms`:
/// one per sense with a gloss, its synonyms as members.
fn sense_records(word: &str, senses: &[Sense], synonyms: &[Synonym]) -> Vec<SenseRecord> {
    let mut records: Vec<(Option<&str>, SenseRecord)> = senses
        .iter()
        .filter(|sense| has_gloss(sense))
        .map(|sense| {
            let definition = sense
                .glosses
                .iter()
                .map(|gloss| gloss.trim())
                .filter(|gloss| !gloss.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            (
                sense.sense_index.as_deref(),
                SenseRecord {
                    definition,
                    members: Vec::new(),
                    metadata: None,
                },
            )
        })
        .collect();
    // The place of a sense among all senses of the entry, glossless ones
    // too: a `sense_index` without a field to match counts them.
    let places: Vec<usize> = senses
        .iter()
        .enumerate()
        .filter(|(_, sense)| has_gloss(sense))
        .map(|(place, _)| place + 1)
        .collect();
    let indexed = records.iter().any(|(index, _)| index.is_some());
    for synonym in synonyms {
        let (Some(text), Some(index)) = (&synonym.word, &synonym.sense_index) else {
            continue;
        };
        if text.is_empty() || *text == word {
            continue;
        }
        let at = if indexed {
            records
                .iter()
                .position(|(own, _)| own.is_some_and(|own| own == index.trim()))
        } else {
            index
                .trim()
                .parse::<usize>()
                .ok()
                .and_then(|number| places.iter().position(|place| *place == number))
        };
        if let Some((_, record)) = at.and_then(|at| records.get_mut(at))
            && !record.members.contains(text)
        {
            record.members.push(text.clone());
        }
    }
    records.into_iter().map(|(_, record)| record).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::format::{self, tests::texts};

    /// Small lines in the shape of `kaikki-es-Espanol`: `banco` as a noun
    /// with numbered synonyms and as an interjection, a plural that is a
    /// form of it, a word of another language and a glossless sense.
    pub(crate) const FIXTURE: &str = concat!(
        r#"{"word": "banco", "pos": "noun", "lang_code": "es", "senses": [{"glosses": ["Empresa que ofrece servicios monetarios."], "sense_index": "1"}, {"glosses": ["Mueble alargado para sentarse."], "sense_index": "2"}, {"glosses": ["Mueble sin respaldo."], "sense_index": "3"}], "synonyms": [{"word": "banca", "sense_index": "2"}, {"word": "banquillo", "sense_index": "3"}, {"word": "escaño"}, {"word": "banco", "sense_index": "1"}, {"word": "banca", "sense_index": "2"}, {"word": "cardumen", "sense_index": "9"}], "forms": [{"form": "bancos", "tags": ["plural"]}, {"form": "es-sust", "tags": ["inflection-template"]}]}"#,
        "\n",
        r#"{"word": "cerrar", "pos": "verb", "lang_code": "es", "senses": [{"tags": ["no-gloss"]}, {"glosses": ["Asegurar con cerradura.", "Impedir el paso."], "sense_index": "1"}], "forms": [{"form": "cierro"}, {"form": "cerró"}, {"form": "cierre usted"}]}"#,
        "\n",
        "\n",
        r#"{"word": "bancos", "pos": "noun", "lang_code": "es", "senses": [{"glosses": ["Plural de banco."], "form_of": [{"word": "banco"}], "sense_index": "1"}]}"#,
        "\n",
        r#"{"word": "banco", "pos": "intj", "lang_code": "es", "senses": [{"glosses": ["Expresa aceptación."], "sense_index": "1"}]}"#,
        "\n",
        r#"{"word": "bank", "pos": "noun", "lang_code": "en", "senses": [{"glosses": ["A company."]}]}"#,
        "\n",
        r#"{"word": "arromanzar", "pos": "verb", "lang_code": "es", "senses": [{"tags": ["no-gloss"]}]}"#,
        "\n",
    );

    /// Verb lines in the shape of `kaikki-en-Spanish`: the table of
    /// `hablar` with a head-line row, a repeated row, a formal and a
    /// negative imperative, a combined form, one of two words, a vos form
    /// and a row without a tense; `ir` and
    /// `ser`, which share `fue`; a noun whose forms are no verb forms.
    const VERBS: &str = concat!(
        r#"{"word": "hablar", "pos": "verb", "lang_code": "es", "senses": [{"glosses": ["to speak"]}], "forms": [{"form": "hablar", "tags": ["infinitive"]}, {"form": "hablo", "tags": ["first-person", "present", "singular"]}, {"form": "hablo", "tags": ["first-person", "indicative", "present", "singular"]}, {"form": "hable", "tags": ["first-person", "present", "singular", "subjunctive"]}, {"form": "hable", "tags": ["present", "singular", "subjunctive", "third-person"]}, {"form": "hable", "tags": ["present", "singular", "subjunctive", "third-person"]}, {"form": "hable", "tags": ["formal", "imperative", "second-person-semantically", "singular", "third-person"]}, {"form": "hable", "tags": ["imperative", "negative", "singular", "third-person"]}, {"form": "háblame", "tags": ["combined-form", "imperative", "informal", "second-person", "singular"]}, {"form": "habla usted", "tags": ["imperative", "formal", "singular"]}, {"form": "hablado", "tags": ["participle", "past"]}, {"form": "hablás", "tags": ["indicative", "informal", "present", "vos-form", "second-person", "singular"]}, {"form": "hablasteis", "tags": ["second-person", "indicative", "plural"]}, {"form": "es-conj", "tags": ["inflection-template"]}]}"#,
        "\n",
        r#"{"word": "ser", "pos": "verb", "lang_code": "es", "senses": [{"glosses": ["to be"]}], "forms": [{"form": "fue", "tags": ["indicative", "preterite", "singular", "third-person"]}]}"#,
        "\n",
        r#"{"word": "ir", "pos": "verb", "lang_code": "es", "senses": [{"glosses": ["to go"]}], "forms": [{"form": "fue", "tags": ["third-person", "singular", "indicative", "present", "perfect"]}]}"#,
        "\n",
        r#"{"word": "casa", "pos": "noun", "lang_code": "es", "senses": [{"glosses": ["house"]}], "forms": [{"form": "casas", "tags": ["plural", "indicative", "present", "third-person", "singular"]}]}"#,
        "\n",
    );

    fn parsed() -> Contents {
        parse(FIXTURE.as_bytes(), "es", "fixture-es").expect("parse")
    }

    fn senses_of<'a>(contents: &'a Contents, word: &str) -> Vec<&'a SenseRecord> {
        let headword = contents
            .headwords
            .iter()
            .find(|headword| headword.word == word)
            .unwrap_or_else(|| panic!("{word}"));
        headword
            .senses
            .iter()
            .map(|at| &contents.senses[*at as usize])
            .collect()
    }

    #[test]
    fn the_entries_of_one_word_are_one_headword_in_file_order() {
        let contents = parsed();
        assert_eq!(contents.source, "fixture-es");
        let words: Vec<_> = contents.headwords.iter().map(|h| h.word.as_str()).collect();
        // No `bank` of the other language, no glossless `arromanzar`.
        assert_eq!(words, ["banco", "bancos", "cerrar"]);
        let banco = senses_of(&contents, "banco");
        let definitions: Vec<_> = banco.iter().map(|s| s.definition.as_str()).collect();
        assert_eq!(
            definitions,
            [
                "Empresa que ofrece servicios monetarios.",
                "Mueble alargado para sentarse.",
                "Mueble sin respaldo.",
                "Expresa aceptación.",
            ]
        );
        assert!(contents.headwords.iter().all(|h| h.translations.is_empty()));
    }

    #[test]
    fn the_synonyms_are_the_members_of_the_sense_they_name() {
        let contents = parsed();
        let banco = senses_of(&contents, "banco");
        // The word itself, one without an index, one of no sense and a
        // repeated one are dropped.
        assert!(banco[0].members.is_empty());
        assert_eq!(banco[1].members, texts(&["banca"]));
        assert_eq!(banco[2].members, texts(&["banquillo"]));
        assert!(banco[3].members.is_empty());
    }

    #[test]
    fn a_form_of_sense_is_a_sense_and_the_glosses_are_joined() {
        let contents = parsed();
        assert_eq!(
            senses_of(&contents, "bancos")[0].definition,
            "Plural de banco."
        );
        let cerrar = senses_of(&contents, "cerrar");
        assert_eq!(cerrar.len(), 1);
        assert_eq!(
            cerrar[0].definition,
            "Asegurar con cerradura. Impedir el paso."
        );
    }

    #[test]
    fn the_forms_go_into_the_form_table() {
        let contents = parsed();
        let forms: Vec<_> = contents
            .forms
            .iter()
            .map(|form| (form.form.as_str(), form.lemmas.clone()))
            .collect();
        // `bancos` from both the list and its form-of sense, once; no
        // template and no form of two words.
        assert_eq!(
            forms,
            [
                ("bancos", texts(&["banco"])),
                ("cerró", texts(&["cerrar"])),
                ("cierro", texts(&["cerrar"])),
            ]
        );
    }

    #[test]
    fn the_verb_tables_give_the_analyses_of_their_forms() {
        use crate::analysis::{Number, Person, TenseMood};
        let finite = |tense_mood, person| Shape::Finite {
            tense_mood,
            number: Number::Singular,
            person,
        };
        let contents = parse(VERBS.as_bytes(), "es", "verbs").expect("parse");
        let analysed: Vec<_> = contents
            .analyses
            .iter()
            .map(|analysed| (analysed.form.as_str(), analysed.lemmas.clone()))
            .collect();
        let of = |lemma: &str, shapes: Vec<Shape>| Analyses {
            lemma: lemma.to_owned(),
            shapes,
        };
        let preterite = finite(TenseMood::PreteriteIndicative, Person::Third);
        // No `hablar`, `háblame`, `habla usted`, `hablasteis` (no tense) or
        // `casas`; `hablo` once.
        assert_eq!(
            analysed,
            [
                (
                    "fue",
                    vec![of("ir", vec![preterite]), of("ser", vec![preterite])]
                ),
                ("hablado", vec![of("hablar", vec![Shape::PastParticiple])]),
                (
                    "hable",
                    vec![of(
                        "hablar",
                        vec![
                            finite(TenseMood::PresentSubjunctive, Person::First),
                            finite(TenseMood::PresentSubjunctive, Person::Third),
                            finite(TenseMood::Imperative, Person::Formal),
                        ]
                    )]
                ),
                (
                    "hablo",
                    vec![of(
                        "hablar",
                        vec![finite(TenseMood::PresentIndicative, Person::First)]
                    )]
                ),
                (
                    "hablás",
                    vec![of(
                        "hablar",
                        vec![finite(TenseMood::PresentIndicative, Person::Vos)]
                    )]
                ),
            ]
        );
    }

    #[test]
    fn the_analyses_leave_the_headwords_and_the_form_table_alone() {
        let contents = parsed();
        assert!(contents.analyses.is_empty());
        let verbs = parse(VERBS.as_bytes(), "es", "verbs").expect("parse");
        let words: Vec<_> = verbs.headwords.iter().map(|h| h.word.as_str()).collect();
        assert_eq!(words, ["casa", "hablar", "ir", "ser"]);
        let forms: Vec<_> = verbs.forms.iter().map(|form| form.form.as_str()).collect();
        assert_eq!(
            forms,
            [
                "casas",
                "fue",
                "hablado",
                "hablasteis",
                "hable",
                "hablo",
                "hablás",
                "háblame"
            ]
        );
    }

    #[test]
    fn the_contents_make_an_index_with_the_same_bytes_each_time() {
        let first = format::encode(&parsed()).expect("encode");
        assert_eq!(format::encode(&parsed()).expect("encode"), first);
    }

    #[test]
    fn a_line_that_is_no_entry_and_a_file_without_the_language_are_errors() {
        let broken = format!("{FIXTURE}{{\"word\": \n");
        let error = parse(broken.as_bytes(), "es", "x").expect_err("broken");
        assert!(
            matches!(error, ParseError::Line { number: 8, .. }),
            "{error}"
        );
        assert!(matches!(
            parse(FIXTURE.as_bytes(), "ca", "x"),
            Err(ParseError::NoEntry)
        ));
        assert!(matches!(
            parse(&b""[..], "es", "x"),
            Err(ParseError::NoEntry)
        ));
    }

    #[test]
    fn a_word_is_letters_with_an_inner_apostrophe_or_middle_dot() {
        for word in ["banco", "cerró", "col·lecció", "l'endemà", "Ñu"] {
            assert!(is_word(word), "{word}");
        }
        for text in ["", "es-sust", "cierre usted", "'hood", "col·", "·a", "a1"] {
            assert!(!is_word(text), "{text}");
        }
    }
}
