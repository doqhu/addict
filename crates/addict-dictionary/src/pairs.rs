//! The translation sources of a word read in one language, for someone
//! whose own language is another: one row per ordered pair (reading
//! language, my language) of distinct languages, in [`PAIRS`].
//!
//! Each row holds the sources that translate from its reading language
//! into my language. A pair of a language with itself has no row.
//!
//! Article sources (definitions and synonyms: `oewn`, `kaikki-es-Espanol`,
//! `omw-es`, `dbnary-ca`, `sinonims-cat`) are not here: they describe a word
//! in its own language and stay keyed by the reading language alone.
//!
//! Row (Russian, X) does double duty today: with Russian chosen as my
//! language, a Cyrillic word is translated into the pressed language X from
//! `wikdict-ru-X`, the same source a Russian word read by someone whose
//! language is X would use.

use addict_core::translation::Source as Language;

use crate::import::Source;

/// The translation sources of one ordered pair of distinct languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    /// The language the words are read in.
    pub reading: Language,
    /// The language they are translated into.
    pub my: Language,
    /// The sources that translate them.
    pub sources: &'static [Source],
}

/// Every ordered pair of distinct languages, in the order of
/// [`Language::ALL`] for the reading language, then for my language.
pub const PAIRS: [Pair; 12] = [
    Pair {
        reading: Language::En,
        my: Language::Es,
        sources: &[Source::WikdictEnEs],
    },
    Pair {
        reading: Language::En,
        my: Language::Ca,
        sources: &[Source::WikdictEnCa],
    },
    Pair {
        reading: Language::En,
        my: Language::Ru,
        sources: &[Source::Freedict],
    },
    Pair {
        reading: Language::Es,
        my: Language::En,
        sources: &[Source::KaikkiEnSpanish, Source::WikdictEsEn],
    },
    Pair {
        reading: Language::Es,
        my: Language::Ca,
        sources: &[Source::WikdictEsCa],
    },
    Pair {
        reading: Language::Es,
        my: Language::Ru,
        sources: &[Source::WikdictEsRu],
    },
    Pair {
        reading: Language::Ca,
        my: Language::En,
        sources: &[Source::KaikkiEnCatalan, Source::WikdictCaEn],
    },
    Pair {
        reading: Language::Ca,
        my: Language::Es,
        sources: &[Source::WikdictCaEs],
    },
    Pair {
        reading: Language::Ca,
        my: Language::Ru,
        sources: &[Source::WikdictCaRu],
    },
    Pair {
        reading: Language::Ru,
        my: Language::En,
        sources: &[Source::WikdictRuEn],
    },
    Pair {
        reading: Language::Ru,
        my: Language::Es,
        sources: &[Source::WikdictRuEs],
    },
    Pair {
        reading: Language::Ru,
        my: Language::Ca,
        sources: &[Source::WikdictRuCa],
    },
];

/// The sources that translate a word read in `reading` into `my`; none for
/// a language paired with itself.
pub fn translations(reading: Language, my: Language) -> &'static [Source] {
    PAIRS
        .iter()
        .find(|pair| pair.reading == reading && pair.my == my)
        .map_or(&[], |pair| pair.sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::Kind;

    /// Whether the source translates its headwords rather than describing
    /// them in their own language. No wildcard arm.
    fn translates(source: Source) -> bool {
        match source.kind() {
            Kind::Translation(_) | Kind::English | Kind::FromRussian => true,
            Kind::Definitions | Kind::Synonyms => false,
        }
    }

    #[test]
    fn russian_target() {
        assert_eq!(
            translations(Language::Es, Language::Ru),
            [Source::WikdictEsRu]
        );
        assert_eq!(
            translations(Language::Ca, Language::Ru),
            [Source::WikdictCaRu]
        );
        assert_eq!(translations(Language::En, Language::Ru), [Source::Freedict]);
    }

    #[test]
    fn russian_reading() {
        assert_eq!(
            translations(Language::Ru, Language::En),
            [Source::WikdictRuEn]
        );
        assert_eq!(
            translations(Language::Ru, Language::Es),
            [Source::WikdictRuEs]
        );
        assert_eq!(
            translations(Language::Ru, Language::Ca),
            [Source::WikdictRuCa]
        );
    }

    #[test]
    fn english_glosses() {
        assert_eq!(
            translations(Language::Es, Language::En),
            [Source::KaikkiEnSpanish, Source::WikdictEsEn]
        );
        assert_eq!(
            translations(Language::Ca, Language::En),
            [Source::KaikkiEnCatalan, Source::WikdictCaEn]
        );
    }

    #[test]
    fn spanish_catalan() {
        assert_eq!(
            translations(Language::Es, Language::Ca),
            [Source::WikdictEsCa]
        );
    }

    #[test]
    fn same_language_has_none() {
        for language in Language::ALL {
            assert!(translations(language, language).is_empty(), "{language:?}");
        }
    }

    /// Every ordered pair of distinct languages has one row with at least
    /// one translation source, in the order of `Language::ALL`.
    #[test]
    fn every_pair_has_one_row() {
        let expected: Vec<(Language, Language)> = Language::ALL
            .into_iter()
            .flat_map(|reading| Language::ALL.into_iter().map(move |my| (reading, my)))
            .filter(|(reading, my)| reading != my)
            .collect();
        let rows: Vec<(Language, Language)> =
            PAIRS.iter().map(|pair| (pair.reading, pair.my)).collect();
        assert_eq!(rows, expected);
        for pair in PAIRS {
            assert!(
                pair.sources.iter().any(|source| matches!(
                    source.kind(),
                    Kind::Translation(_) | Kind::FromRussian
                )),
                "{pair:?}"
            );
        }
    }

    /// Every translation source is in exactly one row, and no article source
    /// is in any. The sources are those of `KNOWN`, which lists every
    /// importable source.
    #[test]
    fn every_translation_source_in_one_row() {
        let mut sources: Vec<Source> = Vec::new();
        for file in crate::import::KNOWN {
            if !sources.contains(&file.source) {
                sources.push(file.source);
            }
        }
        for source in sources {
            let rows = PAIRS
                .iter()
                .filter(|pair| pair.sources.contains(&source))
                .count();
            let expected = usize::from(translates(source));
            assert_eq!(rows, expected, "{source:?}");
        }
    }

    /// The sources of a row read words in its reading language and give its
    /// language: a source of Russian headwords translates into my language,
    /// any other has headwords of the reading language, and its kind gives
    /// the target, or English, as my language says.
    #[test]
    fn rows_agree_with_the_sources() {
        for pair in PAIRS {
            for &source in pair.sources {
                match source.russian_into() {
                    Some(into) => {
                        assert_eq!(pair.reading, Language::Ru, "{source:?}");
                        assert_eq!(into, pair.my, "{source:?}");
                        assert_eq!(source.language(), None, "{source:?}");
                    }
                    None => {
                        assert_eq!(source.language(), Some(pair.reading), "{source:?}");
                    }
                }
                let my = match source.kind() {
                    Kind::Translation(target) => Some(target),
                    Kind::English => Some(Language::En),
                    Kind::FromRussian => source.russian_into(),
                    Kind::Definitions | Kind::Synonyms => None,
                };
                assert_eq!(my, Some(pair.my), "{source:?}");
            }
        }
    }
}
