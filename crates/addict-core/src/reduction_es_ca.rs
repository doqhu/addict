//! Reducing a Spanish or Catalan word to the forms its dictionary headword
//! may have: the port of `experiments/exp9/rules.py`, which gives the same
//! forms in the same order.
//!
//! There is no model and no part of speech: every rule that matches gives a
//! form, and the caller keeps those that are headwords. The word is one as
//! `sentence::words()` and `sentence::lookup_word()` give it, in lowercase:
//! a run of letters in which an inner `'` (U+0027) or `·` (U+00B7) stands
//! between two letters. The middle dot is an ordinary letter here: no rule
//! removes or adds it, so `col·leccions` reduces to `col·lecció`. Lengths
//! are counted in characters, not bytes.
//!
//! The reduction of a word ([`forms`]) is, in this order, without the word
//! itself and without repeats:
//!
//! 1. the exception lemmas of the word, as the caller gives them (sorted);
//! 2. for each of the [`bases`] (Catalan elision, Spanish enclitic
//!    pronouns), in their order: the base, the exception lemmas of the base
//!    and the [`suffix_forms`] of the base;
//! 3. the [`suffix_forms`] of the word.
//!
//! A form shorter than two letters is dropped. Nothing here reads a file or
//! keeps anything between calls.

/// The language whose rules reduce a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// Spanish.
    Es,
    /// Catalan.
    Ca,
}

/// The fewest letters of a form.
const MIN_FORM: usize = 2;

/// What a suffix rule does with the accents of the form it gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The form as it is.
    Keep,
    /// The form without stress accents.
    Strip,
    /// Every form of [`accent_variants`].
    Restore,
}

use Mode::{Keep, Restore, Strip};

/// A group of suffix rules as `rules.py` writes them. A rule matches when
/// the word ends with its suffix and is longer than it; it gives the word
/// with the suffix replaced.
enum Group {
    /// One rule: the suffix, its replacement, the accent mode.
    Rule(&'static str, &'static str, Mode),
    /// `_verb`: each ending (outer) with each infinitive ending (inner),
    /// both lists split on spaces.
    Verb(&'static str, &'static str, Mode),
    /// `_future`: each stem (outer) plus each ending (inner) is the suffix,
    /// and the stem its replacement.
    Future(&'static str, &'static str),
}

use Group::{Future, Rule, Verb};

/// The Spanish suffix rules, in order (227 rules once expanded).
const ES_RULES: &[Group] = &[
    // plural and gender of nouns and adjectives
    Rule("ces", "z", Keep),
    Rule("ones", "ón", Keep),
    Rule("es", "", Keep),
    Rule("es", "", Strip),
    Rule("s", "", Keep),
    Rule("a", "o", Keep),
    Rule("as", "o", Keep),
    Rule("a", "", Keep),
    Rule("as", "", Keep),
    // adverbs, superlatives, diminutives
    Rule("amente", "o", Keep),
    Rule("mente", "", Keep),
    Rule("ísimo", "o", Restore),
    Rule("ísima", "o", Restore),
    Rule("ísimos", "o", Restore),
    Rule("ísimas", "o", Restore),
    Rule("ísimo", "", Restore),
    Rule("ísimo", "e", Restore),
    Rule("quísimo", "co", Keep),
    Rule("guísimo", "go", Keep),
    Rule("ito", "o", Keep),
    Rule("ita", "a", Keep),
    Rule("itos", "o", Keep),
    Rule("itas", "a", Keep),
    Rule("quito", "co", Keep),
    Rule("quita", "ca", Keep),
    Rule("guito", "go", Keep),
    Rule("guita", "ga", Keep),
    Rule("cito", "", Keep),
    Rule("cita", "", Keep),
    // -ar verbs: present, imperfect, preterite, subjunctive, imperative,
    // participle, gerund
    Verb(
        "o as a amos áis an aba abas ábamos abais aban é aste ó asteis aron \
         e es emos éis en ara aras áramos arais aran ase ases ásemos aseis asen \
         ad ado ada ados adas ando",
        "ar",
        Keep,
    ),
    // -er and -ir verbs share their endings; both infinitives are tried
    Verb(
        "o es e emos éis en imos ís ía ías íamos íais ían í iste ió isteis ieron \
         a as amos áis an iera ieras iéramos ierais ieran iese ieses iésemos ieseis iesen \
         ed id ido ida idos idas iendo",
        "er ir",
        Keep,
    ),
    Future("é ás á emos éis án ía ías íamos íais ían", "ar er ir"),
    // regular spelling changes
    Verb("qué que ques quemos quen", "car", Keep),
    Verb("gué gue gues guemos guen", "gar", Keep),
    Verb("güé güe", "guar", Keep),
    Verb("cé ce ces cemos cen", "zar", Keep),
    Verb("jo ja jas jamos jan", "ger gir", Keep),
    Verb("zo za zas zamos zan", "cer cir", Keep),
    Verb("go ga gas gamos gan", "guir", Keep),
    Verb("yó yeron yendo", "er ir", Keep),
];

