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
//! stand. [`translate`] starts the online translation of a word of a sentence
//! as a job of its own, with its own deadline, and [`translation_result`]
//! says what the window shows for its outcome; a word without a sentence
//! sends nothing. Putting any of it on the screen is
//! [`resident`](crate::resident)'s.

use std::{ops::Range, sync::Arc, time::Duration};

use addict_core::{
    input::{Input, Source, WindowState, window_state},
    sentence,
    translation::{Refusal, Request, Translator},
};
use addict_platform::work::{Job, Outcome, Work};

use crate::{
    dictionaries::{Answer, Articles, Indexes, Looked},
    view::{Lookup, Online},
};

/// How long the lookup of one word may take.
const LOOKUP_LIMIT: Duration = Duration::from_secs(2);

/// How long the online translation of one word may take. A network answer is
/// slower than a file read, and this deadline is the only thing that ends a
/// request without an answer.
const TRANSLATE_LIMIT: Duration = Duration::from_secs(10);

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
        // Whatever the lookup of the old text still reports is not awaited.
        self.run += 1;
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

/// Starts the online translation of the word of `input` in its sentence on
/// `work`, with a deadline: the word is the text and the sentence is the
/// context, and nothing else is sent. `on_outcome` runs once on the worker
/// thread. `None`, with nothing started, for an input without a sentence: a
/// lone word is not sent online. Without a runner nothing starts and the
/// state to show is given back.
pub fn translate(
    work: Option<&Work>,
    translator: Arc<dyn Translator>,
    input: &Input,
    on_outcome: impl FnOnce(Outcome<Translated>) + Send + 'static,
) -> Option<Result<Job, Online>> {
    translate_within(TRANSLATE_LIMIT, work, translator, input, on_outcome)
}

/// [`translate`] with the deadline `limit`.
fn translate_within(
    limit: Duration,
    work: Option<&Work>,
    translator: Arc<dyn Translator>,
    input: &Input,
    on_outcome: impl FnOnce(Outcome<Translated>) + Send + 'static,
) -> Option<Result<Job, Online>> {
    let request = Request {
        text: input.word().to_owned(),
        context: Some(input.sentence()?.to_owned()),
    };
    let Some(work) = work else {
        return Some(Err(Online::Refused(Refusal::Other)));
    };
    Some(Ok(work.run(
        limit,
        async move { translator.translate(request).await },
        on_outcome,
    )))
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
        Edit, LOOKUP_LIMIT, Manual, TRANSLATE_LIMIT, Translated, check_indexes, look_up,
        lookup_result, translate, translate_within, translation_result,
    };
    use crate::{
        dictionaries::{
            Answer, Articles, Indexes, Looked, State,
            tests::{FREEDICT, OEWN, Sampled},
        },
        view::{Lookup, Online},
    };

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

    /// The outcome of the online translation of the input of `manual` that
    /// ran on a real runner within `limit`, and whether its callback ran on
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
            manual.input().expect("input"),
            move |outcome| {
                let _ = sender.send((outcome, thread::current().id() != caller));
            },
        )
        .expect("a sentence")
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

    #[test]
    fn a_lone_word_is_not_sent_online() {
        let work = Work::start().expect("runner");
        let translator = Arc::new(Canned::new(Ok("банк")));
        // Typed, and clicked again.
        let mut manual = Manual::default();
        lone(&mut manual, "bank");
        for _ in 0..2 {
            let started = translate(
                Some(&work),
                translator.clone(),
                manual.input().expect("input"),
                |_| panic!("reported for a lone word"),
            );
            assert!(started.is_none());
            manual.choose(0).expect("run");
        }
        // Dropping the runner waits for its jobs to be gone.
        drop(work);
        assert!(translator.asked().is_empty());
    }

    #[test]
    fn without_a_runner_the_online_translation_is_refused_and_nothing_is_sent() {
        let translator = Arc::new(Canned::new(Ok("берег")));
        let manual = bank_of_the_river();
        let started = translate(
            None,
            translator.clone(),
            manual.input().expect("input"),
            |_| panic!("reported without a runner"),
        );
        assert_eq!(
            started.expect("a sentence").err(),
            Some(Online::Refused(Refusal::Other))
        );
        assert!(translator.asked().is_empty());
    }

    #[test]
    fn every_refusal_of_the_translator_is_the_shown_refusal() {
        for refusal in [
            Refusal::Key,
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
            manual.input().expect("input"),
            move |outcome| {
                let _ = sender.send(outcome);
            },
        )
        .expect("a sentence")
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

    /// What the two jobs of one click report, on one channel.
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
        let online = sender.clone();
        // As a click starts them: the lookup, then the online request.
        let lookup = look_up(
            Some(&work),
            Arc::new(Sampled),
            input.word().to_owned(),
            move |outcome| {
                let _ = sender.send(Reported::Article(outcome));
            },
        )
        .expect("job");
        let translation = translate(Some(&work), translator, input, move |outcome| {
            let _ = online.send(Reported::Online(outcome));
        })
        .expect("a sentence")
        .expect("job");

        let first = receiver.recv_timeout(WAIT).expect("the first outcome");
        let Reported::Article(outcome) = first else {
            panic!("the online answer came first: {first:?}");
        };
        assert_eq!(
            lookup_result(outcome, "bank"),
            Some(Lookup::Article(Sample.lookup("bank").expect("bank")))
        );
        let second = receiver.recv_timeout(WAIT).expect("the second outcome");
        let Reported::Online(outcome) = second else {
            panic!("two articles: {second:?}");
        };
        assert_eq!(
            translation_result(outcome),
            Some(Online::Translated("берег".to_owned()))
        );
        drop((lookup, translation));

        // The word and its sentence went out, once, and nothing else did.
        let seen = stub.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].line, "POST /v2/translate HTTP/1.1");
        let body: serde_json::Value = serde_json::from_str(&seen[0].body).expect("JSON");
        assert_eq!(
            body,
            serde_json::json!({
                "text": ["bank"],
                "context": RIVER,
                "source_lang": "EN",
                "target_lang": "RU",
            })
        );
    }

    #[test]
    fn a_key_file_that_is_too_open_is_a_refusal_and_nothing_leaves() {
        let stub = Stub::start(Reply::translation("берег")).expect("stub");
        let key = KeyFile::new("open", 0o644);
        let translator = Arc::new(DeepL::with_base_url(&key.0, stub.base_url()));
        let (outcome, _) = translated(translator, &bank_of_the_river(), WAIT);
        assert_eq!(
            translation_result(outcome),
            Some(Online::Refused(Refusal::Key))
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
}
