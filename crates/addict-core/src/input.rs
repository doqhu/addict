//! The input contract: what a result is built from and what the window shows.
//!
//! An [`Input`] is a word, an optional sentence around it and the [`Source`]
//! it came from. [`window_state`] derives the [`WindowState`] from what is
//! being read and what has arrived, so the window stores no mode of its own.

use std::{error::Error, fmt};

/// Where an input came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Typed or pasted by the user in the window.
    Manual,
    /// Recognised from a screen capture.
    Ocr,
    /// Read at a press from the primary selection, or from the clipboard
    /// when the selection gave no line.
    Clipboard,
}

impl Source {
    /// Every source, each exactly once.
    pub const ALL: [Source; 3] = [Source::Manual, Source::Ocr, Source::Clipboard];
}

/// A word, an optional sentence and their source.
///
/// Only [`Input::new`] builds one, so a held `Input` always has a non-empty
/// word, and a present sentence is never empty. Both texts are stored trimmed
/// of surrounding Unicode whitespace; inner whitespace and letter case are
/// kept as given. The two texts are independent: the sentence is not required
/// to contain the word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    word: String,
    sentence: Option<String>,
    source: Source,
}

impl Input {
    /// Builds an input from raw texts. A sentence that is blank after
    /// trimming counts as absent; a word that is blank after trimming is
    /// [`EmptyWord`], whatever the sentence is.
    pub fn new(word: &str, sentence: Option<&str>, source: Source) -> Result<Self, EmptyWord> {
        let word = word.trim();
        if word.is_empty() {
            return Err(EmptyWord);
        }
        let sentence = sentence
            .map(str::trim)
            .filter(|sentence| !sentence.is_empty())
            .map(str::to_owned);
        Ok(Self {
            word: word.to_owned(),
            sentence,
            source,
        })
    }

    /// The word: trimmed and never empty.
    pub fn word(&self) -> &str {
        &self.word
    }

    /// The sentence: trimmed and never empty when present.
    pub fn sentence(&self) -> Option<&str> {
        self.sentence.as_deref()
    }

    /// Where this input came from.
    pub fn source(&self) -> Source {
        self.source
    }
}

/// The word was empty or only whitespace, so there is no input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyWord;

impl fmt::Display for EmptyWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the word is empty")
    }
}

impl Error for EmptyWord {}

/// What the window shows. Always derived by [`window_state`], never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    /// No input and nothing is being read: the window asks for manual input.
    Empty,
    /// No input yet; `source` is being read.
    Waiting { source: Source },
    /// A word without a sentence, from `source`.
    Dictionary { source: Source },
    /// A word with a sentence, from `source`.
    Contextual { source: Source },
}

/// Derives the window state from the source being read, if any, and the
/// input that has arrived, if any.
///
/// An arrived input always wins and the state carries the input's own
/// source, so a `pending` that was cleared late cannot hide a result.
pub fn window_state(pending: Option<Source>, input: Option<&Input>) -> WindowState {
    match (input, pending) {
        (Some(input), _) => {
            let source = input.source();
            if input.sentence().is_some() {
                WindowState::Contextual { source }
            } else {
                WindowState::Dictionary { source }
            }
        }
        (None, Some(source)) => WindowState::Waiting { source },
        (None, None) => WindowState::Empty,
    }
}

#[cfg(test)]
mod tests {
    use super::{EmptyWord, Input, Source, WindowState, window_state};

    const SENTENCE: &str = "I sat on the bank.";

    /// The position of `source` in [`Source::ALL`]. No wildcard arm: a new
    /// variant does not compile here until it is given its position.
    fn position_in_all(source: Source) -> usize {
        match source {
            Source::Manual => 0,
            Source::Ocr => 1,
            Source::Clipboard => 2,
        }
    }

    #[test]
    fn all_lists_every_source_at_its_position() {
        for (index, source) in Source::ALL.into_iter().enumerate() {
            assert_eq!(position_in_all(source), index, "{source:?}");
        }
    }

    #[test]
    fn word_only_is_dictionary_mode() {
        for source in Source::ALL {
            let input = Input::new(" bank ", None, source).unwrap();
            assert_eq!(input.word(), "bank");
            assert_eq!(input.sentence(), None);
            assert_eq!(input.source(), source);
            assert_eq!(
                window_state(None, Some(&input)),
                WindowState::Dictionary { source }
            );
        }
    }

    #[test]
    fn word_and_sentence_is_contextual_mode() {
        for source in Source::ALL {
            let input = Input::new("bank", Some(SENTENCE), source).unwrap();
            assert_eq!(input.word(), "bank");
            assert_eq!(input.sentence(), Some(SENTENCE));
            assert_eq!(input.source(), source);
            assert_eq!(
                window_state(None, Some(&input)),
                WindowState::Contextual { source }
            );
        }
    }

    #[test]
    fn blank_sentence_is_absent() {
        for source in Source::ALL {
            let input = Input::new("bank", Some("  \n"), source).unwrap();
            assert_eq!(input.sentence(), None);
            assert_eq!(input, Input::new("bank", None, source).unwrap());
            assert_eq!(
                window_state(None, Some(&input)),
                WindowState::Dictionary { source }
            );
        }
    }

    #[test]
    fn empty_word_builds_no_input() {
        // The last two are Unicode whitespace: no-break and ideographic space.
        for word in ["", " ", "\t\n", "\u{a0}", "\u{3000}"] {
            for sentence in [None, Some(SENTENCE)] {
                for source in Source::ALL {
                    assert_eq!(Input::new(word, sentence, source), Err(EmptyWord));
                }
            }
        }
    }

    #[test]
    fn empty_word_is_an_error_value() {
        let error: Box<dyn std::error::Error> = Box::new(EmptyWord);
        assert_eq!(error.to_string(), "the word is empty");
    }

    #[test]
    fn texts_are_trimmed_but_otherwise_kept() {
        let input = Input::new(
            "\u{a0}River  Bank\n",
            Some("\tI sat  on the River  Bank. "),
            Source::Manual,
        )
        .unwrap();
        assert_eq!(input.word(), "River  Bank");
        assert_eq!(input.sentence(), Some("I sat  on the River  Bank."));
    }

    #[test]
    fn sentence_need_not_contain_the_word() {
        let input = Input::new("bank", Some("Nothing relevant here."), Source::Ocr).unwrap();
        assert_eq!(input.sentence(), Some("Nothing relevant here."));
    }

    #[test]
    fn nothing_yet_while_a_source_is_read_is_waiting() {
        for source in Source::ALL {
            assert_eq!(
                window_state(Some(source), None),
                WindowState::Waiting { source }
            );
        }
    }

    #[test]
    fn nothing_yet_and_nothing_pending_is_empty() {
        assert_eq!(window_state(None, None), WindowState::Empty);
    }

    #[test]
    fn arrived_input_wins_over_pending() {
        for pending in Source::ALL {
            for source in Source::ALL {
                let word_only = Input::new("bank", None, source).unwrap();
                assert_eq!(
                    window_state(Some(pending), Some(&word_only)),
                    WindowState::Dictionary { source }
                );
                let with_sentence = Input::new("bank", Some(SENTENCE), source).unwrap();
                assert_eq!(
                    window_state(Some(pending), Some(&with_sentence)),
                    WindowState::Contextual { source }
                );
            }
        }
    }
}