/// The Catalan suffix rules, in order (205 rules once expanded).
const CA_RULES: &[Group] = &[
    // plural and gender of nouns and adjectives
    Rule("ons", "ó", Keep),
    Rule("ins", "í", Keep),
    Rule("ans", "à", Keep),
    Rule("ns", "", Keep),
    Rule("ssos", "s", Keep),
    Rule("os", "", Keep),
    Rule("gues", "g", Keep),
    Rule("gues", "ga", Keep),
    Rule("ques", "c", Keep),
    Rule("ques", "ca", Keep),
    Rule("ges", "ja", Keep),
    Rule("ces", "ça", Keep),
    Rule("ves", "u", Keep),
    Rule("va", "u", Keep),
    Rule("es", "a", Keep),
    Rule("es", "", Keep),
    Rule("s", "", Keep),
    Rule("a", "", Keep),
    // adverbs, superlatives, diminutives
    Rule("ament", "", Keep),
    Rule("ment", "", Keep),
    Rule("íssim", "", Restore),
    Rule("íssima", "", Restore),
    Rule("íssims", "", Restore),
    Rule("íssimes", "", Restore),
    Rule("et", "", Keep),
    Rule("et", "e", Keep),
    Rule("eta", "a", Keep),
    Rule("ets", "", Keep),
    Rule("etes", "a", Keep),
    // -ar verbs
    Verb(
        "o es a em eu en ava aves àvem àveu aven í ares à àrem àreu aren \
         i is in és essis éssim éssiu essin at ada ats ades ant",
        "ar",
        Keep,
    ),
    // inceptive -ir verbs
    Verb("eix eixes eixen eixo eixi eixis eixin", "ir", Keep),
    // -re, -er and -ir verbs: shared endings
    Verb(
        "o es em eu en ia ies íem íeu ien i is in és essis éssim éssiu essin",
        "re er ir",
        Keep,
    ),
    Verb("ut uda uts udes ent", "re er", Keep),
    Verb("it ida its ides int im iu", "ir", Keep),
    Future("é às à em eu an ia ies íem íeu ien", "ar er ir"),
    Verb("ré ràs rà rem reu ran ria ries ríem ríeu rien", "re", Keep),
    // regular spelling changes of -ar verbs
    Verb("cem ceu ci cis cin cés", "çar", Keep),
    Verb("gem geu gi gis gin gés", "jar", Keep),
    Verb("quem queu qui quis quin qués", "car", Keep),
    Verb("guem gueu gui guis guin gués", "gar", Keep),
];

/// A Catalan verb in the third person singular present often has no ending
/// (`perd`, `dorm`): when the word ends in a consonant and holds no `'`,
/// these infinitive endings are added, after all suffix rules.
const CA_BARE_STEM: [&str; 3] = ["re", "ir", "er"];

const CA_VOWELS: &str = "aeiouàèéíòóúïü";

/// An elided article, preposition or pronoun before the word, joined by `'`.
const CA_ELISION_PREFIXES: [&str; 6] = ["l'", "d'", "s'", "m'", "t'", "n'"];

/// A reduced pronoun after a verb, joined by `'` (porta'l, dona'm).
const CA_ELISION_SUFFIXES: [&str; 9] = ["'ls", "'ns", "'hi", "'ho", "'l", "'m", "'n", "'s", "'t"];

const ES_CLITICS: [&str; 11] = [
    "me", "te", "se", "nos", "os", "le", "les", "lo", "la", "los", "las",
];

const ES_MAX_CLITICS: usize = 3;

const ES_ACUTE: &str = "áéíóú";

