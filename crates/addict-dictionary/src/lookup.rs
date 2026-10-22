//! The dictionary-form hook: the headwords to try for a picked word, per
//! reading language, and the lookup of one index through them.
//!
//! [`headwords`] is the one place that says how a word reaches its
//! headwords in each language. Each gives the word without the punctuation
//! around it, then its lowercase, then the dictionary forms of the
//! lowercase word, each once:
//!
//! - English: the forms of `addict_core::reduction`, the lemmas of an
//!   exception list first. [`Combined`](crate::Combined) looks up through
//!   it.
//! - Spanish and Catalan: the forms of `addict_core::reduction_es_ca`, the
//!   port of EXP-9's `rules.py` (elision, enclitic pronouns, suffix rules,
//!   accents, the Catalan bare stem), with the exception lemmas of the word
//!   and of each of its bases.
//!
//! The exception lemmas come from the form tables of the indexes, through a
//! [`Lemmas`] map the caller reads for the forms [`asked_forms`] names. For
//! Spanish and Catalan it is the union of the form tables of every index of
//! the language that is asked, so an index without a form table (WikDict,
//! OMW) still answers for `fue` the `ser` and `ir` another index lists. The
//! tables are used whole, as imported: EXP-9 measured the same coverage
//! with the full tables as with their irregular-only part.
//!
//! [`article`] answers from one index of an ES or CA source with the
//! headwords of its language, and [`grouped`] answers from every index of
//! one language at once, what each gives under its [`Kind`].
//!
//! [`grouped`] also gives the analyses of the word as a verb form, from the
//! analysis tables of the indexes it asks ([`analyses_of`]).
//!
//! A Russian word has a hook of its own, [`russian`], outside
//! [`headwords`]: its spellings without stress marks, as written, in
//! lowercase and with `ё` and `е` taken as one letter
//! ([`russian_spellings`]). It has no dictionary
//! forms: an inflected form finds nothing.

use std::collections::{BTreeMap, BTreeSet};

use addict_core::{
    dictionary::{Article, Sense},
    reduction, reduction_es_ca,
    translation::Source as Language,
};

use crate::{
    analysis::{Analyses, Shape},
    combined::{Found, merged},
    import::Kind,
    index::{Error, Index, SynonymGroup},
    wikdict,
};

/// The exception lemmas of a form, by form: sorted, each once. A form that
/// is not there has none.
pub type Lemmas = BTreeMap<String, Vec<String>>;

/// The headwords to try for `word` read in `language`, in order, each once;
/// empty when nothing of the word is left: the word without the punctuation
/// around it, its lowercase, then the dictionary forms of the lowercase
/// word. `lemmas` holds the exception lemmas of the forms [`asked_forms`]
/// names. No wildcard arm: a new language does not compile until it is
/// given its hook.
pub fn headwords(language: Language, word: &str, lemmas: &Lemmas) -> Vec<String> {
    let written = reduction::stripped(word);
    if written.is_empty() {
        return Vec::new();
    }
    let lowercase = reduction::lowercase(written);
    let of = |form: &str| lemmas.get(form).cloned().unwrap_or_default();
    let forms = match language {
        Language::En => reduction::forms(&lowercase, &of(&lowercase)),
        Language::Es => reduction_es_ca::forms(&lowercase, reduction_es_ca::Language::Es, of),
        Language::Ca => reduction_es_ca::forms(&lowercase, reduction_es_ca::Language::Ca, of),
    };
    let mut candidates = vec![written.to_owned()];
    for candidate in std::iter::once(lowercase).chain(forms) {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

/// The forms whose exception lemmas [`headwords`] reads for `word` read in
/// `language`: the lowercase word, and in Spanish and Catalan its bases as
/// well. Empty when nothing of the word is left. No wildcard arm, as in
/// [`headwords`].
pub fn asked_forms(language: Language, word: &str) -> Vec<String> {
    let written = reduction::stripped(word);
    if written.is_empty() {
        return Vec::new();
    }
    let lowercase = reduction::lowercase(written);
    let bases = match language {
        Language::En => Vec::new(),
        Language::Es => reduction_es_ca::bases(&lowercase, reduction_es_ca::Language::Es),
        Language::Ca => reduction_es_ca::bases(&lowercase, reduction_es_ca::Language::Ca),
    };
    std::iter::once(lowercase).chain(bases).collect()
}

/// The exception lemmas `indexes` give for each of the [`asked_forms`] of
/// `word` read in `language`: their union per form, sorted, each once. An
/// error of an index is the error of the lookup. A form with no lemma is
/// left out, which [`headwords`] reads as no lemma. Public so that a test
/// over real indexes asks them as [`grouped`] does.
pub fn lemmas_of<'a>(
    indexes: impl IntoIterator<Item = &'a Index> + Clone,
    language: Language,
    word: &str,
) -> Result<Lemmas, Error> {
    let mut lemmas = Lemmas::new();
    for form in asked_forms(language, word) {
        let mut union = Vec::new();
        for index in indexes.clone() {
            union.extend(index.exception_lemmas(&form)?);
        }
        union.sort();
        union.dedup();
        if !union.is_empty() {
            lemmas.insert(form, union);
        }
    }
    Ok(lemmas)
}

/// The article of `word` read in `language` from `index` alone: the senses
/// and translations of every headword [`headwords`] gives that the index
/// has, merged in that order, a sense whose definition is already there
/// skipped; the exception lemmas are those of this index's form table.
/// `None` when the index has none of them. An error of the index is the
/// error of the lookup.
pub fn article(index: &Index, language: Language, word: &str) -> Result<Option<Article>, Error> {
    let lemmas = lemmas_of([index], language, word)?;
    Ok(merged(found(index, language, word, &lemmas)?))
}

/// The headwords of `index` that [`headwords`] gives for `word` read in
/// `language` with `lemmas`, with their articles, in that order.
fn found(
    index: &Index,
    language: Language,
    word: &str,
    lemmas: &Lemmas,
) -> Result<Vec<Found>, Error> {
    let mut found = Vec::new();
    for headword in headwords(language, word, lemmas) {
        if let Some(article) = index.find(&headword)? {
            found.push(Found { headword, article });
        }
    }
    Ok(found)
}

/// What the indexes of one language give for a word, by [`Kind`]. Each
/// field is `None` when no index of its kind was asked, and empty when the
/// indexes of its kind have nothing for the word.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grouped {
    /// Metadata from every source, independent of displayed definition precedence.
    pub entries: Vec<addict_core::dictionary::LexicalEntry>,
    /// Source sense grammar and lexical associations, including projected glosses.
    pub sense_metadata: Vec<SourcedSense>,
    /// [`Kind::Definitions`]: the senses, with their own synonyms, merged in
    /// the order of the indexes, a definition already there skipped.
    pub definitions: Option<Vec<Sense>>,
    /// [`Kind::Russian`]: the translations of the word, each once.
    pub russian: Option<Vec<String>>,
    /// [`Kind::Synonyms`]: independent source meanings in source order,
    /// with headword provenance and metadata; duplicates across meanings stay.
    pub synonyms: Option<Vec<SynonymGroup>>,
    /// [`Kind::English`]: the definitions of the senses as English glosses,
    /// each once, none blank.
    pub english: Option<Vec<String>>,
    /// The word as a verb form; `None` when no index has an analysis of
    /// it. Boxed: a result moves through channels and `Result`s, and most
    /// words are no verb form.
    pub verb: Option<Box<VerbForm>>,
}

