//! The capture path without its widgets: one job on the work runner takes
//! an interactive screenshot, recognizes its text and deletes the snapshot.
//!
//! [`start`] starts the job; it tells when the snapshot was taken and ends
//! with one [`Capture`]: what was [`Captured`] and its [`Timing`], measured
//! on the worker. [`captured_result`] says what an outcome of the job is for
//! the window. The snapshot is deleted after every capture that
//! produced a file, whatever the recognition gave, and also when the job is
//! cancelled, stopped at its deadline or dropped: the guard that deletes it
//! lives inside the job. What the window does with the text is
//! [`resident`](crate::resident)'s.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use addict_core::{APP_ID, translation::Source as Language};
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

/// What a capture took, measured on the worker; what was not reached is
/// `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timing {
    /// From the start of the job to the snapshot or to the capture's failure:
    /// the wait for the area is in it.
    pub capture_ms: Option<u64>,
    /// The recognition alone, as the adapter measured it; `None` also when
    /// the adapter found no image to recognize.
    pub recognition_ms: Option<u64>,
    /// The peak RSS of the Tesseract child, as the adapter read it.
    pub child_peak_rss_bytes: Option<u64>,
}

/// What a capture job ended with, and what it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub captured: Captured,
    pub timing: Timing,
}

impl Capture {
    /// A capture that never started or did not finish: nothing was measured.
    fn unfinished() -> Self {
        Self {
            captured: Captured::Failed(Failure::Unfinished),
            timing: Timing::default(),
        }
    }
}

/// The milliseconds of `duration`.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The traineddata of Tesseract for a text read in `language`. No wildcard
/// arm: a new language does not compile until it is given its data.
pub fn ocr_language(language: Language) -> &'static str {
    match language {
        Language::En => "eng",
        Language::Es => "spa",
        Language::Ca => "cat",
    }
}

/// Starts the capture job on `work`, which recognizes the text in
/// `language`. `on_captured` runs on the worker once the snapshot is there,
/// before its recognition; `on_outcome` runs once on the worker when the job
/// has ended. Without a runner nothing starts and the capture's end is given
/// back, with nothing measured.
pub fn start(
    work: Option<&Work>,
    language: Language,
    on_captured: impl FnOnce() + Send + 'static,
    on_outcome: impl FnOnce(Outcome<Capture>) + Send + 'static,
) -> Result<Job, Capture> {
    let work = work.ok_or_else(Capture::unfinished)?;
    let job = async move {
        let started = Instant::now();
        let shot = tokio::time::timeout(CAPTURE_LIMIT, screenshot::capture(APP_ID)).await;
        let capture_ms = Some(millis(started.elapsed()));
        let failed = |failure| Capture {
            captured: Captured::Failed(failure),
            timing: Timing {
                capture_ms,
                ..Timing::default()
            },
        };
        let path = match shot {
            Ok(Ok(path)) => path,
            Ok(Err(failure)) => return failed(Failure::Capture(failure)),
            Err(_) => return failed(Failure::CaptureTimedOut),
        };
        let params = Params::in_language(ocr_language(language));
        let mut capture = recognize_snapshot(path, &params, on_captured).await;
        capture.timing.capture_ms = capture_ms;
        capture
    };
    Ok(work.run(JOB_LIMIT, job, on_outcome))
}

