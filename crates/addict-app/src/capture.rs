//! Linux effect adapters for the interactive screenshot portal and Tesseract.

use std::{
    fs::{self, File},
    future::Future,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use addict_controller::{
    ImageRef,
    deadlines::{CAPTURE_LIMIT, OCR_LIMIT},
};
use addict_core::{
    APP_ID,
    failure::{Ended, capture::CaptureFailure, recognize::OcrFailure},
    translation::Source as Language,
};
use addict_engine::{
    capture::Timing,
    work::{Job, Outcome, Work},
};
use addict_platform::{
    ocr::{self, Params, Tesseract},
    screenshot,
};

/// These worker bounds are a fallback if the controller or shell stops.
const PICKER_SAFETY_LIMIT: Duration = CAPTURE_LIMIT.saturating_add(Duration::from_secs(5));
const OCR_SAFETY_LIMIT: Duration = OCR_LIMIT.saturating_add(Duration::from_secs(5));

/// The file of the configuration directory that names the Tesseract program.
const PROGRAM_FILE: &str = "tesseract";

/// The file of the configuration directory that names the data directory.
const TESSDATA_FILE: &str = "tessdata";

/// How many bytes of either file are read: a path at most `PATH_MAX`.
const SETTING_LIMIT: u64 = 4096;

/// The Tesseract of the one rule, from the two setting files and the
/// directory of the running executable. This reads files, so it runs on the
/// runner.
fn tesseract(config: Option<&Path>) -> Tesseract {
    let (program, tessdata) = tesseract_paths(config);
    let program = program.as_deref().and_then(setting);
    let tessdata = tessdata.as_deref().and_then(setting);
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_owned));
    Tesseract::locate(program.as_deref(), tessdata.as_deref(), exe_dir.as_deref())
}

/// The two configuration files used to locate Tesseract.
pub(super) fn tesseract_paths(config: Option<&Path>) -> (Option<PathBuf>, Option<PathBuf>) {
    (
        config.map(|dir| dir.join(PROGRAM_FILE)),
        config.map(|dir| dir.join(TESSDATA_FILE)),
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
        Language::Ru => "rus",
    }
}

/// A portal file becomes an owned image before it can cross threads.
fn area_image(answer: Result<PathBuf, CaptureFailure>) -> Result<ImageRef, Ended<CaptureFailure>> {
    answer
        .map(|path| ImageRef::new(Snapshot(path)))
        .map_err(Ended::Shell)
}

pub fn start_picker(
    work: Option<&Work>,
    cancelled: impl Future<Output = ()> + Send + 'static,
    complete: impl FnOnce(Result<ImageRef, Ended<CaptureFailure>>, Option<u64>) + Send + 'static,
) -> Option<Job> {
    run_stage(
        work,
        PICKER_SAFETY_LIMIT,
        async move {
            let started = Instant::now();
            let result = area_image(screenshot::capture_until(APP_ID, cancelled).await);
            (result, Some(millis(started.elapsed())))
        },
        move |outcome| {
            let (result, elapsed) = stage_result(outcome, CaptureFailure::Cancelled);
            complete(result, elapsed);
        },
    )
}

pub fn start_recognition(
    work: Option<&Work>,
    config: Option<PathBuf>,
    image: ImageRef,
    language: Language,
    complete: impl FnOnce(Result<String, Ended<OcrFailure>>, Timing) + Send + 'static,
) -> Option<Job> {
    let started = Arc::new(OnceLock::new());
    let clock = started.clone();
    run_stage(
        work,
        OCR_SAFETY_LIMIT,
        async move {
            let tesseract = tesseract(config.as_deref());
            let params = Params::in_language(ocr_language(language));
            clock.get_or_init(Instant::now);
            recognize_snapshot(&tesseract, image, &params).await
        },
        move |outcome| {
            let interrupted = matches!(outcome, Outcome::Cancelled | Outcome::TimedOut);
            let (result, mut timing) = stage_result(outcome, OcrFailure::Cancelled);
            if interrupted {
                timing.recognition_ms = started.get().map(|clock| millis(clock.elapsed()));
            }
            complete(result, timing);
        },
    )
}

