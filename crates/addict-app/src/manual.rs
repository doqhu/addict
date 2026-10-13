//! The manual path without its widgets: the text of the field, the word
//! chosen in it, the [`Input`] built from them and the lookup of the word.
//!
//! A text of exactly one word is that word without context: it is chosen at
//! once, with no click, and its input has no sentence. A text of more words is
//! a sentence, and the clicked word is the input together with it.
//!
//! [`Manual`] is the state and holds no widget, so what an edit or a click
//! does to it is tested without a display. [`look_up`] starts a lookup on the
//! work runner and [`lookup_result`] says what the window shows for its
//! outcome; [`check_indexes`] asks on the runner where the dictionary indexes
//! stand. [`request`] builds the one online request about a
//! [`Subject`]: the word with its sentence as context, the sentence itself,
//! or a lone word; [`translate`] starts it as a job of its own, with its own
//! deadline, and [`translation_result`] says what the window shows for its
//! outcome. [`translation_ended`] and [`translation_unstarted`] say what the
//! evidence records of a request, before anything decides whether its answer
//! is shown. Which of them are sent, and when, is [`online`](crate::online)'s.
//! Putting any of it on the screen is [`resident`](crate::resident)'s.

use std::{ops::Range, sync::Arc, time::Duration};

use addict_core::{
    input::{Input, Source, WindowState, window_state},
    sentence,
    translation::{Refusal, Request, Translator},
};
use addict_platform::work::{Job, Outcome, Work};

use crate::{
    dictionaries::{Answer, Articles, Indexes, Looked},
    evidence::{self, Elapsed, Probe, Record, Requested},
    online::{Kind, Subject},
    view::{Lookup, Online},
};

/// How long the lookup of one word may take.
const LOOKUP_LIMIT: Duration = Duration::from_secs(2);

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

/// What an edit of the field did to the manual input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// The text is the one already held: nothing changed.
    Same,
    /// The text is new and no word of it is chosen: there is no input.
    Cleared,
    /// The text is new and holds exactly one word, which is now the input.
    /// The lookup to start has this number.
    Lone(u64),
}

/// The text of the field, its words and what was chosen in it.
#[derive(Debug, Default)]
pub struct Manual {
    /// The text of the field, as it is shown with its words.
    sentence: String,
    /// The words of `sentence`, as byte ranges of it.
    words: Vec<Range<usize>>,
    /// The index of the chosen word; `None` until a word is clicked, unless
    /// the text has only one.
    chosen: Option<usize>,
    /// The chosen word, with the sentence when the text has another word.
    input: Option<Input>,
    /// The number of the last started lookup. An outcome that carries another
    /// number belongs to a lookup that was replaced.
    run: u64,
    /// The number of the text: only a new text changes it, a click does not.
    /// An online outcome that carries another number belongs to an older
    /// text.
    text: u64,
}

impl Manual {
    pub fn sentence(&self) -> &str {
        &self.sentence
    }

    pub fn words(&self) -> &[Range<usize>] {
        &self.words
    }

    pub fn chosen(&self) -> Option<usize> {
        self.chosen
    }

    pub fn input(&self) -> Option<&Input> {
        self.input.as_ref()
    }

    /// The number of the text the field holds. It changes with every new
    /// text and with nothing else.
    pub fn text_number(&self) -> u64 {
        self.text
    }

    /// What the text is for the online requests: no word, a lone word or a
    /// sentence, which may be too long to be sent itself.
    pub fn kind(&self) -> Kind {
        Kind::of(&self.sentence, self.words.len())
    }

    /// The window state of this moment: nothing is read from a source, so it
    /// depends on the input alone.
    pub fn state(&self) -> WindowState {
        window_state(None, self.input.as_ref())
    }

    /// The field holds `text`: its words are rebuilt, and what was chosen in
    /// the old text is gone, with the outcome of its lookup. A text of exactly
    /// one word has that word chosen at once.
    pub fn edit(&mut self, text: String) -> Edit {
        if text == self.sentence {
            return Edit::Same;
        }
        self.words = sentence::words(&text);
        self.sentence = text;
        self.chosen = None;
        self.input = None;
        // Whatever the lookup of the old text still reports is not awaited,
        // and neither is any online request of it.
        self.run += 1;
        self.text += 1;
        if self.words.len() != 1 {
            return Edit::Cleared;
        }
        self.choose(0).map_or(Edit::Cleared, Edit::Lone)
    }

