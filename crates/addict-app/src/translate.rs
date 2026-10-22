//! The online request without its widgets: what is sent about a word, a
//! sentence or a definition, the job that sends it and what its outcome is.
//!
//! [`request`] builds the one online request about a [`Subject`]: the word
//! with its sentence as context, the sentence itself, or a lone word, in the
//! language [`source_of`] says: the sentence in the reading language the
//! window has chosen, the word in the language that answered its lookup;
//! [`translate`] starts a request as a job of its own on the work runner,
//! with its own deadline, and [`translation_result`] says what the window
//! shows for its outcome. [`translation_ended`] and [`translation_unstarted`]
//! say what the evidence records of a request, before anything decides
//! whether its answer is shown. Which requests are sent, and when, is
//! [`online`](crate::online)'s; the text of the field and the chosen word are
//! [`manual`](crate::manual)'s. Putting any of it on the screen is
//! [`resident`](crate::resident)'s. Nothing is sent for a Russian word
//! ([`is_sent`]), neither the word nor its sentence: it is read offline only.

use std::{sync::Arc, time::Duration};

use addict_core::{
    input::Input,
    translation::{Refusal, Request, Source, Translator, is_russian},
};
use addict_platform::work::{Job, Outcome, Work};

use crate::{
    evidence::{self, Elapsed, Probe, Record, Requested},
    online::Subject,
    view::Online,
};

/// How long one online request may take. A network answer is slower than a
/// file read, and this deadline is the only thing that ends a request
/// without an answer.
const TRANSLATE_LIMIT: Duration = Duration::from_secs(10);

/// The most characters, not bytes, the text of the field may have for its
/// sentence to be sent for translation. A longer text is still the context
/// of its clicked word.
pub const SENTENCE_LIMIT: usize = 500;

/// What an online translation ends with.
pub type Translated = Result<String, Refusal>;

/// The language the online request about `subject` is sent from, for a
/// word read in the `chosen` language whose lookup was `answered` by a
/// language, or by none (it did not finish, or none ran). The sentence is
/// sent from the chosen language; the word, alone or in its sentence, from
/// the language that answered, and from the chosen one when none did. No
/// wildcard arm: a new subject does not compile until it is given its
/// language.
pub fn source_of(subject: Subject, chosen: Source, answered: Option<Source>) -> Source {
    match subject {
        Subject::Sentence(_) => chosen,
        Subject::Word(_) | Subject::Lone => answered.unwrap_or(chosen),
    }
}

/// Whether the online request about `subject` for `input` is sent at all:
/// not when the chosen word is Russian, whether the request is about the
/// word, the lone word or its sentence. A Russian word is read offline only.
/// No wildcard arm: a new subject does not compile until it is given its
/// answer.
pub fn is_sent(input: &Input, subject: Subject) -> bool {
    match subject {
        Subject::Sentence(_) | Subject::Word(_) | Subject::Lone => !is_russian(input.word()),
    }
}

/// The online request about `subject` for `input`, read in `source`, with
/// nothing in it but what is named here. For a word of a sentence: the word,
/// with the sentence as context. For the sentence: the sentence, without
/// context. For a lone word: the word, without context. `None` when the
/// input is not of that kind: a lone word has no sentence, and a word of a
/// sentence is not sent without it.
pub fn request(input: &Input, subject: Subject, source: Source) -> Option<Request> {
    let sentence = input.sentence().map(str::to_owned);
    match subject {
        Subject::Word(_) => Some(Request {
            text: input.word().to_owned(),
            context: Some(sentence?),
            source,
        }),
        Subject::Sentence(_) => Some(Request {
            text: sentence?,
            context: None,
            source,
        }),
        Subject::Lone => sentence.is_none().then(|| Request {
            text: input.word().to_owned(),
            context: None,
            source,
        }),
    }
}

/// Starts the online `request` on `work`, with a deadline; every online
/// request starts here. `on_outcome` runs once on the worker thread. Without
/// a request or without a runner nothing starts and the state to show is
/// given back.
pub fn translate(
    work: Option<&Work>,
    translator: Arc<dyn Translator>,
    request: Option<Request>,
    on_outcome: impl FnOnce(Outcome<Translated>) + Send + 'static,
) -> Result<Job, Online> {
    translate_within(TRANSLATE_LIMIT, work, translator, request, on_outcome)
}

/// [`translate`] with the deadline `limit`.
fn translate_within(
    limit: Duration,
    work: Option<&Work>,
    translator: Arc<dyn Translator>,
    request: Option<Request>,
    on_outcome: impl FnOnce(Outcome<Translated>) + Send + 'static,
) -> Result<Job, Online> {
    let (Some(work), Some(request)) = (work, request) else {
        return Err(Online::Refused(Refusal::Other));
    };
    Ok(work.run(
        limit,
        async move { translator.translate(request).await },
        on_outcome,
    ))
}

/// What the window shows for the `outcome` of an online translation, or
/// `None` when it shows nothing new: a cancelled request is always one that
/// was replaced or dropped. No wildcard arm: a new outcome does not compile
/// until it is given its state.
pub fn translation_result(outcome: Outcome<Translated>) -> Option<Online> {
    match outcome {
        Outcome::Finished(Ok(text)) => Some(Online::Translated(text)),
        Outcome::Finished(Err(refusal)) => Some(Online::Refused(refusal)),
        Outcome::TimedOut => Some(Online::Refused(Refusal::TimedOut)),
        Outcome::Panicked => Some(Online::Refused(Refusal::Other)),
        Outcome::Cancelled => None,
    }
}

/// What the window shows for the `outcome` of an online request for
/// `requested`, sent from `source`, with the record of it for the evidence,
/// or `None` for a
/// request that was given up: it shows nothing and is not recorded. The
/// record is there whenever the request was timed, which gives `elapsed`,
/// whether or not the answer is still awaited: that is asked after this.
/// Call it on the UI thread: making a record reads the memory of the
/// process.
pub fn translation_ended(
    outcome: Outcome<Translated>,
    requested: Requested,
    source: Source,
    elapsed: Option<Elapsed>,
) -> Option<(Online, Option<Record>)> {
    let online = translation_result(outcome)?;
    let record = evidence::online_record(requested, source, &online, elapsed);
    Some((online, record))
}

