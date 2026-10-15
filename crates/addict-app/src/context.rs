//! The accessibility read without its widgets: one job on the work runner
//! asks the accessibility bus for the word selected in Android Studio and
//! its sentence, when Android Studio was the application in front at the
//! press.
//!
//! [`start`] starts the job with the [`Accessibility`] the listener read and
//! the [`Source`] the press settled on ([`choose_source`]). When the product's
//! window was not active at the press, the listener's probe of Android
//! Studio's frames goes with it: the job first waits for its answer, for at
//! most [`PROBE_WAIT`], and an active frame makes the source Android Studio
//! ([`resolve`]); any other answer, none or a dropped probe leaves the
//! source as observed. The job tells the source it settled on before it
//! reads, so that the session keeps it. The job ends with one
//! [`Read`], what the adapter gave, or why it gave nothing: accessibility
//! that is off ends it at once as `AccessibilityOff`, another application in
//! front as `NotFromStudio`, and an unknown one as `NotFromStudio`,
//! `AppNotFound` or `Unavailable`, never `TimedOut`; no text is read in any
//! of these. [`read_result`] says what an outcome of the job is for the
//! window. The job only reads. What the window does
//! with the context is [`resident`](crate::resident)'s; what the adapter does
//! on the bus is [`addict_platform::atspi`]'s, and what was in front is
//! [`addict_platform::foreground`]'s.

use std::{fmt::Debug, future::Future, time::Duration};

use addict_platform::{
    atspi::{self, Context, ContextFailure, Locate},
    foreground::{Accessibility, Probe, Source},
    work::{Job, Outcome, Work},
};
use tokio::time::timeout;

use crate::evidence;

/// How long the whole read may take: the registry, the walk of the tree and
/// the text. The window is up before it starts and does not wait for it.
pub const CONTEXT_LIMIT: Duration = Duration::from_secs(2);

/// What the deadline of the job has over the read's own limit, so that the
/// adapter's [`ContextFailure::TimedOut`] is what a slow bus ends in.
const MARGIN: Duration = Duration::from_secs(1);

/// How long the job waits for the probe of Android Studio's frames: the
/// listener answers within the adapter's call limit of 500 ms once it takes
/// the probe; the rest is for the probe's queue.
pub const PROBE_WAIT: Duration = Duration::from_secs(1);

/// What a read ended with: the selection, or why there is none.
pub type Read = Result<Context, ContextFailure>;

/// What a read was started with, for its record: the measuring of its time,
/// what the listener said of accessibility at the press and the application
/// in front, as the press took it and then as the job settled on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Started {
    pub probe: evidence::Probe,
    pub accessibility: Accessibility,
    pub source: Source,
}

/// Where the read of the window stands; at first no read runs. Each read has
/// a number, so what a given-up read still reports is told from the report of
/// the one that runs.
#[derive(Default)]
pub struct Reading {
    /// The job, from the press to its end; dropping it cancels the job.
    job: Option<Job>,
    /// The number of the last started read.
    number: u64,
    /// What the last started read was started with.
    started: Option<Started>,
}

