//! The imported Russian indexes, `wikdict-ru-en`, `wikdict-ru-es` and
//! `wikdict-ru-ca`, against the lookup rows of the plan's matrix, each word
//! looked up as the app does (`lookup::russian` in the index of the chosen
//! language).
//!
//! Ignored by default: it needs the three indexes, which are not in the
//! repository. `ADDICT_INDEX_DIR` is the directory `addict import` wrote
//! them into.

mod common;

use addict_core::translation::Source as Language;
use addict_dictionary::{Index, import::Source, lookup};
use common::path_from;

#[test]
#[ignore = "needs the imported Russian indexes; set ADDICT_INDEX_DIR"]
fn the_matrix_words_get_their_translations_from_the_real_indexes() {
    let dir = path_from("ADDICT_INDEX_DIR");
    let open = |language| {
        let path = dir.join(Source::from_russian(language).index_name());
        Index::open(&path).unwrap_or_else(|error| panic!("{error}"))
    };
    let [english, spanish, catalan] = Language::ALL.map(open);
    let found = |index: &Index, word: &str| lookup::russian(index, word).expect("lookup");
    let texts = |words: &[&str]| -> Option<Vec<String>> {
        Some(words.iter().map(|word| (*word).to_owned()).collect())
    };
    assert_eq!(
        found(&spanish, "собака"),
        texts(&["perro", "cachorro", "arroba"])
    );
    assert_eq!(found(&english, "Москва"), texts(&["Moscow", "Moskva"]));
    assert_eq!(found(&catalan, "еж"), texts(&["eriçó"]));
    assert_eq!(
        found(&spanish, "«Дом»,"),
        texts(&["casa", "hogar", "domicilio"])
    );
    assert_eq!(found(&spanish, "собаки"), None);
    // STRESS: `книга` is written `кни́га` in ru-en.
    let book = found(&english, "книга").expect("книга");
    assert!(
        book.iter().any(|translation| translation == "book"),
        "{book:?}"
    );
    assert_eq!(found(&english, "кни\u{0301}га"), Some(book));
    // `адрес` is only `а́дрес` in ru-en, and found from the typed word.
    assert_eq!(found(&english, "адрес"), texts(&["address"]));
    assert_eq!(found(&english, "Адрес,"), texts(&["address"]));
}
