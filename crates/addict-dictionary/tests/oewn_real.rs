//! The imported Open English WordNet index against the target word list of
//! EXP-7, and the memory of a process that holds the index open.
//!
//! Ignored by default: it needs the index, which is not in the repository.
//! `ADDICT_INDEX` is the index file written by `addict import` and
//! `ADDICT_TARGET_WORDS` is the list, one word per line, `#` for comments.

mod common;

use std::collections::BTreeSet;

use addict_dictionary::Index;
use common::{path_from, pss_kb};

#[test]
#[ignore = "needs the imported index: set ADDICT_INDEX and ADDICT_TARGET_WORDS"]
fn the_index_covers_the_target_list_as_exp7_measured() {
    let before_open = pss_kb();
    let path = path_from("ADDICT_INDEX");
    let index = Index::open(&path).expect("open the index");
    let after_open = pss_kb();

    let list = std::fs::read_to_string(path_from("ADDICT_TARGET_WORDS")).expect("target list");
    let words: BTreeSet<&str> = list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert_eq!(words.len(), 504, "the target list");

    let (mut found, mut definitions, mut synonyms) = (0, 0, 0);
    for word in &words {
        let Some(article) = index.find(word).expect("lookup") else {
            continue;
        };
        found += 1;
        assert!(article.translations.is_empty(), "{word}");
        if article.senses.iter().any(|s| !s.definition.is_empty()) {
            definitions += 1;
        }
        if article.senses.iter().any(|s| !s.synonyms.is_empty()) {
            synonyms += 1;
        }
    }
    let after_lookups = pss_kb();

    // The same lookups again: the memory must not follow the use.
    for _ in 0..20 {
        for word in &words {
            let _ = index.find(word).expect("lookup");
        }
    }
    let after_repeats = pss_kb();

    let size = std::fs::metadata(&path).expect("metadata").len();
    println!("index: {}", path.display());
    println!("source: {}", index.source());
    println!("index size: {size} bytes");
    println!("headwords: {}", index.headwords());
    println!("exception forms: {}", index.exception_forms());
    println!("found {found}, with definitions {definitions}, with synonyms {synonyms}");
    println!("Pss before open: {before_open} kB");
    println!("Pss with the open index: {after_open} kB");
    println!("Pss after {} lookups: {after_lookups} kB", words.len());
    println!("Pss after {} lookups: {after_repeats} kB", words.len() * 21);

    assert_eq!((found, definitions, synonyms), (369, 369, 337));
    assert_eq!(index.headwords(), 128_009);
    assert_eq!(index.exception_forms(), 4_396);
    for absent in ["boolean", "Bank", "banks", ""] {
        assert_eq!(index.find(absent).expect("lookup"), None, "{absent:?}");
    }
    let bank = index.find("bank").expect("lookup").expect("bank");
    assert!(bank.senses.len() > 10);
    let advice = index.find("advice").expect("lookup").expect("advice");
    assert!(advice.senses.iter().all(|s| s.synonyms.is_empty()));
    assert!(advice.senses.iter().all(|s| !s.definition.is_empty()));
    assert_eq!(index.exception_lemmas("geese").expect("form"), ["goose"]);
}
