//! The imported Russian indexes, `wikdict-ru-en`, `wikdict-ru-es` and
//! `wikdict-ru-ca`, against the lookup rows of the plan's matrix, each word
//! looked up as the app does (`lookup::russian` in the index of the chosen
//! language); and `kaikki-ru-Russkiy` with `wikdict-ru-en`, grouped as the
//! app groups a Russian word for someone whose language is English.
//!
//! Ignored by default: they need the indexes, which are not in the
//! repository. `ADDICT_INDEX_DIR` is the directory `addict import` wrote
//! them into.

mod common;

use addict_core::translation::{Source as Language, reading_languages};
use addict_dictionary::{
    Index,
    import::{Kind, Source},
    lookup,
};
use common::path_from;

#[test]
#[ignore = "needs the imported Russian indexes; set ADDICT_INDEX_DIR"]
fn the_matrix_words_get_their_translations_from_the_real_indexes() {
    let dir = path_from("ADDICT_INDEX_DIR");
    let open = |language| {
        let source = Source::from_russian(language).expect("a Russian source");
        let path = dir.join(source.index_name());
        Index::open(&path).unwrap_or_else(|error| panic!("{error}"))
    };
    let [english, spanish, catalan] = reading_languages(Language::Ru).map(open);
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

#[test]
#[ignore = "needs the imported kaikki-ru-Russkiy and wikdict-ru-en; set ADDICT_INDEX_DIR"]
fn a_russian_word_gets_its_article_and_english_translations_from_the_real_indexes() {
    let dir = path_from("ADDICT_INDEX_DIR");
    let open = |source: Source| {
        let path = dir.join(source.index_name());
        (
            source.kind(),
            Index::open(&path).unwrap_or_else(|error| panic!("{error}")),
        )
    };
    let indexes = [open(Source::KaikkiRu), open(Source::WikdictRuEn)];
    let asked: Vec<(Kind, &Index)> = indexes.iter().map(|(kind, index)| (*kind, index)).collect();
    for word in ["собака", "соба\u{301}ка"] {
        let grouped = lookup::grouped(&asked, Language::Ru, word)
            .expect("lookup")
            .unwrap_or_else(|| panic!("{word}"));
        let senses = grouped.definitions.as_deref().unwrap_or_default();
        assert!(!senses.is_empty(), "{word}: no senses");
        assert!(
            senses
                .iter()
                .all(|sense| sense.definition.chars().any(|c| ('а'..='я').contains(&c))),
            "{word}: {senses:?}"
        );
        assert!(
            senses.iter().any(|sense| !sense.synonyms.is_empty()),
            "{word}: no synonym in {senses:?}"
        );
        let pronunciations = grouped
            .entries
            .iter()
            .flat_map(|entry| &entry.pronunciations);
        assert!(
            pronunciations
                .clone()
                .any(|pronunciation| pronunciation.notation.as_deref() == Some("IPA")),
            "{word}: no IPA"
        );
        assert!(
            pronunciations
                .flat_map(|pronunciation| &pronunciation.audio)
                .any(|audio| audio.url.ends_with(".ogg")),
            "{word}: no ogg URL"
        );
        let english = grouped.translations_into(Language::En).unwrap_or_default();
        assert!(
            english.iter().any(|translation| translation == "dog"),
            "{word}: {english:?}"
        );
    }
}
