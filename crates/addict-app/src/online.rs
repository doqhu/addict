//! The online texts of one text of the field, without a widget, a runner or
//! a translator: what was asked for, what was answered and what is asked next.
//!
//! [`Kept`] belongs to one text of the field and is replaced with it. A click
//! on a word of a sentence asks for the word in the sentence and, once for
//! the text, for the sentence itself; a lone word is asked for only when it is
//! submitted, by Enter or by a paste. What is answered or still pending is not
//! asked for again; what was refused or given up is. A sentence longer than
//! [`SENTENCE_LIMIT`] characters is never asked for as a text. An outcome is
//! kept only when it carries the number of this text and its request is still
//! awaited here.
//!
//! Sending the requests is [`manual`](crate::manual)'s and
//! [`resident`](crate::resident)'s; the texts shown for the state are
//! [`view`](crate::view)'s.

use std::collections::BTreeMap;

use crate::{
    manual::SENTENCE_LIMIT,
    view::{Online, OnlineTexts, Whole},
};

/// What one online request is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Subject {
    /// The word with this index, with the sentence as its context.
    Word(usize),
    /// The sentence itself, without context.
    Sentence,
    /// The only word of the text, without context.
    Lone,
}

/// What the text of the field is for the online requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// No word: nothing is ever asked.
    #[default]
    Empty,
    /// Exactly one word.
    Lone,
    /// More words. `long` when the text is too long to be translated itself.
    Sentence { long: bool },
}

impl Kind {
    /// The kind of `text`, which has `words` words. The length is counted in
    /// characters.
    pub fn of(text: &str, words: usize) -> Self {
        match words {
            0 => Self::Empty,
            1 => Self::Lone,
            // Stops counting one past the limit.
            _ => Self::Sentence {
                long: text.chars().nth(SENTENCE_LIMIT).is_some(),
            },
        }
    }
}

/// What a click on a word asks for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Click {
    /// The requests to send, in this order.
    pub ask: Vec<Subject>,
    /// Whether the pending request of another word is given up.
    pub gave_up: bool,
}

/// The online texts of one text of the field.
#[derive(Debug, Default)]
pub struct Kept {
    /// The number of the text these belong to.
    text: u64,
    next_request: u64,
    pending: BTreeMap<Subject, u64>,
    kind: Kind,
    /// By the index of the word: at most one of them is pending.
    words: BTreeMap<usize, Online>,
    /// `None` until a word is clicked.
    sentence: Option<Whole>,
    /// `None` until the lone word is submitted.
    lone: Option<Online>,
}

/// Whether a request in this state is to be sent now: one that was never
/// sent or was refused is, one that is pending or answered is not.
fn is_to_send(state: Option<&Online>) -> bool {
    match state {
        Some(Online::Pending | Online::Translated(_)) => false,
        Some(Online::Refused(_)) | None => true,
    }
}

impl Kept {
    /// Nothing asked yet for the text with this number and of this kind.
    pub fn of(text: u64, kind: Kind) -> Self {
        Self {
            text,
            kind,
            ..Self::default()
        }
    }

    /// The word with `index` was clicked. In a sentence its request is sent
    /// unless it is pending or answered, and so is the request of the
    /// sentence, unless the text is too long; the pending request of another
    /// word is given up. In any other text nothing happens.
    pub fn click(&mut self, index: usize) -> Click {
        let Kind::Sentence { long } = self.kind else {
            return Click::default();
        };
        let mut click = Click::default();
        // A request given up was not answered: its word is asked for again.
        let before = self.words.len();
        self.words
            .retain(|other, state| *other == index || *state != Online::Pending);
        click.gave_up = self.words.len() != before;
        self.pending
            .retain(|subject, _| !matches!(subject, Subject::Word(other) if *other != index));
        if is_to_send(self.words.get(&index)) {
            self.words.insert(index, Online::Pending);
            click.ask.push(Subject::Word(index));
        }
        match &self.sentence {
            Some(Whole::TooLong | Whole::Asked(Online::Pending | Online::Translated(_))) => {}
            Some(Whole::Asked(Online::Refused(_))) | None if long => {
                self.sentence = Some(Whole::TooLong);
            }
            Some(Whole::Asked(Online::Refused(_))) | None => {
                self.sentence = Some(Whole::Asked(Online::Pending));
                click.ask.push(Subject::Sentence);
            }
        }
        for subject in &click.ask {
            self.begin(*subject);
        }
        click
    }

