//! The imported FreeDict eng-rus index against the target word list of
//! EXP-7, the one article the two imported indexes give together, as
//! written and with the reduction to the dictionary form, the time of the
//! reduced lookups and the memory of a process that holds both open.
//!
//! Ignored by default: it needs the indexes, which are not in the
//! repository. `ADDICT_INDEX_DIR` is the data directory `addict import`
//! wrote `oewn.index` and `freedict.index` into and `ADDICT_TARGET_WORDS` is
//! the list, one word per line, `#` for comments.

mod common;

use std::{collections::BTreeSet, time::Instant};

use addict_core::dictionary::Dictionary;
use addict_dictionary::{Combined, Index};
use common::{path_from, pss_kb};

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
    let oewn = combined.definitions().expect("the definitions index");
    let freedict = combined.translations().expect("the translations index");

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

    // Both, as written.
    let (mut both_found, mut both_definitions, mut both_synonyms, mut both_russian) = (0, 0, 0, 0);
    let mut freedict_senses = 0;
    for word in &words {
        let from_oewn = oewn.find(word).expect("lookup");
        let from_freedict = freedict.find(word).expect("lookup");
        let article = combined.find(word).expect("lookup");
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

    // Both through the contract: the word and its dictionary forms.
    let dictionary: &dyn Dictionary = &combined;
    let (mut reduced_found, mut reduced_definitions) = (0, 0);
    let (mut reduced_synonyms, mut reduced_russian, mut reduced_all) = (0, 0, 0);
    for word in &words {
        let found = combined.find_reduced(word).expect("lookup");
        let article = dictionary.lookup(word);
        assert_eq!(article.is_some(), !found.is_empty(), "{word}");
        // The word as written, when a headword, is the first.
        let exact = combined.find(word).expect("lookup");
        assert_eq!(
            found
                .first()
                .filter(|first| first.headword == **word)
                .map(|first| &first.article),
            exact.as_ref(),
            "{word}"
        );
        let Some(article) = article else {
            continue;
        };
        reduced_found += 1;
        // Nothing of any headword is lost and nothing is added; a sense
        // reached through two headwords is there once.
        for part in &found {
            for sense in &part.article.senses {
                assert!(
                    article
                        .senses
                        .iter()
                        .any(|kept| kept.definition == sense.definition),
                    "{word}"
                );
            }
            for translation in &part.article.translations {
                assert!(article.translations.contains(translation), "{word}");
            }
        }
        for sense in &article.senses {
            assert!(
                found.iter().any(|part| part.article.senses.contains(sense)),
                "{word}"
            );
        }
        let mut seen = BTreeSet::new();
        assert!(
            article.translations.iter().all(|t| seen.insert(t)),
            "{word}: a repeated translation"
        );
        let mut seen = BTreeSet::new();
        assert!(
            article.senses.iter().all(|s| seen.insert(&s.definition)),
            "{word}: a repeated definition"
        );
        let has_definition = article.senses.iter().any(|s| !s.definition.is_empty());
        let has_synonyms = article.senses.iter().any(|s| !s.synonyms.is_empty());
        let has_russian = !article.translations.is_empty();
        reduced_definitions += usize::from(has_definition);
        reduced_synonyms += usize::from(has_synonyms);
        reduced_russian += usize::from(has_russian);
        reduced_all += usize::from(has_definition && has_synonyms && has_russian);
    }
    let after_lookups = pss_kb();

    // The same lookups again: the memory must not follow the use.
    let started = Instant::now();
    for word in &words {
        let _ = dictionary.lookup(word);
    }
    let reduced_time = started.elapsed();
    for _ in 0..19 {
        for word in &words {
            let _ = dictionary.lookup(word);
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
    println!(
        "both, reduced: found {reduced_found}, with definitions {reduced_definitions}, with \
         synonyms {reduced_synonyms}, with Russian {reduced_russian}, with all three {reduced_all}"
    );
    println!(
        "time of {} reduced lookups: {reduced_time:?}, {:?} a lookup",
        words.len(),
        reduced_time / words.len() as u32
    );
    println!("Pss before open: {before_open} kB");
    println!("Pss with both indexes open: {after_open} kB");
    println!(
        "Pss after {} exact and {} reduced lookups: {after_lookups} kB",
        words.len(),
        words.len()
    );
    println!(
        "Pss after {} more reduced lookups: {after_repeats} kB",
        words.len() * 20
    );

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
    // Set A with reduction, as EXP-7 measured it.
    assert_eq!(
        (
            reduced_found,
            reduced_definitions,
            reduced_synonyms,
            reduced_russian
        ),
        (487, 487, 447, 481)
    );
    assert_eq!(reduced_all, 442);

    let headwords = |word: &str| -> Vec<String> {
        let found = combined.find_reduced(word).expect("lookup");
        found.into_iter().map(|part| part.headword).collect()
    };
    for (word, form) in [
        ("banks", "bank"),
        ("running", "run"),
        ("went", "go"),
        ("Bank", "bank"),
    ] {
        assert!(headwords(word).contains(&form.to_owned()), "{word}");
        let article = dictionary.lookup(word).expect(word);
        let of_form = combined.find(form).expect("lookup").expect(form);
        assert!(!of_form.senses.is_empty(), "{form}");
        for sense in &of_form.senses {
            assert!(
                article
                    .senses
                    .iter()
                    .any(|kept| kept.definition == sense.definition),
                "{word}"
            );
        }
        assert!(!article.translations.is_empty(), "{word}");
    }
    // Neither index has these as written; `running` is a headword itself.
    for word in ["banks", "went", "Bank"] {
        assert_eq!(combined.find(word).expect("lookup"), None, "{word}");
    }
    assert_eq!(headwords("running")[0], "running");
    assert_eq!(dictionary.lookup("banks"), dictionary.lookup("bank"));
    assert_eq!(dictionary.lookup("\"went,\""), dictionary.lookup("went"));

    for absent in ["boolean", "mutex", "", "..."] {
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
