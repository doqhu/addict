//! The manual path without its widgets: the text of the field, the word
//! chosen in it, the [`Input`] built from them and the lookup of the word.
//!
//! A text of exactly one word is that word without context: it is chosen at
//! once, with no click, and its input has no sentence. A typed or pasted text
//! of more words is a sentence, and the clicked word is the input together
//! with it. A recognized text often holds several sentences: the clicked word
//! goes with the sentence around it only, and with none when that sentence
//! is the word alone.
//!
//! The text comes from the user, from a screen capture, from the
//! accessibility bus or from the primary selection or the clipboard. A
//! recognized text is always a new text with the source OCR, and an edit of
//! it keeps that source; the source is manual again once the field holds no
//! word. A text that arrived from the bus ([`Manual::arrived`]) is the
//! sentence around the selected word, with that word chosen, and its source
//! is AT-SPI; it is one sentence, as a typed text is. A text selected or
//! copied ([`Manual::copied`]), read when a press gave no other context, is a
//! new text as one line with the source clipboard, and one sentence too; only
//! one of at most [`CLIPBOARD_MAX_CHARS`] counts ([`clipboard_line`]). While
//! a source is being read and there is no input, the window says that it is
//! being read.
//!
//! [`Manual`] is the state and holds no widget, so what an edit or a click
//! does to it is tested without a display. [`offline`] starts a lookup on
//! the work runner, for English, and for Spanish or Catalan only when the
//! last known state of its indexes says there is one; it tries the chosen
//! language first and the others after it, and tells which answered; a
//! Russian word is looked up in the Russian index of the chosen language
//! alone, whatever the state of the other indexes; and
//! [`lookup_result`] and [`result_in`] say what the window shows for its
//! outcome; [`check_indexes`] asks on the runner where the dictionary indexes
//! stand. The online requests about the input are
//! [`translate`](crate::translate)'s. Putting any of it on the screen is
//! [`resident`](crate::resident)'s.

use std::{ops::Range, sync::Arc, time::Duration};

use addict_core::{
    input::{Input, Source, WindowState, window_state},
    sentence,
    translation::{Source as Language, is_russian},
};
use addict_platform::{
    atspi::Context,
    work::{Job, Outcome, Work},
};

use crate::{
    dictionaries::{Answer, Articles, Indexes, Looked, State},
    online::Kind,
    translate::SENTENCE_LIMIT,
    view::Lookup,
};

/// How long the lookup of one word may take.
pub(crate) const LOOKUP_LIMIT: Duration = Duration::from_secs(2);

/// The most characters a selected or copied text may have, as one line, to
/// become the field's text: the most whose sentence is still sent for
/// translation, so a copied file or log does not pass.
pub(crate) const CLIPBOARD_MAX_CHARS: usize = SENTENCE_LIMIT;

/// Why a selected or copied text gives no line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoLine {
    /// Empty or only whitespace.
    Blank,
    /// Longer than [`CLIPBOARD_MAX_CHARS`]: this many characters as one line.
    TooLong(usize),
}

/// The selected or copied text `raw` as the one line the field would hold,
/// or why there is none: blank, or longer than [`CLIPBOARD_MAX_CHARS`]
/// characters.
pub fn clipboard_line(raw: &str) -> Result<String, NoLine> {
    let line = sentence::pasted(raw).ok_or(NoLine::Blank)?;
    match line.chars().count() {
        chars if chars > CLIPBOARD_MAX_CHARS => Err(NoLine::TooLong(chars)),
        _ => Ok(line),
    }
}

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
    /// The text is new and a word of it, one of several, was chosen with it:
    /// it is now the input. The lookup to start has this number.
    Chosen(u64),
}

/// The text of the field, its words, where it came from and what was chosen
/// in it.
#[derive(Debug)]
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
    /// Where the text came from: the user, or a screen capture.
    source: Source,
    /// The source being read now, if one is: a capture being recognized, or
    /// the focused application being asked over the accessibility bus.
    reading: Option<Source>,
}

impl Default for Manual {
    fn default() -> Self {
        Self {
            sentence: String::new(),
            words: Vec::new(),
            chosen: None,
            input: None,
            run: 0,
            text: 0,
            source: Source::Manual,
            reading: None,
        }
    }
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
    /// text of more words.
    pub fn kind(&self) -> Kind {
        Kind::of(self.words.len())
    }

    /// Where the text of the field came from.
    pub fn source(&self) -> Source {
        self.source
    }

    /// The window state of this moment: a source being read while there is
    /// no input, or what the input is.
    pub fn state(&self) -> WindowState {
        window_state(self.reading, self.input.as_ref())
    }

    /// The source being read now, if one is.
    pub fn reading(&self) -> Option<Source> {
        self.reading
    }

    /// Sets the source being read now; `None` when none is.
    pub fn set_reading(&mut self, reading: Option<Source>) {
        self.reading = reading;
    }

    /// The field holds `text`: its words are rebuilt, and what was chosen in
    /// the old text is gone, with the outcome of its lookup. A text of exactly
    /// one word has that word chosen at once. The source stays, unless the
    /// text has no word: then it is manual again. An edit ends the reading
    /// of a source, even one that leaves the text as it was: the user's text
    /// wins over the one being read.
    pub fn edit(&mut self, text: String) -> Edit {
        self.reading = None;
        if text == self.sentence {
            return Edit::Same;
        }
        self.replace(text)
    }

    /// The field holds `text` recognized from a screen capture: it is always
    /// a new text, even when it reads as the one held, its source is OCR and
    /// the reading is over. Never [`Edit::Same`].
    pub fn recognized(&mut self, text: String) -> Edit {
        self.reading = None;
        self.source = Source::Ocr;
        self.replace(text)
    }

    /// The accessibility bus gave the `context` of a selection: the field
    /// holds the sentence around the selected word, or the word alone when
    /// there is none, as a new text with the source AT-SPI, and the reading
    /// is over. The selected word is the one chosen: the word of the text
    /// whose range is the selection's, or else the only one the selection
    /// touches, so a word that stands twice in the sentence is the right
    /// one. A text of one word has that word chosen, as always; when no word
    /// is, nothing is chosen until a click. Never [`Edit::Same`].
    pub fn arrived(&mut self, context: Context) -> Edit {
        self.reading = None;
        self.source = Source::AtSpi;
        let Some(selected) = context.sentence else {
            // A selection of several lines is one line.
            return self.replace(sentence::pasted(&context.word).unwrap_or_default());
        };
        let edit = self.replace(selected.text);
        if edit != Edit::Cleared {
            return edit;
        }
        let range = selected.word;
        let touching: Vec<usize> = (0..self.words.len())
            .filter(|&index| {
                let word = &self.words[index];
                word.start < range.end && range.start < word.end
            })
            .collect();
        let index = self
            .words
            .iter()
            .position(|word| *word == range)
            .or_else(|| (touching.len() == 1).then(|| touching[0]));
        index
            .and_then(|index| self.choose(index))
            .map_or(Edit::Cleared, Edit::Chosen)
    }

