//! Online translation slots on the controller's runner.

use std::sync::Arc;

use addict_core::translation::{English, Request, Source as Language, Translator, is_russian};
use addict_engine::{
    at_once,
    evidence::{Elapsed, Probe, Requested},
    online::{self, Kept, Subject},
    translate,
    view::{Choice, KeyLocation, Online, OnlineRow, online_rows},
    work::{Job, Outcome},
};

use super::{Active, ArticleId, Effect, Message, Model, RequestId, deadlines};

#[derive(Clone, Copy)]
enum Target {
    Text {
        text: u64,
        subject: Subject,
        number: u64,
    },
    Definition {
        article: ArticleId,
        choice: Choice,
        number: u64,
    },
}

impl Target {
    fn requested(self) -> Requested {
        match self {
            Self::Text { subject, .. } => subject.into(),
            Self::Definition { .. } => Requested::Definition,
        }
    }
}

pub(super) struct Translated {
    id: RequestId,
    target: Target,
    source: Language,
    outcome: Outcome<translate::Translated>,
    elapsed: Option<Elapsed>,
}

#[cfg(test)]
pub(super) fn finished_word(
    id: RequestId,
    text: u64,
    index: usize,
    number: u64,
    outcome: Outcome<translate::Translated>,
) -> Message {
    Message::Translated(Translated {
        id,
        target: Target::Text {
            text,
            subject: Subject::Word(index),
            number,
        },
        source: Language::En,
        outcome,
        elapsed: Probe::start(true).ended(),
    })
}

pub(super) struct Translations {
    translator: Arc<dyn Translator>,
    recording: bool,
    kept: Kept,
    deferred: Option<Subject>,
    word: Option<Active>,
    sentence: Option<Active>,
    definition: Option<Active>,
    pub(super) english: English,
    pub(super) at_once: bool,
}

impl Translations {
    pub(super) fn new(translator: Arc<dyn Translator>, recording: bool) -> Self {
        Self {
            translator,
            recording,
            kept: Kept::default(),
            deferred: None,
            word: None,
            sentence: None,
            definition: None,
            english: English::Us,
            at_once: at_once::DEFAULT,
        }
    }

    pub(super) fn rows(
        &self,
        chosen: Option<usize>,
        my: Language,
        key_location: KeyLocation<'_>,
    ) -> Vec<OnlineRow<'static>> {
        online_rows(&self.kept.shown(chosen), key_location, my)
            .into_iter()
            .map(OnlineRow::into_owned)
            .collect()
    }

    fn slot(&mut self, target: Target) -> &mut Option<Active> {
        match target {
            Target::Text {
                subject: Subject::Word(_) | Subject::Lone,
                ..
            } => &mut self.word,
            Target::Text {
                subject: Subject::Sentence(_),
                ..
            } => &mut self.sentence,
            Target::Definition { .. } => &mut self.definition,
        }
    }

    fn retire_text_jobs(&mut self) {
        self.word = None;
        self.sentence = None;
        self.deferred = None;
    }
}

impl Model {
    pub(super) fn reset_online(&mut self) {
        self.online.retire_text_jobs();
        self.cancel_definition();
        self.online.kept = Kept::of(self.manual.text_number(), self.manual.kind());
    }

    pub(super) fn cancel_definition(&mut self) {
        self.online.definition = None;
    }

    pub(super) fn ask_word(&mut self, index: usize, effects: &mut Vec<Effect>) {
        // A one-word sentence in an OCR result has no online request.
        let Some((number, span)) = self.manual.sentence_of(index) else {
            // A deferred word can no longer use the lookup of its own input.
            if let Some(subject) = self.online.deferred.take() {
                self.online.kept.withdraw(subject);
            }
            return;
        };
        let long = online::is_long(&self.manual.sentence()[span]);
        let click = self.online.kept.click(index, number, long);
        if click.gave_up {
            self.online.word = None;
            self.online.deferred = None;
        }
        if click.gave_up_sentence {
            self.online.sentence = None;
        }
        for subject in click.ask {
            self.start_translation(subject, effects);
        }
    }

    pub(super) fn ask_whole(&mut self, effects: &mut Vec<Effect>) {
        let long = online::is_long(self.manual.sentence());
        if let Some(subject) = self.online.kept.ask_whole(long) {
            self.start_translation(subject, effects);
        }
    }

    pub(super) fn ask_lone(&mut self, effects: &mut Vec<Effect>) {
        if self.online.kept.submit() {
            self.start_translation(Subject::Lone, effects);
        }
    }

