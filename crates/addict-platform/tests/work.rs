//! The slow-work rule, one test per row of its matrix: a job ends exactly once
//! as finished, cancelled, timed out or panicked, and never holds the caller.
//!
//! No test sleeps: every wait is a `recv_timeout` with a named limit.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use addict_platform::work::{Outcome, Work};
use tokio::sync::oneshot;

/// Limit for each wait of the test thread.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);
/// A deadline no test is meant to reach.
const FAR: Duration = Duration::from_secs(60);
/// The deadline of the jobs that are meant to reach it.
const SHORT: Duration = Duration::from_millis(200);
/// Limit for dropping a runner with pending jobs.
const STOP_LIMIT: Duration = Duration::from_secs(1);

/// Tells the test that the job owning it was dropped.
struct Dropped(mpsc::Sender<()>);

impl Drop for Dropped {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

/// A job that stays pending until it is stopped, and says when it is gone.
fn pending() -> (
    impl Future<Output = u32> + Send + 'static,
    mpsc::Receiver<()>,
) {
    let (sender, receiver) = mpsc::channel();
    let guard = Dropped(sender);
    let job = async move {
        let _guard = guard;
        tokio::time::sleep(FAR * 2).await;
        0
    };
    (job, receiver)
}

/// The callback is consumed by its one call, which disconnects the channel: a
/// second outcome is impossible once this holds.
fn assert_no_more<T: std::fmt::Debug + PartialEq>(outcomes: &mpsc::Receiver<T>) {
    assert_eq!(
        outcomes.recv_timeout(EVENT_TIMEOUT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}

#[test]
fn job_that_ends_before_its_deadline_reports_finished() {
    let work = Work::start().expect("start the runner");
    let (sender, outcomes) = mpsc::channel();
    let _job = work.run(FAR, async { 7 }, move |outcome| {
        let _ = sender.send(outcome);
    });
    assert_eq!(
        outcomes.recv_timeout(EVENT_TIMEOUT),
        Ok(Outcome::Finished(7))
    );
    assert_no_more(&outcomes);
}

#[test]
fn cancel_stops_a_pending_job_and_reports_cancelled() {
    let work = Work::start().expect("start the runner");
    let (job, dropped) = pending();
    let (sender, outcomes) = mpsc::channel();
    let job = work.run(FAR, job, move |outcome| {
        let _ = sender.send(outcome);
    });
    job.cancel();
    assert_eq!(outcomes.recv_timeout(EVENT_TIMEOUT), Ok(Outcome::Cancelled));
    // The job was gone before the outcome was reported.
    assert_eq!(dropped.try_recv(), Ok(()));
    assert_no_more(&outcomes);
}

#[test]
fn job_pending_at_its_deadline_reports_timed_out_no_earlier() {
    let work = Work::start().expect("start the runner");
    let (job, dropped) = pending();
    let (sender, outcomes) = mpsc::channel();
    let started = Instant::now();
    let _job = work.run(SHORT, job, move |outcome| {
        let _ = sender.send(outcome);
    });
    assert_eq!(outcomes.recv_timeout(EVENT_TIMEOUT), Ok(Outcome::TimedOut));
    let elapsed = started.elapsed();
    assert!(elapsed >= SHORT, "timed out after {elapsed:?}");
    assert_eq!(dropped.try_recv(), Ok(()));
    assert_no_more(&outcomes);
}

#[test]
fn cancel_after_the_end_does_nothing() {
    let work = Work::start().expect("start the runner");
    let (sender, outcomes) = mpsc::channel();
    let job = work.run(FAR, async { 1 }, move |outcome| {
        let _ = sender.send(outcome);
    });
    assert_eq!(
        outcomes.recv_timeout(EVENT_TIMEOUT),
        Ok(Outcome::Finished(1))
    );
    job.cancel();
    drop(job);
    assert_no_more(&outcomes);

    // The runner is unharmed.
    let (sender, outcomes) = mpsc::channel();
    let _job = work.run(FAR, async { 2 }, move |outcome| {
        let _ = sender.send(outcome);
    });
    assert_eq!(
        outcomes.recv_timeout(EVENT_TIMEOUT),
        Ok(Outcome::Finished(2))
    );
}

#[test]
fn dropping_the_handle_cancels_a_pending_job() {
    let work = Work::start().expect("start the runner");
    let (job, dropped) = pending();
    let (sender, outcomes) = mpsc::channel();
    let job = work.run(FAR, job, move |outcome| {
        let _ = sender.send(outcome);
    });
    drop(job);
    assert_eq!(outcomes.recv_timeout(EVENT_TIMEOUT), Ok(Outcome::Cancelled));
    assert_eq!(dropped.try_recv(), Ok(()));
    assert_no_more(&outcomes);
}

#[test]
fn panicking_job_reports_panicked_and_the_runner_keeps_serving() {
    let work = Work::start().expect("start the runner");
    let (sender, outcomes) = mpsc::channel::<Outcome<u32>>();
    let _job = work.run(
        FAR,
        async { panic!("the job of the panic test") },
        move |outcome| {
            let _ = sender.send(outcome);
        },
    );
    assert_eq!(outcomes.recv_timeout(EVENT_TIMEOUT), Ok(Outcome::Panicked));
    assert_no_more(&outcomes);

    let (sender, outcomes) = mpsc::channel();
    let _job = work.run(FAR, async { 3 }, move |outcome| {
        let _ = sender.send(outcome);
    });
    assert_eq!(
        outcomes.recv_timeout(EVENT_TIMEOUT),
        Ok(Outcome::Finished(3))
    );
}

#[test]
fn several_jobs_report_their_own_outcomes_without_delaying_each_other() {
    let work = Work::start().expect("start the runner");
    let (sender, outcomes) = mpsc::channel();
    let report = |name: &'static str| {
        let sender = sender.clone();
        move |outcome| {
            let _ = sender.send((name, outcome));
        }
    };

    // Started first and pending for the whole test: it must hold nobody up.
    let (stuck, _stuck_dropped) = pending();
    let to_cancel = work.run(FAR, stuck, report("cancelled"));
    let (late, _late_dropped) = pending();
    let _to_time_out = work.run(SHORT, late, report("timed out"));
    let (gate, opened) = oneshot::channel::<u32>();
    let _to_finish = work.run(
        FAR,
        async move { opened.await.unwrap_or(0) },
        report("finished"),
    );
    drop(sender);

    gate.send(5).expect("the job waits for the gate");
    // These two race when the test thread is held up past the short deadline.
    let mut first_two = [
        outcomes.recv_timeout(EVENT_TIMEOUT).expect("first outcome"),
        outcomes
            .recv_timeout(EVENT_TIMEOUT)
            .expect("second outcome"),
    ];
    first_two.sort_by_key(|(name, _)| *name);
    assert_eq!(
        first_two,
        [
            ("finished", Outcome::Finished(5)),
            ("timed out", Outcome::TimedOut)
        ]
    );
    to_cancel.cancel();
    assert_eq!(
        outcomes.recv_timeout(EVENT_TIMEOUT),
        Ok(("cancelled", Outcome::Cancelled))
    );
    assert_no_more(&outcomes);
}

#[test]
fn dropping_the_runner_stops_pending_jobs_and_joins_in_time() {
    let work = Work::start().expect("start the runner");
    let (first, first_dropped) = pending();
    let (second, second_dropped) = pending();
    let (sender, outcomes) = mpsc::channel();
    let report = sender.clone();
    let first = work.run(FAR, first, move |outcome| {
        let _ = report.send(outcome);
    });
    let second = work.run(FAR, second, move |outcome| {
        let _ = sender.send(outcome);
    });

    let started = Instant::now();
    drop(work);
    let elapsed = started.elapsed();
    assert!(elapsed < STOP_LIMIT, "the runner took {elapsed:?} to stop");
    // The thread is joined, so both jobs are already gone.
    assert_eq!(first_dropped.try_recv(), Ok(()));
    assert_eq!(second_dropped.try_recv(), Ok(()));
    // The callbacks went with the jobs; whether they were called is not
    // promised, only that nothing is left waiting.
    while outcomes.recv_timeout(EVENT_TIMEOUT).is_ok() {}
    assert_eq!(outcomes.try_recv(), Err(mpsc::TryRecvError::Disconnected));

    // Handles that outlive the runner are harmless.
    first.cancel();
    drop(second);
}

#[test]
fn dropping_the_runner_does_not_wait_for_a_job_that_is_slow_to_drop() {
    /// Far longer than the runner may take to stop.
    const SLOW_DROP: Duration = Duration::from_secs(3);

    struct SlowToDrop;

    impl Drop for SlowToDrop {
        fn drop(&mut self) {
            // Runs on the worker thread, which the runner leaves behind.
            std::thread::sleep(SLOW_DROP);
        }
    }

    let work = Work::start().expect("start the runner");
    let (taken, was_taken) = mpsc::channel();
    let slow = SlowToDrop;
    let _job = work.run(
        FAR,
        async move {
            let _slow = slow;
            let _ = taken.send(());
            tokio::time::sleep(FAR * 2).await;
        },
        |_| {},
    );
    // The job is pending on the worker and owns the value.
    assert_eq!(was_taken.recv_timeout(EVENT_TIMEOUT), Ok(()));

    let started = Instant::now();
    drop(work);
    let elapsed = started.elapsed();
    assert!(
        elapsed < SLOW_DROP - STOP_LIMIT,
        "the runner took {elapsed:?} to stop"
    );
}

#[test]
fn starting_a_job_does_not_wait_for_it_and_the_callback_runs_on_the_worker() {
    let work = Work::start().expect("start the runner");
    let (gate, opened) = oneshot::channel::<u32>();
    let (sender, outcomes) = mpsc::channel();
    let _job = work.run(
        FAR,
        async move { opened.await.unwrap_or(0) },
        move |outcome| {
            let thread = std::thread::current().name().map(str::to_owned);
            let _ = sender.send((outcome, thread));
        },
    );
    // `run` is back while the job cannot have ended: its gate is still shut.
    assert_eq!(outcomes.try_recv(), Err(mpsc::TryRecvError::Empty));
    gate.send(9).expect("the job waits for the gate");
    assert_eq!(
        outcomes.recv_timeout(EVENT_TIMEOUT),
        Ok((Outcome::Finished(9), Some("addict-work".to_owned())))
    );
}