    /// The primary selection or the clipboard gave `text` after a press
    /// without other context: the
    /// field holds it as one line, a new text with the source clipboard, and
    /// the reading is over. A text of one word has that word chosen at once;
    /// in more words nothing is chosen until a click. A blank text leaves no
    /// word, and the source is manual. Never [`Edit::Same`].
    pub fn copied(&mut self, text: &str) -> Edit {
        self.reading = None;
        self.source = Source::Clipboard;
        self.replace(sentence::pasted(text).unwrap_or_default())
    }

    /// `text` is a new text of the field.
    fn replace(&mut self, text: String) -> Edit {
        self.words = sentence::words(&text);
        if self.words.is_empty() {
            self.source = Source::Manual;
        }
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
    /// text: it becomes the input, written as it is looked up, with its
    /// sentence when it has one (see [`Manual::sentence_of`]). Gives the
    /// number of the lookup to start, which replaces any earlier one, or
    /// `None` when there is no such word.
    pub fn choose(&mut self, index: usize) -> Option<u64> {
        let word = self
            .words
            .get(index)
            .and_then(|range| self.sentence.get(range.clone()))?;
        let context = self
            .sentence_of(index)
            .and_then(|(_, span)| self.sentence.get(span));
        let input = Input::new(&sentence::lookup_word(word), context, self.source).ok()?;
        self.chosen = Some(index);
        self.input = Some(input);
        self.run += 1;
        Some(self.run)
    }

    /// The sentence of the word with `index`: its number in the text and its
    /// byte range, or `None` when the word has none. A text of more than one
    /// word that was not recognized is one sentence, the whole text, numbered
    /// 0. In a recognized text it is the sentence around the word, numbered
    /// in the order of the text, and only when it has another word: a word
    /// that is a sentence alone has no context.
    pub fn sentence_of(&self, index: usize) -> Option<(usize, Range<usize>)> {
        let word = self.words.get(index)?;
        match self.source {
            Source::Manual | Source::AtSpi | Source::Clipboard => {
                (self.words.len() > 1).then_some((0, 0..self.sentence.len()))
            }
            Source::Ocr => {
                let span = sentence::around(&self.sentence, word.clone());
                let words = self
                    .words
                    .iter()
                    .filter(|other| span.start <= other.start && other.end <= span.end)
                    .count();
                let number = sentence::sentences(&self.sentence)
                    .iter()
                    .take_while(|other| other.end <= span.start)
                    .count();
                (words > 1).then_some((number, span))
            }
        }
    }

    /// Whether `run` is the last started lookup and its word is still chosen.
    pub fn awaits(&self, run: u64) -> bool {
        run == self.run && self.input.is_some()
    }

    /// The input is to be looked up anew, as it is: the number of the new
    /// lookup, after which the outcome of the one before is not awaited.
    /// Nothing else changes. `None` without an input.
    pub fn again(&mut self) -> Option<u64> {
        self.input.as_ref()?;
        self.run += 1;
        Some(self.run)
    }
}

/// Starts the English lookup of `word` in `articles` on `work`, with a
/// deadline. `on_outcome` runs once on the worker thread. Without a runner
/// nothing starts and the result to show is given back.
#[cfg(test)]
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

/// Starts the lookup of `word` read first in `chosen` in `articles` on
/// `work`, with a deadline, through [`Articles::look_up_first`].
/// `on_outcome` runs once on the worker thread with the language that
/// answered, or `chosen` when the lookup did not finish. Without a runner
/// nothing starts and the result to show is given back.
fn look_up_first(
    work: Option<&Work>,
    articles: Arc<dyn Articles>,
    word: String,
    chosen: Language,
    on_outcome: impl FnOnce(Language, Outcome<Looked>) + Send + 'static,
) -> Result<Job, Lookup> {
    let work = work.ok_or(Lookup::Unfinished)?;
    Ok(work.run(
        LOOKUP_LIMIT,
        async move { articles.look_up_first(chosen, &word) },
        move |outcome| {
            let (language, outcome) = answering(chosen, outcome);
            on_outcome(language, outcome);
        },
    ))
}

/// Starts the lookup of the Russian `word` translated into `chosen` in
/// `articles` on `work`, with a deadline, through
/// [`Articles::look_up_russian`]. `on_outcome` runs once on the worker
/// thread, always with `chosen`: no other language answers. Without a
/// runner nothing starts and the result to show is given back.
fn look_up_russian(
    work: Option<&Work>,
    articles: Arc<dyn Articles>,
    word: String,
    chosen: Language,
    on_outcome: impl FnOnce(Language, Outcome<Looked>) + Send + 'static,
) -> Result<Job, Lookup> {
    let work = work.ok_or(Lookup::Unfinished)?;
    Ok(work.run(
        LOOKUP_LIMIT,
        async move { articles.look_up_russian(chosen, &word) },
        move |outcome| on_outcome(chosen, outcome),
    ))
}

/// The language that answered in `outcome` and the outcome of the lookup
/// alone; `chosen` when the lookup did not finish.
fn answering(
    chosen: Language,
    outcome: Outcome<(Language, Looked)>,
) -> (Language, Outcome<Looked>) {
    match outcome {
        Outcome::Finished((language, looked)) => (language, Outcome::Finished(looked)),
        Outcome::Cancelled => (chosen, Outcome::Cancelled),
        Outcome::TimedOut => (chosen, Outcome::TimedOut),
        Outcome::Panicked => (chosen, Outcome::Panicked),
    }
}

/// The offline result of `word` read in the `chosen` language; `known` is
/// where the indexes stood when last asked, `None` before the first answer.
/// For English, the lookup. For another language, its lookup when `known`
/// says its index is there, open or not readable, so that a damaged file is
/// named. The lookup tries the chosen language first and, when it has no
/// entry for the word, the other languages
/// ([`Articles::look_up_first`]); `on_outcome` is given the language that
/// answered. Otherwise nothing starts and nothing is asked of `articles`:
/// the line that says there is no dictionary is given back, so no article
/// of a word spelt alike in another language is shown. A Russian word
/// ([`is_russian`]) is looked up first of all, and only in the Russian index
/// of the chosen language, whatever `known` says. No wildcard arm: a new
/// language does not compile until it is given its offline result.
pub fn offline(
    work: Option<&Work>,
    articles: Arc<dyn Articles>,
    word: String,
    chosen: Language,
    known: Option<Indexes>,
    on_outcome: impl FnOnce(Language, Outcome<Looked>) + Send + 'static,
) -> Result<Job, Lookup> {
    if is_russian(&word) {
        return look_up_russian(work, articles, word, chosen, on_outcome);
    }
    match chosen {
        Language::En => look_up_first(work, articles, word, chosen, on_outcome),
        Language::Es | Language::Ca => {
            match known.and_then(|indexes| indexes.of_language(chosen)) {
                Some(State::Open | State::Unreadable) => {
                    look_up_first(work, articles, word, chosen, on_outcome)
                }
                Some(State::Missing) | None => Err(Lookup::NoDictionary(chosen)),
            }
        }
    }
}

/// Whether the offline result `lookup` of a word read in `language` asks
/// for the indexes to be checked again: the line that there is no
/// dictionary, for a language with an index of its own, which may have been
/// imported since the last check. No language without an index asks.
pub fn checks_again(lookup: &Lookup, language: Language) -> bool {
    matches!(lookup, Lookup::NoDictionary(_)) && Indexes::OPEN.of_language(language).is_some()
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

/// What the window shows for the `outcome` of the lookup of `word`
/// answered in `language`, or `None` when it shows nothing new. English is
/// [`lookup_result`]. For another language what its indexes gave is shown
/// by kind, in that language, and a language whose indexes were not there
/// after all has the line that says there is no dictionary. A Russian word
/// has its translations into `language`, or the line that says there is no
/// Russian dictionary of it. No wildcard arm: a new language does not
/// compile until it is given its result.
pub fn result_in(outcome: Outcome<Looked>, word: &str, language: Language) -> Option<Lookup> {
    if is_russian(word) {
        return Some(match lookup_result(outcome, word)? {
            Lookup::FromRussian { translations, .. } => Lookup::FromRussian {
                language,
                translations,
            },
            Lookup::NotImported { .. } => Lookup::NoRussianDictionary(language),
            other => other,
        });
    }
    match language {
        Language::En => lookup_result(outcome, word),
        Language::Es | Language::Ca => Some(match lookup_result(outcome, word)? {
            Lookup::Grouped { grouped, .. } => Lookup::Grouped { language, grouped },
            Lookup::NotImported { .. } => Lookup::NoDictionary(language),
            other => other,
        }),
    }
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
            // Only another language gives one, and [`result_in`] names it.
            Answer::Grouped(grouped) => Lookup::Grouped {
                language: Language::En,
                grouped,
            },
            // Only a Russian word gives one, and [`result_in`] names its
            // language.
            Answer::FromRussian(translations) => Lookup::FromRussian {
                language: Language::En,
                translations,
            },
            Answer::NoEntry => Lookup::NoEntry { word: word() },
            Answer::NotImported => Lookup::NotImported { word: word() },
            Answer::Unreadable(file) => Lookup::Unreadable(file),
        }),
        Outcome::TimedOut | Outcome::Panicked => Some(Lookup::Unfinished),
        Outcome::Cancelled => None,
    }
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
        input::{Source, WindowState},
        sentence,
        translation::Source as Language,
    };
    use addict_platform::{
        atspi::Context,
        work::{Outcome, Work},
    };

    use super::{
        CLIPBOARD_MAX_CHARS, Edit, LOOKUP_LIMIT, Manual, NoLine, check_indexes, checks_again,
        clipboard_line, look_up, lookup_result, offline, result_in,
    };
    use crate::{
        dictionaries::{
            Answer, Articles, Indexes, Looked, State,
            tests::{FREEDICT, OEWN, Sampled},
        },
        online::Kind,
        translate::{
            SENTENCE_LIMIT,
            tests::{BANK, RIVER, RIVER_WORD, text_at_the_limit},
        },
        view::Lookup,
    };

    const SENTENCE: &str = "I sat on the bank.";

    /// How long a test waits for an outcome that must come.
    pub(crate) const WAIT: Duration = Duration::from_secs(10);

    pub(crate) fn with_sentence(sentence: &str) -> Manual {
        let mut manual = Manual::default();
        assert_ne!(manual.edit(sentence.to_owned()), Edit::Same);
        manual
    }

    /// The number of the lookup that an edit to the lone word `text` starts.
    pub(crate) fn lone(manual: &mut Manual, text: &str) -> u64 {
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

    /// A source of articles that must not be asked.
    struct Unasked;

    impl Articles for Unasked {
        fn indexes(&self) -> Indexes {
            panic!("the indexes were asked")
        }

        fn look_up(&self, word: &str) -> Looked {
            panic!("{word} was looked up")
        }
    }

    /// The indexes with the Spanish one as `spanish`.
    fn with_spanish(spanish: State) -> Indexes {
        Indexes {
            spanish,
            ..Indexes::OPEN
        }
    }

    /// The indexes with the Catalan one as `catalan`.
    fn with_catalan(catalan: State) -> Indexes {
        Indexes {
            catalan,
            ..Indexes::OPEN
        }
    }

    #[test]
    fn a_spanish_word_without_its_index_or_a_catalan_word_is_not_looked_up() {
        let work = Work::start().expect("runner");
        let known = [
            None,
            Some(Indexes::OPEN),
            Some(with_spanish(State::Missing)),
        ];
        for language in [Language::Es, Language::Ca] {
            for indexes in known {
                // `real`, `once` and `fin` are English words too.
                for word in ["real", "once", "fin", "banco"] {
                    let started = offline(
                        Some(&work),
                        std::sync::Arc::new(Unasked),
                        word.to_owned(),
                        language,
                        indexes,
                        |_, _| panic!("a lookup ended"),
                    );
                    assert_eq!(started.err(), Some(Lookup::NoDictionary(language)));
                }
            }
        }
        // Each language by its own indexes, whatever the other's do.
        for state in [State::Open, State::Unreadable] {
            for (language, known) in [
                (Language::Ca, with_spanish(state)),
                (Language::Es, with_catalan(state)),
            ] {
                let started = offline(
                    Some(&work),
                    std::sync::Arc::new(Unasked),
                    "real".to_owned(),
                    language,
                    Some(known),
                    |_, _| panic!("a lookup ended"),
                );
                assert_eq!(started.err(), Some(Lookup::NoDictionary(language)));
            }
        }
        // Also without a runner: nothing was to run.
        let started = offline(
            None,
            std::sync::Arc::new(Unasked),
            "real".to_owned(),
            Language::Es,
            None,
            |_, _| {},
        );
        assert_eq!(started.err(), Some(Lookup::NoDictionary(Language::Es)));
    }

    /// A source of articles that answers a lookup in its language, and in
    /// no other, with the same answer.
    struct Speaking(Language, Answer);

    impl Articles for Speaking {
        fn indexes(&self) -> Indexes {
            with_spanish(State::Open)
        }

        fn look_up(&self, word: &str) -> Looked {
            panic!("{word} was looked up in English")
        }

        fn look_up_in(&self, language: Language, _word: &str) -> Looked {
            assert_eq!(language, self.0);
            Looked {
                answer: self.1.clone(),
                indexes: with_spanish(State::Open),
            }
        }
    }

    fn banco() -> addict_dictionary::lookup::Grouped {
        addict_dictionary::lookup::Grouped {
            entries: Vec::new(),
            sense_metadata: Vec::new(),
            definitions: Some(vec![addict_core::dictionary::Sense {
                part_of_speech: None,
                entries: Vec::new(),
                definition: "Mueble alargado para sentarse.".to_owned(),
                synonyms: vec!["banca".to_owned()],
            }]),
            russian: Some(Vec::new()),
            synonyms: None,
            verb: None,
            english: None,
        }
    }

    /// The outcome of the offline lookup of `word` read in the language of
    /// `articles` with `known`, which must start a job.
    fn looked_up_offline(articles: Speaking, known: Indexes, word: &str) -> Outcome<Looked> {
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let language = articles.0;
        let job = offline(
            Some(&work),
            std::sync::Arc::new(articles),
            word.to_owned(),
            language,
            Some(known),
            move |answered, outcome| {
                let _ = sender.send((answered, outcome));
            },
        )
        .expect("job");
        let (answered, outcome) = receiver.recv_timeout(WAIT).expect("outcome");
        drop(job);
        assert_eq!(answered, language);
        outcome
    }

    /// A source of articles whose Spanish alone has the word: English and
    /// Catalan have no entry for it.
    struct SpanishOnly;

    impl Articles for SpanishOnly {
        fn indexes(&self) -> Indexes {
            Indexes {
                spanish: State::Open,
                catalan: State::Open,
                ..Indexes::OPEN
            }
        }

        fn look_up(&self, _word: &str) -> Looked {
            Looked {
                answer: Answer::NoEntry,
                indexes: self.indexes(),
            }
        }

        fn look_up_in(&self, language: Language, word: &str) -> Looked {
            match language {
                Language::Es => Looked {
                    answer: Answer::Grouped(banco()),
                    indexes: self.indexes(),
                },
                Language::En | Language::Ca => self.look_up(word),
            }
        }
    }

    /// The language that answered the offline lookup of `word` read first
    /// in `chosen`, and its outcome.
    fn answered_offline(
        articles: impl Articles + 'static,
        chosen: Language,
        known: Option<Indexes>,
        word: &str,
    ) -> (Language, Outcome<Looked>) {
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let job = offline(
            Some(&work),
            std::sync::Arc::new(articles),
            word.to_owned(),
            chosen,
            known,
            move |answered, outcome| {
                let _ = sender.send((answered, outcome));
            },
        )
        .expect("job");
        let reported = receiver.recv_timeout(WAIT).expect("outcome");
        drop(job);
        reported
    }

    #[test]
    fn a_word_the_chosen_language_lacks_is_answered_and_named_in_another() {
        let known = Some(SpanishOnly.indexes());
        for chosen in Language::ALL {
            let (answered, outcome) = answered_offline(SpanishOnly, chosen, known, "cerró");
            assert_eq!(answered, Language::Es, "{chosen:?}");
            assert_eq!(
                result_in(outcome, "cerró", answered),
                Some(Lookup::Grouped {
                    language: Language::Es,
                    grouped: banco(),
                }),
                "{chosen:?}"
            );
        }
        // No language has it: the chosen one's miss.
        let (answered, outcome) = answered_offline(Sampled, Language::En, None, "sat");
        assert_eq!(answered, Language::En);
        assert_eq!(
            lookup_result(outcome, "sat"),
            Some(Lookup::NoEntry {
                word: "sat".to_owned()
            })
        );
    }

    /// A source of articles that answers a Russian word with `answer` and
    /// keeps the language it was asked for; any other lookup panics.
    struct RussianOnly(Answer, std::sync::Arc<std::sync::Mutex<Vec<Language>>>);

    impl Articles for RussianOnly {
        fn indexes(&self) -> Indexes {
            panic!("the indexes were asked")
        }

        fn look_up(&self, word: &str) -> Looked {
            panic!("{word} was looked up in English")
        }

        fn look_up_in(&self, language: Language, word: &str) -> Looked {
            panic!("{word} was looked up in {language:?}")
        }

        fn look_up_russian(&self, into: Language, _word: &str) -> Looked {
            self.1.lock().expect("asked").push(into);
            Looked {
                answer: self.0.clone(),
                indexes: Indexes::OPEN,
            }
        }
    }

    #[test]
    fn a_russian_word_is_looked_up_in_the_russian_index_of_the_chosen_language_alone() {
        let translations = vec!["perro".to_owned(), "cachorro".to_owned()];
        // Whatever the indexes of the language stand at, also unknown.
        let known = [
            None,
            Some(Indexes::OPEN),
            Some(with_spanish(State::Missing)),
            Some(with_catalan(State::Unreadable)),
        ];
        for chosen in Language::ALL {
            for indexes in known {
                for word in ["собака", "«Дом»,", "еж"] {
                    let asked = std::sync::Arc::default();
                    let articles = RussianOnly(
                        Answer::FromRussian(translations.clone()),
                        Arc::clone(&asked),
                    );
                    let (answered, outcome) = answered_offline(articles, chosen, indexes, word);
                    assert_eq!(answered, chosen, "{word}");
                    assert_eq!(*asked.lock().expect("asked"), [chosen]);
                    assert_eq!(
                        result_in(outcome, word, chosen),
                        Some(Lookup::FromRussian {
                            language: chosen,
                            translations: translations.clone(),
                        })
                    );
                }
            }
        }
        let finished = |answer: Answer| {
            Outcome::Finished(Looked {
                answer,
                indexes: Indexes::OPEN,
            })
        };
        assert_eq!(
            result_in(finished(Answer::NotImported), "собака", Language::Ca),
            Some(Lookup::NoRussianDictionary(Language::Ca))
        );
        assert_eq!(
            result_in(finished(Answer::NoEntry), "собаки", Language::Es),
            Some(Lookup::NoEntry {
                word: "собаки".to_owned()
            })
        );
        let file =
            crate::dictionaries::IndexFile::of(addict_dictionary::import::Source::WikdictRuEn);
        assert_eq!(
            result_in(finished(Answer::Unreadable(file)), "Москва", Language::En),
            Some(Lookup::Unreadable(file))
        );
        assert_eq!(
            result_in(Outcome::TimedOut, "собака", Language::Es),
            Some(Lookup::Unfinished)
        );
        assert!(!checks_again(
            &Lookup::NoRussianDictionary(Language::Es),
            Language::Es
        ));
        // Without a runner nothing is asked.
        let started = offline(
            None,
            std::sync::Arc::new(Unasked),
            "собака".to_owned(),
            Language::Es,
            None,
            |_, _| panic!("a lookup ended"),
        );
        assert_eq!(started.err(), Some(Lookup::Unfinished));
    }

    #[test]
    fn a_lookup_that_did_not_finish_is_the_chosen_language_s() {
        for (outcome, shown) in [
            (Outcome::TimedOut, Some(Lookup::Unfinished)),
            (Outcome::Panicked, Some(Lookup::Unfinished)),
            (Outcome::Cancelled, None),
        ] {
            let (language, outcome) = super::answering(Language::Ca, outcome);
            assert_eq!(language, Language::Ca);
            assert_eq!(lookup_result(outcome, "banc"), shown);
        }
    }

    #[test]
    fn only_a_word_of_a_language_without_its_indexes_asks_for_them_again() {
        for language in [Language::Es, Language::Ca] {
            assert!(checks_again(&Lookup::NoDictionary(language), language));
        }
        for lookup in [
            Lookup::Pending,
            Lookup::Unfinished,
            Lookup::NoEntry {
                word: "banco".to_owned(),
            },
        ] {
            for language in Language::ALL {
                assert!(!checks_again(&lookup, language), "{lookup:?}");
            }
        }
    }

    #[test]
    fn a_spanish_word_with_its_index_is_looked_up_offline() {
        let outcome = looked_up_offline(
            Speaking(Language::Es, Answer::Grouped(banco())),
            with_spanish(State::Open),
            "banco",
        );
        assert_eq!(
            result_in(outcome, "banco", Language::Es),
            Some(Lookup::Grouped {
                language: Language::Es,
                grouped: banco(),
            })
        );
        // A file that is no index is looked up too, so that it is named.
        let file = crate::dictionaries::IndexFile::of(addict_dictionary::import::Source::KaikkiEs);
        let outcome = looked_up_offline(
            Speaking(Language::Es, Answer::Unreadable(file)),
            with_spanish(State::Unreadable),
            "banco",
        );
        assert_eq!(
            result_in(outcome, "banco", Language::Es),
            Some(Lookup::Unreadable(file))
        );
        assert_eq!(file.index, "kaikki-es-Espanol.index");
    }

    #[test]
    fn a_catalan_word_with_its_index_is_looked_up_in_catalan() {
        for catalan in [State::Open, State::Unreadable] {
            let outcome = looked_up_offline(
                Speaking(Language::Ca, Answer::Grouped(banco())),
                with_catalan(catalan),
                "banc",
            );
            assert_eq!(
                result_in(outcome, "banc", Language::Ca),
                Some(Lookup::Grouped {
                    language: Language::Ca,
                    grouped: banco(),
                })
            );
        }
        let finished = |answer: Answer| {
            Outcome::Finished(Looked {
                answer,
                indexes: with_catalan(State::Missing),
            })
        };
        assert_eq!(
            result_in(finished(Answer::NotImported), "banc", Language::Ca),
            Some(Lookup::NoDictionary(Language::Ca))
        );
        assert_eq!(
            result_in(finished(Answer::NoEntry), "zzz", Language::Ca),
            Some(Lookup::NoEntry {
                word: "zzz".to_owned()
            })
        );
    }

    #[test]
    fn a_spanish_result_names_a_miss_and_a_removed_index_as_spanish() {
        let finished = |answer: Answer| {
            Outcome::Finished(Looked {
                answer,
                indexes: with_spanish(State::Missing),
            })
        };
        assert_eq!(
            result_in(finished(Answer::NoEntry), "zzz", Language::Es),
            Some(Lookup::NoEntry {
                word: "zzz".to_owned()
            })
        );
        // The index was removed after the check said it was there.
        assert_eq!(
            result_in(finished(Answer::NotImported), "banco", Language::Es),
            Some(Lookup::NoDictionary(Language::Es))
        );
        assert_eq!(
            result_in(Outcome::TimedOut, "banco", Language::Es),
            Some(Lookup::Unfinished)
        );
        assert_eq!(result_in(Outcome::Cancelled, "banco", Language::Es), None);
        // English is the result it always was.
        for outcome in [
            finished(Answer::Article(addict_core::dictionary::Article {
                sense_metadata: Vec::new(),
                entries: Vec::new(),
                senses: Vec::new(),
                translations: vec!["банк".to_owned()],
            })),
            finished(Answer::NotImported),
            Outcome::Panicked,
        ] {
            assert_eq!(
                result_in(outcome.clone(), "bank", Language::En),
                lookup_result(outcome, "bank")
            );
        }
    }

    #[test]
    fn an_english_word_is_looked_up_as_before() {
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let job = offline(
            Some(&work),
            std::sync::Arc::new(Sampled),
            "bank".to_owned(),
            Language::En,
            None,
            move |answered, outcome| {
                let _ = sender.send((answered, outcome));
            },
        )
        .expect("job");
        let (answered, outcome) = receiver.recv_timeout(WAIT).expect("outcome");
        drop(job);
        assert_eq!(answered, Language::En);
        assert_eq!(
            lookup_result(outcome, "bank"),
            Some(Lookup::Article(Sample.lookup("bank").expect("bank")))
        );
        let started = offline(
            None,
            std::sync::Arc::new(Sampled),
            "bank".to_owned(),
            Language::En,
            None,
            |_, _| {},
        );
        assert_eq!(started.err(), Some(Lookup::Unfinished));
    }

    #[test]
    fn the_same_input_again_is_a_new_lookup_of_it() {
        let mut manual = Manual::default();
        assert_eq!(manual.again(), None);
        manual.edit(SENTENCE.to_owned());
        assert_eq!(manual.again(), None);
        let run = manual.choose(BANK).expect("run");
        let input = manual.input().cloned();
        let again = manual.again().expect("again");
        assert_ne!(again, run);
        assert!(!manual.awaits(run));
        assert!(manual.awaits(again));
        assert_eq!(manual.input().cloned(), input);
        assert_eq!(manual.chosen(), Some(BANK));
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
        spanish: State::Missing,
        catalan: State::Missing,
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
    pub(crate) fn is_shown<T>(outcome: &Outcome<T>) -> bool {
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
        assert_eq!(with_sentence(RIVER).kind(), Kind::Sentence);
        // However long: whether it is too long is judged on a sentence.
        assert_eq!(SENTENCE_LIMIT, 500);
        let over = format!("{}.", text_at_the_limit());
        assert_eq!(with_sentence(&over).kind(), Kind::Sentence);
    }

    const OCR_DICTIONARY: WindowState = WindowState::Dictionary {
        source: Source::Ocr,
    };

    #[test]
    fn a_recognized_sentence_is_a_new_text_from_the_capture() {
        let mut manual = with_sentence(SENTENCE);
        manual.choose(4).expect("run");
        let text = manual.text_number();
        assert_eq!(
            manual.recognized("The bank is beside the river.".to_owned()),
            Edit::Cleared
        );
        assert_ne!(manual.text_number(), text);
        assert_eq!(manual.source(), Source::Ocr);
        assert_eq!(manual.input(), None);
        manual.choose(1).expect("run");
        let input = manual.input().expect("input");
        assert_eq!(input.word(), "bank");
        assert_eq!(input.sentence(), Some("The bank is beside the river."));
        assert_eq!(input.source(), Source::Ocr);
        assert_eq!(
            manual.state(),
            WindowState::Contextual {
                source: Source::Ocr
            }
        );
    }

    #[test]
    fn the_same_text_recognized_again_is_still_a_new_text() {
        let mut manual = with_sentence(SENTENCE);
        let run = manual.choose(4).expect("run");
        let text = manual.text_number();
        assert_eq!(manual.recognized(SENTENCE.to_owned()), Edit::Cleared);
        assert_ne!(manual.text_number(), text);
        assert!(!manual.awaits(run));
        assert_eq!(manual.chosen(), None);
        assert_eq!(manual.source(), Source::Ocr);
    }

    /// The index of the first word of `manual` that reads `word`.
    fn index_of(manual: &Manual, word: &str) -> usize {
        manual
            .words()
            .iter()
            .position(|range| &manual.sentence()[range.clone()] == word)
            .expect("word")
    }

    #[test]
    fn a_word_of_a_recognized_text_goes_with_its_sentence_only() {
        let mut manual = Manual::default();
        let text = "I sat on the bank. The river was wide.";
        manual.recognized(text.to_owned());
        let river = index_of(&manual, "river");
        manual.choose(river).expect("run");
        let input = manual.input().expect("input");
        assert_eq!(input.word(), "river");
        assert_eq!(input.sentence(), Some("The river was wide."));
        assert_eq!(input.source(), Source::Ocr);
        assert_eq!(manual.sentence_of(river), Some((1, 19..text.len())));
        let bank = index_of(&manual, "bank");
        assert_eq!(manual.sentence_of(bank), Some((0, 0..18)));
        manual.choose(bank).expect("run");
        assert_eq!(
            manual.input().expect("input").sentence(),
            Some("I sat on the bank.")
        );
    }

    #[test]
    fn a_word_that_is_a_recognized_sentence_alone_has_no_sentence() {
        let mut manual = Manual::default();
        manual.recognized("Contents. I sat on the bank.".to_owned());
        let contents = index_of(&manual, "Contents");
        assert_eq!(manual.sentence_of(contents), None);
        manual.choose(contents).expect("run");
        let input = manual.input().expect("input");
        assert_eq!(input.word(), "Contents");
        assert_eq!(input.sentence(), None);
        assert_eq!(
            manual.state(),
            WindowState::Dictionary {
                source: Source::Ocr
            }
        );
    }

    #[test]
    fn a_word_of_a_typed_text_of_sentences_goes_with_the_whole_text() {
        let text = "A b. C d.";
        let mut manual = with_sentence(text);
        assert_eq!(manual.sentence_of(2), Some((0, 0..text.len())));
        manual.choose(2).expect("run");
        assert_eq!(manual.input().expect("input").sentence(), Some(text));
        // A lone word has no sentence.
        let mut manual = Manual::default();
        lone(&mut manual, "bank");
        assert_eq!(manual.sentence_of(0), None);
        assert_eq!(manual.sentence_of(1), None);
    }

    #[test]
    fn a_recognized_lone_word_is_the_input_at_once() {
        let mut manual = Manual::default();
        let Edit::Lone(run) = manual.recognized("bank".to_owned()) else {
            panic!("not a lone word");
        };
        assert!(manual.awaits(run));
        assert_eq!(manual.input().expect("input").source(), Source::Ocr);
        assert_eq!(manual.state(), OCR_DICTIONARY);
    }

    #[test]
    fn an_edit_of_a_recognized_text_keeps_its_source() {
        let mut manual = Manual::default();
        manual.recognized("The bank is beside the river.".to_owned());
        assert_eq!(
            manual.edit("The bank is beside the road.".to_owned()),
            Edit::Cleared
        );
        assert_eq!(manual.source(), Source::Ocr);
        manual.choose(1).expect("run");
        assert_eq!(manual.input().expect("input").source(), Source::Ocr);
        lone(&mut manual, "bank");
        assert_eq!(manual.state(), OCR_DICTIONARY);
    }

    #[test]
    fn a_field_without_a_word_is_manual_again() {
        for text in ["", " ", "12 \u{2026}"] {
            let mut manual = Manual::default();
            manual.recognized("The bank is beside the river.".to_owned());
            assert_eq!(manual.edit(text.to_owned()), Edit::Cleared, "{text:?}");
            assert_eq!(manual.source(), Source::Manual, "{text:?}");
            assert_eq!(manual.state(), WindowState::Empty, "{text:?}");
            lone(&mut manual, "bank");
            assert_eq!(manual.state(), DICTIONARY, "{text:?}");
        }
        // A recognized text without a word is manual at once.
        let mut manual = Manual::default();
        assert_eq!(manual.recognized("\u{2014} ?!".to_owned()), Edit::Cleared);
        assert_eq!(manual.source(), Source::Manual);
    }

    #[test]
    fn a_capture_being_read_is_the_waiting_state_until_an_input() {
        let mut manual = Manual::default();
        manual.set_reading(Some(Source::Ocr));
        assert_eq!(
            manual.state(),
            WindowState::Waiting {
                source: Source::Ocr
            }
        );
        // An input wins over the reading.
        let mut chosen = Manual::default();
        lone(&mut chosen, "bank");
        chosen.set_reading(Some(Source::Ocr));
        assert_eq!(chosen.state(), DICTIONARY);
        // The recognized text ends the reading; with no word chosen yet the
        // label still names the capture as the source of the text.
        manual.recognized("The bank is beside the river.".to_owned());
        assert_eq!(manual.state(), WindowState::Empty);
        assert_eq!(
            crate::view::texts(manual.state(), manual.source()).source,
            "screen capture (OCR)"
        );
        manual.set_reading(Some(Source::Ocr));
        manual.set_reading(None);
        assert_eq!(manual.state(), WindowState::Empty);
    }

    #[test]
    fn an_edit_ends_the_reading() {
        let mut manual = Manual::default();
        manual.set_reading(Some(Source::Ocr));
        assert_eq!(manual.reading(), Some(Source::Ocr));
        assert!(matches!(manual.edit("river".to_owned()), Edit::Lone(_)));
        assert_eq!(manual.reading(), None);
        assert_eq!(manual.state(), DICTIONARY);
        // Also an edit that leaves the text as it was.
        let mut same = with_sentence("The bank is beside the river.");
        same.set_reading(Some(Source::Ocr));
        assert_eq!(
            same.edit("The bank is beside the river.".to_owned()),
            Edit::Same
        );
        assert_eq!(same.reading(), None);
        assert_eq!(same.state(), WindowState::Empty);
    }

    /// The context of the `nth` `needle` of `text`, as the bus adapter gives
    /// it: the word and its sentence, cut by the core rule.
    fn context(text: &str, needle: &str, nth: usize) -> Context {
        let byte = text.match_indices(needle).nth(nth).expect("needle").0;
        let start = text[..byte].chars().count();
        let end = start + needle.chars().count();
        Context {
            word: needle.to_owned(),
            sentence: sentence::of_selection(text, start, end),
        }
    }

    /// The word of the field that is chosen.
    fn chosen_word(manual: &Manual) -> Option<&str> {
        manual
            .chosen()
            .map(|index| &manual.sentence()[manual.words()[index].clone()])
    }

    #[test]
    fn an_arrived_selection_is_its_sentence_with_the_word_chosen() {
        let mut manual = Manual::default();
        manual.set_reading(Some(Source::AtSpi));
        let text = "// The bank was closed. We sat on the bank of the river.";
        let Edit::Chosen(run) = manual.arrived(context(text, "bank", 1)) else {
            panic!("chosen");
        };
        assert_eq!(manual.sentence(), "We sat on the bank of the river.");
        // The second `bank` of the text is the fifth word of this sentence.
        assert_eq!(manual.chosen(), Some(4));
        assert_eq!(chosen_word(&manual), Some("bank"));
        let input = manual.input().expect("input");
        assert_eq!(input.word(), "bank");
        assert_eq!(input.sentence(), Some("We sat on the bank of the river."));
        assert_eq!(input.source(), Source::AtSpi);
        assert_eq!(
            manual.state(),
            WindowState::Contextual {
                source: Source::AtSpi
            }
        );
        assert_eq!(manual.source(), Source::AtSpi);
        assert_eq!(manual.reading(), None);
        assert!(manual.awaits(run));
    }

    #[test]
    fn the_occurrence_that_was_selected_is_the_one_chosen() {
        let text = "// Lead. The bank by the bank.";
        for (nth, index) in [(0, 1), (1, 4)] {
            let mut manual = Manual::default();
            assert_eq!(
                manual.arrived(context(text, "bank", nth)),
                Edit::Chosen(manual.run)
            );
            assert_eq!(manual.chosen(), Some(index), "{nth}");
        }
    }

    #[test]
    fn an_arrived_text_is_always_a_new_text_and_drops_what_was_chosen() {
        let mut manual = with_sentence(SENTENCE);
        let old = manual.choose(4).expect("run");
        let number = manual.text_number();
        let edit = manual.arrived(context(&format!("// Lead. {SENTENCE}"), "bank", 0));
        // The same text again is still new.
        let Edit::Chosen(run) = edit else {
            panic!("{edit:?}");
        };
        assert_ne!(run, old);
        assert!(!manual.awaits(old));
        assert!(manual.awaits(run));
        assert_ne!(manual.text_number(), number);
    }

    #[test]
    fn an_arrived_word_without_a_sentence_is_a_dictionary_input() {
        let mut manual = Manual::default();
        let word = Context {
            word: "bank".to_owned(),
            sentence: None,
        };
        let Edit::Lone(run) = manual.arrived(word) else {
            panic!("lone");
        };
        assert_eq!(manual.sentence(), "bank");
        assert_eq!(manual.input().expect("input").sentence(), None);
        assert_eq!(manual.input().expect("input").source(), Source::AtSpi);
        assert_eq!(
            manual.state(),
            WindowState::Dictionary {
                source: Source::AtSpi
            }
        );
        assert!(manual.awaits(run));
    }

    #[test]
    fn an_arrived_sentence_that_is_the_word_alone_is_a_dictionary_input() {
        let mut manual = Manual::default();
        let arrived = context("// Lead. Bank.", "Bank", 0);
        assert_eq!(
            arrived.sentence.as_ref().map(|found| found.text.as_str()),
            Some("Bank.")
        );
        let edit = manual.arrived(arrived);
        assert!(matches!(edit, Edit::Lone(_)), "{edit:?}");
        assert_eq!(manual.input().expect("input").sentence(), None);
        assert_eq!(manual.source(), Source::AtSpi);
    }

    #[test]
    fn an_arrived_selection_of_several_words_without_a_sentence_chooses_none() {
        let mut manual = Manual::default();
        let words = Context {
            word: "the\n  bank".to_owned(),
            sentence: None,
        };
        assert_eq!(manual.arrived(words), Edit::Cleared);
        assert_eq!(manual.sentence(), "the bank");
        assert_eq!(manual.chosen(), None);
        assert_eq!(manual.input(), None);
        assert_eq!(manual.state(), WindowState::Empty);
        assert_eq!(manual.source(), Source::AtSpi);
    }

    #[test]
    fn a_part_of_a_word_or_a_wider_selection_chooses_what_it_touches() {
        let text = "We sat on the bank of the river.";
        let wider = format!("// Lead. {text}");
        let mut manual = Manual::default();
        // `ban` is a part of `bank` only.
        assert!(matches!(
            manual.arrived(context(&wider, "ban", 0)),
            Edit::Chosen(_)
        ));
        assert_eq!(chosen_word(&manual), Some("bank"));
        // `the bank` touches two words: none is chosen, the text is there.
        assert_eq!(
            manual.arrived(context(&wider, "the bank", 0)),
            Edit::Cleared
        );
        assert_eq!(manual.sentence(), text);
        assert_eq!(manual.chosen(), None);
        assert_eq!(manual.state(), WindowState::Empty);
    }

    #[test]
    fn a_text_of_an_arrived_selection_that_has_no_word_is_manual() {
        let mut manual = Manual::default();
        let digits = context("// Lead. 42.", "42", 0);
        assert!(digits.sentence.is_some());
        assert_eq!(manual.arrived(digits), Edit::Cleared);
        assert_eq!(manual.source(), Source::Manual);
    }

    #[test]
    fn a_source_being_read_by_the_bus_is_the_waiting_state_until_an_input() {
        let mut manual = Manual::default();
        manual.set_reading(Some(Source::AtSpi));
        assert_eq!(
            manual.state(),
            WindowState::Waiting {
                source: Source::AtSpi
            }
        );
        let texts = crate::view::texts(manual.state(), manual.source());
        assert_eq!(texts.source, "accessibility (AT-SPI)");
        assert_eq!(texts.status, "reading the source…");
        assert_eq!(manual.reading(), Some(Source::AtSpi));
    }

    const CLIPBOARD_DICTIONARY: WindowState = WindowState::Dictionary {
        source: Source::Clipboard,
    };

    #[test]
    fn a_copied_lone_word_is_the_input_at_once() {
        let mut manual = with_sentence(SENTENCE);
        manual.set_reading(Some(Source::Clipboard));
        let text = manual.text_number();
        let Edit::Lone(run) = manual.copied(" bank\n") else {
            panic!("not a lone word");
        };
        assert_ne!(manual.text_number(), text);
        assert!(manual.awaits(run));
        assert_eq!(manual.sentence(), "bank");
        assert_eq!(manual.reading(), None);
        assert_eq!(manual.source(), Source::Clipboard);
        let input = manual.input().expect("input");
        assert_eq!(input.word(), "bank");
        assert_eq!(input.sentence(), None);
        assert_eq!(input.source(), Source::Clipboard);
        assert_eq!(manual.state(), CLIPBOARD_DICTIONARY);
    }

    #[test]
    fn a_copied_sentence_waits_for_a_click_and_is_one_sentence() {
        let mut manual = Manual::default();
        manual.set_reading(Some(Source::Clipboard));
        assert_eq!(
            manual.state(),
            WindowState::Waiting {
                source: Source::Clipboard
            }
        );
        assert_eq!(manual.copied("I sat on the\nbank."), Edit::Cleared);
        assert_eq!(manual.sentence(), SENTENCE);
        assert_eq!(manual.source(), Source::Clipboard);
        assert_eq!(manual.input(), None);
        assert_eq!(manual.state(), WindowState::Empty);
        manual.choose(4).expect("run");
        let input = manual.input().expect("input");
        assert_eq!(input.word(), "bank");
        assert_eq!(input.sentence(), Some(SENTENCE));
        assert_eq!(input.source(), Source::Clipboard);
        assert_eq!(
            manual.state(),
            WindowState::Contextual {
                source: Source::Clipboard
            }
        );
        let texts = crate::view::texts(manual.state(), manual.source());
        assert_eq!(texts.source, "selection or clipboard");
    }

    #[test]
    fn a_blank_copied_text_is_no_input_and_manual() {
        let mut manual = with_sentence(SENTENCE);
        assert_eq!(manual.copied("  \n"), Edit::Cleared);
        assert_eq!(manual.sentence(), "");
        assert_eq!(manual.input(), None);
        assert_eq!(manual.source(), Source::Manual);
        assert_eq!(manual.state(), WindowState::Empty);
    }

    #[test]
    fn an_edit_of_a_copied_text_keeps_its_source() {
        let mut manual = Manual::default();
        manual.copied("The bank is beside the river.");
        assert_eq!(
            manual.edit("The bank is beside the road.".to_owned()),
            Edit::Cleared
        );
        assert_eq!(manual.source(), Source::Clipboard);
        manual.choose(1).expect("run");
        assert_eq!(manual.input().expect("input").source(), Source::Clipboard);
        lone(&mut manual, "bank");
        assert_eq!(manual.state(), CLIPBOARD_DICTIONARY);
    }

    #[test]
    fn a_clipboard_line_is_one_line_of_at_most_the_limit() {
        assert_eq!(clipboard_line(" bank\n").as_deref(), Ok("bank"));
        assert_eq!(
            clipboard_line("the river\nbank").as_deref(),
            Ok("the river bank")
        );
        for blank in ["", "  ", "\n\t"] {
            assert_eq!(clipboard_line(blank), Err(NoLine::Blank), "{blank:?}");
        }
        assert_eq!(CLIPBOARD_MAX_CHARS, 500);
        // Counted in characters after joining, not in bytes or as copied.
        let at_the_limit = "я".repeat(CLIPBOARD_MAX_CHARS);
        assert_eq!(clipboard_line(&at_the_limit), Ok(at_the_limit.clone()));
        let spread = format!("{}\n\n  ", "a ".repeat(250));
        assert_eq!(clipboard_line(&spread).map(|l| l.chars().count()), Ok(499));
        assert_eq!(
            clipboard_line(&format!("{at_the_limit}я")),
            Err(NoLine::TooLong(501))
        );
    }

    #[test]
    fn the_lookup_limit_is_finite_and_short() {
        assert!(!LOOKUP_LIMIT.is_zero());
        assert!(LOOKUP_LIMIT.as_secs() <= 10);
    }
}
