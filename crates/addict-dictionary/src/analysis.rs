//! The analysis of a Spanish or Catalan verb form: which person, number,
//! tense and mood it has, or that it is a past participle or a gerund.
//!
//! A Kaikki conjugation table gives each form a set of tags, and the
//! sources do not tag alike: `kaikki-es-Espanol` tags the simple preterite
//! `present perfect indicative` and the imperative `present imperative`,
//! `kaikki-en-Spanish` and `kaikki-en-Catalan` say `preterite` and give the
//! formal person as `formal`. [`from_tags`] turns every such set into one
//! [`Shape`], so the same analysis from two sources is one, and leaves out
//! the sets that are no analysis of a form of its own: the combined forms
//! with pronouns, the negative imperative (a repeat of the subjunctive),
//! the infinitive (the lemma itself), and a row without a mood or without a
//! tense (the head line of a table, which repeats its rows, and the
//! auxiliary row of the Catalan periphrastic past).
//!
//! The order of [`Shape`] is the order of the window: by tense and mood,
//! then the singular before the plural, then the person.

/// A tense and mood of a finite form, in the order of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TenseMood {
    PresentIndicative,
    PreteriteIndicative,
    ImperfectIndicative,
    FutureIndicative,
    Conditional,
    PresentSubjunctive,
    ImperfectSubjunctive,
    FutureSubjunctive,
    Imperative,
}

/// The number of a finite form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Number {
    Singular,
    Plural,
}

/// The person of a finite form, in the order of the window. `Vos` is the
/// Spanish voseo, `Formal` the second person of `usted` or `vostè`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Person {
    First,
    Second,
    Vos,
    Formal,
    Third,
}

/// What a verb form is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Shape {
    Finite {
        tense_mood: TenseMood,
        number: Number,
        person: Person,
    },
    PastParticiple,
    Gerund,
}

impl Shape {
    /// Whether a form with enclitic pronouns can have this shape: the
    /// pronouns follow only the imperative, the gerund and the infinitive.
    pub fn takes_enclitics(self) -> bool {
        matches!(
            self,
            Self::Finite {
                tense_mood: TenseMood::Imperative,
                ..
            } | Self::Gerund
        )
    }
}

/// The analyses of a form that belong to one lemma: its shapes, sorted,
/// each once.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Analyses {
    pub lemma: String,
    pub shapes: Vec<Shape>,
}

/// The shape a Kaikki tag set gives, or `None` when the set is no analysis
/// of a form of its own (module docs). Tags it does not name, such as a
/// region, `archaic`, `informal`, `impersonal` or a gender, are ignored.
pub fn from_tags(tags: &[String]) -> Option<Shape> {
    let has = |tag: &str| tags.iter().any(|own| own == tag);
    if has("combined-form") || has("negative") || has("infinitive") {
        return None;
    }
    if has("participle") {
        return Some(Shape::PastParticiple);
    }
    if has("gerund") {
        return Some(Shape::Gerund);
    }
    let tense_mood = if has("imperative") {
        TenseMood::Imperative
    } else if has("conditional") {
        TenseMood::Conditional
    } else if has("subjunctive") {
        if has("present") {
            TenseMood::PresentSubjunctive
        } else if has("imperfect") {
            TenseMood::ImperfectSubjunctive
        } else if has("future") {
            TenseMood::FutureSubjunctive
        } else {
            return None;
        }
    } else if has("indicative") {
        if has("preterite") || (has("present") && has("perfect")) {
            TenseMood::PreteriteIndicative
        } else if has("imperfect") {
            TenseMood::ImperfectIndicative
        } else if has("future") {
            TenseMood::FutureIndicative
        } else if has("present") {
            TenseMood::PresentIndicative
        } else {
            return None;
        }
    } else {
        return None;
    };
    let person = if has("formal") {
        Person::Formal
    } else if has("vos-form") {
        Person::Vos
    } else {
        match (
            has("first-person"),
            has("second-person"),
            has("third-person"),
        ) {
            (true, false, false) => Person::First,
            (false, true, false) => Person::Second,
            (false, false, true) => Person::Third,
            _ => return None,
        }
    };
    let number = match (has("singular"), has("plural")) {
        (true, false) => Number::Singular,
        (false, true) => Number::Plural,
        // Vos speaks to one person.
        (false, false) if person == Person::Vos => Number::Singular,
        _ => return None,
    };
    Some(Shape::Finite {
        tense_mood,
        number,
        person,
    })
}

