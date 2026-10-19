//! The imported `kaikki-es-Espanol` index: `banco` as the tracer shows it,
//! the ES target list of EXP-9 looked up through its forms, and the memory of a
//! process that holds the index open.
//!
//! Ignored by default: it needs the index, which is not in the repository.
//! `ADDICT_KAIKKI_ES_INDEX` is the index file written by `addict import`;
//! `ADDICT_TARGET_WORDS_ES`, when set, is the list, one word per line, `#`
//! for comments.

mod common;

use std::collections::BTreeSet;

use addict_core::translation::Source as Language;
use addict_dictionary::{Index, lookup};
use common::{path_from, pss_kb};

#[test]
#[ignore = "needs the imported index: set ADDICT_KAIKKI_ES_INDEX"]
fn banco_and_the_target_list_are_read_from_the_spanish_index() {
    let before_open = pss_kb();
    let path = path_from("ADDICT_KAIKKI_ES_INDEX");
    let index = Index::open(&path).expect("open the index");
    let after_open = pss_kb();

    let banco = index.find("banco").expect("lookup").expect("banco");
    assert!(banco.senses.len() >= 6, "{banco:?}");
    assert!(banco.translations.is_empty());
    let synonyms: Vec<_> = banco.senses.iter().map(|s| s.synonyms.clone()).collect();
    assert_eq!(synonyms[1], ["banca"], "{synonyms:?}");
    assert_eq!(synonyms[2], ["banquillo"], "{synonyms:?}");
    assert_eq!(synonyms[3], ["cardumen"], "{synonyms:?}");
    // The article also holds the senses of the rule forms the index has
    // (such as `bancar`), after those of `banco`.
    let article = lookup::article(&index, Language::Es, "Banco")
        .expect("lookup")
        .expect("Banco");
    assert_eq!(article.senses[..banco.senses.len()], banco.senses[..]);
    assert!(article.translations.is_empty());
    // `zzz` is a headword here (an onomatopoeia): the miss is another word.
    assert_eq!(index.find("qzxv").expect("lookup"), None);
    assert!(
        index
            .exception_lemmas("bancos")
            .expect("form")
            .contains(&"banco".to_owned())
    );

    if let Some(list) = std::env::var_os("ADDICT_TARGET_WORDS_ES") {
        let list = std::fs::read_to_string(list).expect("target list");
        let words: BTreeSet<&str> = list
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();
        let found = words
            .iter()
            .filter(|word| {
                lookup::article(&index, Language::Es, word)
                    .expect("lookup")
                    .is_some()
            })
            .count();
        println!("target list: {} words, {found} found", words.len());
    }
    let after_lookups = pss_kb();

    let size = std::fs::metadata(&path).expect("metadata").len();
    println!("index: {}", path.display());
    println!("source: {}", index.source());
    println!("index size: {size} bytes");
    println!("headwords: {}", index.headwords());
    println!("exception forms: {}", index.exception_forms());
    println!(
        "PSS kB: before open {before_open}, after open {after_open}, after lookups {after_lookups}"
    );
}
