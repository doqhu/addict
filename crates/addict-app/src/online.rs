//! The online texts of one text of the field, without a widget, a runner or
//! a translator: what was asked for, what was answered and what is asked next.
//!
//! [`Kept`] belongs to one text of the field and is replaced with it. A click
//! on a word of a sentence asks for the word in its sentence and, once for
//! that sentence, for the sentence itself; a lone word is asked for only when
//! it is submitted, by Enter or by a paste. A typed text is one sentence; a
//! recognized one may hold several, each with its number, and a click in
//! another sentence gives up the pending request of the one before, as a
//! click on another word does for the word. What is answered or still
//! pending is not asked for again; what was refused or given up is. A
//! sentence longer than [`SENTENCE_LIMIT`] characters is never asked for as
//! a text. An outcome is kept only when it carries the number of this text
//! and its request is still awaited here.
//!
//! Sending the requests is [`translate`](crate::translate)'s and
//! [`resident`](crate::resident)'s; the texts shown for the state are
//! [`view`](crate::view)'s.

use std::collections::BTreeMap;

use crate::{
    translate::SENTENCE_LIMIT,
    view::{Online, OnlineTexts, Whole},
};

/// What one online request is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Subject {
    /// The word with this index, with the sentence as its context.
    Word(usize),
    /// The sentence with this number itself, without context.
    Sentence(usize),
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
    /// More words. Whether a sentence is too long to be translated itself is
    /// judged on the clicked one, by [`is_long`].
    Sentence,
}

impl Kind {
    /// The kind of a text that has `words` words.
    pub fn of(words: usize) -> Self {
        match words {
            0 => Self::Empty,
            1 => Self::Lone,
            _ => Self::Sentence,
        }
    }
}

/// Whether `sentence` is too long to be translated itself: longer than
/// [`SENTENCE_LIMIT`] characters.
pub fn is_long(sentence: &str) -> bool {
    // Stops counting one past the limit.
    sentence.chars().nth(SENTENCE_LIMIT).is_some()
}