/// The record of a request for `requested` from `source` that [`translate`]
/// did not start and gave back as `online`: no time passed. None when
/// `probe` measures nothing. As [`translation_ended`], for the UI thread.
pub fn translation_unstarted(
    online: &Online,
    requested: Requested,
    source: Source,
    probe: Probe,
) -> Option<Record> {
    evidence::online_record(requested, source, online, probe.not_started())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        sync::{Arc, mpsc},
        thread,
        time::Duration,
    };

    use addict_core::{
        dictionary::{Dictionary, Sample},
        translation::{Canned, Refusal, Request, Source as Language, Translating, Translator},
    };
    use addict_deepl::{
        DeepL,
        stub::{Reply, Stub, translation_body},
    };
    use addict_platform::work::{Outcome, Work};

    use super::{
        SENTENCE_LIMIT, TRANSLATE_LIMIT, Translated, is_sent, request, source_of, translate,
        translate_within, translation_ended, translation_result, translation_unstarted,
    };
    use crate::{
        dictionaries::{Looked, tests::Sampled},
        evidence::{Elapsed, Probe, Requested},
        manual::{
            Edit, LOOKUP_LIMIT, Manual, look_up, lookup_result,
            tests::{WAIT, is_shown, lone, with_sentence},
        },
        online::{Kept, Subject, is_long},
        view::{Lookup, Online, OnlineTexts, Whole},
    };

    #[test]
    fn definition_worker_sends_only_explicit_actions_and_allows_guarded_recovery() {
        use crate::view::Shown;
        let work = Work::start().expect("runner");
        let failure = Arc::new(Canned::new(Err(Refusal::Network)));
        let success = Arc::new(Canned::new(Ok("берег")));
        let article = Sample.lookup("bank").expect("article");
        let definition = article.senses[0].definition.clone();
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(article.clone())));
        shown.choose(0);
        shown.choose(0);
        assert!(failure.asked().is_empty());
        assert!(success.asked().is_empty());
        let run = |request, translator: Arc<dyn Translator>| {
            let (sender, receiver) = mpsc::channel();
            let caller = thread::current().id();
            let job = translate(Some(&work), translator, Some(request), move |outcome| {
                sender
                    .send((outcome, caller != thread::current().id()))
                    .expect("receiver");
            })
            .expect("job");
            let (outcome, elsewhere) = receiver.recv_timeout(WAIT).expect("outcome");
            assert!(elsewhere);
            drop(job);
            translation_result(outcome).expect("answer")
        };
        let (first, request) = shown.request_definition().expect("action");
        assert!(shown.request_definition().is_none());
        assert!(shown.answer_definition(first, run(request, failure.clone())));
        assert_eq!(
            failure.asked(),
            [Request {
                text: definition.clone(),
                context: None,
                source: Language::En,
            }]
        );
        let (retry, request) = shown.request_definition().expect("retry action");
        assert!(!shown.answer_definition(first, Online::Translated("old".to_owned())));
        assert!(shown.answer_definition(retry, run(request, success.clone())));
        assert_eq!(
            success.asked(),
            [Request {
                text: definition,
                context: None,
                source: Language::En,
            }]
        );
        assert!(shown.request_definition().is_none());
        assert_eq!(
            shown.definition(),
            Some(&Online::Translated("берег".to_owned()))
        );

        shown.show(Some(Lookup::Article(article.clone())));
        shown.choose(0);
        let (old, request) = shown.request_definition().expect("action");
        let old_answer = run(request, success.clone());
        shown.show(Some(Lookup::Article(article)));
        shown.choose(0);
        let (current, _) = shown.request_definition().expect("new action");
        assert!(!shown.answer_definition(old, old_answer));
        // Cancelled outcomes never attach; timeout and panic use shared refusals.
        assert_eq!(translation_result(Outcome::Cancelled), None);
        assert_eq!(shown.definition(), Some(&Online::Pending));
        assert!(shown.answer_definition(
            current,
            translation_result(Outcome::TimedOut).expect("timeout")
        ));
        let (next, _) = shown.request_definition().expect("explicit retry");
        assert!(
            shown.answer_definition(next, translation_result(Outcome::Panicked).expect("panic"))
        );
        shown.show(None);
        assert!(!shown.answer_definition(next, Online::Translated("stale".to_owned())));
    }

    #[test]
    fn changing_sense_cancels_a_definition_worker_and_returning_requires_a_new_action() {
        use crate::view::Shown;
        let work = Work::start().expect("runner");
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(
            Sample.lookup("bank").expect("article"),
        )));
        shown.choose(0);
        let (old, request) = shown.request_definition().expect("action");
        let (sender, receiver) = mpsc::channel();
        let job = translate(Some(&work), Arc::new(Mute), Some(request), move |outcome| {
            sender.send(outcome).expect("receiver");
        })
        .expect("job");
        assert!(shown.choose(1));
        drop(job);
        assert!(shown.choose(0));
        assert_eq!(shown.definition(), None);
        let (new, _) = shown.request_definition().expect("fresh action");
        assert_ne!(old, new);
        let outcome = receiver.recv_timeout(WAIT).expect("cancelled outcome");
        assert_eq!(outcome, Outcome::Cancelled);
        assert_eq!(translation_result(outcome), None);
        assert!(!shown.answer_definition(old, Online::Translated("late".to_owned())));
        assert_eq!(shown.definition(), Some(&Online::Pending));
    }

    pub(crate) const RIVER: &str = "I sat on the bank of the river.";

    /// `bank` chosen in [`RIVER`].
    fn bank_of_the_river() -> Manual {
        let mut manual = with_sentence(RIVER);
        manual.choose(4).expect("run");
        assert_eq!(manual.input().expect("input").word(), "bank");
        manual
    }

    fn bank_request() -> Request {
        Request {
            text: "bank".to_owned(),
            context: Some(RIVER.to_owned()),
            source: Language::En,
        }
    }

    /// The request about the chosen word of `manual` in its sentence.
    fn word_request(manual: &Manual) -> Request {
        let chosen = manual.chosen().expect("chosen");
        request(
            manual.input().expect("input"),
            Subject::Word(chosen),
            Language::En,
        )
        .expect("a sentence")
    }

    /// The outcome of the online translation of the chosen word of `manual`
    /// in its sentence that ran on a real runner within `limit`, and whether its callback ran on
    /// another thread than this one.
    fn translated(
        translator: Arc<dyn Translator>,
        manual: &Manual,
        limit: Duration,
    ) -> (Outcome<Translated>, bool) {
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let caller = thread::current().id();
        let job = translate_within(
            limit,
            Some(&work),
            translator,
            Some(word_request(manual)),
            move |outcome| {
                let _ = sender.send((outcome, thread::current().id() != caller));
            },
        )
        .expect("job");
        let reported = receiver.recv_timeout(WAIT).expect("outcome");
        drop(job);
        reported
    }

    /// A translator that never answers.
    struct Mute;

    impl Translator for Mute {
        fn translate(&self, _request: Request) -> Translating<'_> {
            Box::pin(std::future::pending())
        }
    }

    /// Sends `request` to `translator` on `work` as the resident does, with
    /// `probe` ended on the worker, and gives the outcome with the time taken
    /// there. `give_up` drops the job at once.
    fn probed(
        work: &Work,
        translator: Arc<dyn Translator>,
        request: Request,
        probe: Probe,
        give_up: bool,
    ) -> (Outcome<Translated>, Option<Elapsed>) {
        let (sender, receiver) = mpsc::channel();
        let job = translate(Some(work), translator, Some(request), move |outcome| {
            let _ = sender.send((outcome, probe.ended()));
        })
        .expect("job");
        if give_up {
            drop(job);
        }
        receiver.recv_timeout(WAIT).expect("outcome")
    }

    fn bank() -> Request {
        Request {
            text: "bank".to_owned(),
            context: None,
            source: Language::En,
        }
    }

    #[test]
    fn a_request_that_could_not_start_is_recorded_as_other_with_no_delay() {
        let work = Work::start().expect("runner");
        let canned = Arc::new(Canned::new(Ok("берег")));
        let probe = Probe::start(true);
        // No runner, and then no request.
        let unstarted = [
            (
                translate(None, canned.clone(), Some(bank()), |_| {}),
                Requested::Word,
                "word",
            ),
            (
                translate(Some(&work), canned.clone(), None, |_| {}),
                Requested::Definition,
                "definition",
            ),
        ];
        for (started, requested, subject) in unstarted {
            let online = started.expect_err("nothing started");
            let record =
                translation_unstarted(&online, requested, Language::En, probe).expect("a record");
            assert_eq!(record.event(), "online");
            let details = record.details();
            assert_eq!(details["subject"], subject);
            assert_eq!(details["outcome"], "other");
            assert_eq!(details["elapsed_ms"], 0);
            assert!(details["peak_rss_bytes"].is_u64() && details["pss_bytes"].is_u64());
        }
        assert!(canned.asked().is_empty());
    }

    #[test]
    fn a_request_given_up_writes_no_record() {
        let work = Work::start().expect("runner");
        let probe = Probe::start(true);
        let (outcome, elapsed) = probed(&work, Arc::new(Mute), bank(), probe, true);
        assert_eq!(outcome, Outcome::Cancelled);
        // The worker only took the time; nothing is shown and no record is
        // made of it, so no memory is read for it either.
        assert!(elapsed.is_some());
        assert_eq!(
            translation_ended(outcome, Requested::Word, Language::En, elapsed),
            None
        );
        // The same request, left to end, is recorded.
        let canned = Arc::new(Canned::new(Ok("берег")));
        let (outcome, elapsed) = probed(&work, canned, bank(), probe, false);
        let (_, record) =
            translation_ended(outcome, Requested::Word, Language::En, elapsed).expect("ended");
        assert!(record.is_some());
    }

    #[test]
    fn an_answer_that_is_no_longer_shown_is_recorded_like_one_that_is() {
        use crate::view::Shown;
        let work = Work::start().expect("runner");
        let article = Sample.lookup("bank").expect("article");
        let mut records = Vec::new();
        for stale in [false, true] {
            let canned = Arc::new(Canned::new(Ok("берег")));
            let mut shown = Shown::default();
            shown.show(Some(Lookup::Article(article.clone())));
            shown.choose(0);
            let (number, request) = shown.request_definition().expect("action");
            let (outcome, elapsed) = probed(&work, canned, request, Probe::start(true), false);
            if stale {
                // Another result is shown before the answer is handled; the
                // job was not cancelled.
                shown.show(None);
            }
            let (online, record) =
                translation_ended(outcome, Requested::Definition, Language::En, elapsed)
                    .expect("ended");
            // The record exists before the window is asked whether it still
            // awaits the answer, and whatever it says.
            let record = record.expect("a record");
            assert_eq!(online, Online::Translated("берег".to_owned()));
            assert_eq!(shown.answer_definition(number, online), !stale);
            assert_eq!(record.event(), "online");
            let details = record.details();
            assert!(details["elapsed_ms"].is_u64());
            records.push((details["subject"].clone(), details["outcome"].clone()));
        }
        assert_eq!(records[0], ("definition".into(), "translated".into()));
        assert_eq!(records[1], records[0]);
    }

    #[test]
    fn a_definition_of_another_language_is_sent_and_recorded_from_it() {
        use crate::view::Shown;
        use addict_core::dictionary::Sense;
        use addict_dictionary::lookup::Grouped;
        let work = Work::start().expect("runner");
        let grouped = Grouped {
            definitions: Some(vec![Sense {
                part_of_speech: None,
                entries: Vec::new(),
                definition: "Asiento largo.".to_owned(),
                synonyms: Vec::new(),
            }]),
            english: Some(vec!["bench".to_owned()]),
            ..Grouped::default()
        };
        for (language, gloss, text, code) in [
            (Language::Es, false, "Asiento largo.", "es"),
            (Language::Ca, false, "Asiento largo.", "ca"),
            (Language::Es, true, "bench", "en"),
        ] {
            let canned = Arc::new(Canned::new(Ok("скамья")));
            let mut shown = Shown::default();
            shown.show(Some(Lookup::Grouped {
                language,
                grouped: grouped.clone(),
            }));
            assert!(if gloss {
                shown.choose_gloss(0)
            } else {
                shown.choose(0)
            });
            let (number, request) = shown.request_definition().expect("action");
            let source = request.source;
            let (outcome, elapsed) =
                probed(&work, canned.clone(), request, Probe::start(true), false);
            assert_eq!(
                canned.asked(),
                [Request {
                    text: text.to_owned(),
                    context: None,
                    source,
                }]
            );
            let (online, record) =
                translation_ended(outcome, Requested::Definition, source, elapsed).expect("ended");
            let details = record.expect("a record").details().clone();
            assert_eq!(details["subject"], "definition");
            assert_eq!(details["source"], code);
            assert!(shown.answer_definition(number, online));
            assert_eq!(
                shown.definition(),
                Some(&Online::Translated("скамья".to_owned()))
            );
        }
    }

    #[test]
    fn without_the_flag_nothing_is_measured_and_nothing_is_recorded() {
        let work = Work::start().expect("runner");
        let probe = Probe::start(false);
        // No clock was read at the start.
        assert_eq!(probe.started(), None);
        // A request that ended: no time was taken on the worker and no
        // file of /proc was read, and the answer is shown without a record.
        let canned = Arc::new(Canned::new(Ok("берег")));
        let (outcome, elapsed) = probed(&work, canned, bank(), probe, false);
        assert_eq!(elapsed, None);
        assert_eq!(
            translation_ended(outcome, Requested::Word, Language::En, elapsed),
            Some((Online::Translated("берег".to_owned()), None))
        );
        // A refusal, and a request that could not start.
        let refusing = Arc::new(Canned::new(Err(Refusal::Quota)));
        let (outcome, elapsed) = probed(&work, refusing.clone(), bank(), probe, false);
        assert_eq!(elapsed, None);
        assert_eq!(
            translation_ended(outcome, Requested::Sentence, Language::En, elapsed),
            Some((Online::Refused(Refusal::Quota), None))
        );
        let online = translate(None, refusing, Some(bank()), |_| {}).expect_err("nothing started");
        assert_eq!(probe.not_started(), None);
        assert_eq!(
            translation_unstarted(&online, Requested::Word, Language::En, probe),
            None
        );
    }

    /// A translator whose every call panics.
    struct Faulty;

    impl Translator for Faulty {
        fn translate(&self, _request: Request) -> Translating<'_> {
            Box::pin(async { panic!("the faulty test translator") })
        }
    }

    #[test]
    fn a_word_of_a_sentence_is_sent_with_its_sentence_and_nothing_else() {
        let translator = Arc::new(Canned::new(Ok("берег")));
        let (outcome, elsewhere) = translated(translator.clone(), &bank_of_the_river(), WAIT);
        assert!(elsewhere);
        assert_eq!(translator.asked(), [bank_request()]);
        assert_eq!(
            translation_result(outcome),
            Some(Online::Translated("берег".to_owned()))
        );
    }

    fn sentence_request() -> Request {
        Request {
            text: RIVER.to_owned(),
            context: None,
            source: Language::En,
        }
    }

    fn river_request() -> Request {
        Request {
            text: "river".to_owned(),
            context: Some(RIVER.to_owned()),
            source: Language::En,
        }
    }

    fn lone_request(word: &str) -> Request {
        Request {
            text: word.to_owned(),
            context: None,
            source: Language::En,
        }
    }

    /// The indices of `bank` and `river` in [`RIVER`].
    pub(crate) const BANK: usize = 4;
    pub(crate) const RIVER_WORD: usize = 7;

    /// A text of exactly [`SENTENCE_LIMIT`] characters that starts with
    /// [`RIVER`]; its bytes are more.
    pub(crate) fn text_at_the_limit() -> String {
        let more = SENTENCE_LIMIT - RIVER.chars().count();
        let text = format!("{RIVER}{}{}", " я".repeat(more / 2), "!".repeat(more % 2));
        assert_eq!(text.chars().count(), SENTENCE_LIMIT);
        assert!(text.len() > SENTENCE_LIMIT);
        text
    }

    #[test]
    fn each_subject_has_its_one_request_and_nothing_else_in_it() {
        let manual = bank_of_the_river();
        let input = manual.input().expect("input");
        assert_eq!(
            request(input, Subject::Word(BANK), Language::En),
            Some(bank_request())
        );
        assert_eq!(
            request(input, Subject::Sentence(0), Language::En),
            Some(sentence_request())
        );
        // A word of a sentence is not sent without it.
        assert_eq!(request(input, Subject::Lone, Language::En), None);

        let mut manual = Manual::default();
        lone(&mut manual, " Bank. ");
        let input = manual.input().expect("input");
        assert_eq!(
            request(input, Subject::Lone, Language::En),
            Some(lone_request("Bank"))
        );
        // A lone word has no sentence to send, as text or as context.
        assert_eq!(request(input, Subject::Word(0), Language::En), None);
        assert_eq!(request(input, Subject::Sentence(0), Language::En), None);
    }

    /// The manual path as the resident drives it, without a window: an edit
    /// starts the online texts of the new text anew, a click and a submit
    /// send what [`Kept`] asks for through [`request`] and [`translate`], on
    /// a real runner, and every outcome is given back to [`Kept`] before the
    /// next step.
    struct Desk {
        manual: Manual,
        kept: Kept,
        work: Work,
        translator: Arc<Canned>,
        /// The reading language.
        language: Language,
    }

    impl Desk {
        fn new(answer: Result<&str, Refusal>) -> Self {
            Self {
                manual: Manual::default(),
                kept: Kept::default(),
                work: Work::start().expect("runner"),
                translator: Arc::new(Canned::new(answer)),
                language: Language::En,
            }
        }

        fn with_text(answer: Result<&str, Refusal>, text: &str) -> Self {
            let mut desk = Self::new(answer);
            desk.edit(text);
            desk
        }

        /// The field holds `text`, typed or put there.
        fn edit(&mut self, text: &str) {
            if self.manual.edit(text.to_owned()) != Edit::Same {
                self.kept = Kept::of(self.manual.text_number(), self.manual.kind());
            }
        }

        /// A paste: the old text is removed, the new one is put and
        /// submitted.
        fn paste(&mut self, text: &str) {
            self.edit("");
            self.edit(text);
            self.submit();
        }

        /// A click on the word with `index`, as the resident handles it.
        fn click(&mut self, index: usize) {
            self.manual.choose(index).expect("run");
            let Some((number, span)) = self.manual.sentence_of(index) else {
                return;
            };
            let long = is_long(&self.manual.sentence()[span]);
            for subject in self.kept.click(index, number, long).ask {
                self.send(subject);
            }
        }

        /// Enter in the field.
        fn submit(&mut self) {
            if self.kept.submit() {
                self.send(Subject::Lone);
            }
        }

        fn send(&mut self, subject: Subject) {
            if let Some(input) = self.manual.input()
                && !is_sent(input, subject)
            {
                self.kept.withdraw(subject);
                return;
            }
            let (sender, receiver) = mpsc::channel();
            let asked = self
                .manual
                .input()
                .and_then(|input| request(input, subject, self.language));
            let started = translate(
                Some(&self.work),
                self.translator.clone(),
                asked,
                move |outcome| {
                    let _ = sender.send(outcome);
                },
            );
            let online = match started {
                Ok(_job) => translation_result(receiver.recv_timeout(WAIT).expect("outcome"))
                    .expect("not cancelled"),
                Err(online) => online,
            };
            assert!(
                self.kept
                    .answer_current(self.manual.text_number(), subject, online)
            );
        }

        fn asked(&self) -> Vec<Request> {
            self.translator.asked()
        }

        /// The switch is set to `language`, as the resident handles it: the
        /// online texts are forgotten and the chosen word, or the submitted
        /// lone word, is asked for again.
        fn switch(&mut self, language: Language) {
            if language == self.language {
                return;
            }
            self.language = language;
            let submitted = self.kept.forget();
            let chosen = self
                .manual
                .chosen()
                .and_then(|index| self.manual.sentence_of(index).map(|of| (index, of)));
            if let Some((index, (number, span))) = chosen {
                let long = is_long(&self.manual.sentence()[span]);
                for subject in self.kept.click(index, number, long).ask {
                    self.send(subject);
                }
            } else if submitted && self.kept.submit() {
                self.send(Subject::Lone);
            }
        }

        fn shown(&self) -> OnlineTexts<'_> {
            self.kept.shown(self.manual.chosen())
        }
    }

    fn online(text: &str) -> Online {
        Online::Translated(text.to_owned())
    }

    /// The Spanish sentence of the matrix, and `banco` in it.
    const BANCO_SENTENCE: &str = "El banco cerró a las tres.";
    const BANCO: usize = 1;

    #[test]
    fn nothing_is_sent_for_a_russian_word_nor_for_its_sentence() {
        for language in Language::ALL {
            // Alone, by Enter or by a paste.
            let mut desk = Desk::with_text(Ok("x"), "собака");
            desk.language = language;
            desk.submit();
            desk.paste("«Дом»,");
            assert!(desk.asked().is_empty(), "{language:?}");
            assert_eq!(desk.shown().lone, None);
            // In a sentence: neither the word nor the sentence, as if never
            // asked.
            let sentence = "Моя собака спит.";
            let mut desk = Desk::with_text(Ok("x"), sentence);
            desk.language = language;
            desk.click(1);
            assert!(desk.asked().is_empty(), "{language:?}");
            assert_eq!(desk.shown().word, None);
            assert_eq!(desk.shown().sentence, None);
            // Clicked again, and after a switch: still nothing.
            desk.click(1);
            desk.switch(if language == Language::En {
                Language::Es
            } else {
                Language::En
            });
            desk.click(0);
            assert!(desk.asked().is_empty(), "{language:?}");
            assert_eq!(desk.shown().sentence, None);
        }
        // A Latin word is sent as always, with its sentence, also in a
        // Russian sentence; a Russian word clicked after it leaves the
        // sentence's kept answer as it is and sends nothing more.
        let mut desk = Desk::with_text(Ok("банк"), "Мой bank.");
        desk.click(1);
        assert_eq!(desk.asked().len(), 2);
        assert_eq!(desk.asked()[0].text, "bank");
        assert_eq!(desk.asked()[1].text, "Мой bank.");
        desk.click(0);
        assert_eq!(desk.asked().len(), 2);
        assert_eq!(desk.shown().word, None);
        assert_eq!(desk.shown().sentence, Some(&Whole::Asked(online("банк"))));
        let input =
            addict_core::input::Input::new("собака", None, addict_core::input::Source::Manual)
                .expect("input");
        assert!(!is_sent(&input, Subject::Lone));
        assert!(!is_sent(&input, Subject::Word(0)));
        assert!(!is_sent(&input, Subject::Sentence(0)));
        let input =
            addict_core::input::Input::new("bank", None, addict_core::input::Source::Manual)
                .expect("input");
        assert!(is_sent(&input, Subject::Lone));
        assert!(is_sent(&input, Subject::Sentence(0)));
    }

    #[test]
    fn a_spanish_word_is_sent_from_es_with_its_sentence() {
        let mut desk = Desk::with_text(Ok("банк"), BANCO_SENTENCE);
        desk.language = Language::Es;
        desk.click(BANCO);
        assert_eq!(
            desk.asked(),
            [
                Request {
                    text: "banco".to_owned(),
                    context: Some(BANCO_SENTENCE.to_owned()),
                    source: Language::Es,
                },
                Request {
                    text: BANCO_SENTENCE.to_owned(),
                    context: None,
                    source: Language::Es,
                },
            ]
        );
        assert_eq!(desk.shown().word, Some(&online("банк")));
    }

    #[test]
    fn a_catalan_sentence_is_sent_from_ca_without_context() {
        let text = "La col·lecció d'art és gran.";
        let mut desk = Desk::with_text(Ok("коллекция"), text);
        desk.language = Language::Ca;
        // `col·lecció` is one word, the second.
        assert_eq!(&text[desk.manual.words()[1].clone()], "col·lecció");
        desk.click(1);
        assert_eq!(
            desk.asked()[1],
            Request {
                text: text.to_owned(),
                context: None,
                source: Language::Ca,
            }
        );
        assert_eq!(desk.asked()[0].text, "col·lecció");
        assert_eq!(desk.asked()[0].source, Language::Ca);
    }

    #[test]
    fn changing_the_language_sends_the_shown_word_and_its_sentence_again() {
        let mut desk = Desk::with_text(Ok("банк"), BANCO_SENTENCE);
        desk.click(BANCO);
        assert_eq!(desk.asked().len(), 2);
        assert!(
            desk.asked()
                .iter()
                .all(|asked| asked.source == Language::En)
        );
        desk.switch(Language::Es);
        assert_eq!(
            desk.asked()[2..],
            [
                Request {
                    text: "banco".to_owned(),
                    context: Some(BANCO_SENTENCE.to_owned()),
                    source: Language::Es,
                },
                Request {
                    text: BANCO_SENTENCE.to_owned(),
                    context: None,
                    source: Language::Es,
                },
            ]
        );
        // The same language again sends nothing.
        desk.switch(Language::Es);
        assert_eq!(desk.asked().len(), 4);
        // A submitted lone word is sent again; one that was not, is not.
        let mut lone = Desk::with_text(Ok("банк"), "banco");
        lone.switch(Language::Es);
        assert!(lone.asked().is_empty());
        lone.submit();
        lone.switch(Language::Ca);
        assert_eq!(
            lone.asked(),
            [
                Request {
                    text: "banco".to_owned(),
                    context: None,
                    source: Language::Es,
                },
                Request {
                    text: "banco".to_owned(),
                    context: None,
                    source: Language::Ca,
                },
            ]
        );
    }

    #[test]
    fn changing_the_language_without_a_word_sends_nothing() {
        let mut desk = Desk::with_text(Ok("банк"), BANCO_SENTENCE);
        desk.switch(Language::Es);
        desk.switch(Language::Ca);
        assert!(desk.asked().is_empty());
        let mut empty = Desk::new(Ok("банк"));
        empty.switch(Language::Es);
        assert!(empty.asked().is_empty());
    }

    #[test]
    fn a_request_is_recorded_with_its_language() {
        let work = Work::start().expect("runner");
        for (language, code) in [
            (Language::En, "en"),
            (Language::Es, "es"),
            (Language::Ca, "ca"),
        ] {
            let canned = Arc::new(Canned::new(Ok("банк")));
            let asked = Request {
                source: language,
                ..bank()
            };
            let (outcome, elapsed) = probed(&work, canned, asked, Probe::start(true), false);
            let (_, record) =
                translation_ended(outcome, Requested::Word, language, elapsed).expect("ended");
            assert_eq!(record.expect("a record").details()["source"], code);
            let online =
                translate(None, Arc::new(Mute), Some(bank()), |_| {}).expect_err("unstarted");
            let record =
                translation_unstarted(&online, Requested::Sentence, language, Probe::start(true))
                    .expect("a record");
            assert_eq!(record.details()["source"], code);
        }
    }

    #[test]
    fn the_first_click_sends_the_word_with_its_sentence_and_the_sentence() {
        let mut desk = Desk::with_text(Ok("берег"), RIVER);
        desk.click(BANK);
        assert_eq!(desk.asked(), [bank_request(), sentence_request()]);
        let whole = Whole::Asked(online("берег"));
        assert_eq!(
            desk.shown(),
            OnlineTexts {
                word: Some(&online("берег")),
                sentence: Some(&whole),
                lone: None,
            }
        );
    }

    /// The text of 21-27-22 of EXP-8 at 2×, psm 6, as Tesseract gave it.
    const PITFALL: &str = "When we talk about problem size, we have to be careful about which size, or sizes, we\nare talking about. This example demonstrates a pitfall of algorithm analysis: the\ntempting shortcut of counting loops. If there is one loop, the algorithm is often linear.\nIf there are two loops (one nested inside the other), the algorithm is often quadratic.\nBut be careful! You have to think about how many times each loop runs. If the num-\nber of iterations is proportional to n for all loops, you can get away with just counting\nthe loops. But if, as in this example, the number of iterations is not always propor-\ntional to n, you have to give it more thought.";

    #[test]
    fn a_word_of_a_recognized_text_sends_its_sentence_only() {
        let mut desk = Desk::new(Ok("число"));
        let text = addict_core::sentence::recognized(PITFALL).expect("text");
        assert_ne!(desk.manual.recognized(text), Edit::Same);
        desk.kept = Kept::of(desk.manual.text_number(), desk.manual.kind());
        // The field is over the limit; the sentence is not.
        assert!(is_long(desk.manual.sentence()));
        let iterations = desk
            .manual
            .words()
            .iter()
            .position(|range| &desk.manual.sentence()[range.clone()] == "iterations")
            .expect("word");
        desk.click(iterations);
        let sentence = "If the number of iterations is proportional to n for all loops, you can get away with just counting the loops.";
        assert_eq!(
            desk.asked(),
            [
                Request {
                    text: "iterations".to_owned(),
                    context: Some(sentence.to_owned()),
                    source: Language::En,
                },
                Request {
                    text: sentence.to_owned(),
                    context: None,
                    source: Language::En,
                },
            ]
        );
        // A word of another sentence sends that sentence.
        let pitfall = desk
            .manual
            .words()
            .iter()
            .position(|range| &desk.manual.sentence()[range.clone()] == "pitfall")
            .expect("word");
        desk.click(pitfall);
        let other = "This example demonstrates a pitfall of algorithm analysis: the tempting shortcut of counting loops.";
        assert_eq!(
            desk.asked()[2..],
            [
                Request {
                    text: "pitfall".to_owned(),
                    context: Some(other.to_owned()),
                    source: Language::En,
                },
                Request {
                    text: other.to_owned(),
                    context: None,
                    source: Language::En,
                },
            ]
        );
    }

    #[test]
    fn the_same_word_again_sends_nothing() {
        let mut desk = Desk::with_text(Ok("берег"), RIVER);
        desk.click(BANK);
        desk.click(BANK);
        desk.click(BANK);
        assert_eq!(desk.asked(), [bank_request(), sentence_request()]);
        assert_eq!(desk.shown().word, Some(&online("берег")));
    }

    #[test]
    fn another_word_sends_itself_only_and_going_back_sends_nothing() {
        let mut desk = Desk::with_text(Ok("берег"), RIVER);
        desk.click(BANK);
        desk.click(RIVER_WORD);
        let sent = [bank_request(), sentence_request(), river_request()];
        assert_eq!(desk.asked(), sent);
        // The sentence stays while another word is the chosen one.
        let whole = Whole::Asked(online("берег"));
        assert_eq!(desk.shown().sentence, Some(&whole));
        desk.click(BANK);
        desk.click(RIVER_WORD);
        assert_eq!(desk.asked(), sent);
    }

    #[test]
    fn what_was_refused_is_sent_again_by_the_next_click() {
        let mut desk = Desk::with_text(Err(Refusal::Quota), RIVER);
        desk.click(BANK);
        let refused = Online::Refused(Refusal::Quota);
        assert_eq!(desk.shown().word, Some(&refused));
        assert_eq!(desk.shown().sentence, Some(&Whole::Asked(refused)));
        desk.click(BANK);
        // The sentence again by a click on any word.
        desk.click(RIVER_WORD);
        assert_eq!(
            desk.asked(),
            [
                bank_request(),
                sentence_request(),
                bank_request(),
                sentence_request(),
                river_request(),
                sentence_request(),
            ]
        );
    }

    #[test]
    fn a_long_text_sends_the_word_with_the_text_as_context_and_no_sentence() {
        let text = format!("{}.", text_at_the_limit());
        assert_eq!(text.chars().count(), SENTENCE_LIMIT + 1);
        let mut desk = Desk::with_text(Ok("берег"), &text);
        desk.click(BANK);
        desk.click(BANK);
        assert_eq!(
            desk.asked(),
            [Request {
                text: "bank".to_owned(),
                context: Some(text.clone()),
                source: Language::En,
            }]
        );
        assert_eq!(desk.shown().sentence, Some(&Whole::TooLong));
        desk.click(RIVER_WORD);
        assert_eq!(desk.asked().len(), 2);
        assert_eq!(
            desk.asked()[1],
            Request {
                text: "river".to_owned(),
                context: Some(text),
                source: Language::En,
            }
        );
    }

    #[test]
    fn a_text_of_exactly_the_limit_is_sent_as_a_sentence() {
        let text = text_at_the_limit();
        let mut desk = Desk::with_text(Ok("берег"), &text);
        desk.click(BANK);
        assert_eq!(desk.asked().len(), 2);
        assert_eq!(
            desk.asked()[1],
            Request {
                text,
                context: None,
                source: Language::En,
            }
        );
    }

    #[test]
    fn a_new_text_drops_the_online_texts_and_asks_anew() {
        let mut desk = Desk::with_text(Ok("берег"), RIVER);
        desk.click(BANK);
        let old = desk.manual.text_number();
        let edited = "I sat on the bank of the river";
        desk.edit(edited);
        assert_eq!(desk.shown(), OnlineTexts::default());
        // A late outcome of the old text is not kept.
        for subject in [Subject::Word(BANK), Subject::Sentence(0)] {
            assert!(!desk.kept.answer_current(old, subject, online("старый")));
        }
        assert_eq!(desk.shown(), OnlineTexts::default());
        assert_eq!(desk.asked().len(), 2);
        desk.click(BANK);
        assert_eq!(desk.asked().len(), 4);
        assert_eq!(
            desk.asked()[2..],
            [
                Request {
                    text: "bank".to_owned(),
                    context: Some(edited.to_owned()),
                    source: Language::En,
                },
                lone_request(edited),
            ]
        );
    }

    #[test]
    fn a_pasted_sentence_sends_nothing_until_a_word_is_clicked() {
        let mut desk = Desk::new(Ok("берег"));
        desk.paste(RIVER);
        // Enter in it sends nothing either.
        desk.submit();
        assert!(desk.asked().is_empty());
        assert_eq!(desk.shown(), OnlineTexts::default());
        desk.click(BANK);
        assert_eq!(desk.asked(), [bank_request(), sentence_request()]);
        desk.submit();
        assert_eq!(desk.asked().len(), 2);
    }

    #[test]
    fn a_typed_lone_word_sends_nothing_typed_or_clicked() {
        let mut desk = Desk::new(Ok("банк"));
        for text in ["b", "ba", "ban", "bank"] {
            desk.edit(text);
            assert_eq!(desk.manual.input().expect("input").word(), text);
        }
        desk.click(0);
        desk.click(0);
        assert!(desk.asked().is_empty());
        assert_eq!(desk.shown(), OnlineTexts::default());
    }

    #[test]
    fn enter_sends_a_lone_word_without_context_once() {
        let mut desk = Desk::with_text(Ok("банк"), "bank");
        desk.submit();
        assert_eq!(desk.asked(), [lone_request("bank")]);
        assert_eq!(
            desk.shown(),
            OnlineTexts {
                word: None,
                sentence: None,
                lone: Some(&online("банк")),
            }
        );
        desk.submit();
        desk.click(0);
        assert_eq!(desk.asked(), [lone_request("bank")]);
    }

    #[test]
    fn a_pasted_lone_word_is_sent_as_enter_sends_it() {
        let mut desk = Desk::new(Ok("банк"));
        desk.paste(" Bank. ");
        assert_eq!(desk.asked(), [lone_request("Bank")]);
        assert_eq!(desk.shown().lone, Some(&online("банк")));
        desk.submit();
        assert_eq!(desk.asked().len(), 1);
        // The same word pasted again is a new text.
        desk.paste(" Bank. ");
        assert_eq!(desk.asked(), [lone_request("Bank"), lone_request("Bank")]);
    }

    #[test]
    fn a_refused_lone_word_is_sent_again_by_the_next_enter() {
        let mut desk = Desk::with_text(Err(Refusal::Key), "bank");
        desk.submit();
        assert_eq!(desk.shown().lone, Some(&Online::Refused(Refusal::Key)));
        desk.submit();
        assert_eq!(desk.asked(), [lone_request("bank"), lone_request("bank")]);
    }

    #[test]
    fn enter_in_an_empty_field_sends_nothing() {
        let mut desk = Desk::new(Ok("банк"));
        desk.submit();
        desk.edit(" 12 ");
        desk.submit();
        assert!(desk.asked().is_empty());
        assert_eq!(desk.shown(), OnlineTexts::default());
    }

    #[test]
    fn without_a_runner_or_a_request_nothing_is_sent_and_it_is_a_refusal() {
        let translator = Arc::new(Canned::new(Ok("берег")));
        let manual = bank_of_the_river();
        let started = translate(
            None,
            translator.clone(),
            Some(word_request(&manual)),
            |_| panic!("reported without a runner"),
        );
        assert_eq!(started.err(), Some(Online::Refused(Refusal::Other)));
        let work = Work::start().expect("runner");
        let started = translate(Some(&work), translator.clone(), None, |_| {
            panic!("reported without a request")
        });
        assert_eq!(started.err(), Some(Online::Refused(Refusal::Other)));
        // Dropping the runner waits for its jobs to be gone.
        drop(work);
        assert!(translator.asked().is_empty());
    }

    #[test]
    fn every_refusal_of_the_translator_is_the_shown_refusal() {
        for refusal in [
            Refusal::Key,
            Refusal::UnreadableKey,
            Refusal::UnsafeKey,
            Refusal::Network,
            Refusal::UnsupportedLanguage,
            Refusal::RejectedKey,
            Refusal::Quota,
            Refusal::TimedOut,
            Refusal::Other,
        ] {
            let translator = Arc::new(Canned::new(Err(refusal)));
            let (outcome, _) = translated(translator, &bank_of_the_river(), WAIT);
            assert_eq!(
                translation_result(outcome),
                Some(Online::Refused(refusal)),
                "{refusal:?}"
            );
        }
    }

    #[test]
    fn a_translator_that_never_answers_ends_at_the_deadline() {
        let (outcome, _) = translated(
            Arc::new(Mute),
            &bank_of_the_river(),
            Duration::from_millis(50),
        );
        assert_eq!(outcome, Outcome::TimedOut);
        assert_eq!(
            translation_result(outcome),
            Some(Online::Refused(Refusal::TimedOut))
        );
    }

    #[test]
    fn a_translator_that_panics_is_a_refusal() {
        let (outcome, _) = translated(Arc::new(Faulty), &bank_of_the_river(), WAIT);
        assert_eq!(outcome, Outcome::Panicked);
        assert_eq!(
            translation_result(outcome),
            Some(Online::Refused(Refusal::Other))
        );
    }

    #[test]
    fn a_dropped_online_request_is_cancelled_and_shows_nothing() {
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let manual = bank_of_the_river();
        let job = translate(
            Some(&work),
            Arc::new(Mute),
            Some(word_request(&manual)),
            move |outcome| {
                let _ = sender.send(outcome);
            },
        )
        .expect("job");
        // What another click, an edit or a paste does to the pending request.
        drop(job);
        let outcome = receiver.recv_timeout(WAIT).expect("outcome");
        assert_eq!(outcome, Outcome::Cancelled);
        assert_eq!(translation_result(outcome), None);
    }

    #[test]
    fn only_a_cancelled_online_request_shows_nothing() {
        for outcome in [
            Outcome::Finished(Ok("берег".to_owned())),
            Outcome::Finished(Err(Refusal::Key)),
            Outcome::Cancelled,
            Outcome::TimedOut,
            Outcome::Panicked,
        ] {
            let shown = is_shown(&outcome);
            assert_eq!(translation_result(outcome).is_some(), shown);
        }
    }

    #[test]
    fn the_translate_limit_is_finite_and_its_own() {
        assert!(!TRANSLATE_LIMIT.is_zero());
        assert_eq!(TRANSLATE_LIMIT.as_secs(), 10);
        assert!(TRANSLATE_LIMIT > LOOKUP_LIMIT);
    }

    /// A key file of one test with this mode, removed when the test ends.
    struct KeyFile(std::path::PathBuf);

    impl KeyFile {
        fn new(tag: &str, mode: u32) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let path = std::env::temp_dir()
                .join(format!("addict-manual-{tag}-{}.key", std::process::id()));
            let _ = std::fs::remove_file(&path);
            std::fs::write(&path, "a-key-of-the-stub\n").expect("write the key file");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                .expect("set the mode of the key file");
            Self(path)
        }
    }

    impl Drop for KeyFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// What the jobs of one click report, on one channel.
    #[derive(Debug)]
    enum Reported {
        Article(Outcome<Looked>),
        Online(Outcome<Translated>),
    }

    #[test]
    fn the_article_does_not_wait_for_a_slow_online_answer() {
        let stub = Stub::start(Reply::Answer {
            status: 200,
            body: translation_body("берег"),
            after: Duration::from_millis(300),
        })
        .expect("stub");
        let key = KeyFile::new("slow", 0o600);
        let translator: Arc<dyn Translator> =
            Arc::new(DeepL::with_base_url(&key.0, stub.base_url()));
        let work = Work::start().expect("runner");
        let manual = bank_of_the_river();
        let input = manual.input().expect("input");
        let (sender, receiver) = mpsc::channel();
        // As a click starts them: the lookup, then the two online requests.
        let lookup_sender = sender.clone();
        let lookup = look_up(
            Some(&work),
            Arc::new(Sampled),
            input.word().to_owned(),
            move |outcome| {
                let _ = lookup_sender.send(Reported::Article(outcome));
            },
        )
        .expect("job");
        let mut kept = Kept::of(manual.text_number(), manual.kind());
        let asked = kept.click(BANK, 0, false).ask;
        assert_eq!(asked, [Subject::Word(BANK), Subject::Sentence(0)]);
        let translations: Vec<_> = asked
            .into_iter()
            .map(|subject| {
                let online = sender.clone();
                translate(
                    Some(&work),
                    translator.clone(),
                    request(input, subject, Language::En),
                    move |outcome| {
                        let _ = online.send(Reported::Online(outcome));
                    },
                )
                .expect("job")
            })
            .collect();

        let first = receiver.recv_timeout(WAIT).expect("the first outcome");
        let Reported::Article(outcome) = first else {
            panic!("an online answer came first: {first:?}");
        };
        assert_eq!(
            lookup_result(outcome, "bank"),
            Some(Lookup::Article(Sample.lookup("bank").expect("bank")))
        );
        for _ in 0..2 {
            let later = receiver.recv_timeout(WAIT).expect("an online outcome");
            let Reported::Online(outcome) = later else {
                panic!("two articles: {later:?}");
            };
            assert_eq!(
                translation_result(outcome),
                Some(Online::Translated("берег".to_owned()))
            );
        }
        drop((lookup, translations));

        // The word with its sentence and the sentence went out, once each,
        // and nothing else did.
        let seen = stub.seen();
        assert_eq!(seen.len(), 2);
        let mut bodies = Vec::new();
        for request in &seen {
            assert_eq!(request.line, "POST /v2/translate HTTP/1.1");
            bodies.push(serde_json::from_str::<serde_json::Value>(&request.body).expect("JSON"));
        }
        let word = serde_json::json!({
            "text": ["bank"],
            "context": RIVER,
            "source_lang": "EN",
            "target_lang": "RU",
        });
        let sentence = serde_json::json!({
            "text": [RIVER],
            "source_lang": "EN",
            "target_lang": "RU",
        });
        assert!(bodies.contains(&word), "{bodies:?}");
        assert!(bodies.contains(&sentence), "{bodies:?}");
    }

    #[test]
    fn the_spanish_and_catalan_bodies_name_their_source_and_russian() {
        let stub = Stub::start(Reply::translation("банк")).expect("stub");
        let key = KeyFile::new("spanish", 0o600);
        let translator: Arc<dyn Translator> =
            Arc::new(DeepL::with_base_url(&key.0, stub.base_url()));
        let mut spanish = Manual::default();
        spanish.edit(BANCO_SENTENCE.to_owned());
        spanish.choose(BANCO).expect("run");
        let input = spanish.input().expect("input");
        let work = Work::start().expect("runner");
        for (subject, language) in [
            (Subject::Word(BANCO), Language::Es),
            (Subject::Sentence(0), Language::Ca),
        ] {
            let (sender, receiver) = mpsc::channel();
            let _job = translate(
                Some(&work),
                translator.clone(),
                request(input, subject, language),
                move |outcome| {
                    let _ = sender.send(outcome);
                },
            )
            .expect("job");
            receiver.recv_timeout(WAIT).expect("outcome");
        }
        let bodies: Vec<serde_json::Value> = stub
            .seen()
            .iter()
            .map(|seen| serde_json::from_str(&seen.body).expect("JSON"))
            .collect();
        assert_eq!(
            bodies,
            [
                serde_json::json!({
                    "text": ["banco"],
                    "context": BANCO_SENTENCE,
                    "source_lang": "ES",
                    "target_lang": "RU",
                }),
                serde_json::json!({
                    "text": [BANCO_SENTENCE],
                    "source_lang": "CA",
                    "target_lang": "RU",
                }),
            ]
        );
    }

    #[test]
    fn a_word_answered_in_another_language_is_sent_and_recorded_from_it() {
        // Catalan chosen, `cerró` answered by Spanish: the word goes as
        // Spanish, its sentence as Catalan.
        const CERRO: usize = 2;
        assert_eq!(
            source_of(Subject::Word(CERRO), Language::Ca, Some(Language::Es)),
            Language::Es
        );
        assert_eq!(
            source_of(Subject::Lone, Language::Ca, Some(Language::Es)),
            Language::Es
        );
        assert_eq!(
            source_of(Subject::Sentence(0), Language::Ca, Some(Language::Es)),
            Language::Ca
        );
        // No language answered: the chosen one.
        for subject in [Subject::Word(CERRO), Subject::Lone, Subject::Sentence(0)] {
            for chosen in Language::ALL {
                assert_eq!(source_of(subject, chosen, None), chosen);
                assert_eq!(source_of(subject, chosen, Some(chosen)), chosen);
            }
        }
        let stub = Stub::start(Reply::translation("закрылся")).expect("stub");
        let key = KeyFile::new("fallback", 0o600);
        let translator: Arc<dyn Translator> =
            Arc::new(DeepL::with_base_url(&key.0, stub.base_url()));
        let mut manual = Manual::default();
        manual.edit(BANCO_SENTENCE.to_owned());
        manual.choose(CERRO).expect("run");
        let input = manual.input().expect("input");
        assert_eq!(input.word(), "cerró");
        let work = Work::start().expect("runner");
        let mut records = Vec::new();
        for subject in [Subject::Word(CERRO), Subject::Sentence(0)] {
            let source = source_of(subject, Language::Ca, Some(Language::Es));
            let (sender, receiver) = mpsc::channel();
            let probe = Probe::start(true);
            let _job = translate(
                Some(&work),
                translator.clone(),
                request(input, subject, source),
                move |outcome| {
                    let _ = sender.send((outcome, probe.ended()));
                },
            )
            .expect("job");
            let (outcome, elapsed) = receiver.recv_timeout(WAIT).expect("outcome");
            let (online, record) =
                translation_ended(outcome, subject.into(), source, elapsed).expect("ended");
            assert_eq!(online, Online::Translated("закрылся".to_owned()));
            records.push(record.expect("a record").details()["source"].clone());
        }
        let bodies: Vec<serde_json::Value> = stub
            .seen()
            .iter()
            .map(|seen| serde_json::from_str(&seen.body).expect("JSON"))
            .collect();
        assert_eq!(
            bodies,
            [
                serde_json::json!({
                    "text": ["cerró"],
                    "context": BANCO_SENTENCE,
                    "source_lang": "ES",
                    "target_lang": "RU",
                }),
                serde_json::json!({
                    "text": [BANCO_SENTENCE],
                    "source_lang": "CA",
                    "target_lang": "RU",
                }),
            ]
        );
        assert_eq!(records, ["es", "ca"]);
    }

    #[test]
    fn without_a_key_a_spanish_word_is_the_key_refusal_and_nothing_leaves() {
        let stub = Stub::start(Reply::translation("банк")).expect("stub");
        let missing =
            std::env::temp_dir().join(format!("addict-manual-missing-{}.key", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        let translator = Arc::new(DeepL::with_base_url(&missing, stub.base_url()));
        let mut spanish = Manual::default();
        spanish.edit(BANCO_SENTENCE.to_owned());
        spanish.choose(BANCO).expect("run");
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let _job = translate(
            Some(&work),
            translator,
            request(
                spanish.input().expect("input"),
                Subject::Word(BANCO),
                Language::Es,
            ),
            move |outcome| {
                let _ = sender.send(outcome);
            },
        )
        .expect("job");
        let outcome = receiver.recv_timeout(WAIT).expect("outcome");
        assert_eq!(
            translation_result(outcome),
            Some(Online::Refused(Refusal::Key))
        );
        assert!(stub.seen().is_empty());
    }

    #[test]
    fn a_key_file_that_is_too_open_is_a_refusal_and_nothing_leaves() {
        let stub = Stub::start(Reply::translation("берег")).expect("stub");
        let key = KeyFile::new("open", 0o644);
        let translator = Arc::new(DeepL::with_base_url(&key.0, stub.base_url()));
        let (outcome, _) = translated(translator, &bank_of_the_river(), WAIT);
        assert_eq!(
            translation_result(outcome),
            Some(Online::Refused(Refusal::UnsafeKey))
        );
        assert!(stub.seen().is_empty());
    }

    #[test]
    fn a_silent_server_ends_at_the_deadline() {
        let stub = Stub::start(Reply::Silence).expect("stub");
        let key = KeyFile::new("silent", 0o600);
        let translator = Arc::new(DeepL::with_base_url(&key.0, stub.base_url()));
        let (outcome, _) = translated(translator, &bank_of_the_river(), Duration::from_secs(2));
        assert_eq!(
            translation_result(outcome),
            Some(Online::Refused(Refusal::TimedOut))
        );
        assert_eq!(stub.seen().len(), 1);
    }

    #[test]
    fn every_online_failure_preserves_the_field_and_dictionary_article() {
        for refusal in [
            Refusal::Key,
            Refusal::UnreadableKey,
            Refusal::UnsafeKey,
            Refusal::RejectedKey,
            Refusal::Network,
            Refusal::Quota,
            Refusal::UnsupportedLanguage,
            Refusal::TimedOut,
            Refusal::Other,
        ] {
            let mut desk = Desk::with_text(Err(refusal), RIVER);
            desk.click(BANK);
            assert_eq!(desk.shown().word, Some(&Online::Refused(refusal)));
            assert_eq!(desk.manual.input().expect("input").sentence(), Some(RIVER));
            let (sender, receiver) = mpsc::channel();
            let job = look_up(
                Some(&desk.work),
                Arc::new(Sampled),
                "bank".to_owned(),
                move |outcome| {
                    sender.send(outcome).expect("lookup receiver");
                },
            )
            .expect("lookup job");
            let lookup =
                lookup_result(receiver.recv_timeout(WAIT).expect("lookup outcome"), "bank");
            assert_eq!(
                lookup,
                Some(Lookup::Article(Sample.lookup("bank").expect("article")))
            );
            drop(job);
            desk.edit("river");
            assert_eq!(desk.manual.sentence(), "river");
            desk.submit();
            assert_eq!(desk.shown().lone, Some(&Online::Refused(refusal)));
        }
    }
}
