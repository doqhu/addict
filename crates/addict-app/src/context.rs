//! The accessibility read without its widgets: one job on the work runner
//! asks the accessibility bus for the word selected in Android Studio and
//! its sentence.
//!
//! [`start`] starts the job; it ends with one [`Read`], what the adapter
//! gave, or why it gave nothing. [`read_result`] says what an outcome of the
//! job is for the window. The job only reads. What the window does with the
//! context is [`resident`](crate::resident)'s; what the adapter does on the
//! bus is [`addict_platform::atspi`]'s.

use std::time::Duration;

use addict_platform::{
    atspi::{self, Context, ContextFailure, Locate},
    work::{Job, Outcome, Work},
};

/// How long the whole read may take: the registry, the walk of the tree and
/// the text. The window is up before it starts and does not wait for it.
pub const CONTEXT_LIMIT: Duration = Duration::from_secs(2);

/// What the deadline of the job has over the read's own limit, so that the
/// adapter's [`ContextFailure::TimedOut`] is what a slow bus ends in.
const MARGIN: Duration = Duration::from_secs(1);

/// What a read ended with: the selection, or why there is none.
pub type Read = Result<Context, ContextFailure>;

/// Where the read of the window stands; at first no read runs. Each read has
/// a number, so what a given-up read still reports is told from the report of
/// the one that runs.
#[derive(Default)]
pub struct Reading {
    /// The job, from the press to its end; dropping it cancels the job.
    job: Option<Job>,
    /// The number of the last started read.
    number: u64,
}

impl Reading {
    /// A new read starts: the one that ran is given up, and the number of
    /// the new one is given back.
    pub fn begin(&mut self) -> u64 {
        self.job = None;
        self.number += 1;
        self.number
    }

    /// The job of the read that [`Reading::begin`] numbered.
    pub fn run(&mut self, job: Job) {
        self.job = Some(job);
    }

    /// Whether the read with `number` is the one that runs: a report with
    /// another number, or one that comes when no read runs, is from a read
    /// that was given up.
    pub fn is_running(&self, number: u64) -> bool {
        self.job.is_some() && number == self.number
    }

    /// The read ended. Whether one ran.
    pub fn end(&mut self) -> bool {
        self.job.take().is_some()
    }
}

/// Starts the read of the session's accessibility bus on `work`.
/// `on_outcome` runs once on the worker when the job has ended. Without a
/// runner nothing starts and the failure is given back.
pub fn start(
    work: Option<&Work>,
    on_outcome: impl FnOnce(Outcome<Read>) + Send + 'static,
) -> Result<Job, ContextFailure> {
    start_at(work, Locate::Session, CONTEXT_LIMIT, on_outcome)
}

/// [`start`] on the bus `locate` with the deadline `limit` for the read.
fn start_at(
    work: Option<&Work>,
    locate: Locate,
    limit: Duration,
    on_outcome: impl FnOnce(Outcome<Read>) + Send + 'static,
) -> Result<Job, ContextFailure> {
    let work = work.ok_or_else(|| ContextFailure::Failed("no work runner".to_owned()))?;
    Ok(work.run(
        limit.saturating_add(MARGIN),
        async move { atspi::read(&locate, limit).await },
        on_outcome,
    ))
}

/// What the window is given for the `outcome` of the job, or `None` when it
/// was cancelled: then it was dropped on purpose and shows nothing. No
/// wildcard arm: a new outcome does not compile until it is given its end.
pub fn read_result(outcome: Outcome<Read>) -> Option<Read> {
    match outcome {
        Outcome::Finished(read) => Some(read),
        Outcome::TimedOut => Some(Err(ContextFailure::TimedOut)),
        Outcome::Panicked => Some(Err(ContextFailure::Failed("the read panicked".to_owned()))),
        Outcome::Cancelled => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, thread, time::Duration};

    use addict_core::sentence::Selected;
    use addict_platform::{
        atspi::{Context, ContextFailure, Locate},
        work::{Outcome, Work},
    };

    use super::{CONTEXT_LIMIT, MARGIN, Reading, read_result, start, start_at};
    use crate::manual::tests::WAIT;

    #[test]
    fn without_a_runner_nothing_starts() {
        let started = start(None, |_| panic!("ended"));
        assert!(matches!(started, Err(ContextFailure::Failed(_))));
    }

    #[test]
    fn a_read_runs_off_the_calling_thread_and_reports_why_it_failed() {
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let caller = thread::current().id();
        let missing = std::env::temp_dir().join("addict-context-test-no-such-bus");
        let job = start_at(
            Some(&work),
            Locate::Address(format!("unix:path={}", missing.display())),
            CONTEXT_LIMIT,
            move |outcome| {
                let _ = sender.send((outcome, thread::current().id() != caller));
            },
        )
        .expect("job");
        let (outcome, off_thread) = receiver.recv_timeout(WAIT).expect("outcome");
        drop(job);
        assert!(off_thread);
        assert!(matches!(
            read_result(outcome),
            Some(Err(ContextFailure::Unavailable(_)))
        ));
    }

    #[test]
    fn only_the_last_read_is_the_one_that_runs() {
        let work = Work::start().expect("runner");
        let mut reading = Reading::default();
        assert!(!reading.is_running(0));
        assert!(!reading.end());

        let (sender, receiver) = mpsc::channel();
        let first = reading.begin();
        reading.run(work.run(
            Duration::from_secs(30),
            std::future::pending::<()>(),
            move |outcome| {
                let _ = sender.send(outcome);
            },
        ));
        assert!(reading.is_running(first));

        // A new read gives the first up: it is cancelled and is no longer
        // the one that runs, though it still reports.
        let second = reading.begin();
        assert_eq!(
            receiver.recv_timeout(WAIT).expect("outcome"),
            Outcome::Cancelled
        );
        assert_ne!(first, second);
        assert!(!reading.is_running(first));
        // Between `begin` and the job no read runs.
        assert!(!reading.is_running(second));
        reading.run(work.run(
            Duration::from_secs(30),
            std::future::pending::<()>(),
            |_| {},
        ));
        assert!(reading.is_running(second));
        assert!(!reading.is_running(first));

        assert!(reading.end());
        assert!(!reading.is_running(second));
        assert!(!reading.end());
    }

    #[test]
    fn every_end_but_a_cancel_is_shown() {
        let context = Context {
            word: "bank".to_owned(),
            sentence: Some(Selected {
                text: "the bank".to_owned(),
                word: 4..8,
            }),
        };
        assert_eq!(
            read_result(Outcome::Finished(Ok(context.clone()))),
            Some(Ok(context))
        );
        assert_eq!(
            read_result(Outcome::Finished(Err(ContextFailure::NoSelection))),
            Some(Err(ContextFailure::NoSelection))
        );
        assert_eq!(
            read_result(Outcome::TimedOut),
            Some(Err(ContextFailure::TimedOut))
        );
        assert!(matches!(
            read_result(Outcome::Panicked),
            Some(Err(ContextFailure::Failed(_)))
        ));
        assert_eq!(read_result(Outcome::Cancelled), None);
    }

    #[test]
    fn the_limit_is_short_and_the_job_outlives_it() {
        assert_eq!(CONTEXT_LIMIT.as_secs(), 2);
        assert!(!MARGIN.is_zero());
    }
}
