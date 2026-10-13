//! The online translator contract: a text and an optional context in, the
//! Russian text or a [`Refusal`] out.
//!
//! [`Translator::translate`] gives a future and runs nothing itself: core has
//! no runtime, and running the future off the UI thread, under a deadline, is
//! the caller's duty. A refusal says only of what kind it is; the texts shown
//! for it are the presentation's. `Canned` is a translator with one fixed
//! answer for tests, present only with the `sample` feature.

use std::{future::Future, pin::Pin};

/// What is asked of a translator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The English text to translate.
    pub text: String,
    /// The text around it, which guides the translation and is not
    /// translated itself.
    pub context: Option<String>,
}

/// Why a request gave no translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// There is no key to use: it is missing, empty, unreadable or kept in a
    /// way that is refused. Nothing was sent.
    Key,
    /// The service rejected the key.
    RejectedKey,
    /// The quota of the key is used up.
    Quota,
    /// No answer came in time.
    TimedOut,
    /// Anything else: no network, another status, an answer that cannot be
    /// read.
    Other,
}

/// The pending answer of a translator.
pub type Translating<'a> = Pin<Box<dyn Future<Output = Result<String, Refusal>> + Send + 'a>>;

/// An online translator from English into Russian. It is shared with the
/// thread that runs the requests.
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

    use super::{Canned, Refusal, Request, Translating, Translator};

    fn request() -> Request {
        Request {
            text: "bank".to_owned(),
            context: Some("I sat on the bank of the river.".to_owned()),
        }
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
