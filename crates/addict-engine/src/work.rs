//! The rule for slow work: it runs off the caller's thread, has a deadline,
//! can be cancelled and ends with exactly one outcome.
//!
//! [`Work`] owns one worker thread. [`Work::run`] hands it a future and a
//! plain `Send` callback; the callback receives the [`Outcome`] on the worker
//! thread, so the caller decides how to wake its own thread. An idle runner is
//! parked: nothing here polls or ticks.

use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, mpsc as std_mpsc},
    thread::JoinHandle,
    time::Duration,
};

use tokio::sync::{Notify, mpsc};

/// How long stopping the worker may take when the runner is dropped.
const STOP_LIMIT: Duration = Duration::from_secs(1);

/// How a job ended. Every job that the worker took reports exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome<T> {
    /// The job ran to its end before the deadline.
    Finished(T),
    /// The job was cancelled, or its handle was dropped, while it was pending.
    Cancelled,
    /// The job was still pending at its deadline and was stopped.
    TimedOut,
    /// The job panicked; the runner keeps serving other jobs.
    Panicked,
}

/// The runner. Dropping it stops the pending jobs without reporting them and
/// waits for the thread, for at most about one second.
#[derive(Debug)]
pub struct Work {
    jobs: Option<mpsc::UnboundedSender<Supervisor>>,
    /// Disconnects when the worker thread is over.
    ended: std_mpsc::Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

/// One pending job. [`Job::cancel`] and dropping the handle both cancel it;
/// after the outcome was reported neither does anything.
#[derive(Debug)]
pub struct Job {
    cancel: Arc<Notify>,
}

/// A job together with its deadline, cancel signal and callback.
type Supervisor = Pin<Box<dyn Future<Output = ()> + Send>>;

impl Work {
    /// Spawns the worker thread `addict-work`.
    pub fn start() -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (jobs, mut incoming) = mpsc::unbounded_channel::<Supervisor>();
        let (end, ended) = std_mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("addict-work".to_owned())
            .spawn(move || {
                // Dropped when the thread is over, however it ends.
                let _end = end;
                runtime.block_on(async move {
                    // Parked here while idle; ends when the runner is dropped.
                    while let Some(supervisor) = incoming.recv().await {
                        tokio::spawn(supervisor);
                    }
                });
                // Drops every job still pending, before `_end` says the
                // thread is over: the stop limit covers this teardown.
                drop(runtime);
            })?;
        Ok(Self {
            jobs: Some(jobs),
            ended,
            thread: Some(thread),
        })
    }

    /// Starts `job` and returns at once. The job is stopped when `deadline`
    /// has passed since the worker took it. `on_outcome` runs once on the
    /// worker thread and must not block, and neither may the job: it shares
    /// the thread with the other jobs.
    pub fn run<T, F, C>(&self, deadline: Duration, job: F, on_outcome: C) -> Job
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
        C: FnOnce(Outcome<T>) + Send + 'static,
    {
        let cancel = Arc::new(Notify::new());
        let supervisor = Box::pin(supervise(deadline, job, cancel.clone(), on_outcome));
        if let Some(jobs) = &self.jobs {
            // The worker only goes away when the runner is dropped.
            let _ = jobs.send(supervisor);
        }
        Job { cancel }
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        // Closing the channel wakes the worker and ends its loop.
        self.jobs.take();
        let Some(thread) = self.thread.take() else {
            return;
        };
        match self.ended.recv_timeout(STOP_LIMIT) {
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                // A job broke the rule and blocks the thread: leave it behind
                // rather than hang the caller.
            }
            _ => {
                let _ = thread.join();
            }
        }
    }
}

impl Job {
    /// Stops the job at its next await; the outcome is [`Outcome::Cancelled`].
    pub fn cancel(&self) {
        // The permit is kept if the worker has not taken the job yet.
        self.cancel.notify_one();
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// What ended the wait for a job.
enum End<T> {
    Joined(Result<T, tokio::task::JoinError>),
    Cancelled,
    Deadline,
}

/// Runs the job as its own task, so that a panic is contained and seen, and
/// races it against the cancel signal and the deadline.
async fn supervise<T, F, C>(deadline: Duration, job: F, cancel: Arc<Notify>, on_outcome: C)
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
    C: FnOnce(Outcome<T>) + Send + 'static,
{
    let mut task = tokio::spawn(job);
    let end = tokio::select! {
        joined = &mut task => End::Joined(joined),
        () = cancel.notified() => End::Cancelled,
        () = tokio::time::sleep(deadline) => End::Deadline,
    };
    let outcome = match end {
        End::Joined(Ok(value)) => Outcome::Finished(value),
        End::Joined(Err(error)) if error.is_panic() => Outcome::Panicked,
        End::Joined(Err(_)) => Outcome::Cancelled,
        End::Cancelled => stop(task, Outcome::Cancelled).await,
        End::Deadline => stop(task, Outcome::TimedOut).await,
    };
    on_outcome(outcome);
}

/// Stops the job and waits until it is gone, so the outcome is reported only
/// after the job and anything it produced were dropped.
async fn stop<T>(task: tokio::task::JoinHandle<T>, outcome: Outcome<T>) -> Outcome<T> {
    task.abort();
    let _ = task.await;
    outcome
}