    /// The text was submitted, by Enter or by a paste. Says whether the
    /// request of a lone word is sent: only for a text of one word whose
    /// request is neither pending nor answered.
    pub fn submit(&mut self) -> bool {
        if self.kind != Kind::Lone || !is_to_send(self.lone.as_ref()) {
            return false;
        }
        self.lone = Some(Online::Pending);
        self.begin(Subject::Lone);
        true
    }

    /// Assigns a fresh identity whenever a request is started or retried.
    fn begin(&mut self, subject: Subject) {
        self.next_request += 1;
        self.pending.insert(subject, self.next_request);
    }

    /// The identity of the individual request currently awaited for a subject.
    pub fn request_number(&self, subject: Subject) -> Option<u64> {
        self.pending.get(&subject).copied()
    }

    /// A test convenience: complete the currently awaited request.
    #[cfg(test)]
    pub fn answer_current(&mut self, text: u64, subject: Subject, online: Online) -> bool {
        let number = self.request_number(subject).unwrap_or(0);
        self.answer(text, subject, number, online)
    }

    /// Keeps only the outcome of this text and this individual pending request.
    pub fn answer(&mut self, text: u64, subject: Subject, number: u64, online: Online) -> bool {
        if text != self.text || self.request_number(subject) != Some(number) {
            return false;
        }
        match subject {
            Subject::Word(index) => match self.words.get_mut(&index) {
                Some(state @ Online::Pending) => *state = online,
                _ => return false,
            },
            Subject::Sentence => match &mut self.sentence {
                Some(Whole::Asked(state @ Online::Pending)) => *state = online,
                _ => return false,
            },
            Subject::Lone => match &mut self.lone {
                Some(state @ Online::Pending) => *state = online,
                _ => return false,
            },
        }
        self.pending.remove(&subject);
        true
    }

