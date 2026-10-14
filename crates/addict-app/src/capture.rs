//! The capture path without its widgets: one job on the work runner takes
//! an interactive screenshot, recognizes its text and deletes the snapshot.
//!
//! [`start`] starts the job; it tells when the snapshot was taken and ends
//! with one [`Captured`]. [`captured_result`] says what an outcome of the job
//! is for the window. The snapshot is deleted after every capture that
//! produced a file, whatever the recognition gave, and also when the job is
//! cancelled, stopped at its deadline or dropped: the guard that deletes it
//! lives inside the job. What the window does with the text is
//! [`resident`](crate::resident)'s.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use addict_core::APP_ID;
use addict_platform::{
    ocr::{self, OcrFailure, Params},
    screenshot::{self, CaptureFailure},
    work::{Job, Outcome, Work},
};

/// How long the user may take to choose the area, or to cancel.
pub const CAPTURE_LIMIT: Duration = Duration::from_secs(300);

/// How long the recognition of one snapshot may take.
pub const OCR_LIMIT: Duration = Duration::from_secs(15);

/// The deadline of the whole job: both limits and a margin for the rest.
pub const JOB_LIMIT: Duration = CAPTURE_LIMIT
    .saturating_add(OCR_LIMIT)
    .saturating_add(Duration::from_secs(5));

/// What a capture ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// The recognized text, as Tesseract gave it, trimmed.
    Text(String),
    /// No text.
    Failed(Failure),
}

/// Why a capture gave no text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The screenshot gave no snapshot.
    Capture(CaptureFailure),
    /// The user took longer than [`CAPTURE_LIMIT`].
    CaptureTimedOut,
    /// The recognition gave no text.
    Ocr(OcrFailure),
    /// The job could not start, panicked or was stopped at its deadline.
    Unfinished,
}

/// Starts the capture job on `work`. `on_captured` runs on the worker once
/// the snapshot is there, before its recognition; `on_outcome` runs once on
/// the worker when the job has ended. Without a runner nothing starts and
/// the capture's end is given back.
pub fn start(
    work: Option<&Work>,
    on_captured: impl FnOnce() + Send + 'static,
    on_outcome: impl FnOnce(Outcome<Captured>) + Send + 'static,
) -> Result<Job, Captured> {
    let work = work.ok_or(Captured::Failed(Failure::Unfinished))?;
    let job = async move {
        let path = match tokio::time::timeout(CAPTURE_LIMIT, screenshot::capture(APP_ID)).await {
            Ok(Ok(path)) => path,
            Ok(Err(failure)) => return Captured::Failed(Failure::Capture(failure)),
            Err(_) => return Captured::Failed(Failure::CaptureTimedOut),
        };
        recognize_snapshot(path, on_captured).await
    };
    Ok(work.run(JOB_LIMIT, job, on_outcome))
}

/// Recognizes the snapshot at `path` and deletes it, however this ends.
async fn recognize_snapshot(path: PathBuf, on_captured: impl FnOnce()) -> Captured {
    let snapshot = Snapshot(path);
    on_captured();
    let recognition = ocr::recognize(snapshot.path(), &Params::ENGLISH, OCR_LIMIT).await;
    drop(snapshot);
    match recognition.result {
        Ok(text) => Captured::Text(text),
        Err(failure) => Captured::Failed(Failure::Ocr(failure)),
    }
}

/// The snapshot file, deleted on drop.
struct Snapshot(PathBuf);

impl Snapshot {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// What the window is given for the `outcome` of the job, or `None` when it
/// was cancelled: then it was dropped on purpose and shows nothing. No
/// wildcard arm: a new outcome does not compile until it is given its end.
pub fn captured_result(outcome: Outcome<Captured>) -> Option<Captured> {
    match outcome {
        Outcome::Finished(captured) => Some(captured),
        Outcome::TimedOut | Outcome::Panicked => Some(Captured::Failed(Failure::Unfinished)),
        Outcome::Cancelled => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
    };

    use addict_platform::{
        ocr::OcrFailure,
        screenshot::CaptureFailure,
        work::{Outcome, Work},
    };

    use super::{
        CAPTURE_LIMIT, Captured, Failure, JOB_LIMIT, OCR_LIMIT, captured_result,
        recognize_snapshot, start,
    };
    use crate::manual::tests::WAIT;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../experiments/exp4/ocr-fixture.png"
    );

    /// A copy of the fixture in a place of its own, as a snapshot would be.
    fn snapshot(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "addict-capture-test-{name}-{}.png",
            std::process::id()
        ));
        std::fs::copy(FIXTURE, &path).expect("copy");
        path
    }

    #[test]
    fn the_snapshot_is_recognized_and_deleted() {
        let path = snapshot("recognized");
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let told = Arc::new(AtomicBool::new(false));
        let on_captured = {
            let told = told.clone();
            move || told.store(true, Ordering::SeqCst)
        };
        let _job = work.run(
            JOB_LIMIT,
            recognize_snapshot(path.clone(), on_captured),
            move |outcome| {
                let _ = sender.send(outcome);
            },
        );
        let outcome = receiver.recv_timeout(WAIT).expect("outcome");
        assert_eq!(
            captured_result(outcome),
            Some(Captured::Text("The bank is beside the river.".to_owned()))
        );
        assert!(told.load(Ordering::SeqCst));
        assert!(!path.exists());
    }

    #[test]
    fn a_cancelled_recognition_still_deletes_the_snapshot() {
        let path = snapshot("cancelled");
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let (captured, recognizing) = mpsc::channel();
        let job = work.run(
            JOB_LIMIT,
            recognize_snapshot(path.clone(), move || {
                let _ = captured.send(());
            }),
            move |outcome| {
                let _ = sender.send(outcome);
            },
        );
        recognizing.recv_timeout(WAIT).expect("captured");
        job.cancel();
        let outcome = receiver.recv_timeout(WAIT).expect("outcome");
        assert_eq!(outcome, Outcome::Cancelled);
        assert_eq!(captured_result(outcome), None);
        assert!(!path.exists());
    }

    #[test]
    fn a_missing_snapshot_is_no_image() {
        let path = std::env::temp_dir().join("addict-capture-test-absent.png");
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let _job = work.run(JOB_LIMIT, recognize_snapshot(path, || {}), move |outcome| {
            let _ = sender.send(outcome);
        });
        let outcome = receiver.recv_timeout(WAIT).expect("outcome");
        assert_eq!(
            captured_result(outcome),
            Some(Captured::Failed(Failure::Ocr(OcrFailure::NoImage)))
        );
    }

    #[test]
    fn without_a_runner_nothing_starts() {
        let started = start(None, || panic!("captured"), |_| panic!("ended"));
        assert_eq!(started.err(), Some(Captured::Failed(Failure::Unfinished)));
    }

    #[test]
    fn every_end_but_a_cancel_is_shown() {
        let failed = Captured::Failed(Failure::Capture(CaptureFailure::Ended));
        assert_eq!(
            captured_result(Outcome::Finished(failed.clone())),
            Some(failed)
        );
        for outcome in [Outcome::TimedOut, Outcome::Panicked] {
            assert_eq!(
                captured_result(outcome),
                Some(Captured::Failed(Failure::Unfinished))
            );
        }
        assert_eq!(captured_result(Outcome::Cancelled), None);
    }

    #[test]
    fn the_limits_are_finite_and_the_job_outlives_both() {
        assert_eq!(CAPTURE_LIMIT.as_secs(), 300);
        assert_eq!(OCR_LIMIT.as_secs(), 15);
        assert!(JOB_LIMIT > CAPTURE_LIMIT + OCR_LIMIT);
    }
}