/// The suffix rules of `language`, expanded, in order: suffix, replacement,
/// mode.
fn rules(language: Language) -> Vec<(String, &'static str, Mode)> {
    let groups = match language {
        Language::Es => ES_RULES,
        Language::Ca => CA_RULES,
    };
    let mut rules = Vec::new();
    for group in groups {
        match *group {
            Rule(suffix, replacement, mode) => rules.push((suffix.to_owned(), replacement, mode)),
            Verb(endings, infinitives, mode) => {
                for ending in endings.split_whitespace() {
                    for infinitive in infinitives.split_whitespace() {
                        rules.push((ending.to_owned(), infinitive, mode));
                    }
                }
            }
            Future(endings, stems) => {
                for stem in stems.split_whitespace() {
                    for ending in endings.split_whitespace() {
                        rules.push((format!("{stem}{ending}"), stem, Keep));
                    }
                }
            }
        }
    }
    rules
}

/// `text` without stress accents: á à é è í ó ò ú become a a e e i o o u.
/// The diaeresis, ñ and ç are letters of their own and are kept.
fn strip_accents(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            'á' | 'à' => 'a',
            'é' | 'è' => 'e',
            'í' => 'i',
            'ó' | 'ò' => 'o',
            'ú' => 'u',
            other => other,
        })
        .collect()
}

/// The stress accents `vowel` may carry in `language`, in the order tried.
fn accents(vowel: char, language: Language) -> &'static [char] {
    match (language, vowel) {
        (Language::Es, 'a') => &['á'],
        (Language::Es, 'e') => &['é'],
        (Language::Es, 'o') => &['ó'],
        (Language::Ca, 'a') => &['à'],
        (Language::Ca, 'e') => &['è', 'é'],
        (Language::Ca, 'o') => &['ò', 'ó'],
        (_, 'i') => &['í'],
        (_, 'u') => &['ú'],
        _ => &[],
    }
}

/// `form`, then `form` without accents, then that with one stress accent on
/// one vowel, the last vowel first (Catalan e and o: grave, then acute).
fn accent_variants(form: &str, language: Language) -> Vec<String> {
    let plain: Vec<char> = strip_accents(form).chars().collect();
    let mut variants = vec![form.to_owned(), plain.iter().collect()];
    for at in (0..plain.len()).rev() {
        for &accented in accents(plain[at], language) {
            let mut variant = plain.clone();
            variant[at] = accented;
            variants.push(variant.into_iter().collect());
        }
    }
    variants
}

/// The number of characters of `text`.
fn letters(text: &str) -> usize {
    text.chars().count()
}

/// Appends `form` when it is long enough, is not `word` and is not there.
fn add(forms: &mut Vec<String>, word: &str, form: String) {
    if letters(&form) >= MIN_FORM && form != word && !forms.contains(&form) {
        forms.push(form);
    }
}

/// The forms the suffix rules of `language` give for `word`, in rule order,
/// each once, each at least two letters long and none equal to `word`. In
/// Catalan, the bare-stem endings follow.
pub fn suffix_forms(word: &str, language: Language) -> Vec<String> {
    let mut forms = Vec::new();
    let Some(last) = word.chars().next_back() else {
        return forms;
    };
    let length = letters(word);
    for (suffix, replacement, mode) in rules(language) {
        let Some(stem) = word.strip_suffix(suffix.as_str()) else {
            continue;
        };
        if letters(&suffix) >= length {
            continue;
        }
        let form = format!("{stem}{replacement}");
        match mode {
            Keep => add(&mut forms, word, form),
            Strip => add(&mut forms, word, strip_accents(&form)),
            Restore => {
                for variant in accent_variants(&form, language) {
                    add(&mut forms, word, variant);
                }
            }
        }
    }
    if language == Language::Ca && !word.contains('\'') && !CA_VOWELS.contains(last) {
        for ending in CA_BARE_STEM {
            add(&mut forms, word, format!("{word}{ending}"));
        }
    }
    forms
}

/// Catalan: without the reduced pronoun after the verb; without the elided
/// prefix; without both (`d'aigua'l` gives `d'aigua`, `aigua'l`, `aigua`).
fn ca_bases(word: &str) -> Vec<String> {
    let mut bases = Vec::new();
    let mut fronts = vec![word];
    fronts.extend(
        CA_ELISION_PREFIXES
            .iter()
            .filter_map(|prefix| word.strip_prefix(prefix)),
    );
    for front in fronts {
        let mut candidates = vec![front];
        candidates.extend(
            CA_ELISION_SUFFIXES
                .iter()
                .filter_map(|suffix| front.strip_suffix(suffix)),
        );
        for candidate in candidates {
            add(&mut bases, word, candidate.to_owned());
        }
    }
    bases
}