    /// What the window shows while the word with the index `chosen` is the
    /// chosen one.
    pub fn shown(&self, chosen: Option<usize>) -> OnlineTexts<'_> {
        OnlineTexts {
            word: chosen.and_then(|index| self.words.get(&index)),
            sentence: self.sentence.as_ref(),
            lone: self.lone.as_ref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use addict_core::translation::Refusal;

    use super::{Click, Kept, Kind, Subject};
    use crate::{
        manual::SENTENCE_LIMIT,
        view::{Online, OnlineTexts, Whole},
    };

    /// The number of the text of these tests.
    const TEXT: u64 = 7;

    /// The indices of `bank` and `river` in `I sat on the bank of the river.`
    const BANK: usize = 4;
    const RIVER: usize = 7;

    const PENDING: Online = Online::Pending;
    const REFUSED: Online = Online::Refused(Refusal::Other);

    fn sentence() -> Kept {
        Kept::of(TEXT, Kind::Sentence { long: false })
    }

    fn translated(text: &str) -> Online {
        Online::Translated(text.to_owned())
    }

    fn asks(ask: &[Subject]) -> Click {
        Click {
            ask: ask.to_vec(),
            gave_up: false,
        }
    }

    /// `bank` clicked, and both requests answered.
    fn answered() -> Kept {
        let mut kept = sentence();
        kept.click(BANK);
        assert!(kept.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert!(kept.answer_current(TEXT, Subject::Sentence, translated("Я сидел.")));
        kept
    }

    #[test]
    fn the_kind_of_a_text_follows_its_words_and_its_length() {
        assert_eq!(Kind::of("", 0), Kind::Empty);
        assert_eq!(Kind::of(" 12 ", 0), Kind::Empty);
        assert_eq!(Kind::of(" bank. ", 1), Kind::Lone);
        assert_eq!(Kind::of("a bank", 2), Kind::Sentence { long: false });
        // Characters, not bytes: 500 Cyrillic letters are 1000 bytes.
        let most = format!("a {}", "я".repeat(SENTENCE_LIMIT - 2));
        assert_eq!(most.chars().count(), 500);
        assert_eq!(Kind::of(&most, 2), Kind::Sentence { long: false });
        let over = format!("{most}я");
        assert_eq!(over.chars().count(), 501);
        assert_eq!(Kind::of(&over, 2), Kind::Sentence { long: true });
        // One long word is a lone word, whatever its length.
        assert_eq!(Kind::of(&"a".repeat(600), 1), Kind::Lone);
    }

    #[test]
    fn nothing_is_shown_before_a_click() {
        let kept = sentence();
        assert_eq!(kept.shown(None), OnlineTexts::default());
    }

    #[test]
    fn the_first_click_asks_for_the_word_and_then_for_the_sentence() {
        let mut kept = sentence();
        assert_eq!(
            kept.click(BANK),
            asks(&[Subject::Word(BANK), Subject::Sentence])
        );
        let pending = Whole::Asked(PENDING);
        assert_eq!(
            kept.shown(Some(BANK)),
            OnlineTexts {
                word: Some(&PENDING),
                sentence: Some(&pending),
                lone: None,
            }
        );
        assert!(kept.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert!(kept.answer_current(TEXT, Subject::Sentence, REFUSED));
        let refused = Whole::Asked(REFUSED);
        assert_eq!(
            kept.shown(Some(BANK)),
            OnlineTexts {
                word: Some(&translated("берег")),
                sentence: Some(&refused),
                lone: None,
            }
        );
    }

    #[test]
    fn the_same_word_again_asks_for_nothing_pending_or_answered() {
        let mut kept = sentence();
        kept.click(BANK);
        assert_eq!(kept.click(BANK), Click::default());
        // Still awaited: the answers of the first click are kept.
        assert!(kept.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert!(kept.answer_current(TEXT, Subject::Sentence, translated("Я сидел.")));
        assert_eq!(kept.click(BANK), Click::default());
        assert_eq!(kept.shown(Some(BANK)).word, Some(&translated("берег")));
    }

    #[test]
    fn another_word_asks_for_itself_only_and_gives_the_pending_one_up() {
        let mut kept = sentence();
        kept.click(BANK);
        assert!(kept.answer_current(TEXT, Subject::Sentence, translated("Я сидел.")));
        assert_eq!(
            kept.click(RIVER),
            Click {
                ask: vec![Subject::Word(RIVER)],
                gave_up: true,
            }
        );
        let whole = Whole::Asked(translated("Я сидел."));
        assert_eq!(
            kept.shown(Some(RIVER)),
            OnlineTexts {
                word: Some(&PENDING),
                sentence: Some(&whole),
                lone: None,
            }
        );
        // Whatever the request given up still reports is not kept.
        assert!(!kept.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert_eq!(kept.shown(Some(BANK)).word, None);
    }

    #[test]
    fn another_word_keeps_a_pending_sentence_pending() {
        let mut kept = sentence();
        kept.click(BANK);
        assert_eq!(kept.click(RIVER).ask, [Subject::Word(RIVER)]);
        assert!(kept.answer_current(TEXT, Subject::Sentence, translated("Я сидел.")));
    }

    #[test]
    fn back_to_an_answered_word_asks_for_nothing_and_shows_what_was_kept() {
        let mut kept = answered();
        assert_eq!(kept.click(RIVER), asks(&[Subject::Word(RIVER)]));
        assert!(kept.answer_current(TEXT, Subject::Word(RIVER), translated("реки")));
        assert_eq!(kept.click(BANK), Click::default());
        assert_eq!(kept.shown(Some(BANK)).word, Some(&translated("берег")));
        assert_eq!(kept.click(RIVER), Click::default());
        assert_eq!(kept.shown(Some(RIVER)).word, Some(&translated("реки")));
    }

    #[test]
    fn back_to_an_answered_word_gives_the_pending_one_up() {
        let mut kept = answered();
        kept.click(RIVER);
        assert_eq!(
            kept.click(BANK),
            Click {
                ask: Vec::new(),
                gave_up: true,
            }
        );
        // `river` was not answered: it is asked for again.
        assert_eq!(
            kept.click(RIVER),
            Click {
                ask: vec![Subject::Word(RIVER)],
                gave_up: false,
            }
        );
    }

    #[test]
    fn a_word_given_up_is_asked_for_again() {
        let mut kept = sentence();
        kept.click(BANK);
        kept.click(RIVER);
        assert_eq!(
            kept.click(BANK),
            Click {
                ask: vec![Subject::Word(BANK)],
                gave_up: true,
            }
        );
    }

    #[test]
    fn a_refused_request_is_asked_for_again_and_an_answered_one_is_not() {
        // The word refused, the sentence answered.
        let mut kept = sentence();
        kept.click(BANK);
        assert!(kept.answer_current(TEXT, Subject::Word(BANK), REFUSED));
        assert!(kept.answer_current(TEXT, Subject::Sentence, translated("Я сидел.")));
        assert_eq!(kept.shown(Some(BANK)).word, Some(&REFUSED));
        assert_eq!(kept.click(BANK), asks(&[Subject::Word(BANK)]));
        assert_eq!(kept.shown(Some(BANK)).word, Some(&PENDING));

        // The sentence refused, the word answered: any word click asks again.
        let mut kept = sentence();
        kept.click(BANK);
        assert!(kept.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert!(kept.answer_current(TEXT, Subject::Sentence, REFUSED));
        assert_eq!(kept.click(BANK), asks(&[Subject::Sentence]));
        assert!(kept.answer_current(TEXT, Subject::Sentence, REFUSED));
        assert_eq!(
            kept.click(RIVER),
            asks(&[Subject::Word(RIVER), Subject::Sentence])
        );
    }

    #[test]
    fn a_long_text_asks_for_the_word_only_and_says_so_with_nothing_to_retry() {
        let mut kept = Kept::of(TEXT, Kind::Sentence { long: true });
        assert_eq!(kept.shown(None).sentence, None);
        assert_eq!(kept.click(BANK), asks(&[Subject::Word(BANK)]));
        assert_eq!(kept.shown(Some(BANK)).sentence, Some(&Whole::TooLong));
        assert!(kept.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert_eq!(kept.click(BANK), Click::default());
        assert_eq!(kept.click(RIVER), asks(&[Subject::Word(RIVER)]));
        assert_eq!(kept.shown(Some(RIVER)).sentence, Some(&Whole::TooLong));
        // No request of the sentence is awaited.
        assert!(!kept.answer_current(TEXT, Subject::Sentence, translated("Я сидел.")));
    }

    #[test]
    fn the_outcome_of_another_text_is_not_kept() {
        let mut kept = sentence();
        kept.click(BANK);
        for subject in [Subject::Word(BANK), Subject::Sentence] {
            assert!(!kept.answer_current(TEXT - 1, subject, translated("старый")));
            assert!(!kept.answer_current(TEXT + 1, subject, translated("чужой")));
        }
        assert_eq!(kept.shown(Some(BANK)).word, Some(&PENDING));

        // A new text starts with nothing, and ignores what the old one asked.
        let mut next = Kept::of(TEXT + 1, Kind::Sentence { long: false });
        assert_eq!(next.shown(Some(BANK)), OnlineTexts::default());
        assert!(!next.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        // Even when the same word is pending in it.
        next.click(BANK);
        assert!(!next.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert!(!next.answer_current(TEXT, Subject::Sentence, translated("Я сидел.")));
        assert_eq!(next.shown(Some(BANK)).word, Some(&PENDING));
    }

    #[test]
    fn an_outcome_that_is_not_awaited_is_not_kept() {
        let mut kept = sentence();
        for subject in [Subject::Word(BANK), Subject::Sentence, Subject::Lone] {
            assert!(!kept.answer_current(TEXT, subject, translated("ничей")));
        }
        assert_eq!(kept.shown(Some(BANK)), OnlineTexts::default());
        // A second outcome for an answered request changes nothing.
        let mut kept = answered();
        assert!(!kept.answer_current(TEXT, Subject::Word(BANK), REFUSED));
        assert!(!kept.answer_current(TEXT, Subject::Sentence, REFUSED));
        assert_eq!(kept.shown(Some(BANK)).word, Some(&translated("берег")));
    }

    #[test]
    fn a_lone_word_is_asked_for_by_a_submit_and_never_by_a_click() {
        let mut kept = Kept::of(TEXT, Kind::Lone);
        // Typed, and clicked.
        assert_eq!(kept.click(0), Click::default());
        assert_eq!(kept.shown(Some(0)), OnlineTexts::default());
        assert!(kept.submit());
        assert_eq!(
            kept.shown(Some(0)),
            OnlineTexts {
                word: None,
                sentence: None,
                lone: Some(&PENDING),
            }
        );
        // Pending, and answered.
        assert!(!kept.submit());
        assert!(kept.answer_current(TEXT, Subject::Lone, translated("банк")));
        assert!(!kept.submit());
        assert_eq!(kept.click(0), Click::default());
        assert_eq!(
            kept.shown(Some(0)),
            OnlineTexts {
                word: None,
                sentence: None,
                lone: Some(&translated("банк")),
            }
        );
    }

    #[test]
    fn a_refused_lone_word_is_asked_for_again_by_the_next_submit() {
        let mut kept = Kept::of(TEXT, Kind::Lone);
        assert!(kept.submit());
        assert!(kept.answer_current(TEXT, Subject::Lone, REFUSED));
        assert_eq!(kept.shown(Some(0)).lone, Some(&REFUSED));
        assert!(kept.submit());
        assert!(!kept.answer_current(TEXT + 1, Subject::Lone, translated("банк")));
        assert!(kept.answer_current(TEXT, Subject::Lone, translated("банк")));
    }

    #[test]
    fn a_submit_of_an_empty_field_or_a_sentence_asks_for_nothing() {
        for kind in [
            Kind::Empty,
            Kind::Sentence { long: false },
            Kind::Sentence { long: true },
        ] {
            let mut kept = Kept::of(TEXT, kind);
            assert!(!kept.submit(), "{kind:?}");
            assert_eq!(kept.shown(None), OnlineTexts::default(), "{kind:?}");
        }
        // And changes nothing of what a click asked for.
        let mut kept = answered();
        assert!(!kept.submit());
        assert_eq!(kept.shown(Some(BANK)).word, Some(&translated("берег")));
        assert_eq!(kept.shown(Some(BANK)).lone, None);
    }

    #[test]
    fn a_click_in_an_empty_text_asks_for_nothing() {
        let mut kept = Kept::of(TEXT, Kind::Empty);
        assert_eq!(kept.click(0), Click::default());
        assert_eq!(kept.shown(Some(0)), OnlineTexts::default());
    }
    #[test]
    fn returning_to_a_pending_word_rejects_its_previous_request() {
        let mut kept = sentence();
        kept.click(BANK);
        let old = kept
            .request_number(Subject::Word(BANK))
            .expect("old request");
        let whole = kept
            .request_number(Subject::Sentence)
            .expect("sentence request");
        kept.click(RIVER);
        kept.click(BANK);
        let new = kept
            .request_number(Subject::Word(BANK))
            .expect("new request");
        assert_ne!(old, new);
        assert!(!kept.answer(TEXT, Subject::Word(BANK), old, translated("старый")));
        assert_eq!(kept.shown(Some(BANK)).word, Some(&PENDING));
        assert!(kept.answer(TEXT, Subject::Sentence, whole, translated("Я сидел.")));
        assert!(kept.answer(TEXT, Subject::Word(BANK), new, translated("берег")));
    }

    #[test]
    fn retrying_a_failed_subject_rejects_the_previous_outcome() {
        for subject in [Subject::Sentence, Subject::Lone] {
            let mut kept = if subject == Subject::Lone {
                Kept::of(TEXT, Kind::Lone)
            } else {
                sentence()
            };
            if subject == Subject::Lone {
                kept.submit();
            } else {
                kept.click(BANK);
            }
            let old = kept.request_number(subject).expect("request");
            assert!(kept.answer(TEXT, subject, old, REFUSED));
            if subject == Subject::Lone {
                kept.submit();
            } else {
                kept.click(BANK);
            }
            let new = kept.request_number(subject).expect("retry");
            assert_ne!(old, new);
            assert!(!kept.answer(TEXT, subject, old, translated("старый")));
            assert!(kept.answer(TEXT, subject, new, translated("новый")));
        }
    }
}