    /// The word with `index` is chosen, by a click or as the only word of the
    /// text: it becomes the input, written as it is looked up, with the text
    /// as its sentence when the text has another word. Gives the number of
    /// the lookup to start, which replaces any earlier one, or `None` when
    /// there is no such word.
    pub fn choose(&mut self, index: usize) -> Option<u64> {
        let word = self
            .words
            .get(index)
            .and_then(|range| self.sentence.get(range.clone()))?;
        // One word is a word without context, whatever stands around it.
        let context = (self.words.len() > 1).then_some(self.sentence.as_str());
        let input = Input::new(&sentence::lookup_word(word), context, Source::Manual).ok()?;
        self.chosen = Some(index);
        self.input = Some(input);
        self.run += 1;
        Some(self.run)
    }

    /// Whether `run` is the last started lookup and its word is still chosen.
    pub fn awaits(&self, run: u64) -> bool {
        run == self.run && self.input.is_some()
    }
}

/// Starts the lookup of `word` in `articles` on `work`, with a deadline.
/// `on_outcome` runs once on the worker thread. Without a runner nothing
/// starts and the result to show is given back.
pub fn look_up(
    work: Option<&Work>,
    articles: Arc<dyn Articles>,
    word: String,
    on_outcome: impl FnOnce(Outcome<Looked>) + Send + 'static,
) -> Result<Job, Lookup> {
    let work = work.ok_or(Lookup::Unfinished)?;
    Ok(work.run(
        LOOKUP_LIMIT,
        async move { articles.look_up(&word) },
        on_outcome,
    ))
}

/// Asks `articles` on `work`, with the deadline of a lookup, where the
/// indexes stand. `on_indexes` runs on the worker thread when the answer
/// came, and not at all otherwise. Without a runner nothing starts.
pub fn check_indexes(
    work: Option<&Work>,
    articles: Arc<dyn Articles>,
    on_indexes: impl FnOnce(Indexes) + Send + 'static,
) -> Option<Job> {
    let job = work?.run(
        LOOKUP_LIMIT,
        async move { articles.indexes() },
        move |outcome| {
            if let Outcome::Finished(indexes) = outcome {
                on_indexes(indexes);
            }
        },
    );
    Some(job)
}

/// What the window shows for the `outcome` of the lookup of `word`, or `None`
/// when it shows nothing new: a cancelled lookup is always one that was
/// replaced or dropped. No wildcard arm: a new outcome does not compile until
/// it is given its result.
pub fn lookup_result(outcome: Outcome<Looked>, word: &str) -> Option<Lookup> {
    let word = || word.to_owned();
    match outcome {
        Outcome::Finished(looked) => Some(match looked.answer {
            Answer::Article(article) => Lookup::Article(article),
            Answer::NoEntry => Lookup::NoEntry { word: word() },
            Answer::NotImported => Lookup::NotImported { word: word() },
            Answer::Unreadable(file) => Lookup::Unreadable(file),
        }),
        Outcome::TimedOut | Outcome::Panicked => Some(Lookup::Unfinished),
        Outcome::Cancelled => None,
    }
}

