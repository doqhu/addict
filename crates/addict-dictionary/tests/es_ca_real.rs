//! The imported Spanish and Catalan indexes against the EXP-9 target lists,
//! each word looked up through its dictionary forms as the app does
//! (`lookup::grouped` over the open indexes of the language in
//! `import::KNOWN` order).
//!
//! Ignored by default: it needs the eight indexes, which are not in the
//! repository. `ADDICT_INDEX_DIR` is the directory `addict import` wrote
//! them into; `ADDICT_TARGET_WORDS_ES` and `ADDICT_TARGET_WORDS_CA` are the
//! lists, one word per line, `#` for comments, a `# [section]` line starting
//! a section.
//!
//! The analyses of verb forms are checked against the indexes alone: the
//! three Kaikki files imported with their tables.

mod common;

use addict_core::translation::Source as Language;
use addict_dictionary::{
    Index,
    analysis::{Number, Person, Shape, TenseMood},
    import::{KNOWN, Kind},
    lookup,
};
use common::path_from;

/// The words of a target list with their sections, in file order.
fn target_words(text: &str) -> Vec<(String, String)> {
    let mut section = String::new();
    let mut words = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix("# [")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.to_owned();
        } else if !line.is_empty() && !line.starts_with('#') {
            words.push((section.clone(), line.to_owned()));
        }
    }
    words
}

/// The headwords of `indexes` that `word` reaches, as `grouped` asks them:
/// the exception lemmas of all the indexes together.
fn headwords_found(indexes: &[(Kind, Index)], language: Language, word: &str) -> Vec<String> {
    let lemmas = lookup::lemmas_of(indexes.iter().map(|(_, index)| index), language, word)
        .expect("form table");
    let mut found = Vec::new();
    for headword in lookup::headwords(language, word, &lemmas) {
        let any = indexes
            .iter()
            .any(|(_, index)| index.find(&headword).expect("lookup").is_some());
        if any {
            found.push(headword);
        }
    }
    found
}

/// The counts of one list: words found, and words with each kind.
#[derive(Debug, Default)]
struct Counts {
    words: usize,
    found: usize,
    russian: usize,
    definitions: usize,
    synonyms: usize,
    english: usize,
}

/// The four indexes of `language` in `ADDICT_INDEX_DIR`, in `KNOWN` order.
fn open_indexes(language: Language) -> Vec<(Kind, Index)> {
    let dir = path_from("ADDICT_INDEX_DIR");
    let indexes: Vec<(Kind, Index)> = KNOWN
        .iter()
        .map(|known| known.source)
        .filter(|source| source.language() == Some(language))
        .map(|source| {
            let path = dir.join(source.index_name());
            let index = Index::open(&path).unwrap_or_else(|error| panic!("{error}"));
            (source.kind(), index)
        })
        .collect();
    assert_eq!(indexes.len(), 4, "four indexes of {language:?}");
    indexes
}