fn run_stage<T: Send + 'static>(
    work: Option<&Work>,
    safety_limit: Duration,
    stage: impl Future<Output = T> + Send + 'static,
    complete: impl FnOnce(Outcome<T>) + Send + 'static,
) -> Option<Job> {
    match work {
        Some(work) => Some(work.run(safety_limit, stage, complete)),
        None => {
            drop(stage);
            complete(Outcome::Panicked);
            None
        }
    }
}

fn stage_result<T, F, M: Default>(
    outcome: Outcome<(Result<T, Ended<F>>, M)>,
    cancelled: F,
) -> (Result<T, Ended<F>>, M) {
    match outcome {
        Outcome::Finished(result) => result,
        Outcome::Cancelled => (Err(Ended::Shell(cancelled)), M::default()),
        Outcome::TimedOut | Outcome::Panicked => (Err(Ended::Unfinished), M::default()),
    }
}

async fn recognize_snapshot(
    tesseract: &Tesseract,
    image: ImageRef,
    params: &Params,
) -> (Result<String, Ended<OcrFailure>>, Timing) {
    let Some(snapshot) = image.downcast_ref::<Snapshot>() else {
        return (Err(Ended::Shell(OcrFailure::NoImage)), Timing::default());
    };
    let recognition = ocr::recognize(tesseract, &snapshot.0, params, OCR_SAFETY_LIMIT).await;
    drop(image);
    // Without an image nothing was recognized, so nothing was measured.
    let recognition_ms = match &recognition.result {
        Err(OcrFailure::NoImage) => None,
        _ => Some(millis(recognition.elapsed)),
    };
    (
        recognition.result.map_err(Ended::Shell),
        Timing {
            capture_ms: None,
            recognition_ms,
            child_peak_rss_bytes: recognition.peak_rss_bytes,
        },
    )
}

/// The snapshot file, deleted when the last image reference is dropped.
struct Snapshot(PathBuf);

impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::{Mutex, MutexGuard, mpsc},
        time::Duration,
    };

    use addict_controller::{
        ImageRef,
        deadlines::{CAPTURE_LIMIT, OCR_LIMIT},
    };
    use addict_core::{
        failure::{Ended, capture::CaptureFailure, recognize::OcrFailure},
        translation::Source as Language,
    };
    use addict_engine::{
        capture::Timing,
        evidence::{Read, child_processes, ocr_record},
        work::{Outcome, Work},
    };
    use addict_platform::ocr::{self, Params};

    use super::{
        OCR_SAFETY_LIMIT, PICKER_SAFETY_LIMIT, area_image, ocr_language, recognize_snapshot,
        setting, stage_result, start_picker, start_recognition, tesseract,
    };

    /// Limit for each wait of the test thread.
    const WAIT: Duration = Duration::from_secs(10);

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../addict-platform/tests/data/ocr-fixture.png"
    );

    /// The Russian fixture and its sentence.
    const RUSSIAN_FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../addict-platform/tests/data/ocr-fixture-rus.png"
    );
    const RUSSIAN_SENTENCE: &str = "Банк стоит на берегу реки.";

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
        snapshot_of(FIXTURE, name)
    }

    /// A copy of `fixture` in a place of its own, as a snapshot would be.
    fn snapshot_of(fixture: &str, name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "addict-capture-test-{name}-{}.png",
            std::process::id()
        ));
        std::fs::copy(fixture, &path).expect("copy");
        path
    }

    fn image(path: &Path) -> ImageRef {
        area_image(Ok(path.to_owned())).expect("image")
    }

    fn read(path: &Path, language: Language) -> (Result<String, Ended<OcrFailure>>, Timing) {
        let work = Work::start().expect("runner");
        let (send, receive) = mpsc::channel();
        let _job = start_recognition(
            Some(&work),
            None,
            image(path),
            language,
            move |result, timing| {
                send.send((result, timing)).expect("result receiver");
            },
        );
        receive.recv_timeout(WAIT).expect("recognition result")
    }

    #[test]
    fn the_snapshot_is_recognized_and_deleted() {
        let _children = children();
        let path = snapshot("recognized");
        let (result, timing) = read(&path, Language::En);
        assert_eq!(result, Ok("The bank is beside the river.".to_owned()));
        assert_eq!(timing.capture_ms, None);
        assert!(
            timing
                .recognition_ms
                .is_some_and(|ms| ms < WAIT.as_millis() as u64)
        );
        assert!(timing.child_peak_rss_bytes.is_some_and(|bytes| bytes > 0));
        assert!(!path.exists());
    }

    #[test]
    fn a_cancelled_recognition_still_deletes_the_snapshot() {
        let _children = children();
        let path = snapshot("cancelled");
        let work = Work::start().expect("runner");
        let (sender, receiver) = mpsc::channel();
        let (started, recognizing) = mpsc::channel();
        let image = image(&path);
        let job = work.run(
            OCR_SAFETY_LIMIT,
            async move {
                started.send(()).expect("started receiver");
                std::future::pending::<()>().await;
                recognize_snapshot(&tesseract(None), image, &Params::ENGLISH).await
            },
            move |outcome| {
                sender.send(outcome).expect("result receiver");
            },
        );
        recognizing.recv_timeout(WAIT).expect("started");
        job.cancel();
        let result = stage_result(
            receiver.recv_timeout(WAIT).expect("outcome"),
            OcrFailure::Cancelled,
        );
        assert_eq!(
            result,
            (Err(Ended::Shell(OcrFailure::Cancelled)), Timing::default())
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_failed_answer_is_reported_and_never_recognized() {
        for failure in [
            CaptureFailure::Cancelled,
            CaptureFailure::Ended,
            CaptureFailure::Unavailable,
            CaptureFailure::Failed("screenshot: refused".to_owned()),
            CaptureFailure::NoImage,
        ] {
            assert_eq!(area_image(Err(failure.clone())), Err(Ended::Shell(failure)));
        }
    }

    #[test]
    fn a_good_answer_recognizes_only_its_snapshot_and_deletes_it() {
        let _children = children();
        let path = snapshot("answered");
        let other = snapshot("answered-other");
        let (recognized, _) = read(&path, Language::En);
        assert_eq!(recognized, Ok("The bank is beside the river.".to_owned()));
        assert!(!path.exists());
        assert!(other.exists());
        std::fs::remove_file(other).expect("remove");
    }

    #[test]
    fn a_portal_that_stays_silent_is_out_of_time() {
        let work = Work::start().expect("runner");
        let (send, receive) = mpsc::channel();
        let _job = work.run(
            Duration::from_millis(10),
            std::future::pending::<()>(),
            move |outcome| {
                send.send(outcome).expect("result receiver");
            },
        );
        assert_eq!(
            receive.recv_timeout(WAIT).expect("outcome"),
            Outcome::TimedOut
        );
    }

    #[test]
    fn a_portal_answer_in_time_is_kept() {
        assert_eq!(
            area_image(Err(CaptureFailure::Ended)),
            Err(Ended::Shell(CaptureFailure::Ended))
        );
        let path = snapshot("in-time");
        let taken = image(&path);
        let retained = taken.clone();
        drop(taken);
        assert!(path.exists());
        drop(retained);
        assert!(!path.exists());
    }

    #[test]
    fn a_missing_snapshot_is_no_image() {
        let _children = children();
        let path = std::env::temp_dir().join("addict-capture-test-absent.png");
        assert_eq!(
            read(&path, Language::En),
            (Err(Ended::Shell(OcrFailure::NoImage)), Timing::default())
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
        assert_eq!(
            Language::ALL.map(ocr_language),
            ["eng", "spa", "cat", "rus"]
        );
    }

    #[test]
    fn a_russian_snapshot_is_recognized_with_the_russian_data() {
        let _children = children();
        let path = snapshot_of(RUSSIAN_FIXTURE, "russian");
        let (result, _) = read(&path, Language::Ru);
        assert_eq!(result, Ok(RUSSIAN_SENTENCE.to_owned()));
        assert!(!path.exists());
    }

    /// Whether the Tesseract of the rule lists the data `language` as
    /// installed.
    fn installed(language: &str) -> bool {
        let tesseract = tesseract(None);
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
        if !installed("eng") || installed("cat") {
            return;
        }
        let path = snapshot("catalan");
        let (result, _) = read(&path, Language::Ca);
        assert_eq!(result, Err(Ended::Shell(OcrFailure::LanguageMissing)));
    }

    #[test]
    fn without_a_runner_nothing_starts() {
        let (send, receive) = mpsc::channel();
        let started = start_picker(None, std::future::pending(), move |result, elapsed| {
            send.send((result, elapsed)).expect("result receiver");
        });
        assert!(started.is_none());
        assert_eq!(
            receive.recv_timeout(WAIT).expect("outcome"),
            (Err(Ended::Unfinished), None)
        );
    }

    #[test]
    fn every_end_but_a_cancel_is_shown() {
        let failed = Err(Ended::Shell(CaptureFailure::Ended));
        assert_eq!(
            stage_result::<ImageRef, _, _>(
                Outcome::Finished((failed.clone(), Some(1200))),
                CaptureFailure::Cancelled
            ),
            (failed, Some(1200))
        );
        for outcome in [Outcome::TimedOut, Outcome::Panicked] {
            assert_eq!(
                stage_result::<ImageRef, _, Option<u64>>(outcome, CaptureFailure::Cancelled),
                (Err(Ended::Unfinished), None)
            );
        }
        assert_eq!(
            stage_result::<ImageRef, _, Option<u64>>(Outcome::Cancelled, CaptureFailure::Cancelled),
            (Err(Ended::Shell(CaptureFailure::Cancelled)), None)
        );
    }

    #[test]
    fn the_limits_are_finite_and_the_job_outlives_both() {
        assert_eq!(CAPTURE_LIMIT.as_secs(), 300);
        assert_eq!(OCR_LIMIT.as_secs(), 15);
        assert!(PICKER_SAFETY_LIMIT > CAPTURE_LIMIT);
        assert!(OCR_SAFETY_LIMIT > OCR_LIMIT);
    }

    #[test]
    fn no_child_process_is_left_after_a_recognition() {
        let _children = children();
        // The count sees a child while it runs.
        let mut sleeper = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        assert!(child_processes().is_some_and(|count| count >= 1));
        sleeper.kill().expect("kill sleep");
        sleeper.wait().expect("reap sleep");

        let work = Work::start().expect("runner");
        let (sender, receiver) = std::sync::mpsc::channel();
        let _job = work.run(
            OCR_LIMIT * 2,
            async {
                ocr::recognize(
                    &tesseract(None),
                    Path::new(FIXTURE),
                    &Params::ENGLISH,
                    OCR_LIMIT,
                )
                .await
            },
            move |outcome| {
                let _ = sender.send(outcome);
            },
        );
        let Outcome::Finished(recognition) = receiver.recv_timeout(WAIT).expect("outcome") else {
            panic!("the recognition finished");
        };
        assert!(recognition.result.is_ok());
        let timing = Timing {
            capture_ms: None,
            recognition_ms: Some(1),
            child_peak_rss_bytes: recognition.peak_rss_bytes,
        };
        let record = ocr_record(Read::Recognized, &timing);
        assert_eq!(record.details()["child_processes"], 0);
        assert!(
            record.details()["pss_bytes"]
                .as_u64()
                .is_some_and(|bytes| bytes > 0)
        );
    }
}