/// Spanish: the stems left under one, two and three enclitic pronouns
/// (dá-me-lo), one list per number of pronouns, each in the order tried; a
/// stem may repeat.
fn es_clitic_levels(word: &str) -> Vec<Vec<String>> {
    let mut levels = Vec::new();
    let mut stems = vec![word.to_owned()];
    for _ in 0..ES_MAX_CLITICS {
        stems = stems
            .iter()
            .flat_map(|stem| {
                ES_CLITICS.iter().filter_map(move |clitic| {
                    stem.strip_suffix(clitic)
                        .filter(|rest| !rest.is_empty())
                        .map(str::to_owned)
                })
            })
            .collect();
        levels.push(stems.clone());
    }
    levels
}

/// Spanish: the verb under one, two or three enclitic pronouns (dá-me-lo).
/// A base is kept when, without accents, it ends in `r` or `ndo`, or when it
/// carries an acute accent; it is given without accents, then as written if
/// that differs (`oírlo` gives `oir`, `oír`).
fn es_bases(word: &str) -> Vec<String> {
    let mut bases = Vec::new();
    for stem in es_clitic_levels(word).into_iter().flatten() {
        let plain = strip_accents(&stem);
        let kept = plain.ends_with('r')
            || plain.ends_with("ndo")
            || stem.chars().any(|character| ES_ACUTE.contains(character));
        if !kept {
            continue;
        }
        add(&mut bases, word, plain);
        add(&mut bases, word, stem);
    }
    bases
}

/// Every Spanish stem of `word` under one, two or three enclitic pronouns,
/// the most pronouns removed first, each without accents and then as
/// written, each once: the stems [`bases`] gives and those it does not keep
/// for lack of an `r`, an `ndo` or an accent (`dale` gives `da`). For the
/// caller that checks a stem otherwise, such as by the analyses of a verb
/// form: the stem under the most pronouns is the likelier verb, since a
/// written accent such as that of `dímelo` comes with two or more
/// (`dí-me-lo`, not the rare `dime` of `dimir` + `lo`).
pub fn enclitic_stems(word: &str) -> Vec<String> {
    let mut stems = Vec::new();
    for stem in es_clitic_levels(word).into_iter().rev().flatten() {
        add(&mut stems, word, strip_accents(&stem));
        add(&mut stems, word, stem);
    }
    stems
}

/// The bases of `word`: Catalan elision, or the Spanish verb under its
/// enclitic pronouns, in the order tried; never `word` itself.
pub fn bases(word: &str, language: Language) -> Vec<String> {
    match language {
        Language::Es => es_bases(word),
        Language::Ca => ca_bases(word),
    }
}