impl Reading {
    /// A new read starts: the one that ran is given up, and the number of
    /// the new one is given back.
    pub fn begin(&mut self, started: Started) -> u64 {
        self.job = None;
        self.number += 1;
        self.started = Some(started);
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

    /// What the read that runs was started with; none when no read runs.
    pub fn running(&self) -> Option<Started> {
        self.job.as_ref().and(self.started)
    }

    /// The read with `number` settled on `source`; one that was given up
    /// changes nothing.
    pub fn settle(&mut self, number: u64, source: Source) {
        if self.is_running(number)
            && let Some(started) = self.started.as_mut()
        {
            started.source = source;
        }
    }

    /// The read ended. Whether one ran.
    pub fn end(&mut self) -> bool {
        self.job.take().is_some()
    }
}

/// What the press takes as the application in front. A press while the
/// product's own window is active keeps the source of the press that opened
/// it (`kept`): the window is in front now, which says nothing about where
/// the text is. Otherwise it is what the listener `observed`.
pub fn choose_source(window_active: bool, kept: Source, observed: Source) -> Source {
    if window_active { kept } else { observed }
}

/// What the job takes as the application in front: Android Studio when the
/// probe found one of its frames active, else what the press `observed`.
pub fn resolve(observed: Source, studio_active: bool) -> Source {
    if studio_active {
        Source::AndroidStudio
    } else {
        observed
    }
}

/// The source after the answer of `probe`, waited for at most
/// [`PROBE_WAIT`]; without a probe, `observed`. A probe that is dropped or
/// late leaves `observed`: nothing is read on a guess.
async fn settle<P, E>(observed: Source, probe: Option<P>) -> Source
where
    P: Future<Output = Result<bool, E>>,
    E: Debug,
{
    let Some(probe) = probe else {
        return observed;
    };
    let active = match timeout(PROBE_WAIT, probe).await {
        Ok(Ok(active)) => active,
        Ok(Err(e)) => {
            eprintln!("addict: the probe of Android Studio was dropped: {e:?}");
            false
        }
        Err(_) => {
            eprintln!("addict: the probe of Android Studio did not answer within {PROBE_WAIT:?}");
            false
        }
    };
    resolve(observed, active)
}

/// Starts the read of the session's accessibility bus on `work`, with
/// `accessibility` as the listener read it, `source` in front and the
/// `probe` taken at the press, if any. `on_source` runs once on the worker
/// with the source the job settled on, before the read; `on_outcome` runs
/// once on the worker when the job has ended. Without a runner nothing
/// starts and the failure is given back.
pub fn start(
    work: Option<&Work>,
    accessibility: Accessibility,
    source: Source,
    probe: Option<Probe>,
    on_source: impl FnOnce(Source) + Send + 'static,
    on_outcome: impl FnOnce(Outcome<Read>) + Send + 'static,
) -> Result<Job, ContextFailure> {
    start_at(
        work,
        Locate::Session,
        Press {
            accessibility,
            source,
            probe,
        },
        CONTEXT_LIMIT,
        on_source,
        on_outcome,
    )
}

/// What a press gives the job.
struct Press<P> {
    accessibility: Accessibility,
    source: Source,
    probe: Option<P>,
}

/// [`start`] on the bus `locate` with the deadline `limit` for the read.
fn start_at<P, E>(
    work: Option<&Work>,
    locate: Locate,
    press: Press<P>,
    limit: Duration,
    on_source: impl FnOnce(Source) + Send + 'static,
    on_outcome: impl FnOnce(Outcome<Read>) + Send + 'static,
) -> Result<Job, ContextFailure>
where
    P: Future<Output = Result<bool, E>> + Send + 'static,
    E: Debug,
{
    let work = work.ok_or_else(|| ContextFailure::Failed("no work runner".to_owned()))?;
    let Press {
        accessibility,
        source,
        probe,
    } = press;
    Ok(work.run(
        limit.saturating_add(PROBE_WAIT).saturating_add(MARGIN),
        async move {
            let source = settle(source, probe).await;
            on_source(source);
            atspi::read(&locate, accessibility, source, limit).await
        },
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

    use std::future::{self, Ready};

    use addict_core::sentence::Selected;
    use addict_platform::{
        atspi::{Context, ContextFailure, Locate},
        foreground::{Accessibility, Probe, Source},
        work::{Outcome, Work},
    };

    use super::{
        CONTEXT_LIMIT, MARGIN, PROBE_WAIT, Press, Read, Reading, Started, choose_source,
        read_result, resolve, settle, start, start_at,
    };
    use crate::{evidence, manual::tests::WAIT};

    /// A press without a probe: the window was active.
    fn press(accessibility: Accessibility, source: Source) -> Press<Probe> {
        Press {
            accessibility,
            source,
            probe: None,
        }
    }

    /// A probe that has its answer already.
    type Answered = Ready<Result<bool, ()>>;

    #[test]
    fn without_a_runner_nothing_starts() {
        let started = start(
            None,
            Accessibility::On,
            Source::AndroidStudio,
            None,
            |_| panic!("settled"),
            |_| panic!("ended"),
        );
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
            press(Accessibility::On, Source::AndroidStudio),
            CONTEXT_LIMIT,
            |_| {},
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

    /// What the job ends with on an absent bus, with `accessibility` and
    /// `source`: only a gate can answer other than Unavailable.
    fn read_on_an_absent_bus(work: &Work, accessibility: Accessibility, source: Source) -> Read {
        let missing = std::env::temp_dir().join("addict-context-test-no-such-bus-gate");
        let (sender, receiver) = mpsc::channel();
        let job = start_at(
            Some(work),
            Locate::Address(format!("unix:path={}", missing.display())),
            press(accessibility, source),
            CONTEXT_LIMIT,
            |_| {},
            move |outcome| {
                let _ = sender.send(outcome);
            },
        )
        .expect("job");
        let outcome = receiver.recv_timeout(WAIT).expect("outcome");
        drop(job);
        read_result(outcome).expect("not cancelled")
    }

    #[test]
    fn another_application_ends_the_job_without_a_bus() {
        let work = Work::start().expect("runner");
        assert_eq!(
            read_on_an_absent_bus(&work, Accessibility::On, Source::Other),
            Err(ContextFailure::NotFromStudio(Source::Other))
        );
        // An unknown source asks the bus for the names of the applications.
        assert!(matches!(
            read_on_an_absent_bus(&work, Accessibility::On, Source::Unknown),
            Err(ContextFailure::Unavailable(_))
        ));
    }

    #[test]
    fn accessibility_off_ends_the_job_without_a_bus() {
        let work = Work::start().expect("runner");
        for source in [Source::AndroidStudio, Source::Other, Source::Unknown] {
            assert_eq!(
                read_on_an_absent_bus(&work, Accessibility::Off, source),
                Err(ContextFailure::AccessibilityOff),
                "{source:?}"
            );
        }
    }

    #[test]
    fn a_focused_window_keeps_the_source_of_its_press() {
        use Source::{AndroidStudio, Other, Unknown};
        // The window is in front: the observed source is the window's own.
        assert_eq!(choose_source(true, AndroidStudio, Other), AndroidStudio);
        assert_eq!(choose_source(true, Other, Other), Other);
        assert_eq!(choose_source(true, Unknown, AndroidStudio), Unknown);
        // The window is not in front: what was observed.
        assert_eq!(choose_source(false, AndroidStudio, Other), Other);
        assert_eq!(choose_source(false, Unknown, AndroidStudio), AndroidStudio);
        assert_eq!(choose_source(false, AndroidStudio, Unknown), Unknown);
    }

    #[test]
    fn an_active_frame_of_studio_makes_studio_the_source() {
        use Source::{AndroidStudio, Other, Unknown};
        for observed in [AndroidStudio, Other, Unknown] {
            assert_eq!(resolve(observed, true), AndroidStudio);
            assert_eq!(resolve(observed, false), observed);
        }
    }

    /// What `settle` gives for `observed` and `probe`, run on a runner.
    fn settled<P>(observed: Source, probe: Option<P>) -> Source
    where
        P: std::future::Future<Output = Result<bool, ()>> + Send + 'static,
    {
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let _job = work.run(WAIT, settle(observed, probe), move |outcome| {
            let _ = sender.send(outcome);
        });
        match receiver.recv_timeout(WAIT).expect("outcome") {
            Outcome::Finished(source) => source,
            outcome => panic!("{outcome:?}"),
        }
    }

    #[test]
    fn the_probe_decides_only_when_it_says_true() {
        use Source::{AndroidStudio, Other, Unknown};
        assert_eq!(
            settled(Unknown, Some(future::ready(Ok(true)))),
            AndroidStudio
        );
        assert_eq!(settled(Other, Some(future::ready(Ok(true)))), AndroidStudio);
        assert_eq!(settled(Unknown, Some(future::ready(Ok(false)))), Unknown);
        assert_eq!(settled(Other, Some(future::ready(Ok(false)))), Other);
        // Dropped: the listener is gone.
        assert_eq!(settled(Other, Some(future::ready(Err(())))), Other);
        // No probe: the window was active.
        assert_eq!(settled(Unknown, None::<Answered>), Unknown);
    }

    #[test]
    fn a_probe_that_does_not_answer_leaves_the_observed_source() {
        let started = std::time::Instant::now();
        let probe = future::pending::<Result<bool, ()>>();
        assert_eq!(settled(Source::Other, Some(probe)), Source::Other);
        assert!(started.elapsed() >= PROBE_WAIT);
    }

    #[test]
    fn the_job_reads_as_studio_and_says_so_when_the_probe_found_it() {
        let work = Work::start().expect("runner");
        let missing = std::env::temp_dir().join("addict-context-test-no-such-bus-probe");
        let (sender, receiver) = mpsc::channel();
        let (settled, settled_to) = mpsc::channel();
        let job = start_at(
            Some(&work),
            Locate::Address(format!("unix:path={}", missing.display())),
            Press {
                accessibility: Accessibility::On,
                source: Source::Other,
                probe: Some(future::ready(Ok::<_, ()>(true))),
            },
            CONTEXT_LIMIT,
            move |source| {
                let _ = settled.send(source);
            },
            move |outcome| {
                let _ = sender.send(outcome);
            },
        )
        .expect("job");
        assert_eq!(
            settled_to.recv_timeout(WAIT).expect("source"),
            Source::AndroidStudio
        );
        // Read as Android Studio's: the bus is opened, and it is absent.
        let outcome = receiver.recv_timeout(WAIT).expect("outcome");
        drop(job);
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
        assert_eq!(reading.running(), None);

        let (sender, receiver) = mpsc::channel();
        let started = Started {
            probe: evidence::Probe::start(true),
            accessibility: Accessibility::On,
            source: Source::Unknown,
        };
        let first = reading.begin(started);
        reading.run(work.run(
            Duration::from_secs(30),
            std::future::pending::<()>(),
            move |outcome| {
                let _ = sender.send(outcome);
            },
        ));
        assert!(reading.is_running(first));
        assert_eq!(reading.running(), Some(started));

        // A new read gives the first up: it is cancelled and is no longer
        // the one that runs, though it still reports.
        let second = reading.begin(started);
        assert_eq!(
            receiver.recv_timeout(WAIT).expect("outcome"),
            Outcome::Cancelled
        );
        assert_ne!(first, second);
        assert!(!reading.is_running(first));
        // Between `begin` and the job no read runs, and it has nothing to give.
        assert!(!reading.is_running(second));
        assert_eq!(reading.running(), None);
        reading.run(work.run(
            Duration::from_secs(30),
            std::future::pending::<()>(),
            |_| {},
        ));
        assert!(reading.is_running(second));
        assert!(!reading.is_running(first));

        // A given-up read does not settle; the one that runs does.
        reading.settle(first, Source::Other);
        assert_eq!(reading.running(), Some(started));
        reading.settle(second, Source::AndroidStudio);
        assert_eq!(
            reading.running().map(|started| started.source),
            Some(Source::AndroidStudio)
        );
        assert!(reading.end());
        assert!(!reading.is_running(second));
        assert_eq!(reading.running(), None);
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
        // The listener's probe ends within its call limit of 500 ms.
        assert!(PROBE_WAIT >= Duration::from_millis(500));
    }
}