/// The length of a stored shape.
pub const SHAPE_LEN: usize = 3;

/// The kind byte of a past participle and of a gerund; a finite form has
/// the place of its [`TenseMood`].
const PAST_PARTICIPLE: u8 = 9;
const GERUND: u8 = 10;

const TENSE_MOODS: [TenseMood; 9] = [
    TenseMood::PresentIndicative,
    TenseMood::PreteriteIndicative,
    TenseMood::ImperfectIndicative,
    TenseMood::FutureIndicative,
    TenseMood::Conditional,
    TenseMood::PresentSubjunctive,
    TenseMood::ImperfectSubjunctive,
    TenseMood::FutureSubjunctive,
    TenseMood::Imperative,
];

const PERSONS: [Person; 5] = [
    Person::First,
    Person::Second,
    Person::Vos,
    Person::Formal,
    Person::Third,
];

const NUMBERS: [Number; 2] = [Number::Singular, Number::Plural];

/// The stored bytes of `shape`: its kind, its person and its number, zero
/// for a form without them.
pub fn encode(shape: Shape) -> [u8; SHAPE_LEN] {
    match shape {
        Shape::Finite {
            tense_mood,
            number,
            person,
        } => [tense_mood as u8, person as u8, number as u8],
        Shape::PastParticiple => [PAST_PARTICIPLE, 0, 0],
        Shape::Gerund => [GERUND, 0, 0],
    }
}