/// The dictionary forms to try for `word` read in `language` (module docs):
/// the exception lemmas of the word, then for each base the base, its
/// exception lemmas and its suffix forms, then the suffix forms of the word.
/// `lemmas` gives the exception lemmas of a form, already sorted. Never
/// `word` itself and no form twice.
pub fn forms(word: &str, language: Language, lemmas: impl Fn(&str) -> Vec<String>) -> Vec<String> {
    let mut forms = Vec::new();
    let extend = |forms: &mut Vec<String>, items: Vec<String>| {
        for form in items {
            add(forms, word, form);
        }
    };
    extend(&mut forms, lemmas(word));
    for base in bases(word, language) {
        extend(&mut forms, vec![base.clone()]);
        extend(&mut forms, lemmas(&base));
        extend(&mut forms, suffix_forms(&base, language));
    }
    extend(&mut forms, suffix_forms(word, language));
    forms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_only(word: &str, language: Language) -> Vec<String> {
        forms(word, language, |_| Vec::new())
    }

    fn texts(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn the_enclitic_stems_are_every_stem_under_the_pronouns() {
        // The most pronouns removed first: `dí-me-lo` before `díme-lo`.
        assert_eq!(
            enclitic_stems("dímelo"),
            texts(&["di", "dí", "dime", "díme"])
        );
        assert_eq!(
            bases("dímelo", Language::Es),
            texts(&["dime", "díme", "di", "dí"])
        );
        assert_eq!(enclitic_stems("dale"), texts(&["da"]));
        assert!(bases("dale", Language::Es).is_empty());
        assert_eq!(enclitic_stems("vete"), texts(&["ve"]));
        // Every base is a stem.
        for word in ["dámelo", "oírlo", "diciéndomelo", "comerlos", "mesa"] {
            let stems = enclitic_stems(word);
            for base in bases(word, Language::Es) {
                assert!(stems.contains(&base), "{word}: {base}");
            }
        }
        assert!(enclitic_stems("casa").is_empty());
    }

    #[test]
    fn the_tables_have_as_many_rules_as_rules_py() {
        assert_eq!(rules(Language::Es).len(), 227);
        assert_eq!(rules(Language::Ca).len(), 205);
    }

    #[test]
    fn the_four_words_of_the_epic_reach_their_lemmas() {
        assert!(rules_only("cerró", Language::Es).contains(&"cerrar".to_owned()));
        assert_eq!(rules_only("col·leccions", Language::Ca)[0], "col·lecció");
        assert_eq!(rules_only("d'art", Language::Ca)[0], "art");
        let dámelo = rules_only("dámelo", Language::Es);
        let at = |form: &str| dámelo.iter().position(|found| found == form);
        assert!(
            at("da").expect("da") < at("dar").expect("dar"),
            "{dámelo:?}"
        );
    }

    #[test]
    fn bases_follow_rules_py() {
        assert_eq!(bases("oírlo", Language::Es)[..2], texts(&["oir", "oír"]));
        assert!(bases("pala", Language::Es).is_empty());
        assert!(bases("bancos", Language::Es).is_empty());
        assert_eq!(
            bases("d'aigua'l", Language::Ca),
            texts(&["d'aigua", "aigua'l", "aigua"])
        );
        assert!(bases("casa", Language::Ca).is_empty());
        assert_eq!(bases("l'home", Language::Ca), texts(&["home"]));
    }

    #[test]
    fn the_catalan_bare_stem_is_added_after_a_consonant_without_an_apostrophe() {
        assert_eq!(
            suffix_forms("perd", Language::Ca),
            texts(&["perdre", "perdir", "perder"])
        );
        assert!(suffix_forms("dorm", Language::Ca).contains(&"dormir".to_owned()));
        assert!(!rules_only("porta'l", Language::Ca).contains(&"porta'lre".to_owned()));
        assert!(!suffix_forms("casa", Language::Ca).contains(&"casare".to_owned()));
        // Spanish has no bare-stem rule.
        assert!(suffix_forms("perd", Language::Es).is_empty());
    }

    #[test]
    fn restore_gives_every_accent_variant() {
        assert_eq!(
            accent_variants("rapido", Language::Es),
            texts(&["rapido", "rapido", "rapidó", "rapído", "rápido"])
        );
        let cafe = accent_variants("cafe", Language::Ca);
        assert_eq!(cafe[2..4], texts(&["cafè", "café"]));
        assert_eq!(strip_accents("àéèíóòú ü ï ñ ç"), "aeeioou ü ï ñ ç");
        assert!(rules_only("rapidísimo", Language::Es).contains(&"rápido".to_owned()));
        assert!(rules_only("rapidíssim", Language::Ca).contains(&"ràpid".to_owned()));
    }

    #[test]
    fn a_form_shorter_than_two_letters_is_dropped() {
        assert!(
            rules_only("as", Language::Es)
                .iter()
                .all(|form| letters(form) >= MIN_FORM)
        );
        // `és` is longer than `s` in letters only when counted in characters.
        assert!(suffix_forms("és", Language::Es).is_empty());
        assert!(suffix_forms("", Language::Ca).is_empty());
        assert!(rules_only("", Language::Es).is_empty());
    }

    #[test]
    fn the_middle_dot_is_an_ordinary_letter() {
        let found = suffix_forms("col·leccions", Language::Ca);
        assert!(found.contains(&"col·leccion".to_owned()));
        assert!(found.iter().all(|form| form.contains('·')));
    }

    #[test]
    fn exception_lemmas_come_first_and_are_read_for_bases() {
        let fue = forms("fue", Language::Es, |form| match form {
            "fue" => texts(&["ir", "ser"]),
            _ => Vec::new(),
        });
        assert_eq!(fue[..2], texts(&["ir", "ser"]));
        let mut unique = fue.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), fue.len());
        assert!(!fue.contains(&"fue".to_owned()));
        let havia = forms("n'havia", Language::Ca, |form| match form {
            "havia" => texts(&["haver"]),
            _ => Vec::new(),
        });
        assert_eq!(havia[..2], texts(&["havia", "haver"]));
        // A lemma equal to the word is dropped.
        assert!(!forms("casa", Language::Ca, |_| texts(&["casa"])).contains(&"casa".to_owned()));
    }
}