pub use addict_core::dictionary::SourcedSense;

/// What [`grouped`] says of a word as a verb form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbForm {
    /// Its analyses, one per lemma, the lemmas sorted, none empty
    /// ([`analyses_of`]).
    pub analyses: Vec<Analyses>,
    /// The word as picked, when it is a Catalan form of `anar` that is also
    /// the auxiliary of the periphrastic past ([`PERIPHRASTIC`]).
    pub periphrastic: Option<String>,
}

/// The Catalan forms of `anar` that are also the auxiliary of the
/// periphrastic past, `va` + infinitive.
pub const PERIPHRASTIC: [&str; 6] = ["va", "vaig", "vas", "vam", "vau", "van"];

/// The analyses `indexes` give for `word` read in `language` as a verb
/// form, their union per lemma, the lemmas sorted, the shapes sorted and
/// each once: those of the word as written and in lowercase. When there are
/// none and the language is Spanish, those of the first of its
/// [`enclitic_stems`](reduction_es_ca::enclitic_stems), the most pronouns
/// removed first, that has any kept to the shapes enclitic pronouns follow
/// ([`Shape::takes_enclitics`]): `dímelo` has those of `di` as an
/// imperative of `decir`, not as a preterite of `dar`, nor those of `dime`
/// as an imperative of `dimir`. A word without a written accent is tried
/// only through stems of one syllable: a pronoun on a longer verb form
/// takes an accent (`tómate`), so `tomate` and `filete` are no `toma` and
/// `file` with `te`, while `dale` and `ponte` are `da` and `pon`. Nothing here depends on the sentence. An error of an
/// index is the error of the lookup.
pub fn analyses_of<'a>(
    indexes: impl IntoIterator<Item = &'a Index> + Clone,
    language: Language,
    word: &str,
) -> Result<Vec<Analyses>, Error> {
    let written = reduction::stripped(word);
    if written.is_empty() {
        return Ok(Vec::new());
    }
    let lowercase = reduction::lowercase(written);
    let mut forms = vec![written.to_owned()];
    if lowercase != written {
        forms.push(lowercase.clone());
    }
    let found = union(indexes.clone(), &forms, |_| true)?;
    if !found.is_empty() || language != Language::Es {
        return Ok(found);
    }
    let accented = lowercase
        .chars()
        .any(|character| "áéíóú".contains(character));
    for stem in reduction_es_ca::enclitic_stems(&lowercase) {
        if !accented && syllables(&stem) > 1 {
            continue;
        }
        let found = union(indexes.clone(), &[stem], Shape::takes_enclitics)?;
        if !found.is_empty() {
            return Ok(found);
        }
    }
    Ok(Vec::new())
}

/// How many syllables `word` has, counted as its runs of vowels: enough to
/// tell `da` from `toma`.
fn syllables(word: &str) -> usize {
    let vowel = |character: char| "aeiouáéíóúü".contains(character);
    let mut count = 0;
    let mut previous = false;
    for character in word.chars() {
        let now = vowel(character);
        count += usize::from(now && !previous);
        previous = now;
    }
    count
}

/// The analyses of `forms` in `indexes`, the shapes `kept` keeps, as one
/// list per lemma; a lemma with no shape kept is left out.
fn union<'a>(
    indexes: impl IntoIterator<Item = &'a Index>,
    forms: &[String],
    kept: impl Fn(Shape) -> bool,
) -> Result<Vec<Analyses>, Error> {
    let mut union: BTreeMap<String, BTreeSet<Shape>> = BTreeMap::new();
    for index in indexes {
        for form in forms {
            for analyses in index.analyses(form)? {
                let shapes = analyses.shapes.into_iter().filter(|shape| kept(*shape));
                union.entry(analyses.lemma).or_default().extend(shapes);
            }
        }
    }
    Ok(union
        .into_iter()
        .filter(|(_, shapes)| !shapes.is_empty())
        .map(|(lemma, shapes)| Analyses {
            lemma,
            shapes: shapes.into_iter().collect(),
        })
        .collect())
}