fn run(
    language: Language,
    list_variable: &str,
    form_sections: &[&str],
    expected: [usize; 4],
    checks: &[(&str, &str)],
) {
    let indexes = open_indexes(language);
    let asked: Vec<(Kind, &Index)> = indexes.iter().map(|(kind, index)| (*kind, index)).collect();

    let list = std::fs::read_to_string(path_from(list_variable)).expect("target list");
    let words = target_words(&list);
    let mut counts = Counts::default();
    let mut missed_forms = Vec::new();
    for (section, word) in &words {
        counts.words += 1;
        let Some(grouped) = lookup::grouped(&asked, language, word).expect("lookup") else {
            if form_sections.contains(&section.as_str()) {
                missed_forms.push(format!("{section}: {word}"));
            }
            continue;
        };
        counts.found += 1;
        let has = |list: &Option<Vec<String>>| list.as_ref().is_some_and(|list| !list.is_empty());
        let senses = grouped.definitions.clone().unwrap_or_default();
        let own = senses.iter().any(|sense| !sense.synonyms.is_empty());
        let kinds = [
            ("RU", has(&grouped.russian), &mut counts.russian),
            ("Def", !senses.is_empty(), &mut counts.definitions),
            (
                "Syn",
                grouped
                    .synonyms
                    .as_ref()
                    .is_some_and(|groups| !groups.is_empty())
                    || own,
                &mut counts.synonyms,
            ),
            ("EN", has(&grouped.english), &mut counts.english),
        ];
        for (name, present, count) in kinds {
            if present {
                *count += 1;
            } else {
                println!("{language:?} missing {name}: {word}");
            }
        }
    }
    println!(
        "{language:?}: {} words, {} found; RU {}, Def {}, Syn {}, EN {}",
        counts.words,
        counts.found,
        counts.russian,
        counts.definitions,
        counts.synonyms,
        counts.english
    );
    assert!(missed_forms.is_empty(), "not found: {missed_forms:?}");
    let [russian, definitions, synonyms, english] = expected;
    assert!(counts.russian >= russian, "{counts:?}");
    assert!(counts.definitions >= definitions, "{counts:?}");
    assert!(counts.synonyms >= synonyms, "{counts:?}");
    assert!(counts.english >= english, "{counts:?}");

    for (word, lemma) in checks {
        let found = headwords_found(&indexes, language, word);
        println!("{word}: {found:?}");
        assert!(
            found.iter().any(|headword| headword == lemma),
            "{word} -> {lemma}: {found:?}"
        );
    }
}

#[test]
#[ignore = "needs the imported indexes: set ADDICT_INDEX_DIR and ADDICT_TARGET_WORDS_ES"]
fn the_spanish_indexes_cover_the_target_list_as_exp9_measured() {
    run(
        Language::Es,
        "ADDICT_TARGET_WORDS_ES",
        &["forms", "irregular"],
        // EXP-9's 500, 508, 504, 508, less what EXP-9 counted from contents
        // the importers do not keep: RU of `pingüino` (Kaikki ES
        // translations), Syn of `según` and `despacio` (Kaikki EN synonyms).
        [499, 508, 502, 508],
        &[("cerró", "cerrar"), ("dámelo", "dar")],
    );
}

#[test]
#[ignore = "needs the imported indexes: set ADDICT_INDEX_DIR and ADDICT_TARGET_WORDS_CA"]
fn the_catalan_indexes_cover_the_target_list_as_exp9_measured() {
    run(
        Language::Ca,
        "ADDICT_TARGET_WORDS_CA",
        &["forms", "irregular", "elision", "middle-dot"],
        // EXP-9's 496, 506, 503, 506, less what EXP-9 counted from contents
        // the importers do not keep: Syn of `i` and `al` (DBnary synonyms).
        [496, 506, 501, 506],
        &[("col·leccions", "col·lecció"), ("d'art", "art")],
    );
}

/// What the indexes of `language` say of `word` as a verb form: each lemma
/// with its shapes, and the word the periphrastic note names.
fn verb_form(
    indexes: &[(Kind, Index)],
    language: Language,
    word: &str,
) -> (Vec<(String, Vec<Shape>)>, Option<String>) {
    let asked: Vec<(Kind, &Index)> = indexes.iter().map(|(kind, index)| (*kind, index)).collect();
    let grouped = lookup::grouped(&asked, language, word)
        .expect("lookup")
        .unwrap_or_else(|| panic!("{word} is no headword"));
    let Some(verb) = grouped.verb else {
        return (Vec::new(), None);
    };
    let analyses = verb
        .analyses
        .into_iter()
        .map(|analyses| (analyses.lemma, analyses.shapes))
        .collect();
    println!("{word}: {analyses:?} {:?}", verb.periphrastic);
    (analyses, verb.periphrastic)
}

fn sg(tense_mood: TenseMood, person: Person) -> Shape {
    Shape::Finite {
        tense_mood,
        number: Number::Singular,
        person,
    }
}

