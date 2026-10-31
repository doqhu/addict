//! The capture path without its widgets: one job on the work runner takes
//! an interactive screenshot, recognizes its text and deletes the snapshot.
//!
//! [`start`] starts the job; it tells when the snapshot was taken and ends
//! with one [`Capture`]: what was [`Captured`] and its [`Timing`], measured
//! on the worker. A portal answer without a snapshot ends the job there, with
//! no recognition. [`captured_result`] says what an outcome of the job is for
//! the window. The snapshot is deleted after every capture that
//! produced a file, whatever the recognition gave, and also when the job is
//! cancelled, stopped at its deadline or dropped: the guard that deletes it
//! lives inside the job. What the window does with the text is
//! [`resident`](crate::resident)'s.
//!
//! Which Tesseract runs is decided in the job, before each recognition, by
//! [`tesseract`]: the files `tesseract`, a program path, and `tessdata`, a
//! data directory, of the configuration directory,
//! [`addict_deepl::key::config_dir`], and the directory of the running
//! executable go to [`Tesseract::locate`]. Each file is its first nonblank
//! line, trimmed; a file that is missing, cannot be read, is not a regular
//! file, holds no nonblank line or a relative path is unset, and nothing is
//! said about it.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use addict_core::{APP_ID, translation::Source as Language};
use addict_platform::{
    ocr::{self, OcrFailure, Params, Tesseract},
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

/// The file of the configuration directory that names the Tesseract program.
const PROGRAM_FILE: &str = "tesseract";

/// The file of the configuration directory that names the data directory.
const TESSDATA_FILE: &str = "tessdata";

/// How many bytes of either file are read: a path at most `PATH_MAX`.
const SETTING_LIMIT: u64 = 4096;

/// The Tesseract of the one rule, from the two setting files and the
/// directory of the running executable. This reads files, so it runs on the
/// runner.
pub fn tesseract() -> Tesseract {
    let config = addict_deepl::key::config_dir();
    let setting = |name| config.as_ref().and_then(|dir| setting(&dir.join(name)));
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_owned));
    Tesseract::locate(
        setting(PROGRAM_FILE).as_deref(),
        setting(TESSDATA_FILE).as_deref(),
        exe_dir.as_deref(),
    )
}

/// The path the setting file at `path` names: its first nonblank line,
/// trimmed; `None` when there is none or the file cannot be read.
fn setting(path: &Path) -> Option<PathBuf> {
    let content = read_setting(path).ok()?;
    let line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    Some(PathBuf::from(line))
}

/// The first bytes of the regular file at `path`, as text.
fn read_setting(path: &Path) -> io::Result<String> {
    // Before opening: opening a pipe would wait for its writer.
    if !fs::metadata(path)?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut content = String::new();
    File::open(path)?
        .take(SETTING_LIMIT)
        .read_to_string(&mut content)?;
    Ok(content)
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
        let answer = portal_answer(CAPTURE_LIMIT, screenshot::capture(APP_ID)).await;
        let capture_ms = Some(millis(started.elapsed()));
        after_answer(answer, capture_ms, language, tesseract, on_captured).await
    };
    Ok(work.run(JOB_LIMIT, job, on_outcome))
}

/// What the portal's `shot` answered within `limit`.
async fn portal_answer(
    limit: Duration,
    shot: impl Future<Output = Result<PathBuf, CaptureFailure>>,
) -> Result<PathBuf, Failure> {
    match tokio::time::timeout(limit, shot).await {
        Ok(answer) => answer.map_err(Failure::Capture),
        Err(_) => Err(Failure::CaptureTimedOut),
    }
}

/// The rest of the job once the portal's `answer` is there, `capture_ms` after
/// its start. A failed answer is reported as it is: no Tesseract is looked for
/// and nothing is recognized. `locate` gives the Tesseract of a snapshot.
async fn after_answer(
    answer: Result<PathBuf, Failure>,
    capture_ms: Option<u64>,
    language: Language,
    locate: impl FnOnce() -> Tesseract,
    on_captured: impl FnOnce(),
) -> Capture {
    let path = match answer {
        Ok(path) => path,
        Err(failure) => {
            return Capture {
                captured: Captured::Failed(failure),
                timing: Timing {
                    capture_ms,
                    ..Timing::default()
                },
            };
        }
    };
    let params = Params::in_language(ocr_language(language));
    let mut capture = recognize_snapshot(&locate(), path, &params, on_captured).await;
    capture.timing.capture_ms = capture_ms;
    capture
}