/// What `indexes`, each with its kind, give for `word` read in `language`,
/// grouped by kind in the order of the list. Every index is asked for the
/// same headwords: those [`headwords`] gives with the exception lemmas of
/// all the indexes together, read once. Every sense of every headword found
/// is kept; only the definitions are merged, a definition already there
/// skipped.
/// `None` when no index has a headword for the word. An error of an index
/// is the error of the lookup; it carries the path of that index.
pub fn grouped(
    indexes: &[(Kind, &Index)],
    language: Language,
    word: &str,
) -> Result<Option<Grouped>, Error> {
    let mut grouped = Grouped::default();
    let mut found_any = false;
    let mut definitions = Vec::new();
    let lemmas = lemmas_of(indexes.iter().map(|(_, index)| *index), language, word)?;
    // Read once, and only when needed: by a synonyms index, or once the
    // word is found.
    let mut analyses: Option<Vec<Analyses>> = None;
    let analyses_once = || -> Result<Vec<Analyses>, Error> {
        analyses_of(indexes.iter().map(|(_, index)| *index), language, word)
    };
    // The word as it may stand among the members of a sense: as written and
    // in lowercase, never its forms, so the members of a form found keep
    // the synonyms of the word.
    let written = reduction::stripped(word);
    let itself = [written.to_owned(), reduction::lowercase(written)];
    let found_in = |index: &Index| found(index, language, word, &lemmas);
    for (kind, index) in indexes {
        let found = found_in(index)?;
        crate::combined::append_unique(
            &mut grouped.entries,
            found
                .iter()
                .flat_map(|found| found.article.entries.iter().cloned()),
        );
        crate::combined::append_unique(
            &mut grouped.sense_metadata,
            found
                .iter()
                .flat_map(|found| found.article.sense_metadata.iter().cloned()),
        );
        match kind {
            Kind::Definitions => {
                found_any |= !found.is_empty();
                grouped.definitions.get_or_insert_with(Vec::new);
                definitions.extend(found.into_iter().map(|found| Found {
                    article: Article {
                        translations: Vec::new(),
                        ..found.article
                    },
                    ..found
                }));
            }
            Kind::Russian => {
                found_any |= !found.is_empty();
                let russian = grouped.russian.get_or_insert_with(Vec::new);
                let translations = found.iter().flat_map(|found| &found.article.translations);
                push_new(russian, translations.cloned());
            }
            Kind::Synonyms => {
                if analyses.is_none() {
                    analyses = Some(analyses_once()?);
                }
                let analyses = analyses.as_deref().unwrap_or_default();
                let groups = grouped.synonyms.get_or_insert_with(Vec::new);
                for headword in headwords(language, word, &lemmas) {
                    let Some(meanings) = index.synonym_groups(&headword)? else {
                        continue;
                    };
                    found_any = true;
                    // Only this candidate's positive verb analysis constrains its
                    // meanings; other headwords can supply independent readings.
                    // Exact spellings still keep their independent readings.
                    let derived_verb = !itself.contains(&headword)
                        && analyses.iter().any(|analysis| {
                            analysis.lemma == headword && !analysis.shapes.is_empty()
                        });
                    for mut meaning in meanings {
                        if derived_verb
                            && meaning
                                .metadata
                                .as_ref()
                                .and_then(|metadata| metadata.class())
                                .is_some_and(|class| class != crate::format::GrammaticalClass::Verb)
                        {
                            continue;
                        }
                        meaning.synonyms.retain(|member| !itself.contains(member));
                        if !meaning.synonyms.is_empty() {
                            groups.push(meaning);
                        }
                    }
                }
            }
            Kind::English => {
                found_any |= !found.is_empty();
                let english = grouped.english.get_or_insert_with(Vec::new);
                let glosses = found
                    .iter()
                    .flat_map(|found| &found.article.senses)
                    .filter(|sense| !sense.definition.trim().is_empty());
                push_new(english, glosses.map(|sense| sense.definition.clone()));
            }
            // Never among the indexes of a reading language: a Russian
            // index is asked by [`russian`] alone. Nothing of it is kept.
            Kind::FromRussian => found_any |= !found.is_empty(),
        }
    }
    if !found_any {
        return Ok(None);
    }
    if let Some(senses) = &mut grouped.definitions {
        *senses = merged(definitions).map_or_else(Vec::new, |article| article.senses);
    }
    let analyses = match analyses {
        Some(analyses) => analyses,
        None => analyses_once()?,
    };
    if !analyses.is_empty() {
        let periphrastic = language == Language::Ca && PERIPHRASTIC.contains(&itself[1].as_str());
        grouped.verb = Some(Box::new(VerbForm {
            analyses,
            periphrastic: periphrastic.then(|| written.to_owned()),
        }));
    }
    Ok(Some(grouped))
}

/// The headwords to try for the Russian `word`, in order, each once; empty
/// when nothing of the word is left. The word without its stress marks
/// ([`wikdict::STRESS_MARKS`], as the headwords are keyed) and without the
/// punctuation around it, and its lowercase; then each of them with every
/// `ё` written `е`; then each of those with one `е` written `ё`, from the
/// first `е` on. Nothing else is done to the word: no dictionary forms.
pub fn russian_spellings(word: &str) -> Vec<String> {
    let unstressed = wikdict::unstressed_word(word);
    let written = reduction::stripped(&unstressed);
    if written.is_empty() {
        return Vec::new();
    }
    let cased = [written.to_owned(), reduction::lowercase(written)];
    let plain: Vec<String> = cased
        .iter()
        .map(|spelling| spelling.replace('ё', "е").replace('Ё', "Е"))
        .collect();
    let mut spellings = Vec::new();
    let mut add = |spelling: String| {
        if !spellings.contains(&spelling) {
            spellings.push(spelling);
        }
    };
    for spelling in cased.iter().chain(&plain) {
        add(spelling.clone());
    }
    for spelling in &plain {
        for (at, letter) in spelling.char_indices() {
            let yo = match letter {
                'е' => "ё",
                'Е' => "Ё",
                _ => continue,
            };
            let end = at + letter.len_utf8();
            add(format!("{}{yo}{}", &spelling[..at], &spelling[end..]));
        }
    }
    spellings
}

/// The translations the Russian index `index` gives for the Russian
/// `word`: those of the first of its [`russian_spellings`] that is a
/// headword of the index, each once, in the order of the index. `None` when
/// no spelling is a headword, or the headword has no translation. An error
/// of the index is the error of the lookup.
pub fn russian(index: &Index, word: &str) -> Result<Option<Vec<String>>, Error> {
    for spelling in russian_spellings(word) {
        if let Some(article) = index.find(&spelling)? {
            let mut translations = Vec::new();
            push_new(&mut translations, article.translations);
            return Ok((!translations.is_empty()).then_some(translations));
        }
    }
    Ok(None)
}