/// Recognizes the snapshot at `path` with `params` and deletes it, however
/// this ends. The timing holds the recognition alone.
async fn recognize_snapshot(path: PathBuf, params: &Params, on_captured: impl FnOnce()) -> Capture {
    let snapshot = Snapshot(path);
    on_captured();
    let recognition = ocr::recognize(snapshot.path(), params, OCR_LIMIT).await;
    drop(snapshot);
    // Without an image nothing was recognized, so nothing was measured.
    let recognition_ms = match &recognition.result {
        Err(OcrFailure::NoImage) => None,
        _ => Some(millis(recognition.elapsed)),
    };
    let captured = match recognition.result {
        Ok(text) => Captured::Text(text),
        Err(failure) => Captured::Failed(Failure::Ocr(failure)),
    };
    Capture {
        captured,
        timing: Timing {
            capture_ms: None,
            recognition_ms,
            child_peak_rss_bytes: recognition.peak_rss_bytes,
        },
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

/// What the window is given for the `outcome` of the job, with what it
/// took, or `None` when it was cancelled: then it was dropped on purpose and
/// shows nothing. A job that did not finish has nothing measured. No
/// wildcard arm: a new outcome does not compile until it is given its end.
pub fn captured_result(outcome: Outcome<Capture>) -> Option<(Captured, Timing)> {
    let capture = match outcome {
        Outcome::Finished(capture) => capture,
        Outcome::TimedOut | Outcome::Panicked => Capture::unfinished(),
        Outcome::Cancelled => return None,
    };
    Some((capture.captured, capture.timing))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        path::PathBuf,
        sync::{
            Arc, Mutex, MutexGuard,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
    };

    use addict_core::translation::Source as Language;
    use addict_platform::{
        ocr::{OcrFailure, Params},
        screenshot::CaptureFailure,
        work::{Outcome, Work},
    };

    use super::{
        CAPTURE_LIMIT, Capture, Captured, Failure, JOB_LIMIT, OCR_LIMIT, Timing, captured_result,
        ocr_language, recognize_snapshot, start,
    };
    use crate::manual::tests::WAIT;

    pub(crate) const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../addict-platform/tests/data/ocr-fixture.png"
    );

    /// Held by every test that starts a child process, so a test that counts
    /// the children of this process sees none of another test's.
    pub(crate) fn children() -> MutexGuard<'static, ()> {
        static CHILDREN: Mutex<()> = Mutex::new(());
        CHILDREN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

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
        let _children = children();
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
            recognize_snapshot(path.clone(), &Params::ENGLISH, on_captured),
            move |outcome| {
                let _ = sender.send(outcome);
            },
        );
        let outcome = receiver.recv_timeout(WAIT).expect("outcome");
        let (captured, timing) = captured_result(outcome).expect("shown");
        assert_eq!(
            captured,
            Captured::Text("The bank is beside the river.".to_owned())
        );
        // The recognition alone: the capture's own time is the job's.
        assert_eq!(timing.capture_ms, None);
        assert!(
            timing
                .recognition_ms
                .is_some_and(|ms| ms < WAIT.as_millis() as u64)
        );
        assert!(timing.child_peak_rss_bytes.is_some_and(|bytes| bytes > 0));
        assert!(told.load(Ordering::SeqCst));
        assert!(!path.exists());
    }

    #[test]
    fn a_cancelled_recognition_still_deletes_the_snapshot() {
        let _children = children();
        let path = snapshot("cancelled");
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let (captured, recognizing) = mpsc::channel();
        let job = work.run(
            JOB_LIMIT,
            recognize_snapshot(path.clone(), &Params::ENGLISH, move || {
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
        let _children = children();
        let path = std::env::temp_dir().join("addict-capture-test-absent.png");
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let _job = work.run(
            JOB_LIMIT,
            recognize_snapshot(path, &Params::ENGLISH, || {}),
            move |outcome| {
                let _ = sender.send(outcome);
            },
        );
        let outcome = receiver.recv_timeout(WAIT).expect("outcome");
        // Nothing was recognized, so nothing of a recognition is measured.
        assert_eq!(
            captured_result(outcome),
            Some((
                Captured::Failed(Failure::Ocr(OcrFailure::NoImage)),
                Timing::default()
            ))
        );
    }

    #[test]
    fn every_language_is_recognized_with_its_own_data() {
        assert_eq!(Language::ALL.map(ocr_language), ["eng", "spa", "cat"]);
    }

    /// Whether Tesseract lists the data `language` as installed.
    fn installed(language: &str) -> bool {
        std::process::Command::new("tesseract")
            .arg("--list-langs")
            .output()
            .is_ok_and(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.trim() == language)
            })
    }

    #[test]
    fn a_catalan_snapshot_without_catalan_data_is_language_missing() {
        let _children = children();
        // Only where Tesseract runs and the Catalan data is not there.
        if !installed("eng") || installed("cat") {
            return;
        }
        let path = snapshot("catalan");
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let params = Params::in_language(ocr_language(Language::Ca));
        let _job = work.run(
            JOB_LIMIT,
            async move { recognize_snapshot(path, &params, || {}).await },
            move |outcome| {
                let _ = sender.send(outcome);
            },
        );
        let outcome = receiver.recv_timeout(WAIT).expect("outcome");
        let (captured, _) = captured_result(outcome).expect("shown");
        assert_eq!(
            captured,
            Captured::Failed(Failure::Ocr(OcrFailure::LanguageMissing))
        );
    }

    #[test]
    fn without_a_runner_nothing_starts() {
        let started = start(
            None,
            Language::Ca,
            || panic!("captured"),
            |_| panic!("ended"),
        );
        assert_eq!(
            started.err(),
            Some(Capture {
                captured: Captured::Failed(Failure::Unfinished),
                timing: Timing::default(),
            })
        );
    }

    #[test]
    fn every_end_but_a_cancel_is_shown() {
        let failed = Captured::Failed(Failure::Capture(CaptureFailure::Ended));
        let timing = Timing {
            capture_ms: Some(1200),
            ..Timing::default()
        };
        assert_eq!(
            captured_result(Outcome::Finished(Capture {
                captured: failed.clone(),
                timing,
            })),
            Some((failed, timing))
        );
        // A job that did not finish has nothing measured.
        for outcome in [Outcome::TimedOut, Outcome::Panicked] {
            assert_eq!(
                captured_result(outcome),
                Some((Captured::Failed(Failure::Unfinished), Timing::default()))
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