/// The shape of its stored bytes; `None` for bytes [`encode`] never gives.
pub fn decode(bytes: [u8; SHAPE_LEN]) -> Option<Shape> {
    match bytes {
        [PAST_PARTICIPLE, 0, 0] => Some(Shape::PastParticiple),
        [GERUND, 0, 0] => Some(Shape::Gerund),
        [kind, person, number] => Some(Shape::Finite {
            tense_mood: *TENSE_MOODS.get(usize::from(kind))?,
            person: *PERSONS.get(usize::from(person))?,
            number: *NUMBERS.get(usize::from(number))?,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(tags: &[&str]) -> Option<Shape> {
        let tags: Vec<String> = tags.iter().map(|tag| (*tag).to_owned()).collect();
        from_tags(&tags)
    }

    fn finite(tense_mood: TenseMood, person: Person, number: Number) -> Option<Shape> {
        Some(Shape::Finite {
            tense_mood,
            number,
            person,
        })
    }

    use Number::{Plural, Singular};
    use Person::{First, Formal, Second, Third, Vos};
    use TenseMood::*;

    #[test]
    fn the_spanish_sources_give_one_shape_for_one_analysis() {
        // `fue`: kaikki-en-Spanish, then kaikki-es-Espanol.
        let en = shape(&["indicative", "preterite", "singular", "third-person"]);
        let es = shape(&[
            "third-person",
            "singular",
            "indicative",
            "present",
            "perfect",
        ]);
        assert_eq!(en, finite(PreteriteIndicative, Third, Singular));
        assert_eq!(es, en);
        // `hable`, imperative: present in es-Espanol, none in en-Spanish.
        assert_eq!(
            shape(&["third-person", "singular", "imperative", "present"]),
            shape(&["imperative", "singular", "third-person"])
        );
        // `hablaba`, `hablase`, `hablare`.
        assert_eq!(
            shape(&[
                "first-person",
                "singular",
                "indicative",
                "past",
                "imperfect"
            ]),
            finite(ImperfectIndicative, First, Singular)
        );
        assert_eq!(
            shape(&[
                "imperfect",
                "imperfect-se",
                "subjunctive",
                "first-person",
                "singular"
            ]),
            finite(ImperfectSubjunctive, First, Singular)
        );
        assert_eq!(
            shape(&[
                "archaic",
                "future",
                "subjunctive",
                "first-person",
                "singular"
            ]),
            finite(FutureSubjunctive, First, Singular)
        );
        assert_eq!(
            shape(&["conditional", "first-person", "singular"]),
            shape(&["conditional", "indicative", "first-person", "singular"])
        );
        assert_eq!(
            shape(&["future", "indicative", "second-person", "plural"]),
            finite(FutureIndicative, Second, Plural)
        );
    }

    #[test]
    fn the_formal_and_the_vos_person_are_their_own() {
        let usted = [
            "formal",
            "imperative",
            "second-person-semantically",
            "singular",
            "third-person",
        ];
        assert_eq!(shape(&usted), finite(Imperative, Formal, Singular));
        // kaikki-en-Catalan has no `third-person` beside `formal`.
        assert_eq!(
            shape(&[
                "formal",
                "imperative",
                "second-person-semantically",
                "plural"
            ]),
            finite(Imperative, Formal, Plural)
        );
        assert_eq!(
            shape(&[
                "indicative",
                "informal",
                "present",
                "vos-form",
                "second-person",
                "singular"
            ]),
            finite(PresentIndicative, Vos, Singular)
        );
        assert_eq!(
            shape(&["imperative", "present", "vos-form"]),
            finite(Imperative, Vos, Singular)
        );
        // `informal` is the plain second person.
        assert_eq!(
            shape(&["imperative", "informal", "second-person", "singular"]),
            finite(Imperative, Second, Singular)
        );
    }

    #[test]
    fn participles_and_gerunds_have_no_person() {
        assert_eq!(shape(&["participle", "past"]), Some(Shape::PastParticiple));
        assert_eq!(
            shape(&["masculine", "participle", "past", "singular"]),
            Some(Shape::PastParticiple)
        );
        assert_eq!(
            shape(&["impersonal", "participle"]),
            Some(Shape::PastParticiple)
        );
        assert_eq!(shape(&["impersonal", "gerund"]), Some(Shape::Gerund));
    }

    #[test]
    fn what_is_no_analysis_of_its_own_is_left_out() {
        for tags in [
            // A negative imperative, which repeats the subjunctive.
            &["imperative", "negative", "singular", "third-person"][..],
            &[
                "combined-form",
                "imperative",
                "second-person",
                "singular",
                "object-first-person",
            ],
            &["infinitive"],
            // The head line of a table, no mood.
            &["first-person", "present", "singular"],
            // The Catalan auxiliary row of `anar`, no tense.
            &["first-person", "indicative", "singular"],
            &["subjunctive", "first-person", "singular"],
            // Not a verb form.
            &["feminine"],
            &["alternative"],
            &[],
            // No person or two, no number or two.
            &["indicative", "present", "singular"],
            &[
                "indicative",
                "present",
                "first-person",
                "third-person",
                "singular",
            ],
            &["indicative", "present", "first-person"],
            &[
                "indicative",
                "present",
                "first-person",
                "singular",
                "plural",
            ],
        ] {
            assert_eq!(shape(tags), None, "{tags:?}");
        }
    }

    #[test]
    fn the_order_is_tense_and_mood_then_number_then_person() {
        let mut shapes = vec![
            Shape::Gerund,
            finite(Imperative, Third, Singular).unwrap(),
            finite(PresentSubjunctive, Third, Singular).unwrap(),
            Shape::PastParticiple,
            finite(Imperative, Formal, Singular).unwrap(),
            finite(PresentSubjunctive, First, Plural).unwrap(),
            finite(PresentSubjunctive, First, Singular).unwrap(),
        ];
        shapes.sort();
        assert_eq!(
            shapes,
            [
                finite(PresentSubjunctive, First, Singular).unwrap(),
                finite(PresentSubjunctive, Third, Singular).unwrap(),
                finite(PresentSubjunctive, First, Plural).unwrap(),
                finite(Imperative, Formal, Singular).unwrap(),
                finite(Imperative, Third, Singular).unwrap(),
                Shape::PastParticiple,
                Shape::Gerund,
            ]
        );
    }

    #[test]
    fn every_shape_is_read_back_and_other_bytes_are_not_a_shape() {
        let mut every = vec![Shape::PastParticiple, Shape::Gerund];
        for tense_mood in TENSE_MOODS {
            for person in PERSONS {
                for number in NUMBERS {
                    every.push(Shape::Finite {
                        tense_mood,
                        number,
                        person,
                    });
                }
            }
        }
        for shape in &every {
            assert_eq!(decode(encode(*shape)), Some(*shape), "{shape:?}");
        }
        let mut read = 0;
        for kind in 0..=u8::MAX {
            for person in 0..=6 {
                for number in 0..=3 {
                    read += usize::from(decode([kind, person, number]).is_some());
                }
            }
        }
        assert_eq!(read, every.len());
        assert_eq!(decode([PAST_PARTICIPLE, 1, 0]), None);
    }
}