/// Appends each of `items` that `list` does not have yet.
fn push_new(list: &mut Vec<String>, items: impl IntoIterator<Item = String>) {
    for item in items {
        if !list.contains(&item) {
            list.push(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use addict_core::dictionary::Sense;

    use super::*;
    use crate::{
        format::{Contents, ExceptionForm, Headword, SenseRecord, encode, tests::texts},
        testdir::TempDir,
    };

    fn lemmas(entries: &[(&str, &[&str])]) -> Lemmas {
        entries
            .iter()
            .map(|(form, lemmas)| ((*form).to_owned(), texts(lemmas)))
            .collect()
    }

    #[test]
    fn a_russian_word_is_tried_as_written_in_lowercase_and_with_yo_as_ye() {
        assert_eq!(russian_spellings("собака"), texts(&["собака"]));
        assert_eq!(russian_spellings("«Дом»,"), texts(&["Дом", "дом"]));
        assert_eq!(russian_spellings("еж"), texts(&["еж", "ёж"]));
        assert_eq!(russian_spellings("ёж"), texts(&["ёж", "еж"]));
        assert_eq!(
            russian_spellings("Ёлке"),
            texts(&["Ёлке", "ёлке", "Елке", "елке", "Елкё", "елкё"])
        );
        assert_eq!(russian_spellings("кни\u{0301}га"), texts(&["книга"]));
        assert_eq!(
            russian_spellings("б\u{045D}отехнология"),
            texts(&["биотехнология", "биотёхнология"])
        );
        assert_eq!(russian_spellings("«Что\u{0300}»"), texts(&["Что", "что"]));
        assert!(russian_spellings("…").is_empty());
        assert!(russian_spellings("").is_empty());
    }

    /// An index of Russian headwords in the shape of `wikdict-ru-*`.
    fn russian_index(dir: &TempDir, name: &str, entries: &[(&str, &[&str])]) -> Index {
        let entries: Vec<Entry<'_>> = entries
            .iter()
            .map(|(word, translations)| (*word, &[][..], *translations))
            .collect();
        canned(dir, name, &entries)
    }

    #[test]
    fn a_russian_word_gets_the_translations_of_its_first_headword() {
        let dir = TempDir::new("lookup-ru");
        let spanish = russian_index(
            &dir,
            "ru-es.index",
            &[
                ("собака", &["perro", "cachorro", "arroba"]),
                ("дом", &["casa", "hogar", "domicilio"]),
                ("Москва", &["Moscú"]),
            ],
        );
        let english = russian_index(&dir, "ru-en.index", &[("Москва", &["Moscow", "Moskva"])]);
        let catalan = russian_index(&dir, "ru-ca.index", &[("ёж", &["eriçó"])]);
        let found = |index: &Index, word: &str| russian(index, word).expect("look up");
        // HAPPY_ES, HAPPY_EN, YO, CASE_PUNCT.
        assert_eq!(
            found(&spanish, "собака"),
            Some(texts(&["perro", "cachorro", "arroba"]))
        );
        assert_eq!(
            found(&english, "Москва"),
            Some(texts(&["Moscow", "Moskva"]))
        );
        assert_eq!(found(&catalan, "еж"), Some(texts(&["eriçó"])));
        assert_eq!(found(&catalan, "Ёж"), Some(texts(&["eriçó"])));
        assert_eq!(
            found(&spanish, "«Дом»,"),
            Some(texts(&["casa", "hogar", "domicilio"]))
        );
        // STRESS: the headword is keyed without the mark, and so is the word.
        let english = russian_index(
            &dir,
            "ru-en-stress.index",
            &[("книга", &["book"]), ("Москва", &["Moscow", "Moskva"])],
        );
        assert_eq!(found(&english, "книга"), Some(texts(&["book"])));
        assert_eq!(found(&english, "кни\u{0301}га"), Some(texts(&["book"])));
        // NO_ENTRY: an inflected form is not reduced.
        assert_eq!(found(&spanish, "собаки"), None);
        assert_eq!(found(&spanish, "…"), None);
        // A lowercase word does not reach a capitalised headword.
        assert_eq!(found(&english, "москва"), None);
    }

    #[test]
    fn english_headwords_are_what_they_were() {
        let went = lemmas(&[("went", &["go"])]);
        let english = headwords(Language::En, "“Went,”", &went);
        assert_eq!(english[..3], texts(&["Went", "went", "go"]));
        assert_eq!(
            headwords(Language::En, "banks", &Lemmas::new()),
            [vec!["banks".to_owned()], reduction::forms("banks", &[])].concat()
        );
        // Only the entry of the lowercase word is read.
        let other = lemmas(&[("Went", &["walk"]), ("wen", &["x"])]);
        assert_eq!(
            headwords(Language::En, "Went", &other),
            [texts(&["Went", "went"]), reduction::forms("went", &[])].concat()
        );
        assert_eq!(asked_forms(Language::En, "“Went,”"), texts(&["went"]));
        assert!(headwords(Language::En, "…", &went).is_empty());
    }

    #[test]
    fn spanish_and_catalan_words_reach_their_forms_after_the_word_and_its_lowercase() {
        let none = Lemmas::new();
        let cerró = headwords(Language::Es, "«Cerró»,", &none);
        assert_eq!(cerró[..2], texts(&["Cerró", "cerró"]));
        assert_eq!(
            cerró[2..],
            reduction_es_ca::forms("cerró", reduction_es_ca::Language::Es, |_| Vec::new())
        );
        assert!(cerró.contains(&"cerrar".to_owned()));
        let dámelo = headwords(Language::Es, "dámelo", &none);
        assert!(dámelo.contains(&"dar".to_owned()), "{dámelo:?}");
        assert_eq!(
            headwords(Language::Ca, "col·leccions", &none)[1],
            "col·lecció"
        );
        assert_eq!(headwords(Language::Ca, "d'art", &none)[1], "art");
        // Typeset text: the word as the app looks it up.
        let typeset = addict_core::sentence::lookup_word("d\u{2019}art");
        assert_eq!(headwords(Language::Ca, &typeset, &none)[1], "art");
        // The exception lemmas of the word come first among the forms.
        let fue = lemmas(&[("fue", &["ir", "ser"])]);
        assert_eq!(
            headwords(Language::Es, "Fue", &fue)[..4],
            texts(&["Fue", "fue", "ir", "ser"])
        );
        // A base's own lemmas follow the base.
        let havia = lemmas(&[("havia", &["haver"])]);
        assert_eq!(
            headwords(Language::Ca, "n'havia", &havia)[..3],
            texts(&["n'havia", "havia", "haver"])
        );
        for language in [Language::Es, Language::Ca] {
            assert!(headwords(language, "…", &none).is_empty());
            assert!(asked_forms(language, "…").is_empty());
        }
        assert_eq!(
            asked_forms(Language::Ca, "D'aigua'l"),
            texts(&["d'aigua'l", "d'aigua", "aigua'l", "aigua"])
        );
        assert_eq!(asked_forms(Language::Es, "bancos"), texts(&["bancos"]));
    }

    fn spanish() -> Contents {
        Contents {
            source: "spanish-1".to_owned(),
            senses: vec![
                SenseRecord {
                    part_of_speech: None,
                    definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                    members: Vec::new(),
                    metadata: None,
                },
                SenseRecord {
                    part_of_speech: None,
                    definition: "Mueble alargado para sentarse.".to_owned(),
                    members: texts(&["banca"]),
                    metadata: None,
                },
                SenseRecord {
                    part_of_speech: None,
                    definition: "Plural de banco.".to_owned(),
                    members: Vec::new(),
                    metadata: None,
                },
            ],
            headwords: vec![
                Headword {
                    entries: Vec::new(),
                    word: "banco".to_owned(),
                    senses: vec![0, 1],
                    translations: Vec::new(),
                },
                Headword {
                    entries: Vec::new(),
                    word: "bancos".to_owned(),
                    senses: vec![2],
                    translations: Vec::new(),
                },
            ],
            analyses: Vec::new(),
            forms: vec![ExceptionForm {
                form: "bancos".to_owned(),
                lemmas: texts(&["banco"]),
            }],
        }
    }

    #[test]
    fn a_spanish_word_reaches_its_lemma_through_its_forms() {
        let dir = TempDir::new("lookup-es");
        let path = dir.path().join("es.index");
        std::fs::write(&path, encode(&spanish()).expect("encode")).expect("write");
        let index = Index::open(&path).expect("open");
        let banco_senses = vec![
            Sense {
                part_of_speech: None,
                entries: Vec::new(),
                definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                synonyms: Vec::new(),
            },
            Sense {
                part_of_speech: None,
                entries: Vec::new(),
                definition: "Mueble alargado para sentarse.".to_owned(),
                synonyms: texts(&["banca"]),
            },
        ];
        let banco = article(&index, Language::Es, "Banco").expect("find");
        assert_eq!(
            banco,
            Some(Article {
                sense_metadata: Vec::new(),
                entries: Vec::new(),
                senses: banco_senses.clone(),
                translations: Vec::new(),
            })
        );
        // `bancos` itself first, then `banco`, which the form table and the
        // `-s` rule both give.
        let bancos = article(&index, Language::Es, "bancos")
            .expect("find")
            .expect("bancos");
        assert_eq!(bancos.senses[0].definition, "Plural de banco.");
        assert_eq!(bancos.senses[1..], banco_senses[..]);
        assert_eq!(article(&index, Language::Es, "zzz").expect("find"), None);
    }

    /// One headword of a canned index: the word, its senses as a
    /// definition and its members, and its translations.
    type Entry<'a> = (&'a str, &'a [(&'a str, &'a [&'a str])], &'a [&'a str]);

    /// Writes an index of `entries` named `name` into `dir` and opens it.
    fn canned(dir: &TempDir, name: &str, entries: &[Entry<'_>]) -> Index {
        canned_with_forms(dir, name, entries, &[])
    }

    /// [`canned`] with a form table of `forms`, each a form and its lemmas.
    fn canned_with_forms(
        dir: &TempDir,
        name: &str,
        entries: &[Entry<'_>],
        forms: &[(&str, &[&str])],
    ) -> Index {
        written(dir, name, &canned_contents(name, entries, forms))
    }

    /// `contents` written to `name` in `dir` and opened.
    fn written(dir: &TempDir, name: &str, contents: &Contents) -> Index {
        let path = dir.path().join(name);
        std::fs::write(&path, encode(contents).expect("encode")).expect("write");
        Index::open(&path).expect("open")
    }

    /// The contents of [`canned_with_forms`].
    fn canned_contents(name: &str, entries: &[Entry<'_>], forms: &[(&str, &[&str])]) -> Contents {
        let mut forms = forms.to_vec();
        forms.sort_by_key(|(form, _)| *form);
        let forms = forms
            .into_iter()
            .map(|(form, lemmas)| ExceptionForm {
                form: form.to_owned(),
                lemmas: texts(lemmas),
            })
            .collect();
        let mut entries = entries.to_vec();
        entries.sort_by_key(|(word, _, _)| *word);
        let mut contents = Contents {
            source: name.to_owned(),
            senses: Vec::new(),
            headwords: Vec::new(),
            analyses: Vec::new(),
            forms,
        };
        for (word, senses, translations) in entries {
            let mut numbers = Vec::new();
            for (definition, members) in senses {
                numbers.push(contents.senses.len() as u32);
                contents.senses.push(SenseRecord {
                    part_of_speech: None,
                    definition: (*definition).to_owned(),
                    members: texts(members),
                    metadata: None,
                });
            }
            contents.headwords.push(Headword {
                entries: Vec::new(),
                word: word.to_owned(),
                senses: numbers,
                translations: texts(translations),
            });
        }
        contents
    }

    fn group(headword: &str, members: &[&str]) -> SynonymGroup {
        SynonymGroup {
            headword: headword.to_owned(),
            definition: String::new(),
            metadata: None,
            synonyms: texts(members),
        }
    }

    /// The four kinds of Spanish indexes, each having `banco`.
    struct Spanish {
        russian: Index,
        definitions: Index,
        synonyms: Index,
        english: Index,
    }

    fn spanish_set(dir: &TempDir) -> Spanish {
        Spanish {
            russian: canned(
                dir,
                "es-ru.index",
                &[
                    ("banco", &[], &["банк", "скамейка"]),
                    ("mesa", &[], &["стол"]),
                ],
            ),
            definitions: canned(
                dir,
                "es-def.index",
                &[
                    (
                        "banco",
                        &[
                            ("Empresa que ofrece servicios monetarios.", &[]),
                            ("Mueble alargado para sentarse.", &["banco", "banca"]),
                        ],
                        &[],
                    ),
                    ("cosa", &[("Lo que existe.", &[])], &[]),
                ],
            ),
            synonyms: canned(
                dir,
                "es-syn.index",
                &[(
                    "banco",
                    &[
                        ("", &["banco", "entidad", "banca"]),
                        ("", &["asiento", "banca"]),
                    ],
                    &[],
                )],
            ),
            english: canned(
                dir,
                "es-en.index",
                &[(
                    "banco",
                    &[
                        ("bank (financial institution)", &[]),
                        ("bench", &[]),
                        ("bench", &[]),
                    ],
                    &[],
                )],
            ),
        }
    }

    #[test]
    fn derived_verb_meanings_filter_only_explicit_incompatible_classes() {
        use crate::format::{AnalysedForm, ExceptionForm, GrammaticalClass};
        let dir = TempDir::new("synonym-grammar");
        let mut contents = crate::mythes::parse(&b"UTF-8\nfer|5\n(adj)|ferotge\n(v)|prendre\n(v; beure)|prendre|trincar\n(mystery)|unknown\n|legacy\nfeu|1\n(n)|exact reading\nfar|1\n(n)|farola\n"[..], "labels").expect("parse");
        contents.forms.push(ExceptionForm {
            form: "feu".to_owned(),
            lemmas: texts(&["far", "fer"]),
        });
        contents.analyses.push(AnalysedForm {
            form: "feu".to_owned(),
            lemmas: vec![Analyses {
                lemma: "fer".to_owned(),
                shapes: vec![Shape::Gerund],
            }],
        });
        let index = written(&dir, "labels.index", &contents);
        let get = |index: &Index, word| {
            grouped(&[(Kind::Synonyms, index)], Language::Ca, word)
                .expect("lookup")
                .expect("found")
                .synonyms
                .expect("groups")
        };
        let all = get(&index, "feu");
        assert_eq!(all.len(), 6);
        assert!(
            all.iter()
                .any(|group| group.headword == "far" && group.synonyms == texts(&["farola"]))
        );
        let derived: Vec<_> = all
            .into_iter()
            .filter(|group| group.headword != "far")
            .collect();
        assert_eq!(derived.len(), 5);
        assert_eq!(derived[0].headword, "feu");
        assert_eq!(derived[0].synonyms, texts(&["exact reading"]));
        assert_eq!(
            derived[0].metadata.as_ref().expect("noun").class(),
            Some(GrammaticalClass::Noun)
        );
        assert!(
            derived
                .iter()
                .all(|group| !group.synonyms.contains(&"ferotge".to_owned()))
        );
        assert_eq!(derived[1].synonyms, texts(&["prendre"]));
        assert_eq!(derived[2].synonyms, texts(&["prendre", "trincar"]));
        assert_eq!(derived[3].metadata.as_ref().expect("unknown").class(), None);
        assert_eq!(derived[4].metadata, None);
        assert_eq!(get(&index, "fer").len(), 5);
        contents.analyses.clear();
        let no_analysis = written(&dir, "no-analysis.index", &contents);
        assert_eq!(get(&no_analysis, "feu").len(), 7);
        // Legacy indexes stay readable but cannot recover discarded labels.
        for sense in &mut contents.senses {
            sense.metadata = None;
        }
        contents.analyses.push(AnalysedForm {
            form: "feu".to_owned(),
            lemmas: vec![Analyses {
                lemma: "fer".to_owned(),
                shapes: vec![Shape::Gerund],
            }],
        });
        let legacy = written(&dir, "legacy.index", &contents);
        assert_eq!(get(&legacy, "feu").len(), 7);
    }

    #[test]
    fn causes_keeps_causa_nouns_alongside_analysed_causar_verbs() {
        use crate::format::{AnalysedForm, ExceptionForm, GrammaticalClass};
        let dir = TempDir::new("synonym-independent-readings");
        let mut contents = crate::mythes::parse(&b"UTF-8\ncausa|3\n(n)|motiu\n(n)|proces\n(n)|ideal\ncausar|4\n(v)|provocar\n(v)|originar\n(v)|produir\n(v)|ocasionar\n"[..], "causes").expect("parse");
        contents.forms.push(ExceptionForm {
            form: "causes".to_owned(),
            lemmas: texts(&["causa", "causar"]),
        });
        contents.analyses.push(AnalysedForm {
            form: "causes".to_owned(),
            lemmas: vec![Analyses {
                lemma: "causar".to_owned(),
                shapes: vec![Shape::Gerund],
            }],
        });
        let index = written(&dir, "causes.index", &contents);
        let groups = grouped(&[(Kind::Synonyms, &index)], Language::Ca, "causes")
            .expect("lookup")
            .expect("found")
            .synonyms
            .expect("groups");
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

    #[test]
    fn every_kind_gives_its_part_of_the_article() {
        let dir = TempDir::new("grouped-all");
        let set = spanish_set(&dir);
        let indexes = [
            (Kind::Russian, &set.russian),
            (Kind::Definitions, &set.definitions),
            (Kind::Synonyms, &set.synonyms),
            (Kind::English, &set.english),
        ];
        let banco = grouped(&indexes, Language::Es, "«Banco»,")
            .expect("look up")
            .expect("banco");
        assert_eq!(banco.russian, Some(texts(&["банк", "скамейка"])));
        assert_eq!(
            banco.definitions,
            Some(vec![
                Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                    synonyms: Vec::new(),
                },
                Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "Mueble alargado para sentarse.".to_owned(),
                    synonyms: texts(&["banca"]),
                },
            ])
        );
        // Independent meanings retain duplicates across groups and omit the headword.
        assert_eq!(
            banco.synonyms,
            Some(vec![
                group("banco", &["entidad", "banca"]),
                group("banco", &["asiento", "banca"])
            ])
        );
        assert_eq!(
            banco.english,
            Some(texts(&["bank (financial institution)", "bench"]))
        );
    }

    #[test]
    fn a_spanish_verb_form_reaches_the_russian_of_its_infinitive() {
        let dir = TempDir::new("grouped-cerro");
        let russian = canned(&dir, "es-ru.index", &[("cerrar", &[], &["закрыть"])]);
        let cerró = grouped(&[(Kind::Russian, &russian)], Language::Es, "cerró")
            .expect("look up")
            .expect("cerró");
        assert_eq!(cerró.russian, Some(texts(&["закрыть"])));
    }

    #[test]
    fn the_form_table_of_one_index_serves_every_index_of_the_language() {
        let dir = TempDir::new("grouped-fue");
        // WikDict has no form table; the Kaikki index lists `fue`.
        let russian = canned(
            &dir,
            "es-ru.index",
            &[("ir", &[], &["идти"]), ("ser", &[], &["быть"])],
        );
        let definitions = canned_with_forms(
            &dir,
            "es-def.index",
            &[("ser", &[("Existir.", &[])], &[])],
            &[("fue", &["ser", "ir"])],
        );
        let indexes = [(Kind::Russian, &russian), (Kind::Definitions, &definitions)];
        let fue = grouped(&indexes, Language::Es, "fue")
            .expect("look up")
            .expect("fue");
        // The lemmas sorted: `ir`, then `ser`.
        assert_eq!(fue.russian, Some(texts(&["идти", "быть"])));
        assert_eq!(
            fue.definitions.expect("definitions")[0].definition,
            "Existir."
        );
        // One index alone reads only its own form table.
        assert_eq!(article(&russian, Language::Es, "fue").expect("find"), None);
    }

    #[test]
    fn a_capitalised_word_is_tried_as_written_then_in_lowercase_then_by_its_forms() {
        let dir = TempDir::new("grouped-order");
        let definitions = canned(
            &dir,
            "es-def.index",
            &[
                ("Cerró", &[("As written.", &[])], &[]),
                ("cerrar", &[("The infinitive.", &[])], &[]),
                ("cerró", &[("In lowercase.", &[])], &[]),
            ],
        );
        let found = grouped(&[(Kind::Definitions, &definitions)], Language::Es, "Cerró")
            .expect("look up")
            .expect("Cerró");
        let order: Vec<_> = found
            .definitions
            .expect("definitions")
            .into_iter()
            .map(|sense| sense.definition)
            .collect();
        assert_eq!(
            order,
            texts(&["As written.", "In lowercase.", "The infinitive."])
        );
    }

    #[test]
    fn a_synonym_that_is_also_a_form_of_the_word_is_kept() {
        let dir = TempDir::new("grouped-own-synonyms");
        // `banco` is a rule form of `banca` (a -> o), and its synonym too.
        let synonyms = canned(
            &dir,
            "es-syn.index",
            &[("banca", &[("", &["banca", "banco", "asiento"])], &[])],
        );
        let banca = grouped(&[(Kind::Synonyms, &synonyms)], Language::Es, "Banca")
            .expect("look up")
            .expect("banca");
        assert_eq!(
            banca.synonyms,
            Some(vec![group("banca", &["banco", "asiento"])])
        );
    }

    #[test]
    fn a_lemma_reached_through_a_form_is_not_its_own_synonym() {
        let dir = TempDir::new("grouped-lemma-synonym");
        let synonyms = canned(
            &dir,
            "es-syn.index",
            &[("banco", &[("", &["banco", "entidad", "asiento"])], &[])],
        );
        let bancos = grouped(&[(Kind::Synonyms, &synonyms)], Language::Es, "bancos")
            .expect("look up")
            .expect("bancos");
        let list = bancos.synonyms.expect("synonyms");
        assert!(
            list.iter()
                .all(|group| !group.synonyms.contains(&"banco".to_owned())),
            "{list:?}"
        );
        assert_eq!(list, vec![group("banco", &["entidad", "asiento"])]);
    }

    #[test]
    fn a_catalan_definitions_index_alone_gives_definitions_and_no_other_kind() {
        let dir = TempDir::new("grouped-ca");
        let definitions = canned(
            &dir,
            "ca-def.index",
            &[("col·lecció", &[("Conjunt de coses.", &[])], &[])],
        );
        let found = grouped(
            &[(Kind::Definitions, &definitions)],
            Language::Ca,
            "col·lecció",
        )
        .expect("look up")
        .expect("col·lecció");
        assert_eq!(
            found,
            Grouped {
                entries: Vec::new(),
                sense_metadata: Vec::new(),
                definitions: Some(vec![Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "Conjunt de coses.".to_owned(),
                    synonyms: Vec::new(),
                }]),
                russian: None,
                synonyms: None,
                english: None,
                verb: None,
            }
        );
    }

    /// [`canned`] with an analysis table of `analyses`, each a form, a
    /// lemma and a shape of it.
    fn canned_with_analyses(
        dir: &TempDir,
        name: &str,
        entries: &[Entry<'_>],
        analyses: &[(&str, &str, Shape)],
    ) -> Index {
        let mut contents = canned_contents(name, entries, &[]);
        let mut gathered: BTreeMap<String, BTreeMap<String, BTreeSet<Shape>>> = BTreeMap::new();
        for (form, lemma, shape) in analyses {
            gathered
                .entry((*form).to_owned())
                .or_default()
                .entry((*lemma).to_owned())
                .or_default()
                .insert(*shape);
        }
        contents.analyses = gathered
            .into_iter()
            .map(|(form, lemmas)| crate::format::AnalysedForm {
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
        written(dir, name, &contents)
    }

    mod shapes {
        use crate::analysis::{Number, Person, Shape, TenseMood};

        pub fn sg(tense_mood: TenseMood, person: Person) -> Shape {
            Shape::Finite {
                tense_mood,
                number: Number::Singular,
                person,
            }
        }
    }

    fn of(lemma: &str, shapes: &[Shape]) -> Analyses {
        Analyses {
            lemma: lemma.to_owned(),
            shapes: shapes.to_vec(),
        }
    }

    #[test]
    fn the_analyses_of_a_form_are_the_union_of_the_indexes_per_lemma() {
        use crate::analysis::{Person::*, TenseMood::*};
        use shapes::sg;
        let dir = TempDir::new("analyses-union");
        let preterite = sg(PreteriteIndicative, Third);
        let entries: &[Entry<'_>] = &[("fue", &[("Forma de ser.", &[])], &[])];
        let one = canned_with_analyses(
            &dir,
            "one.index",
            entries,
            &[
                ("fue", "ser", preterite),
                ("fue", "ir", preterite),
                ("hable", "hablar", sg(PresentSubjunctive, First)),
                ("hable", "hablar", sg(Imperative, Third)),
            ],
        );
        let two = canned_with_analyses(
            &dir,
            "two.index",
            &[("hable", &[("Forma de hablar.", &[])], &[])],
            &[
                ("fue", "ir", preterite),
                ("hable", "hablar", sg(PresentSubjunctive, Third)),
                ("hable", "hablar", sg(PresentSubjunctive, First)),
                ("hable", "hablar", sg(Imperative, Formal)),
            ],
        );
        let indexes = [(Kind::Definitions, &one), (Kind::English, &two)];
        let fue = grouped(&indexes, Language::Es, "Fue,")
            .expect("look up")
            .expect("fue");
        let fue = fue.verb.expect("a verb form");
        assert_eq!(
            fue.analyses,
            [of("ir", &[preterite]), of("ser", &[preterite])]
        );
        assert_eq!(fue.periphrastic, None);
        let hable = grouped(&indexes, Language::Es, "hable")
            .expect("look up")
            .expect("hable");
        assert_eq!(
            hable.verb.expect("a verb form").analyses,
            [of(
                "hablar",
                &[
                    sg(PresentSubjunctive, First),
                    sg(PresentSubjunctive, Third),
                    sg(Imperative, Formal),
                    sg(Imperative, Third),
                ]
            )]
        );
        // The rest of the article is what it was without analyses.
        let plain_dir = TempDir::new("analyses-union-plain");
        let plain = canned(&plain_dir, "one.index", entries);
        let without = grouped(&[(Kind::Definitions, &plain)], Language::Es, "fue")
            .expect("look up")
            .expect("fue");
        assert_eq!(without.verb, None);
        let with = grouped(&[(Kind::Definitions, &one)], Language::Es, "fue")
            .expect("look up")
            .expect("fue");
        assert_eq!(Grouped { verb: None, ..with }, without);
    }

    #[test]
    fn a_word_with_enclitics_has_the_imperative_of_its_stem() {
        use crate::analysis::{Person::*, TenseMood::*};
        use shapes::sg;
        let dir = TempDir::new("analyses-enclitics");
        let index = canned_with_analyses(
            &dir,
            "es.index",
            &[
                ("decir", &[("Hablar.", &[])], &[]),
                ("dímelo", &[("Forma de decir.", &[])], &[]),
                ("dale", &[("Forma de dar.", &[])], &[]),
                ("este", &[("Demostrativo.", &[])], &[]),
                ("tomate", &[("Fruto.", &[])], &[]),
                ("hablar", &[("Emitir palabras.", &[])], &[]),
            ],
            &[
                ("di", "dar", sg(PreteriteIndicative, First)),
                ("di", "decir", sg(Imperative, Second)),
                ("dime", "dimir", sg(Imperative, Second)),
                ("da", "dar", sg(Imperative, Second)),
                ("toma", "tomar", sg(Imperative, Second)),
                ("es", "ser", sg(PresentIndicative, Third)),
                ("hablo", "hablar", sg(PresentIndicative, First)),
            ],
        );
        let analyses = |language, word| {
            grouped(&[(Kind::Definitions, &index)], language, word)
                .expect("look up")
                .map(|found| found.verb.map_or_else(Vec::new, |verb| verb.analyses))
        };
        assert_eq!(
            analyses(Language::Es, "dímelo"),
            Some(vec![of("decir", &[sg(Imperative, Second)])])
        );
        // A stem with no imperative gives nothing: `este` is no `es`.
        assert_eq!(analyses(Language::Es, "este"), Some(Vec::new()));
        // Without an accent only a stem of one syllable: `dale` is `da`,
        // `tomate` is no `toma` + `te`.
        assert_eq!(
            analyses(Language::Es, "dale"),
            Some(vec![of("dar", &[sg(Imperative, Second)])])
        );
        assert_eq!(analyses(Language::Es, "tomate"), Some(Vec::new()));
        // A lemma has no analysis line.
        assert_eq!(analyses(Language::Es, "hablar"), Some(Vec::new()));
        // Catalan has no stems.
        assert_eq!(analyses(Language::Ca, "dímelo"), Some(Vec::new()));
    }

    #[test]
    fn a_catalan_periphrastic_auxiliary_is_named_only_with_its_analyses() {
        use crate::analysis::{Person::*, TenseMood::*};
        use shapes::sg;
        let dir = TempDir::new("analyses-periphrastic");
        let entries: &[Entry<'_>] = &[
            ("va", &[("Forma d'anar.", &[])], &[]),
            ("parla", &[("Forma de parlar.", &[])], &[]),
        ];
        let tagged = canned_with_analyses(
            &dir,
            "ca.index",
            entries,
            &[
                ("va", "anar", sg(PresentIndicative, Third)),
                ("parla", "parlar", sg(PresentIndicative, Third)),
            ],
        );
        let found = |index: &Index, language, word| {
            grouped(&[(Kind::English, index)], language, word)
                .expect("look up")
                .expect("found")
        };
        let verb = |index: &Index, language, word| found(index, language, word).verb;
        let va = verb(&tagged, Language::Ca, "Va").expect("a verb form");
        assert_eq!(va.analyses, [of("anar", &[sg(PresentIndicative, Third)])]);
        assert_eq!(va.periphrastic.as_deref(), Some("Va"));
        let parla = verb(&tagged, Language::Ca, "parla").expect("a verb form");
        assert_eq!(parla.periphrastic, None);
        let spanish = verb(&tagged, Language::Es, "va").expect("a verb form");
        assert_eq!(spanish.periphrastic, None);
        // No tags: no analyses and no note.
        let plain_dir = TempDir::new("analyses-periphrastic-plain");
        let plain = canned(&plain_dir, "ca.index", entries);
        assert_eq!(verb(&plain, Language::Ca, "va"), None);
    }

    #[test]
    fn a_kind_whose_index_lacks_the_word_is_empty_and_not_absent() {
        let dir = TempDir::new("grouped-lacks");
        let set = spanish_set(&dir);
        let indexes = [
            (Kind::Russian, &set.russian),
            (Kind::Definitions, &set.definitions),
        ];
        let cosa = grouped(&indexes, Language::Es, "cosa")
            .expect("look up")
            .expect("cosa");
        assert_eq!(cosa.russian, Some(Vec::new()));
        assert_eq!(cosa.definitions.expect("definitions").len(), 1);
        assert_eq!(cosa.synonyms, None);
        assert_eq!(cosa.english, None);
        // Russian alone: no definitions asked, so none are said to be empty.
        let mesa = grouped(&indexes, Language::Es, "mesa")
            .expect("look up")
            .expect("mesa");
        assert_eq!(mesa.russian, Some(texts(&["стол"])));
        assert_eq!(mesa.definitions, Some(Vec::new()));
    }

    #[test]
    fn a_word_with_no_synonyms_anywhere_has_empty_lists() {
        let dir = TempDir::new("grouped-no-synonyms");
        let set = spanish_set(&dir);
        let indexes = [
            (Kind::Definitions, &set.definitions),
            (Kind::Synonyms, &set.synonyms),
        ];
        let cosa = grouped(&indexes, Language::Es, "cosa")
            .expect("look up")
            .expect("cosa");
        assert_eq!(cosa.synonyms, Some(Vec::new()));
        let senses = cosa.definitions.expect("definitions");
        assert!(senses.iter().all(|sense| sense.synonyms.is_empty()));
    }

    #[test]
    fn a_word_in_no_index_has_no_article() {
        let dir = TempDir::new("grouped-none");
        let set = spanish_set(&dir);
        let indexes = [
            (Kind::Russian, &set.russian),
            (Kind::Definitions, &set.definitions),
            (Kind::Synonyms, &set.synonyms),
            (Kind::English, &set.english),
        ];
        assert_eq!(
            grouped(&indexes, Language::Es, "zzz").expect("look up"),
            None
        );
        assert_eq!(grouped(&indexes, Language::Es, "…").expect("look up"), None);
        assert_eq!(grouped(&[], Language::Es, "banco").expect("look up"), None);
    }

    #[test]
    fn an_index_that_cannot_be_read_is_the_error_with_its_path() {
        let dir = TempDir::new("grouped-error");
        let set = spanish_set(&dir);
        let broken = dir.path().join("es-syn.index");
        std::fs::write(&broken, b"").expect("truncate");
        let indexes = [
            (Kind::Definitions, &set.definitions),
            (Kind::Synonyms, &set.synonyms),
        ];
        let error = grouped(&indexes, Language::Es, "banco").expect_err("unreadable");
        let (Error::NotImported(path) | Error::Io { path, .. } | Error::Damaged { path, .. }) =
            &error;
        assert_eq!(path, &broken, "{error}");
    }
}
