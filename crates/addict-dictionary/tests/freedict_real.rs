//! The imported FreeDict eng-rus index against the target word list of
//! EXP-7, the one article the two imported indexes give together, and the
//! memory of a process that holds both open.
//!
//! Ignored by default: it needs the indexes, which are not in the
//! repository. `ADDICT_INDEX_DIR` is the data directory `addict import`
//! wrote `oewn.index` and `freedict.index` into and `ADDICT_TARGET_WORDS` is
//! the list, one word per line, `#` for comments.

use std::{collections::BTreeSet, path::PathBuf};

use addict_core::dictionary::Dictionary;
use addict_dictionary::{Combined, Index};

fn path_from(variable: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {variable}"))
}

/// The proportional set size of this process, in kB.
fn pss_kb() -> u64 {
    let rollup = std::fs::read_to_string("/proc/self/smaps_rollup").expect("smaps_rollup");
    rollup
        .lines()
        .find_map(|line| line.strip_prefix("Pss:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse().ok())
        .expect("a Pss line")
}

#[test]
#[ignore = "needs both imported indexes: set ADDICT_INDEX_DIR and ADDICT_TARGET_WORDS"]
fn the_two_indexes_give_one_article_for_the_target_list() {
    let dir = path_from("ADDICT_INDEX_DIR");
    let freedict_path = dir.join("freedict.index");
    let oewn_path = dir.join("oewn.index");

    let before_open = pss_kb();
    let combined = Combined::new(
        Index::open(&oewn_path).expect("open oewn.index"),
        Index::open(&freedict_path).expect("open freedict.index"),
    );
    let after_open = pss_kb();
    let (oewn, freedict) = (combined.definitions(), combined.translations());

    let list = std::fs::read_to_string(path_from("ADDICT_TARGET_WORDS")).expect("target list");
    let words: BTreeSet<&str> = list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert_eq!(words.len(), 504, "the target list");

    // FreeDict alone, as EXP-7 measured it.
    let (mut found, mut definitions, mut russian) = (0, 0, 0);
    for word in &words {
        let Some(article) = freedict.find(word).expect("lookup") else {
            continue;
        };
        found += 1;
        if article.senses.iter().any(|s| !s.definition.is_empty()) {
            definitions += 1;
        }
        if !article.translations.is_empty() {
            russian += 1;
        }
        assert!(
            article.senses.iter().all(|s| s.synonyms.is_empty()),
            "{word}"
        );
    }

    // Both through the contract.
    let dictionary: &dyn Dictionary = &combined;
    let (mut both_found, mut both_definitions, mut both_synonyms, mut both_russian) = (0, 0, 0, 0);
    let mut freedict_senses = 0;
    for word in &words {
        let from_oewn = oewn.find(word).expect("lookup");
        let from_freedict = freedict.find(word).expect("lookup");
        let article = dictionary.lookup(word);
        assert_eq!(
            article.is_some(),
            from_oewn.is_some() || from_freedict.is_some(),
            "{word}"
        );
        let Some(article) = article else {
            continue;
        };
        both_found += 1;
        // Every word FreeDict has is translated; no other word is.
        match &from_freedict {
            Some(translated) => {
                assert!(!article.translations.is_empty(), "{word}");
                assert_eq!(article.translations, translated.translations, "{word}");
            }
            None => assert!(article.translations.is_empty(), "{word}"),
        }
        // The senses fall back: FreeDict's only for a word OEWN lacks.
        match (&from_oewn, &from_freedict) {
            (Some(defined), _) => assert_eq!(article.senses, defined.senses, "{word}"),
            (None, Some(translated)) => {
                freedict_senses += 1;
                assert_eq!(article.senses, translated.senses, "{word}");
            }
            (None, None) => unreachable!("an article of no index"),
        }
        let mut seen = BTreeSet::new();
        assert!(
            article.translations.iter().all(|t| seen.insert(t)),
            "{word}: a repeated translation"
        );
        if article.senses.iter().any(|s| !s.definition.is_empty()) {
            both_definitions += 1;
        }
        if article.senses.iter().any(|s| !s.synonyms.is_empty()) {
            both_synonyms += 1;
        }
        if !article.translations.is_empty() {
            both_russian += 1;
        }
    }
    let after_lookups = pss_kb();

    // The same lookups again: the memory must not follow the use.
    for _ in 0..20 {
        for word in &words {
            let _ = combined.find(word).expect("lookup");
        }
    }
    let after_repeats = pss_kb();

    for (path, index) in [(&freedict_path, freedict), (&oewn_path, oewn)] {
        let size = std::fs::metadata(path).expect("metadata").len();
        println!("index: {}", path.display());
        println!("  source: {}", index.source());
        println!("  size: {size} bytes");
        println!("  headwords: {}", index.headwords());
        println!("  exception forms: {}", index.exception_forms());
    }
    println!("freedict: found {found}, with definitions {definitions}, with Russian {russian}");
    println!(
        "both: found {both_found}, with definitions {both_definitions}, with synonyms \
         {both_synonyms}, with Russian {both_russian}; senses from FreeDict for {freedict_senses}"
    );
    println!("Pss before open: {before_open} kB");
    println!("Pss with both indexes open: {after_open} kB");
    println!("Pss after {} lookups: {after_lookups} kB", words.len());
    println!("Pss after {} lookups: {after_repeats} kB", words.len() * 21);

    assert_eq!(freedict.source(), "freedict-eng-rus-2025.11.23");
    assert_eq!(freedict.headwords(), 57_491);
    assert_eq!(freedict.exception_forms(), 0);
    assert_eq!((found, definitions, russian), (344, 335, 344));
    assert_eq!(oewn.headwords(), 128_009);
    // OEWN's 369 words and 15 only FreeDict has, 14 of them with a definition.
    assert_eq!(
        (both_found, both_definitions, both_synonyms, both_russian),
        (384, 383, 337, 344)
    );
    assert_eq!(freedict_senses, 15);

    for absent in ["boolean", "mutex", ""] {
        assert_eq!(dictionary.lookup(absent), None, "{absent:?}");
    }
    let bank = dictionary.lookup("bank").expect("bank");
    assert_eq!(
        bank.senses,
        oewn.find("bank").expect("lookup").expect("bank").senses
    );
    assert_eq!(bank.translations[..3], ["бе́рег", "брег", "банк"]);
    // OEWN has "advice" and gives no synonyms; FreeDict translates it.
    let advice = dictionary.lookup("advice").expect("advice");
    assert!(advice.senses.iter().all(|s| s.synonyms.is_empty()));
    assert!(!advice.translations.is_empty());
    // OEWN lacks "the": its senses are FreeDict's definitions.
    assert_eq!(oewn.find("the").expect("lookup"), None);
    let the = dictionary.lookup("the").expect("the");
    assert!(the.senses.iter().all(|s| s.synonyms.is_empty()));
    assert!(!the.translations.is_empty());
}