    pub(super) fn ask_online_again(&mut self, effects: &mut Vec<Effect>) {
        self.online.retire_text_jobs();
        let whole = self.online.kept.asks_whole();
        let submitted = self.online.kept.forget();
        if whole {
            self.ask_whole(effects);
        }
        if submitted {
            self.ask_lone(effects);
        } else if let Some(index) = self.manual.chosen() {
            self.ask_word(index, effects);
        }
    }

    pub(super) fn send_deferred(&mut self, effects: &mut Vec<Effect>) {
        if let Some(subject) = self.online.deferred.take() {
            self.start_translation(subject, effects);
        }
    }

    fn start_translation(&mut self, subject: Subject, effects: &mut Vec<Effect>) {
        let Some(number) = self.online.kept.request_number(subject) else {
            return;
        };
        let input = self.manual.input();
        if input.is_some_and(|input| !translate::is_sent(input, subject, self.settings.my_language))
        {
            self.online.kept.withdraw(subject);
            return;
        }
        if !matches!(subject, Subject::Sentence(_)) && self.lookup.is_some() {
            self.online.deferred = Some(subject);
            return;
        }
        let russian = is_russian(input.map_or(self.manual.sentence(), |input| input.word()));
        let source = translate::source_of(subject, self.settings.reading(), self.answered, russian);
        let english = self.online.english;
        let request = match (input, subject) {
            (Some(input), _) => {
                translate::request(input, subject, source, self.settings.my_language, english)
            }
            (None, Subject::Sentence(_)) => translate::whole_request(
                self.manual.sentence(),
                source,
                self.settings.my_language,
                english,
            ),
            (None, Subject::Word(_) | Subject::Lone) => None,
        };
        let target = Target::Text {
            text: self.manual.text_number(),
            subject,
            number,
        };
        self.send_online(target, source, request, effects);
    }

    pub(super) fn definition_action(&mut self, effects: &mut Vec<Effect>) -> bool {
        let Some(choice) = self.shown.choice() else {
            return false;
        };
        let Some((number, request)) = self.shown.request_definition() else {
            return false;
        };
        let target = Target::Definition {
            article: self.article_id(),
            choice,
            number,
        };
        self.send_online(target, request.source, Some(request), effects);
        true
    }

    fn send_online(
        &mut self,
        target: Target,
        source: Language,
        request: Option<Request>,
        effects: &mut Vec<Effect>,
    ) {
        let id = self.next_request();
        let send = self.send.clone();
        let probe = Probe::start(self.online.recording);
        let unstarted_source = request
            .as_ref()
            .map_or(self.settings.reading(), |request| request.source);
        let started: Result<Job, Online> = translate::translate(
            Some(&self.work),
            deadlines::TRANSLATE_LIMIT,
            self.online.translator.clone(),
            request,
            move |outcome| {
                let elapsed = probe.ended();
                let _ = send.send(Message::Translated(Translated {
                    id,
                    target,
                    source,
                    outcome,
                    elapsed,
                }));
            },
        );
        match started {
            Ok(job) => *self.online.slot(target) = Some(Active { id, _job: job }),
            Err(online) => {
                if let Some(record) = translate::translation_unstarted(
                    &online,
                    target.requested(),
                    unstarted_source,
                    probe,
                ) {
                    effects.push(Effect::RecordOnline(record));
                }
                self.accept_translation(target, online);
            }
        }
    }

    pub(super) fn translated(&mut self, ended: Translated, effects: &mut Vec<Effect>) -> bool {
        let Translated {
            id,
            target,
            source,
            outcome,
            elapsed,
        } = ended;
        let Some((online, record)) =
            translate::translation_ended(outcome, target.requested(), source, elapsed)
        else {
            return false;
        };
        if let Some(record) = record {
            effects.push(Effect::RecordOnline(record));
        }
        let slot = self.online.slot(target);
        if !slot.as_ref().is_some_and(|active| active.id == id) {
            return false;
        }
        *slot = None;
        self.accept_translation(target, online)
    }

    fn accept_translation(&mut self, target: Target, online: Online) -> bool {
        match target {
            Target::Text {
                text,
                subject,
                number,
            } => self.online.kept.answer(text, subject, number, online),
            Target::Definition {
                article,
                choice,
                number,
            } => {
                article == self.article_id()
                    && self.shown.choice() == Some(choice)
                    && self.shown.answer_definition(number, online)
            }
        }
    }
}