/// Recognizes the snapshot at `path` with `tesseract` and `params` and
/// deletes it, however this ends. The timing holds the recognition alone.
async fn recognize_snapshot(
    tesseract: &Tesseract,
    path: PathBuf,
    params: &Params,
    on_captured: impl FnOnce(),
) -> Capture {
    let snapshot = Snapshot(path);
    on_captured();
    let recognition = ocr::recognize(tesseract, snapshot.path(), params, OCR_LIMIT).await;
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
        time::Duration,
    };

    use addict_core::translation::Source as Language;
    use addict_platform::{
        ocr::{OcrFailure, Params},
        screenshot::CaptureFailure,
        work::{Outcome, Work},
    };

    use super::{
        CAPTURE_LIMIT, Capture, Captured, Failure, JOB_LIMIT, OCR_LIMIT, Timing, after_answer,
        captured_result, ocr_language, portal_answer, recognize_snapshot, setting, start,
        tesseract,
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
        let shot = path.clone();
        let _job =
            work.run(
                JOB_LIMIT,
                async move {
                    recognize_snapshot(&tesseract(), shot, &Params::ENGLISH, on_captured).await
                },
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
        let shot = path.clone();
        let job = work.run(
            JOB_LIMIT,
            async move {
                recognize_snapshot(&tesseract(), shot, &Params::ENGLISH, move || {
                    let _ = captured.send(());
                })
                .await
            },
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

    /// What `job` ends with on a runner of its own.
    fn run_job<T: Send + 'static>(job: impl Future<Output = T> + Send + 'static) -> Outcome<T> {
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let _job = work.run(JOB_LIMIT, job, move |outcome| {
            let _ = sender.send(outcome);
        });
        receiver.recv_timeout(WAIT).expect("outcome")
    }

    #[test]
    fn a_failed_answer_is_reported_and_never_recognized() {
        for failure in [
            Failure::Capture(CaptureFailure::Cancelled),
            Failure::Capture(CaptureFailure::Ended),
            Failure::Capture(CaptureFailure::Unavailable),
            Failure::Capture(CaptureFailure::Failed("screenshot: refused".to_owned())),
            Failure::Capture(CaptureFailure::NoImage),
            Failure::CaptureTimedOut,
        ] {
            let answer = Err(failure.clone());
            // A panic here would end the job as unfinished, not as `failure`.
            let outcome = run_job(after_answer(
                answer,
                Some(1200),
                Language::En,
                || panic!("Tesseract was looked for"),
                || panic!("the snapshot was announced"),
            ));
            assert_eq!(
                captured_result(outcome),
                Some((
                    Captured::Failed(failure),
                    Timing {
                        capture_ms: Some(1200),
                        ..Timing::default()
                    }
                ))
            );
        }
    }

    #[test]
    fn a_good_answer_recognizes_only_its_snapshot_and_deletes_it() {
        let _children = children();
        let path = snapshot("answered");
        let other = snapshot("answered-other");
        let told = Arc::new(AtomicBool::new(false));
        let announced = told.clone();
        let outcome = run_job(after_answer(
            Ok(path.clone()),
            Some(1200),
            Language::En,
            tesseract,
            move || announced.store(true, Ordering::SeqCst),
        ));
        let (captured, timing) = captured_result(outcome).expect("shown");
        assert_eq!(
            captured,
            Captured::Text("The bank is beside the river.".to_owned())
        );
        assert_eq!(timing.capture_ms, Some(1200));
        assert!(told.load(Ordering::SeqCst));
        assert!(!path.exists());
        assert!(other.exists());
        std::fs::remove_file(other).expect("remove");
    }

    #[test]
    fn a_portal_that_stays_silent_is_out_of_time() {
        let answer = run_job(portal_answer(
            Duration::from_millis(10),
            std::future::pending(),
        ));
        assert_eq!(answer, Outcome::Finished(Err(Failure::CaptureTimedOut)));
    }

    #[test]
    fn a_portal_answer_in_time_is_kept() {
        let failed = run_job(portal_answer(WAIT, async { Err(CaptureFailure::Ended) }));
        assert_eq!(
            failed,
            Outcome::Finished(Err(Failure::Capture(CaptureFailure::Ended)))
        );
        let path = PathBuf::from("/shot.png");
        let shot = path.clone();
        let taken = run_job(portal_answer(WAIT, async move { Ok(shot) }));
        assert_eq!(taken, Outcome::Finished(Ok(path)));
    }

    #[test]
    fn a_missing_snapshot_is_no_image() {
        let _children = children();
        let path = std::env::temp_dir().join("addict-capture-test-absent.png");
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let _job = work.run(
            JOB_LIMIT,
            async move { recognize_snapshot(&tesseract(), path, &Params::ENGLISH, || {}).await },
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
    fn a_setting_is_its_first_nonblank_line_trimmed() {
        let directory = std::env::temp_dir().join(format!(
            "addict-capture-test-setting-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).expect("directory");
        let file = directory.join("tesseract");
        std::fs::write(&file, "\n  \n  /opt/t/tesseract  \n/other\n").expect("setting");
        assert_eq!(setting(&file), Some(PathBuf::from("/opt/t/tesseract")));
        // Blank, missing, not a regular file, not text: unset.
        std::fs::write(&file, " \n\t\n").expect("setting");
        assert_eq!(setting(&file), None);
        std::fs::write(&file, b"/opt/\xff\n").expect("setting");
        assert_eq!(setting(&file), None);
        assert_eq!(setting(&directory.join("absent")), None);
        assert_eq!(setting(&directory), None);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn every_language_is_recognized_with_its_own_data() {
        assert_eq!(Language::ALL.map(ocr_language), ["eng", "spa", "cat"]);
    }

    /// Whether the Tesseract of the rule lists the data `language` as
    /// installed.
    fn installed(language: &str) -> bool {
        let tesseract = tesseract();
        let mut command = std::process::Command::new(tesseract.program());
        if let Some(tessdata) = tesseract.tessdata() {
            command.arg("--tessdata-dir").arg(tessdata);
        }
        command.arg("--list-langs").output().is_ok_and(|output| {
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
            async move { recognize_snapshot(&tesseract(), path, &params, || {}).await },
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
