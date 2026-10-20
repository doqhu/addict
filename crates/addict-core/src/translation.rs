//! The online translator contract: a text in the language of its
//! [`Source`] and an optional context in, the Russian text or a [`Refusal`]
//! out.
//!
//! The language of a request is named in one place, [`Request::source`]:
//! English, Spanish or Catalan. The target is always Russian.
//!
//! [`Translator::translate`] gives a future and runs nothing itself: core has
//! no runtime, and running the future off the UI thread, under a deadline, is
//! the caller's duty. A refusal says only of what kind it is; the texts shown
//! for it are the presentation's. `Canned` is a translator with one fixed
//! answer for tests, present only with the `sample` feature.

use std::{future::Future, pin::Pin};

/// The language a text is read in and translated from. English is the
/// default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Source {
    #[default]
    En,
    Es,
    Ca,
}

impl Source {
    /// Every language, in the order the window offers them.
    pub const ALL: [Source; 3] = [Source::En, Source::Es, Source::Ca];

    /// The lowercase code of the language: `en`, `es` or `ca`. No wildcard
    /// arm: a new language does not compile until it is given its code.
    pub fn code(self) -> &'static str {
        match self {
            Source::En => "en",
            Source::Es => "es",
            Source::Ca => "ca",
        }
    }

    /// The language of `code`, as [`Source::code`] writes it, exactly; any
    /// other text is none.
    pub fn of_code(code: &str) -> Option<Source> {
        Source::ALL.into_iter().find(|source| source.code() == code)
    }
}

/// Whether `word` is Russian: it has a letter of the Cyrillic block,
/// U+0400 to U+04FF. It is the one rule that routes a picked word to the
/// Russian dictionaries; nothing else about the word is weighed. A word of
/// no Cyrillic letter is read in the chosen [`Source`] as always.
pub fn is_russian(word: &str) -> bool {
    word.chars()
        .any(|character| ('\u{0400}'..='\u{04FF}').contains(&character))
}

/// What is asked of a translator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The text to translate, in the language of `source`.
    pub text: String,
    /// The text around it, which guides the translation and is not
    /// translated itself.
    pub context: Option<String>,
    /// The language of `text` and `context`.
    pub source: Source,
}

/// Why a request gave no translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The key file is missing or has no valid key. Nothing was sent.
    Key,
    /// The key file cannot be read. Nothing was sent.
    UnreadableKey,
    /// The key file has permissions outside 0600. Nothing was sent.
    UnsafeKey,
    /// The service could not be reached.
    Network,
    /// The service rejected the language pair of the request.
    UnsupportedLanguage,
    /// The service rejected the key.
    RejectedKey,
    /// The quota of the key is used up.
    Quota,
    /// No answer came in time.
    TimedOut,
    /// Anything else: another status, an answer that cannot be
    /// read.
    Other,
}

/// The pending answer of a translator.
pub type Translating<'a> = Pin<Box<dyn Future<Output = Result<String, Refusal>> + Send + 'a>>;

/// An online translator from the language of a request into Russian. It is
/// shared with the thread that runs the requests.
pub trait Translator: Send + Sync {
    /// The Russian text for `request`. Nothing happens until the future is
    /// polled, and dropping it gives the request up.
    fn translate(&self, request: Request) -> Translating<'_>;
}

/// The translator of the tests: every request gets the same answer at once,
/// and the requests are kept in the order they came.
#[cfg(any(test, feature = "sample"))]
#[derive(Debug)]
pub struct Canned {
    answer: Result<String, Refusal>,
    asked: std::sync::Mutex<Vec<Request>>,
}

#[cfg(any(test, feature = "sample"))]
impl Canned {
    /// A translator that answers every request with `answer`.
    pub fn new(answer: Result<&str, Refusal>) -> Self {
        Self {
            answer: answer.map(str::to_owned),
            asked: std::sync::Mutex::default(),
        }
    }