/// The online request about `subject` for `input`, with nothing in it but
/// what is named here. For a word of a sentence: the word, with the sentence
/// as context. For the sentence: the sentence, without context. For a lone
/// word: the word, without context. `None` when the input is not of that
/// kind: a lone word has no sentence, and a word of a sentence is not sent
/// without it.
pub fn request(input: &Input, subject: Subject) -> Option<Request> {
    let sentence = input.sentence().map(str::to_owned);
    match subject {
        Subject::Word(_) => Some(Request {
            text: input.word().to_owned(),
            context: Some(sentence?),
        }),
        Subject::Sentence => Some(Request {
            text: sentence?,
            context: None,
        }),
        Subject::Lone => sentence.is_none().then(|| Request {
            text: input.word().to_owned(),
            context: None,
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
/// `requested`, with the record of it for the evidence, or `None` for a
/// request that was given up: it shows nothing and is not recorded. The
/// record is there whenever the request was timed, which gives `elapsed`,
/// whether or not the answer is still awaited: that is asked after this.
/// Call it on the UI thread: making a record reads the memory of the
/// process.
pub fn translation_ended(
    outcome: Outcome<Translated>,
    requested: Requested,
    elapsed: Option<Elapsed>,
) -> Option<(Online, Option<Record>)> {
    let online = translation_result(outcome)?;
    let record = evidence::online_record(requested, &online, elapsed);
    Some((online, record))
}

/// The record of a request for `requested` that [`translate`] did not start
/// and gave back as `online`: no time passed. None when `probe` measures
/// nothing. As [`translation_ended`], for the UI thread.
pub fn translation_unstarted(
    online: &Online,
    requested: Requested,
    probe: Probe,
) -> Option<Record> {
    evidence::online_record(requested, online, probe.not_started())
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, mpsc},
        thread,
        time::Duration,
    };

    use addict_core::{
        dictionary::{Dictionary, Sample},
        input::{Source, WindowState},
        translation::{Canned, Refusal, Request, Translating, Translator},
    };
    use addict_deepl::{
        DeepL,
        stub::{Reply, Stub, translation_body},
    };
    use addict_platform::work::{Outcome, Work};

    use super::{
        Edit, LOOKUP_LIMIT, Manual, SENTENCE_LIMIT, TRANSLATE_LIMIT, Translated, check_indexes,
        look_up, lookup_result, request, translate, translate_within, translation_ended,
        translation_result, translation_unstarted,
    };
    use crate::{
        dictionaries::{
            Answer, Articles, Indexes, Looked, State,
            tests::{FREEDICT, OEWN, Sampled},
        },
        evidence::{Elapsed, Probe, Requested},
        online::{Kept, Kind, Subject},
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
                context: None
            }]
        );
        let (retry, request) = shown.request_definition().expect("retry action");
        assert!(!shown.answer_definition(first, Online::Translated("old".to_owned())));
        assert!(shown.answer_definition(retry, run(request, success.clone())));
        assert_eq!(
            success.asked(),
            [Request {
                text: definition,
                context: None
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

    const SENTENCE: &str = "I sat on the bank.";

    /// How long a test waits for an outcome that must come.
    const WAIT: Duration = Duration::from_secs(10);

    fn with_sentence(sentence: &str) -> Manual {
        let mut manual = Manual::default();
        assert_ne!(manual.edit(sentence.to_owned()), Edit::Same);
        manual
    }

    /// The number of the lookup that an edit to the lone word `text` starts.
    fn lone(manual: &mut Manual, text: &str) -> u64 {
        match manual.edit(text.to_owned()) {
            Edit::Lone(run) => run,
            other => panic!("{text:?}: {other:?}"),
        }
    }

    const DICTIONARY: WindowState = WindowState::Dictionary {
        source: Source::Manual,
    };

    const CONTEXTUAL: WindowState = WindowState::Contextual {
        source: Source::Manual,
    };

    /// The outcome of a lookup of `word` that ran on a real runner, and
    /// whether its callback ran on another thread than this one.
    fn looked_up(articles: impl Articles + 'static, word: &str) -> (Outcome<Looked>, bool) {
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let caller = thread::current().id();
        let job = look_up(
            Some(&work),
            std::sync::Arc::new(articles),
            word.to_owned(),
            move |outcome| {
                let _ = sender.send((outcome, thread::current().id() != caller));
            },
        )
        .expect("job");
        let reported = receiver.recv_timeout(WAIT).expect("outcome");
        drop(job);
        reported
    }

    #[test]
    fn nothing_pasted_is_the_empty_state() {
        let manual = Manual::default();
        assert_eq!(manual.state(), WindowState::Empty);
        assert!(manual.words().is_empty());
        assert_eq!(manual.input(), None);
    }

    #[test]
    fn a_pasted_sentence_has_its_words_and_no_input_yet() {
        let manual = with_sentence(SENTENCE);
        let words: Vec<_> = manual
            .words()
            .iter()
            .map(|range| &manual.sentence()[range.clone()])
            .collect();
        assert_eq!(words, ["I", "sat", "on", "the", "bank"]);
        assert_eq!(manual.chosen(), None);
        assert_eq!(manual.state(), WindowState::Empty);
    }

    #[test]
    fn a_blank_field_has_no_words_and_no_input() {
        for text in [" ", "\t\n", "\u{a0}"] {
            let mut manual = with_sentence(text);
            assert!(manual.words().is_empty(), "{text:?}");
            assert_eq!(manual.choose(0), None, "{text:?}");
            assert_eq!(manual.state(), WindowState::Empty, "{text:?}");
        }
    }

    #[test]
    fn a_clicked_word_is_a_manual_input_with_its_sentence() {
        let mut manual = with_sentence(SENTENCE);
        let run = manual.choose(4).expect("run");
        let input = manual.input().expect("input");
        assert_eq!(input.word(), "bank");
        assert_eq!(input.sentence(), Some(SENTENCE));
        assert_eq!(input.source(), Source::Manual);
        assert_eq!(manual.chosen(), Some(4));
        assert_eq!(manual.state(), CONTEXTUAL);
        assert!(manual.awaits(run));
    }

    #[test]
    fn an_index_without_a_word_changes_nothing() {
        let mut manual = with_sentence(SENTENCE);
        let run = manual.choose(4).expect("run");
        assert_eq!(manual.choose(5), None);
        assert_eq!(manual.chosen(), Some(4));
        assert_eq!(manual.input().expect("input").word(), "bank");
        assert!(manual.awaits(run));
    }

    #[test]
    fn another_word_replaces_the_lookup_of_the_first() {
        let mut manual = with_sentence(SENTENCE);
        let bank = manual.choose(4).expect("run");
        let on = manual.choose(2).expect("run");
        assert_ne!(bank, on);
        // A late outcome for `bank` is not awaited; the one for `on` is.
        assert!(!manual.awaits(bank));
        assert!(manual.awaits(on));
        assert_eq!(manual.input().expect("input").word(), "on");
        assert_eq!(manual.chosen(), Some(2));
    }

    #[test]
    fn an_edit_drops_the_input_and_the_awaited_lookup() {
        let mut manual = with_sentence(SENTENCE);
        let run = manual.choose(4).expect("run");
        assert_eq!(manual.edit("I sat on the bank".to_owned()), Edit::Cleared);
        assert_eq!(manual.input(), None);
        assert_eq!(manual.chosen(), None);
        assert_eq!(manual.state(), WindowState::Empty);
        assert!(!manual.awaits(run));
        assert_eq!(manual.words().len(), 5);
        // A click in the new text starts a lookup with a number of its own.
        let again = manual.choose(4).expect("run");
        assert_ne!(again, run);
        assert!(manual.awaits(again));
    }

    #[test]
    fn the_same_text_again_is_not_an_edit() {
        let mut manual = with_sentence(SENTENCE);
        let run = manual.choose(4).expect("run");
        assert_eq!(manual.edit(SENTENCE.to_owned()), Edit::Same);
        assert!(manual.awaits(run));
        assert_eq!(manual.chosen(), Some(4));
    }

    #[test]
    fn a_lone_word_is_the_input_at_once_and_has_no_sentence() {
        for text in [
            "bank",
            " Bank. ",
            "(bank)",
            "\u{201c}bank\u{201d}\n",
            "12 bank",
        ] {
            let mut manual = Manual::default();
            let run = lone(&mut manual, text);
            let input = manual.input().expect("input");
            assert!(input.word().eq_ignore_ascii_case("bank"), "{text:?}");
            assert_eq!(input.sentence(), None, "{text:?}");
            assert_eq!(input.source(), Source::Manual, "{text:?}");
            assert_eq!(manual.chosen(), Some(0), "{text:?}");
            assert_eq!(manual.state(), DICTIONARY, "{text:?}");
            assert!(manual.awaits(run), "{text:?}");
            // The field's text is kept as it is, for the shown word.
            assert_eq!(manual.sentence(), text);
        }
    }

    #[test]
    fn a_lone_word_keeps_its_case() {
        let mut manual = Manual::default();
        lone(&mut manual, " Bank. ");
        assert_eq!(manual.input().expect("input").word(), "Bank");
    }

    #[test]
    fn each_typed_prefix_replaces_the_lookup_of_the_one_before() {
        let mut manual = Manual::default();
        let mut runs = Vec::new();
        for text in ["b", "ba", "ban", "bank"] {
            let run = lone(&mut manual, text);
            assert_eq!(manual.input().expect("input").word(), text);
            assert!(!runs.contains(&run), "{text:?}");
            runs.push(run);
        }
        // Only the outcome for `bank` is awaited.
        let (last, earlier) = runs.split_last().expect("runs");
        assert!(manual.awaits(*last));
        for run in earlier {
            assert!(!manual.awaits(*run), "{run}");
        }
    }

    #[test]
    fn a_lone_word_clicked_again_starts_a_new_lookup() {
        let mut manual = Manual::default();
        let first = lone(&mut manual, "bank");
        let second = manual.choose(0).expect("run");
        assert_ne!(first, second);
        assert!(!manual.awaits(first));
        assert!(manual.awaits(second));
        assert_eq!(manual.input().expect("input").sentence(), None);
        assert_eq!(manual.state(), DICTIONARY);
    }

    #[test]
    fn the_same_word_of_a_sentence_clicked_again_starts_a_new_lookup() {
        let mut manual = with_sentence(SENTENCE);
        let first = manual.choose(4).expect("run");
        let second = manual.choose(4).expect("run");
        assert_ne!(first, second);
        assert!(!manual.awaits(first));
        assert!(manual.awaits(second));
        assert_eq!(manual.input().expect("input").word(), "bank");
        assert_eq!(manual.state(), CONTEXTUAL);
    }

    #[test]
    fn a_sentence_shortened_to_one_word_is_a_lone_word() {
        let mut manual = with_sentence(SENTENCE);
        let old = manual.choose(2).expect("run");
        let run = lone(&mut manual, "bank");
        let input = manual.input().expect("input");
        assert_eq!(input.word(), "bank");
        assert_eq!(input.sentence(), None);
        assert_eq!(manual.chosen(), Some(0));
        assert_eq!(manual.state(), DICTIONARY);
        assert!(!manual.awaits(old));
        assert!(manual.awaits(run));
    }

    #[test]
    fn a_lone_word_grown_into_a_sentence_has_no_input_until_a_click() {
        let mut manual = Manual::default();
        let old = lone(&mut manual, "bank");
        assert_eq!(manual.edit("bank holiday".to_owned()), Edit::Cleared);
        assert_eq!(manual.input(), None);
        assert_eq!(manual.chosen(), None);
        assert_eq!(manual.state(), WindowState::Empty);
        assert!(!manual.awaits(old));
        manual.choose(0).expect("run");
        assert_eq!(
            manual.input().expect("input").sentence(),
            Some("bank holiday")
        );
        assert_eq!(manual.state(), CONTEXTUAL);
    }

    #[test]
    fn an_emptied_field_has_no_words_and_no_input() {
        for text in ["", " ", "\t", "12 \u{2026}", "\u{2014} ?!", "'"] {
            // From a sentence with a chosen word, and from a lone word.
            let mut from_sentence = with_sentence(SENTENCE);
            let chosen = from_sentence.choose(4).expect("run");
            let mut from_word = Manual::default();
            let alone = lone(&mut from_word, "bank");
            for (mut manual, run) in [(from_sentence, chosen), (from_word, alone)] {
                assert_eq!(manual.edit(text.to_owned()), Edit::Cleared, "{text:?}");
                assert!(manual.words().is_empty(), "{text:?}");
                assert_eq!(manual.input(), None, "{text:?}");
                assert_eq!(manual.chosen(), None, "{text:?}");
                assert_eq!(manual.state(), WindowState::Empty, "{text:?}");
                assert!(!manual.awaits(run), "{text:?}");
            }
        }
    }

    #[test]
    fn a_hyphenated_compound_is_two_words_and_not_a_lone_word() {
        let mut manual = Manual::default();
        assert_eq!(manual.edit("well-known".to_owned()), Edit::Cleared);
        assert_eq!(manual.words().len(), 2);
        manual.choose(1).expect("run");
        let input = manual.input().expect("input");
        assert_eq!(input.word(), "known");
        assert_eq!(input.sentence(), Some("well-known"));
    }

    #[test]
    fn a_typographic_apostrophe_is_looked_up_as_the_plain_one() {
        let sentence = "I don\u{2019}t sit on the bank.";
        let mut manual = with_sentence(sentence);
        manual.choose(1).expect("run");
        let input = manual.input().expect("input");
        assert_eq!(input.word(), "don't");
        // The sentence is kept as written, in the input and for the window.
        assert_eq!(input.sentence(), Some(sentence));
        assert_eq!(manual.sentence(), sentence);
        assert_eq!(
            &manual.sentence()[manual.words()[1].clone()],
            "don\u{2019}t"
        );
        // Alone too.
        let mut manual = Manual::default();
        lone(&mut manual, "don\u{2019}t");
        assert_eq!(manual.input().expect("input").word(), "don't");
    }

    #[test]
    fn a_new_text_over_a_chosen_word_drops_the_old_input() {
        // What a paste does: the whole text is another one.
        let mut manual = with_sentence(SENTENCE);
        let old = manual.choose(4).expect("run");
        assert_eq!(
            manual.edit("The charge was dropped.".to_owned()),
            Edit::Cleared
        );
        assert_eq!(manual.input(), None);
        assert_eq!(manual.chosen(), None);
        assert!(!manual.awaits(old));
        assert_eq!(manual.words().len(), 4);
    }

    #[test]
    fn the_last_lone_word_is_the_one_looked_up_and_its_article_is_the_result() {
        let mut manual = Manual::default();
        lone(&mut manual, "ban");
        let run = lone(&mut manual, "bank");
        let word = manual.input().expect("input").word().to_owned();
        assert_eq!(word, "bank");
        let (outcome, _) = looked_up(Sampled, &word);
        assert!(manual.awaits(run));
        assert_eq!(
            lookup_result(outcome, &word),
            Some(Lookup::Article(Sample.lookup("bank").expect("bank")))
        );
    }

    #[test]
    fn no_outcome_is_awaited_before_a_click() {
        let manual = with_sentence(SENTENCE);
        for run in 0..3 {
            assert!(!manual.awaits(run), "{run}");
        }
    }

    #[test]
    fn a_lookup_runs_off_the_calling_thread_and_finds_the_article() {
        let (outcome, elsewhere) = looked_up(Sampled, "bank");
        assert!(elsewhere);
        let article = Sample.lookup("bank").expect("bank");
        assert_eq!(
            lookup_result(outcome, "bank"),
            Some(Lookup::Article(article))
        );
    }

    #[test]
    fn a_capitalised_word_finds_the_same_article() {
        let mut manual = with_sentence("Bank holidays");
        manual.choose(0).expect("run");
        let word = manual.input().expect("input").word().to_owned();
        assert_eq!(word, "Bank");
        let (outcome, _) = looked_up(Sampled, &word);
        assert_eq!(
            lookup_result(outcome, &word),
            Some(Lookup::Article(Sample.lookup("bank").expect("bank")))
        );
    }

    #[test]
    fn a_word_outside_the_sample_is_named_in_the_result() {
        let (outcome, _) = looked_up(Sampled, "sat");
        assert_eq!(
            lookup_result(outcome, "sat"),
            Some(Lookup::NoEntry {
                word: "sat".to_owned()
            })
        );
    }

    /// A source of articles whose every call panics.
    struct Broken;

    impl Articles for Broken {
        fn indexes(&self) -> Indexes {
            panic!("the broken test source");
        }

        fn look_up(&self, _word: &str) -> Looked {
            panic!("the broken test source");
        }
    }

    /// A source of articles that always gives the same answer, with only the
    /// translations index open.
    struct Answering(Answer);

    const TRANSLATIONS_ONLY: Indexes = Indexes {
        definitions: State::Missing,
        translations: State::Open,
    };

    impl Articles for Answering {
        fn indexes(&self) -> Indexes {
            TRANSLATIONS_ONLY
        }

        fn look_up(&self, _word: &str) -> Looked {
            Looked {
                answer: self.0.clone(),
                indexes: TRANSLATIONS_ONLY,
            }
        }
    }

    #[test]
    fn a_word_with_nothing_imported_is_named_with_that_and_did_finish() {
        let (outcome, elsewhere) = looked_up(Answering(Answer::NotImported), "bank");
        assert!(elsewhere);
        assert_eq!(
            lookup_result(outcome, "bank"),
            Some(Lookup::NotImported {
                word: "bank".to_owned()
            })
        );
    }

    #[test]
    fn an_index_that_cannot_be_read_is_the_result_and_not_an_unfinished_lookup() {
        for file in [OEWN, FREEDICT] {
            let (outcome, _) = looked_up(Answering(Answer::Unreadable(file)), "bank");
            assert_eq!(
                lookup_result(outcome, "bank"),
                Some(Lookup::Unreadable(file))
            );
        }
    }

    #[test]
    fn the_outcome_of_a_lookup_says_where_the_indexes_stand() {
        let (outcome, _) = looked_up(Answering(Answer::NoEntry), "bank");
        let Outcome::Finished(looked) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(looked.indexes, TRANSLATIONS_ONLY);
    }

    /// What `check_indexes` reported for `articles` on a real runner, and
    /// whether it reported from another thread; `None` when it reported
    /// nothing.
    fn checked(articles: impl Articles + 'static) -> Option<(Indexes, bool)> {
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let caller = thread::current().id();
        let job = check_indexes(Some(&work), std::sync::Arc::new(articles), move |indexes| {
            let _ = sender.send((indexes, thread::current().id() != caller));
        })
        .expect("job");
        // The sender is dropped with the callback when nothing is reported.
        let reported = receiver.recv_timeout(WAIT).ok();
        drop(job);
        reported
    }

    #[test]
    fn the_indexes_are_checked_off_the_calling_thread() {
        assert_eq!(
            checked(Answering(Answer::NoEntry)),
            Some((TRANSLATIONS_ONLY, true))
        );
        assert_eq!(checked(Sampled), Some((Indexes::OPEN, true)));
    }

    #[test]
    fn a_check_that_panics_reports_nothing() {
        assert_eq!(checked(Broken), None);
    }

    #[test]
    fn without_a_runner_no_check_starts() {
        let started = check_indexes(None, std::sync::Arc::new(Sampled), |_| {
            panic!("reported without a runner");
        });
        assert!(started.is_none());
    }

    #[test]
    fn a_lookup_that_panics_did_not_finish() {
        let (outcome, _) = looked_up(Broken, "bank");
        assert_eq!(outcome, Outcome::Panicked);
        assert_eq!(lookup_result(outcome, "bank"), Some(Lookup::Unfinished));
    }

    #[test]
    fn without_a_runner_the_lookup_did_not_finish() {
        let (sender, receiver) = mpsc::channel();
        let started = look_up(
            None,
            std::sync::Arc::new(Sampled),
            "bank".to_owned(),
            move |outcome| {
                let _ = sender.send(outcome);
            },
        );
        assert_eq!(started.err(), Some(Lookup::Unfinished));
        // The callback was dropped without being run.
        assert!(receiver.recv().is_err());
    }

    #[test]
    fn a_lookup_past_its_deadline_did_not_finish() {
        assert_eq!(
            lookup_result(Outcome::TimedOut, "bank"),
            Some(Lookup::Unfinished)
        );
    }

    /// Whether the window shows something for `outcome`. No wildcard arm: a
    /// new outcome does not compile here until it is listed.
    fn is_shown<T>(outcome: &Outcome<T>) -> bool {
        match outcome {
            Outcome::Finished(_) | Outcome::TimedOut | Outcome::Panicked => true,
            Outcome::Cancelled => false,
        }
    }

    #[test]
    fn only_a_cancelled_lookup_shows_nothing() {
        for outcome in [
            Outcome::Finished(Sampled.look_up("bank")),
            Outcome::Finished(Sampled.look_up("sat")),
            Outcome::Finished(Answering(Answer::NotImported).look_up("bank")),
            Outcome::Finished(Answering(Answer::Unreadable(OEWN)).look_up("bank")),
            Outcome::Cancelled,
            Outcome::TimedOut,
            Outcome::Panicked,
        ] {
            let shown = is_shown(&outcome);
            assert_eq!(lookup_result(outcome, "bank").is_some(), shown);
        }
    }

    const RIVER: &str = "I sat on the bank of the river.";

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
        }
    }

    /// The request about the chosen word of `manual` in its sentence.
    fn word_request(manual: &Manual) -> Request {
        let chosen = manual.chosen().expect("chosen");
        request(manual.input().expect("input"), Subject::Word(chosen)).expect("a sentence")
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
            let record = translation_unstarted(&online, requested, probe).expect("a record");
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
        assert_eq!(translation_ended(outcome, Requested::Word, elapsed), None);
        // The same request, left to end, is recorded.
        let canned = Arc::new(Canned::new(Ok("берег")));
        let (outcome, elapsed) = probed(&work, canned, bank(), probe, false);
        let (_, record) = translation_ended(outcome, Requested::Word, elapsed).expect("ended");
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
                translation_ended(outcome, Requested::Definition, elapsed).expect("ended");
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
            translation_ended(outcome, Requested::Word, elapsed),
            Some((Online::Translated("берег".to_owned()), None))
        );
        // A refusal, and a request that could not start.
        let refusing = Arc::new(Canned::new(Err(Refusal::Quota)));
        let (outcome, elapsed) = probed(&work, refusing.clone(), bank(), probe, false);
        assert_eq!(elapsed, None);
        assert_eq!(
            translation_ended(outcome, Requested::Sentence, elapsed),
            Some((Online::Refused(Refusal::Quota), None))
        );
        let online = translate(None, refusing, Some(bank()), |_| {}).expect_err("nothing started");
        assert_eq!(probe.not_started(), None);
        assert_eq!(translation_unstarted(&online, Requested::Word, probe), None);
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
        }
    }

    fn river_request() -> Request {
        Request {
            text: "river".to_owned(),
            context: Some(RIVER.to_owned()),
        }
    }

    fn lone_request(word: &str) -> Request {
        Request {
            text: word.to_owned(),
            context: None,
        }
    }

    /// The indices of `bank` and `river` in [`RIVER`].
    const BANK: usize = 4;
    const RIVER_WORD: usize = 7;

    /// A text of exactly [`SENTENCE_LIMIT`] characters that starts with
    /// [`RIVER`]; its bytes are more.
    fn text_at_the_limit() -> String {
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
        assert_eq!(request(input, Subject::Word(BANK)), Some(bank_request()));
        assert_eq!(request(input, Subject::Sentence), Some(sentence_request()));
        // A word of a sentence is not sent without it.
        assert_eq!(request(input, Subject::Lone), None);

        let mut manual = Manual::default();
        lone(&mut manual, " Bank. ");
        let input = manual.input().expect("input");
        assert_eq!(request(input, Subject::Lone), Some(lone_request("Bank")));
        // A lone word has no sentence to send, as text or as context.
        assert_eq!(request(input, Subject::Word(0)), None);
        assert_eq!(request(input, Subject::Sentence), None);
    }

    #[test]
    fn the_number_of_the_text_changes_with_the_text_and_not_with_a_click() {
        let mut manual = with_sentence(RIVER);
        let text = manual.text_number();
        let first = manual.choose(BANK).expect("run");
        let second = manual.choose(BANK).expect("run");
        manual.choose(RIVER_WORD).expect("run");
        assert_ne!(first, second);
        assert_eq!(manual.text_number(), text);
        assert_eq!(manual.edit(RIVER.to_owned()), Edit::Same);
        assert_eq!(manual.text_number(), text);
        // Every new text has a number of its own, the same text put back too.
        assert_eq!(manual.edit(String::new()), Edit::Cleared);
        let emptied = manual.text_number();
        assert_ne!(emptied, text);
        assert_eq!(manual.edit(RIVER.to_owned()), Edit::Cleared);
        assert_ne!(manual.text_number(), emptied);
        assert_ne!(manual.text_number(), text);
        let before = manual.text_number();
        lone(&mut manual, "bank");
        assert_ne!(manual.text_number(), before);
    }

    #[test]
    fn the_kind_of_the_text_is_counted_on_the_field_as_it_is() {
        assert_eq!(Manual::default().kind(), Kind::Empty);
        assert_eq!(with_sentence(" bank. ").kind(), Kind::Lone);
        assert_eq!(with_sentence(RIVER).kind(), Kind::Sentence { long: false });
        assert_eq!(SENTENCE_LIMIT, 500);
        let most = text_at_the_limit();
        assert_eq!(with_sentence(&most).kind(), Kind::Sentence { long: false });
        let over = format!("{most}.");
        assert_eq!(with_sentence(&over).kind(), Kind::Sentence { long: true });
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
    }

    impl Desk {
        fn new(answer: Result<&str, Refusal>) -> Self {
            Self {
                manual: Manual::default(),
                kept: Kept::default(),
                work: Work::start().expect("runner"),
                translator: Arc::new(Canned::new(answer)),
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

        fn click(&mut self, index: usize) {
            self.manual.choose(index).expect("run");
            for subject in self.kept.click(index).ask {
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
            let (sender, receiver) = mpsc::channel();
            let asked = self
                .manual
                .input()
                .and_then(|input| request(input, subject));
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

        fn shown(&self) -> OnlineTexts<'_> {
            self.kept.shown(self.manual.chosen())
        }
    }

    fn online(text: &str) -> Online {
        Online::Translated(text.to_owned())
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
        for subject in [Subject::Word(BANK), Subject::Sentence] {
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
        let asked = kept.click(BANK).ask;
        assert_eq!(asked, [Subject::Word(BANK), Subject::Sentence]);
        let translations: Vec<_> = asked
            .into_iter()
            .map(|subject| {
                let online = sender.clone();
                translate(
                    Some(&work),
                    translator.clone(),
                    request(input, subject),
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
    fn the_lookup_limit_is_finite_and_short() {
        assert!(!LOOKUP_LIMIT.is_zero());
        assert!(LOOKUP_LIMIT.as_secs() <= 10);
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