#[test]
#[ignore = "needs the imported indexes: set ADDICT_INDEX_DIR"]
fn the_verb_forms_of_the_story_have_their_analyses() {
    use Person::{First, Formal, Second, Third};
    use TenseMood::*;
    let spanish = open_indexes(Language::Es);
    let lemma = |analyses: &[(String, Vec<Shape>)], name: &str| -> Vec<Shape> {
        analyses
            .iter()
            .find(|(lemma, _)| lemma == name)
            .unwrap_or_else(|| panic!("{name}: {analyses:?}"))
            .1
            .clone()
    };

    let (hable, _) = verb_form(&spanish, Language::Es, "hable");
    assert_eq!(hable.len(), 1, "{hable:?}");
    assert_eq!(
        lemma(&hable, "hablar"),
        [
            sg(PresentSubjunctive, First),
            sg(PresentSubjunctive, Third),
            sg(Imperative, Formal),
            sg(Imperative, Third),
        ]
    );

    let (fue, _) = verb_form(&spanish, Language::Es, "fue");
    let preterite = vec![sg(PreteriteIndicative, Third)];
    assert_eq!(
        fue,
        [
            ("ir".to_owned(), preterite.clone()),
            ("ser".to_owned(), preterite)
        ]
    );

    let (dicho, _) = verb_form(&spanish, Language::Es, "dicho");
    assert_eq!(
        dicho,
        [
            ("decir".to_owned(), vec![Shape::PastParticiple]),
            ("dezir".to_owned(), vec![Shape::PastParticiple]),
        ]
    );

    // Nouns: none of their own, and `filete` is no `file` + `te`, which
    // would carry an accent. (`tomate` is the vos imperative of `tomarse`.)
    for word in ["hablar", "ciudades", "árboles", "filete"] {
        assert_eq!(
            verb_form(&spanish, Language::Es, word),
            (Vec::new(), None),
            "{word}"
        );
    }

    let catalan = open_indexes(Language::Ca);
    let (va, note) = verb_form(&catalan, Language::Ca, "va");
    assert_eq!(
        va,
        [("anar".to_owned(), vec![sg(PresentIndicative, Third)])]
    );
    assert_eq!(note.as_deref(), Some("va"));
    let (parli, note) = verb_form(&catalan, Language::Ca, "parli");
    assert_eq!(
        parli,
        [(
            "parlar".to_owned(),
            vec![
                sg(PresentSubjunctive, First),
                sg(PresentSubjunctive, Third),
                sg(Imperative, Formal),
            ]
        )]
    );
    assert_eq!(note, None);
    assert_eq!(
        verb_form(&catalan, Language::Ca, "anar"),
        (Vec::new(), None)
    );

    let (dale, _) = verb_form(&spanish, Language::Es, "dale");
    // `da` is the imperative of tú and of vos.
    assert_eq!(
        dale,
        [(
            "dar".to_owned(),
            vec![sg(Imperative, Second), sg(Imperative, Person::Vos)]
        )]
    );
    let (dímelo, _) = verb_form(&spanish, Language::Es, "dímelo");
    assert_eq!(dímelo, [("decir".to_owned(), vec![sg(Imperative, Second)])]);
}

