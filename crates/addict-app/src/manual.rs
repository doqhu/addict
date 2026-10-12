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
//! outcome. Putting any of it on the screen is
//! [`resident`](crate::resident)'s.

use std::{ops::Range, sync::Arc, time::Duration};

use addict_core::{
    dictionary::{Article, Dictionary},
    input::{Input, Source, WindowState, window_state},
    sentence,
};
use addict_platform::work::{Job, Outcome, Work};

use crate::view::Lookup;

/// How long the lookup of one word may take.
const LOOKUP_LIMIT: Duration = Duration::from_secs(2);

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

/// Starts the lookup of `word` in `dictionary` on `work`, with a deadline.
/// `on_outcome` runs once on the worker thread. Without a runner nothing
/// starts and the result to show is given back.
pub fn look_up(
    work: Option<&Work>,
    dictionary: Arc<dyn Dictionary>,
    word: String,
    on_outcome: impl FnOnce(Outcome<Option<Article>>) + Send + 'static,
) -> Result<Job, Lookup> {
    let work = work.ok_or(Lookup::Unfinished)?;
    Ok(work.run(
        LOOKUP_LIMIT,
        async move { dictionary.lookup(&word) },
        on_outcome,
    ))
}

/// What the window shows for the `outcome` of the lookup of `word`, or `None`
/// when it shows nothing new: a cancelled lookup is always one that was
/// replaced or dropped. No wildcard arm: a new outcome does not compile until
/// it is given its result.
pub fn lookup_result(outcome: Outcome<Option<Article>>, word: &str) -> Option<Lookup> {
    match outcome {
        Outcome::Finished(Some(article)) => Some(Lookup::Article(article)),
        Outcome::Finished(None) => Some(Lookup::NoEntry {
            word: word.to_owned(),
        }),
        Outcome::TimedOut | Outcome::Panicked => Some(Lookup::Unfinished),
        Outcome::Cancelled => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, thread, time::Duration};

    use addict_core::{
        dictionary::{Article, Dictionary, Sample},
        input::{Source, WindowState},
    };
    use addict_platform::work::{Outcome, Work};

    use super::{Edit, LOOKUP_LIMIT, Manual, look_up, lookup_result};
    use crate::view::Lookup;

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
    fn looked_up(
        dictionary: impl Dictionary + 'static,
        word: &str,
    ) -> (Outcome<Option<Article>>, bool) {
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let caller = thread::current().id();
        let job = look_up(
            Some(&work),
            std::sync::Arc::new(dictionary),
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
        let (outcome, _) = looked_up(Sample, &word);
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
        let (outcome, elsewhere) = looked_up(Sample, "bank");
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
        let (outcome, _) = looked_up(Sample, &word);
        assert_eq!(outcome, Outcome::Finished(Sample.lookup("bank")));
        assert!(matches!(outcome, Outcome::Finished(Some(_))));
    }

    #[test]
    fn a_word_outside_the_sample_is_named_in_the_result() {
        let (outcome, _) = looked_up(Sample, "sat");
        assert_eq!(
            lookup_result(outcome, "sat"),
            Some(Lookup::NoEntry {
                word: "sat".to_owned()
            })
        );
    }

    /// A dictionary whose lookup panics.
    struct Broken;

    impl Dictionary for Broken {
        fn lookup(&self, _word: &str) -> Option<Article> {
            panic!("the broken test dictionary");
        }
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
            std::sync::Arc::new(Sample),
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
            Outcome::Finished(Sample.lookup("bank")),
            Outcome::Finished(None),
            Outcome::Cancelled,
            Outcome::TimedOut,
            Outcome::Panicked,
        ] {
            let shown = is_shown(&outcome);
            assert_eq!(lookup_result(outcome, "bank").is_some(), shown);
        }
    }

    #[test]
    fn the_lookup_limit_is_finite_and_short() {
        assert!(!LOOKUP_LIMIT.is_zero());
        assert!(LOOKUP_LIMIT.as_secs() <= 10);
    }
}
