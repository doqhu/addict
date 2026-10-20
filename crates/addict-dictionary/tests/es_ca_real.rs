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

mod common;

use addict_core::translation::Source as Language;
use addict_dictionary::{
    Index,
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

fn run(
    language: Language,
    list_variable: &str,
    form_sections: &[&str],
    expected: [usize; 4],
    checks: &[(&str, &str)],
) {
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
            ("Syn", has(&grouped.synonyms) || own, &mut counts.synonyms),
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