    /// The requests whose futures were polled, oldest first.
    pub fn asked(&self) -> Vec<Request> {
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[cfg(any(test, feature = "sample"))]
impl Translator for Canned {
    fn translate(&self, request: Request) -> Translating<'_> {
        Box::pin(async move {
            self.asked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request);
            self.answer.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::Arc,
        task::{Context, Poll, Waker},
    };

    use super::{Canned, Refusal, Request, Source, Translating, Translator, is_russian};

    fn request() -> Request {
        Request {
            text: "bank".to_owned(),
            context: Some("I sat on the bank of the river.".to_owned()),
            source: Source::En,
        }
    }

    #[test]
    fn a_word_with_a_cyrillic_letter_is_russian_and_no_other_is() {
        for russian in [
            "собака",
            "«Дом»,",
            "ёж",
            "Ёж",
            "e-мейл",
            "\u{0400}",
            "\u{04FF}",
        ] {
            assert!(is_russian(russian), "{russian:?}");
        }
        for other in [
            "bank",
            "cerró",
            "col·lecció",
            "",
            "…",
            "\u{03FF}",
            "\u{0500}",
        ] {
            assert!(!is_russian(other), "{other:?}");
        }
    }

    #[test]
    fn english_is_the_default_language() {
        assert_eq!(Source::default(), Source::En);
    }

    #[test]
    fn every_language_has_its_own_code_and_is_found_by_it() {
        assert_eq!(Source::ALL.map(Source::code), ["en", "es", "ca"]);
        for source in Source::ALL {
            assert_eq!(Source::of_code(source.code()), Some(source));
        }
        for other in ["", "fr", "EN", " en", "es\n", "en-GB"] {
            assert_eq!(Source::of_code(other), None, "{other:?}");
        }
    }

    #[test]
    fn the_canned_translator_keeps_the_language_of_a_request() {
        let translator = Canned::new(Ok("банк"));
        let spanish = Request {
            text: "banco".to_owned(),
            context: Some("El banco cerró a las tres.".to_owned()),
            source: Source::Es,
        };
        assert_eq!(
            ready(translator.translate(spanish.clone())),
            Ok("банк".to_owned())
        );
        assert_eq!(translator.asked(), [spanish]);
    }

    /// The answer of a future that is ready at its first poll.
    fn ready(mut translating: Translating<'_>) -> Result<String, Refusal> {
        let mut context = Context::from_waker(Waker::noop());
        match translating.as_mut().poll(&mut context) {
            Poll::Ready(answer) => answer,
            Poll::Pending => panic!("the canned answer is not ready"),
        }
    }

    #[test]
    fn the_canned_translator_gives_its_answer_and_keeps_the_request() {
        let translator = Canned::new(Ok("берег"));
        assert_eq!(
            ready(translator.translate(request())),
            Ok("берег".to_owned())
        );
        assert_eq!(translator.asked(), [request()]);
    }

    #[test]
    fn a_request_that_is_never_polled_is_not_asked() {
        let translator = Canned::new(Ok("берег"));
        drop(translator.translate(request()));
        assert!(translator.asked().is_empty());
    }

    #[test]
    fn every_refusal_comes_through_the_contract() {
        let all = [
            Refusal::Key,
            Refusal::UnreadableKey,
            Refusal::UnsafeKey,
            Refusal::Network,
            Refusal::UnsupportedLanguage,
            Refusal::RejectedKey,
            Refusal::Quota,
            Refusal::TimedOut,
            Refusal::Other,
        ];
        for refusal in all {
            // No wildcard arm: a new kind does not compile until it is
            // listed above.
            match refusal {
                Refusal::Key
                | Refusal::UnreadableKey
                | Refusal::UnsafeKey
                | Refusal::Network
                | Refusal::UnsupportedLanguage
                | Refusal::RejectedKey
                | Refusal::Quota
                | Refusal::TimedOut
                | Refusal::Other => {}
            }
            let translator = Canned::new(Err(refusal));
            assert_eq!(ready(translator.translate(request())), Err(refusal));
        }
    }

    #[test]
    fn the_translator_is_usable_through_the_contract_from_another_thread() {
        let translator: Arc<dyn Translator> = Arc::new(Canned::new(Ok("берег")));
        let shared = translator.clone();
        let answer = std::thread::spawn(move || ready(shared.translate(request())))
            .join()
            .expect("thread");
        assert_eq!(answer, Ok("берег".to_owned()));
    }
}