#[test]
#[ignore = "needs reimported Catalan thesaurus and existing indexes: set ADDICT_INDEX_DIR and ADDICT_CATALAN_THESAURUS"]
fn catalan_feu_keeps_fourteen_verb_meanings_and_direct_fer_keeps_fifteen() {
    use addict_dictionary::{format::GrammaticalClass, mythes};
    let source = std::fs::File::open(path_from("ADDICT_CATALAN_THESAURUS"))
        .expect("approved local thesaurus");
    let source =
        mythes::parse(std::io::BufReader::new(source), "source-check").expect("parse source");
    assert!(
        source
            .headwords
            .iter()
            .all(|headword| headword.word != "feu")
    );
    let fer = source
        .headwords
        .iter()
        .find(|headword| headword.word == "fer")
        .expect("fer");
    assert_eq!(fer.senses.len(), 15);
    let indexes = open_indexes(Language::Ca);
    let asked: Vec<_> = indexes.iter().map(|(kind, index)| (*kind, index)).collect();
    let get = |word| {
        lookup::grouped(&asked, Language::Ca, word)
            .expect("lookup")
            .expect("found")
    };
    let feu = get("feu");
    assert!(
        feu.verb
            .as_ref()
            .expect("positive analyses")
            .analyses
            .iter()
            .any(|analysis| analysis.lemma == "fer")
    );
    let all_groups = feu.synonyms.expect("groups");
    // Other headwords retain their accessible independent readings.
    assert!(all_groups.iter().any(|group| group.headword == "far"));
    assert!(all_groups.iter().any(|group| group.headword == "fre"));
    let groups: Vec<_> = all_groups
        .into_iter()
        .filter(|group| group.headword == "fer")
        .collect();
    assert_eq!(groups.len(), 14, "fer groups: {groups:?}");
    for group in &groups {
        assert_eq!(group.headword, "fer");
        assert_eq!(
            group.metadata.as_ref().expect("metadata").class(),
            Some(GrammaticalClass::Verb)
        );
        assert!(
            group
                .synonyms
                .iter()
                .all(|word| !["ferotge", "feroç", "salvatge"].contains(&word.as_str()))
        );
    }
    let direct_all = get("fer").synonyms.expect("direct groups");
    let direct: Vec<_> = direct_all
        .iter()
        .filter(|group| group.headword == "fer")
        .cloned()
        .collect();
    println!(
        "feu: {} verb groups; direct fer: {} source meanings ({} groups including existing heuristic headwords)",
        groups.len(),
        direct.len(),
        direct_all.len()
    );
    assert_eq!(direct.len(), 15);
    assert_eq!(
        direct[0].metadata.as_ref().expect("adjective").class(),
        Some(GrammaticalClass::Adjective)
    );
    for (group, position) in direct.iter().zip(&fer.senses) {
        let source = &source.senses[*position as usize];
        assert_eq!(group.metadata, source.metadata);
        assert_eq!(group.synonyms, source.members);
    }
    assert_eq!(groups, direct[1..]);
    let thesaurus = asked
        .iter()
        .find(|(kind, _)| *kind == Kind::Synonyms)
        .expect("thesaurus");
    assert_eq!(
        lookup::grouped(&[*thesaurus], Language::Ca, "feu")
            .expect("alone")
            .expect("found by rules")
            .synonyms
            .expect("unfiltered")
            .iter()
            .filter(|group| group.headword == "fer")
            .count(),
        15
    );
}

#[test]
#[ignore = "needs reimported Catalan thesaurus and existing indexes: set ADDICT_INDEX_DIR"]
fn catalan_causes_keeps_causa_nouns_alongside_causar_verbs() {
    use addict_dictionary::format::GrammaticalClass;
    let indexes = open_indexes(Language::Ca);
    let asked: Vec<_> = indexes.iter().map(|(kind, index)| (*kind, index)).collect();
    let causes = lookup::grouped(&asked, Language::Ca, "causes")
        .expect("lookup")
        .expect("found");
    assert!(
        causes
            .verb
            .as_ref()
            .expect("positive analyses")
            .analyses
            .iter()
            .any(|analysis| analysis.lemma == "causar")
    );
    let groups = causes.synonyms.expect("groups");
    let readings = |headword: &str, class| {
        groups
            .iter()
            .filter(|group| {
                group.headword == headword
                    && group
                        .metadata
                        .as_ref()
                        .is_some_and(|metadata| metadata.class() == Some(class))
            })
            .count()
    };
    assert_eq!(readings("causa", GrammaticalClass::Noun), 3);
    assert_eq!(readings("causar", GrammaticalClass::Verb), 4);
}