/// What a click on a word asks for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Click {
    /// The requests to send, in this order.
    pub ask: Vec<Subject>,
    /// Whether the pending request of another word is given up.
    pub gave_up: bool,
    /// Whether the pending request of another sentence is given up.
    pub gave_up_sentence: bool,
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
    /// By the number of the sentence: at most one of them is pending.
    sentences: BTreeMap<usize, Whole>,
    /// The number of the sentence of each clicked word, by its index.
    sentence_of: BTreeMap<usize, usize>,
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

    /// The word with `index` of the sentence numbered `sentence` was
    /// clicked; `long` says whether that sentence is too long to be sent. In
    /// a text of more words its request is sent unless it is pending or
    /// answered, and so is the request of its sentence, unless the sentence
    /// is too long; the pending request of another word, and that of another
    /// sentence, is given up. In any other text nothing happens.
    pub fn click(&mut self, index: usize, sentence: usize, long: bool) -> Click {
        let Kind::Sentence = self.kind else {
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
        self.sentence_of.insert(index, sentence);
        // The same for the sentence: one given up is asked for again.
        let before = self.sentences.len();
        self.sentences
            .retain(|other, state| *other == sentence || *state != Whole::Asked(Online::Pending));
        click.gave_up_sentence = self.sentences.len() != before;
        self.pending.retain(
            |subject, _| !matches!(subject, Subject::Sentence(other) if *other != sentence),
        );
        match self.sentences.get(&sentence) {
            Some(Whole::TooLong | Whole::Asked(Online::Pending | Online::Translated(_))) => {}
            Some(Whole::Asked(Online::Refused(_))) | None if long => {
                self.sentences.insert(sentence, Whole::TooLong);
            }
            Some(Whole::Asked(Online::Refused(_))) | None => {
                self.sentences
                    .insert(sentence, Whole::Asked(Online::Pending));
                click.ask.push(Subject::Sentence(sentence));
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

    /// The reading language changed: every online text of this text is
    /// forgotten, and what was asked is asked anew by the next click or
    /// submit. The numbers of the requests go on, so the outcome of one sent
    /// before is not kept. Whether the lone word had been submitted, so that
    /// it is sent again.
    pub fn forget(&mut self) -> bool {
        let submitted = self.lone.is_some();
        *self = Self {
            text: self.text,
            kind: self.kind,
            next_request: self.next_request,
            ..Self::default()
        };
        submitted
    }

    /// The request about `subject`, just asked for, is not sent after all:
    /// it is no longer awaited, and the word, the lone word or the sentence
    /// has no online text, as if it had never been asked. A sentence whose
    /// answer is already kept keeps it.
    pub fn withdraw(&mut self, subject: Subject) {
        match subject {
            Subject::Word(index) => {
                self.words.remove(&index);
            }
            Subject::Lone => self.lone = None,
            Subject::Sentence(number) => {
                if self.sentences.get(&number) == Some(&Whole::Asked(Online::Pending)) {
                    self.sentences.remove(&number);
                }
            }
        }
        self.pending.remove(&subject);
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
            Subject::Sentence(number) => match self.sentences.get_mut(&number) {
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
    /// chosen one: its own texts and those of its sentence.
    pub fn shown(&self, chosen: Option<usize>) -> OnlineTexts<'_> {
        OnlineTexts {
            word: chosen.and_then(|index| self.words.get(&index)),
            sentence: chosen
                .and_then(|index| self.sentence_of.get(&index))
                .and_then(|number| self.sentences.get(number)),
            lone: self.lone.as_ref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use addict_core::translation::Refusal;

    use super::{Click, Kept, Kind, Subject, is_long};
    use crate::{
        translate::SENTENCE_LIMIT,
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
        Kept::of(TEXT, Kind::Sentence)
    }

    #[test]
    fn a_new_language_forgets_every_text_and_keeps_no_old_answer() {
        let mut kept = sentence();
        assert_eq!(
            kept.click(BANK, 0, false).ask,
            [Subject::Word(BANK), Subject::Sentence(0)]
        );
        let old = kept.request_number(Subject::Word(BANK)).expect("pending");
        assert!(!kept.forget());
        assert_eq!(kept.shown(Some(BANK)), OnlineTexts::default());
        // The same click asks for both again.
        assert_eq!(
            kept.click(BANK, 0, false).ask,
            [Subject::Word(BANK), Subject::Sentence(0)]
        );
        let new = kept.request_number(Subject::Word(BANK)).expect("pending");
        assert_ne!(old, new);
        assert!(!kept.answer(TEXT, Subject::Word(BANK), old, translated("берег")));
        assert!(kept.answer(TEXT, Subject::Word(BANK), new, translated("банк")));

        let mut lone = Kept::of(TEXT, Kind::Lone);
        assert!(!lone.forget());
        assert!(lone.submit());
        assert!(lone.forget());
        assert_eq!(lone.shown(None), OnlineTexts::default());
        assert!(lone.submit());
    }

    #[test]
    fn a_withdrawn_request_is_as_if_never_asked_and_an_answer_stays() {
        let mut kept = sentence();
        kept.click(BANK, 0, false);
        kept.withdraw(Subject::Word(BANK));
        kept.withdraw(Subject::Sentence(0));
        assert_eq!(kept.request_number(Subject::Word(BANK)), None);
        assert_eq!(kept.request_number(Subject::Sentence(0)), None);
        assert_eq!(kept.shown(Some(BANK)), OnlineTexts::default());
        // Asked again by the next click.
        assert_eq!(
            kept.click(BANK, 0, false).ask,
            [Subject::Word(BANK), Subject::Sentence(0)]
        );
        // A sentence already answered keeps its answer.
        assert!(kept.answer_current(TEXT, Subject::Sentence(0), translated("Я сидел.")));
        kept.withdraw(Subject::Sentence(0));
        assert_eq!(
            kept.shown(Some(BANK)).sentence,
            Some(&Whole::Asked(translated("Я сидел.")))
        );

        let mut lone = Kept::of(TEXT, Kind::Lone);
        assert!(lone.submit());
        lone.withdraw(Subject::Lone);
        assert_eq!(lone.shown(None), OnlineTexts::default());
        assert!(lone.submit());
    }

    fn translated(text: &str) -> Online {
        Online::Translated(text.to_owned())
    }

    fn asks(ask: &[Subject]) -> Click {
        Click {
            ask: ask.to_vec(),
            ..Click::default()
        }
    }

    /// `bank` clicked, and both requests answered.
    fn answered() -> Kept {
        let mut kept = sentence();
        kept.click(BANK, 0, false);
        assert!(kept.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert!(kept.answer_current(TEXT, Subject::Sentence(0), translated("Я сидел.")));
        kept
    }

    #[test]
    fn the_kind_of_a_text_follows_its_words() {
        assert_eq!(Kind::of(0), Kind::Empty);
        assert_eq!(Kind::of(1), Kind::Lone);
        assert_eq!(Kind::of(2), Kind::Sentence);
        assert_eq!(Kind::of(40), Kind::Sentence);
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
            kept.click(BANK, 0, false),
            asks(&[Subject::Word(BANK), Subject::Sentence(0)])
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
        assert!(kept.answer_current(TEXT, Subject::Sentence(0), REFUSED));
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
        kept.click(BANK, 0, false);
        assert_eq!(kept.click(BANK, 0, false), Click::default());
        // Still awaited: the answers of the first click are kept.
        assert!(kept.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert!(kept.answer_current(TEXT, Subject::Sentence(0), translated("Я сидел.")));
        assert_eq!(kept.click(BANK, 0, false), Click::default());
        assert_eq!(kept.shown(Some(BANK)).word, Some(&translated("берег")));
    }

    #[test]
    fn another_word_asks_for_itself_only_and_gives_the_pending_one_up() {
        let mut kept = sentence();
        kept.click(BANK, 0, false);
        assert!(kept.answer_current(TEXT, Subject::Sentence(0), translated("Я сидел.")));
        assert_eq!(
            kept.click(RIVER, 0, false),
            Click {
                ask: vec![Subject::Word(RIVER)],
                gave_up: true,
                gave_up_sentence: false,
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
        kept.click(BANK, 0, false);
        assert_eq!(kept.click(RIVER, 0, false).ask, [Subject::Word(RIVER)]);
        assert!(kept.answer_current(TEXT, Subject::Sentence(0), translated("Я сидел.")));
    }

    #[test]
    fn back_to_an_answered_word_asks_for_nothing_and_shows_what_was_kept() {
        let mut kept = answered();
        assert_eq!(kept.click(RIVER, 0, false), asks(&[Subject::Word(RIVER)]));
        assert!(kept.answer_current(TEXT, Subject::Word(RIVER), translated("реки")));
        assert_eq!(kept.click(BANK, 0, false), Click::default());
        assert_eq!(kept.shown(Some(BANK)).word, Some(&translated("берег")));
        assert_eq!(kept.click(RIVER, 0, false), Click::default());
        assert_eq!(kept.shown(Some(RIVER)).word, Some(&translated("реки")));
    }

    #[test]
    fn back_to_an_answered_word_gives_the_pending_one_up() {
        let mut kept = answered();
        kept.click(RIVER, 0, false);
        assert_eq!(
            kept.click(BANK, 0, false),
            Click {
                ask: Vec::new(),
                gave_up: true,
                gave_up_sentence: false,
            }
        );
        // `river` was not answered: it is asked for again.
        assert_eq!(
            kept.click(RIVER, 0, false),
            Click {
                ask: vec![Subject::Word(RIVER)],
                gave_up: false,
                gave_up_sentence: false,
            }
        );
    }

    #[test]
    fn a_word_given_up_is_asked_for_again() {
        let mut kept = sentence();
        kept.click(BANK, 0, false);
        kept.click(RIVER, 0, false);
        assert_eq!(
            kept.click(BANK, 0, false),
            Click {
                ask: vec![Subject::Word(BANK)],
                gave_up: true,
                gave_up_sentence: false,
            }
        );
    }

    #[test]
    fn a_refused_request_is_asked_for_again_and_an_answered_one_is_not() {
        // The word refused, the sentence answered.
        let mut kept = sentence();
        kept.click(BANK, 0, false);
        assert!(kept.answer_current(TEXT, Subject::Word(BANK), REFUSED));
        assert!(kept.answer_current(TEXT, Subject::Sentence(0), translated("Я сидел.")));
        assert_eq!(kept.shown(Some(BANK)).word, Some(&REFUSED));
        assert_eq!(kept.click(BANK, 0, false), asks(&[Subject::Word(BANK)]));
        assert_eq!(kept.shown(Some(BANK)).word, Some(&PENDING));

        // The sentence refused, the word answered: any word click asks again.
        let mut kept = sentence();
        kept.click(BANK, 0, false);
        assert!(kept.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert!(kept.answer_current(TEXT, Subject::Sentence(0), REFUSED));
        assert_eq!(kept.click(BANK, 0, false), asks(&[Subject::Sentence(0)]));
        assert!(kept.answer_current(TEXT, Subject::Sentence(0), REFUSED));
        assert_eq!(
            kept.click(RIVER, 0, false),
            asks(&[Subject::Word(RIVER), Subject::Sentence(0)])
        );
    }

    #[test]
    fn a_long_text_asks_for_the_word_only_and_says_so_with_nothing_to_retry() {
        let mut kept = Kept::of(TEXT, Kind::Sentence);
        assert_eq!(kept.shown(None).sentence, None);
        assert_eq!(kept.click(BANK, 0, true), asks(&[Subject::Word(BANK)]));
        assert_eq!(kept.shown(Some(BANK)).sentence, Some(&Whole::TooLong));
        assert!(kept.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert_eq!(kept.click(BANK, 0, true), Click::default());
        assert_eq!(kept.click(RIVER, 0, true), asks(&[Subject::Word(RIVER)]));
        assert_eq!(kept.shown(Some(RIVER)).sentence, Some(&Whole::TooLong));
        // No request of the sentence is awaited.
        assert!(!kept.answer_current(TEXT, Subject::Sentence(0), translated("Я сидел.")));
    }

    #[test]
    fn the_outcome_of_another_text_is_not_kept() {
        let mut kept = sentence();
        kept.click(BANK, 0, false);
        for subject in [Subject::Word(BANK), Subject::Sentence(0)] {
            assert!(!kept.answer_current(TEXT - 1, subject, translated("старый")));
            assert!(!kept.answer_current(TEXT + 1, subject, translated("чужой")));
        }
        assert_eq!(kept.shown(Some(BANK)).word, Some(&PENDING));

        // A new text starts with nothing, and ignores what the old one asked.
        let mut next = Kept::of(TEXT + 1, Kind::Sentence);
        assert_eq!(next.shown(Some(BANK)), OnlineTexts::default());
        assert!(!next.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        // Even when the same word is pending in it.
        next.click(BANK, 0, false);
        assert!(!next.answer_current(TEXT, Subject::Word(BANK), translated("берег")));
        assert!(!next.answer_current(TEXT, Subject::Sentence(0), translated("Я сидел.")));
        assert_eq!(next.shown(Some(BANK)).word, Some(&PENDING));
    }

    #[test]
    fn an_outcome_that_is_not_awaited_is_not_kept() {
        let mut kept = sentence();
        for subject in [Subject::Word(BANK), Subject::Sentence(0), Subject::Lone] {
            assert!(!kept.answer_current(TEXT, subject, translated("ничей")));
        }
        assert_eq!(kept.shown(Some(BANK)), OnlineTexts::default());
        // A second outcome for an answered request changes nothing.
        let mut kept = answered();
        assert!(!kept.answer_current(TEXT, Subject::Word(BANK), REFUSED));
        assert!(!kept.answer_current(TEXT, Subject::Sentence(0), REFUSED));
        assert_eq!(kept.shown(Some(BANK)).word, Some(&translated("берег")));
    }

    #[test]
    fn a_lone_word_is_asked_for_by_a_submit_and_never_by_a_click() {
        let mut kept = Kept::of(TEXT, Kind::Lone);
        // Typed, and clicked.
        assert_eq!(kept.click(0, 0, false), Click::default());
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
        assert_eq!(kept.click(0, 0, false), Click::default());
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
        for kind in [Kind::Empty, Kind::Sentence] {
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
        assert_eq!(kept.click(0, 0, false), Click::default());
        assert_eq!(kept.shown(Some(0)), OnlineTexts::default());
    }
    #[test]
    fn returning_to_a_pending_word_rejects_its_previous_request() {
        let mut kept = sentence();
        kept.click(BANK, 0, false);
        let old = kept
            .request_number(Subject::Word(BANK))
            .expect("old request");
        let whole = kept
            .request_number(Subject::Sentence(0))
            .expect("sentence request");
        kept.click(RIVER, 0, false);
        kept.click(BANK, 0, false);
        let new = kept
            .request_number(Subject::Word(BANK))
            .expect("new request");
        assert_ne!(old, new);
        assert!(!kept.answer(TEXT, Subject::Word(BANK), old, translated("старый")));
        assert_eq!(kept.shown(Some(BANK)).word, Some(&PENDING));
        assert!(kept.answer(TEXT, Subject::Sentence(0), whole, translated("Я сидел.")));
        assert!(kept.answer(TEXT, Subject::Word(BANK), new, translated("берег")));
    }

    #[test]
    fn retrying_a_failed_subject_rejects_the_previous_outcome() {
        for subject in [Subject::Sentence(0), Subject::Lone] {
            let mut kept = if subject == Subject::Lone {
                Kept::of(TEXT, Kind::Lone)
            } else {
                sentence()
            };
            if subject == Subject::Lone {
                kept.submit();
            } else {
                kept.click(BANK, 0, false);
            }
            let old = kept.request_number(subject).expect("request");
            assert!(kept.answer(TEXT, subject, old, REFUSED));
            if subject == Subject::Lone {
                kept.submit();
            } else {
                kept.click(BANK, 0, false);
            }
            let new = kept.request_number(subject).expect("retry");
            assert_ne!(old, new);
            assert!(!kept.answer(TEXT, subject, old, translated("старый")));
            assert!(kept.answer(TEXT, subject, new, translated("новый")));
        }
    }

    /// Words of a recognized text of two sentences: `bank` and `river` in
    /// the first, `boat` and `oar` in the second.
    const BOAT: usize = 10;
    const OAR: usize = 12;

    #[test]
    fn a_click_in_another_sentence_asks_for_that_sentence() {
        let mut kept = answered();
        assert_eq!(
            kept.click(BOAT, 1, false),
            asks(&[Subject::Word(BOAT), Subject::Sentence(1)])
        );
        assert_eq!(
            kept.shown(Some(BOAT)).sentence,
            Some(&Whole::Asked(PENDING))
        );
        assert!(kept.answer_current(TEXT, Subject::Word(BOAT), translated("лодка")));
        assert!(kept.answer_current(TEXT, Subject::Sentence(1), translated("Лодка.")));
        // Another word of the second sentence asks for itself only.
        assert_eq!(kept.click(OAR, 1, false), asks(&[Subject::Word(OAR)]));
        assert_eq!(
            kept.shown(Some(OAR)).sentence,
            Some(&Whole::Asked(translated("Лодка.")))
        );
    }

    #[test]
    fn back_to_an_answered_sentence_asks_for_nothing_and_shows_it() {
        let mut kept = answered();
        kept.click(BOAT, 1, false);
        assert!(kept.answer_current(TEXT, Subject::Word(BOAT), translated("лодка")));
        assert!(kept.answer_current(TEXT, Subject::Sentence(1), translated("Лодка.")));
        assert_eq!(kept.click(BANK, 0, false), Click::default());
        assert_eq!(
            kept.shown(Some(BANK)).sentence,
            Some(&Whole::Asked(translated("Я сидел.")))
        );
        assert_eq!(kept.click(RIVER, 0, false), asks(&[Subject::Word(RIVER)]));
        assert_eq!(
            kept.shown(Some(RIVER)).sentence,
            Some(&Whole::Asked(translated("Я сидел.")))
        );
    }

    #[test]
    fn a_click_in_another_sentence_gives_the_pending_sentence_up() {
        let mut kept = sentence();
        kept.click(BANK, 0, false);
        let first = kept
            .request_number(Subject::Sentence(0))
            .expect("first sentence");
        assert_eq!(
            kept.click(BOAT, 1, false),
            Click {
                ask: vec![Subject::Word(BOAT), Subject::Sentence(1)],
                gave_up: true,
                gave_up_sentence: true,
            }
        );
        // Whatever the request given up still reports is not kept.
        assert!(!kept.answer(TEXT, Subject::Sentence(0), first, translated("Я сидел.")));
        assert_eq!(kept.shown(Some(BANK)).sentence, None);
        // Back in the first sentence: it is asked for again, and the second,
        // still pending, is given up.
        assert_eq!(
            kept.click(BANK, 0, false),
            Click {
                ask: vec![Subject::Word(BANK), Subject::Sentence(0)],
                gave_up: true,
                gave_up_sentence: true,
            }
        );
        assert_ne!(kept.request_number(Subject::Sentence(0)), Some(first));
        assert_eq!(kept.request_number(Subject::Sentence(1)), None);
    }

    #[test]
    fn whether_a_sentence_is_too_long_is_its_own() {
        let mut kept = sentence();
        assert_eq!(kept.click(BANK, 0, true), asks(&[Subject::Word(BANK)]));
        assert_eq!(kept.shown(Some(BANK)).sentence, Some(&Whole::TooLong));
        assert_eq!(
            kept.click(BOAT, 1, false),
            Click {
                ask: vec![Subject::Word(BOAT), Subject::Sentence(1)],
                gave_up: true,
                gave_up_sentence: false,
            }
        );
        assert_eq!(
            kept.shown(Some(BOAT)).sentence,
            Some(&Whole::Asked(PENDING))
        );
        assert_eq!(kept.shown(Some(BANK)).sentence, Some(&Whole::TooLong));
    }

    #[test]
    fn a_word_never_clicked_shows_no_sentence() {
        let kept = answered();
        assert_eq!(kept.shown(Some(RIVER)).sentence, None);
        assert_eq!(kept.shown(None).sentence, None);
    }

    #[test]
    fn a_sentence_is_long_past_the_limit_in_characters() {
        // Characters, not bytes: 500 Cyrillic letters are 1000 bytes.
        let most = format!("a {}", "я".repeat(SENTENCE_LIMIT - 2));
        assert_eq!(most.chars().count(), 500);
        assert!(!is_long(&most));
        let over = format!("{most}я");
        assert_eq!(over.chars().count(), 501);
        assert!(is_long(&over));
        assert!(!is_long("a bank"));
    }
}
