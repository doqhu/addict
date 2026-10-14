//! Text recognition: one Tesseract child per image, with a limit.
//!
//! The contract is [`recognize`]: an image path and the [`Params`] go in; the
//! text or one [`OcrFailure`] comes out, with the time the recognition took
//! and the peak memory of the child.
//!
//! The child runs as `tesseract <image> result -l <language> --psm <mode>`
//! with its working directory in a fresh `0700` temporary directory, stdin
//! null and `PR_SET_PDEATHSIG(SIGKILL)`. Its exit is awaited through a pidfd,
//! so the wait stays asynchronous, and it is reaped with `wait4`, which also
//! gives its `ru_maxrss`; `tokio::process` would reap it itself and drop that.
//! The limit, or dropping the future, kills the child with SIGKILL and reaps
//! it; the temporary directory is removed in every case.
//!
//! A child that ends with another status is a missing language when its
//! stderr says so, or else when `tesseract --list-langs`, run then with the
//! same rules and what is left of the limit, does not list the language. A
//! listing that fails or outlives the limit leaves the failure as it was.

use std::{
    fs,
    io::{self, Read},
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::{fs::DirBuilderExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use tokio::io::{Interest, unix::AsyncFd};

/// The most text a recognition may give, in bytes; more is a failure.
pub const OUTPUT_LIMIT: u64 = 64 * 1024;

/// The base name of Tesseract's output inside the temporary directory; it
/// writes `result.txt`.
const OUTPUT_BASE: &str = "result";

/// Where Tesseract's stderr goes inside the temporary directory.
const STDERR_FILE: &str = "stderr";

/// Where the output of `--list-langs` goes inside the temporary directory.
const LANGUAGES_FILE: &str = "languages";

/// The Tesseract program, found on `PATH`.
const PROGRAM: &str = "tesseract";

/// The prefix of every temporary directory.
pub const TEMP_PREFIX: &str = "addict-ocr-";

/// What Tesseract writes to stderr when the language data cannot be loaded.
const LANGUAGE_MISSING: &str = "Failed loading language";

/// The Tesseract parameters of a recognition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// The `-l` language, the name of its traineddata.
    pub language: &'static str,
    /// The `--psm` page segmentation mode.
    pub page_segmentation: u8,
}

impl Params {
    /// English, one uniform block of text.
    pub const ENGLISH: Params = Params {
        language: "eng",
        page_segmentation: 6,
    };
}

/// Why a recognition gave no text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrFailure {
    /// No file is at the image path.
    NoImage,
    /// `tesseract` is not installed: spawning it found no program.
    TesseractMissing,
    /// The data of the language is not installed.
    LanguageMissing,
    /// Any other failure: the exit status with the first stderr line, an
    /// output over [`OUTPUT_LIMIT`], or the step that failed with its error.
    Failed(String),
    /// Tesseract ended well but found no text.
    Empty,
    /// The child was still running at the limit; it was killed and reaped.
    TimedOut,
}

/// What one recognition gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recognition {
    /// The text, trimmed, or why there is none.
    pub result: Result<String, OcrFailure>,
    /// How long the call took.
    pub elapsed: Duration,
    /// The peak resident memory of the child, when it was reaped after its
    /// own exit.
    pub peak_rss_bytes: Option<u64>,
}

/// Recognizes the text of `image` with `params`; the child may run for
/// `limit` at most.
pub async fn recognize(image: &Path, params: &Params, limit: Duration) -> Recognition {
    run(
        PROGRAM,
        image,
        params,
        limit,
        &std::env::temp_dir(),
        &mut |_| {},
    )
    .await
}

/// [`recognize`] with the program to run, the parent of the temporary
/// directory and a callback that is given the pid of the child once it runs.
async fn run(
    program: &str,
    image: &Path,
    params: &Params,
    limit: Duration,
    temp_root: &Path,
    started: &mut (dyn FnMut(u32) + Send),
) -> Recognition {
    let start = Instant::now();
    let (result, peak_rss_bytes) =
        match attempt(program, image, params, limit, temp_root, started).await {
            Ok((text, peak)) => (Ok(text), peak),
            Err((failure, peak)) => (Err(failure), peak),
        };
    Recognition {
        result,
        elapsed: start.elapsed(),
        peak_rss_bytes,
    }
}

type Attempt = Result<(String, Option<u64>), (OcrFailure, Option<u64>)>;

async fn attempt(
    program: &str,
    image: &Path,
    params: &Params,
    limit: Duration,
    temp_root: &Path,
    started: &mut (dyn FnMut(u32) + Send),
) -> Attempt {
    let begun = Instant::now();
    let failed = |failure| Err((failure, None));
    // Absolute: the child runs in another directory.
    let image = match fs::canonicalize(image) {
        Ok(image) if image.is_file() => image,
        _ => return failed(OcrFailure::NoImage),
    };
    // Declared first, so it is removed after the child is gone.
    let directory = match TempDir::create(temp_root) {
        Ok(directory) => directory,
        Err(e) => return failed(OcrFailure::Failed(format!("temporary directory: {e}"))),
    };
    let stderr = match fs::File::create(directory.0.join(STDERR_FILE)) {
        Ok(file) => file,
        Err(e) => return failed(OcrFailure::Failed(format!("stderr file: {e}"))),
    };
    let mut command = Command::new(program);
    command
        .arg(&image)
        .arg(OUTPUT_BASE)
        .args(["-l", params.language, "--psm"])
        .arg(params.page_segmentation.to_string())
        .current_dir(&directory.0)
        .stdout(Stdio::null())
        .stderr(stderr);
    let mut child = match spawn(&mut command) {
        Ok(child) => child,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return failed(OcrFailure::TesseractMissing);
        }
        Err(e) => return failed(OcrFailure::Failed(format!("spawn tesseract: {e}"))),
    };
    started(child.pid as u32);

    let exit = match tokio::time::timeout(limit, child.exited()).await {
        Ok(Ok(exit)) => exit,
        Ok(Err(e)) => return failed(OcrFailure::Failed(format!("wait for tesseract: {e}"))),
        Err(_) => {
            // Killed and reaped before the outcome is given.
            drop(child);
            return failed(OcrFailure::TimedOut);
        }
    };
    let peak = Some(exit.peak_rss_bytes);
    let exited_well = libc::WIFEXITED(exit.status) && libc::WEXITSTATUS(exit.status) == 0;
    let result = match outcome(&directory.0, exit.status) {
        // Only after a failed exit: the recognition itself stays one child.
        Err(OcrFailure::Failed(detail)) if !exited_well => {
            let remaining = limit.saturating_sub(begun.elapsed());
            match listed(program, params.language, &directory.0, remaining).await {
                Some(false) => Err(OcrFailure::LanguageMissing),
                Some(true) | None => Err(OcrFailure::Failed(detail)),
            }
        }
        result => result,
    };
    match result {
        Ok(text) => Ok((text, peak)),
        Err(failure) => Err((failure, peak)),
    }
}

/// Whether `language` is a line of what `<program> --list-langs` prints,
/// run in `directory` for `limit` at most; `None` when the listing did not
/// end well in time. A listing still running at the limit, or dropped, is
/// killed and reaped.
async fn listed(program: &str, language: &str, directory: &Path, limit: Duration) -> Option<bool> {
    let path = directory.join(LANGUAGES_FILE);
    let output = fs::File::create(&path).ok()?;
    let errors = output.try_clone().ok()?;
    let mut command = Command::new(program);
    command
        .arg("--list-langs")
        .current_dir(directory)
        // Tesseract prints the list to stdout; old versions to stderr.
        .stdout(output)
        .stderr(errors);
    let mut child = spawn(&mut command).ok()?;
    let exit = tokio::time::timeout(limit, child.exited())
        .await
        .ok()?
        .ok()?;
    if !(libc::WIFEXITED(exit.status) && libc::WEXITSTATUS(exit.status) == 0) {
        return None;
    }
    let listing = read_head(&path).ok()?;
    Some(listing.lines().any(|line| line.trim() == language))
}

/// Spawns `command` with stdin null and `PR_SET_PDEATHSIG(SIGKILL)`; the
/// child is reaped by the [`Child`] that is given back, not by `std`.
fn spawn(command: &mut Command) -> io::Result<Child> {
    command.stdin(Stdio::null());
    let parent = std::process::id();
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            // The parent may have gone before the death signal was armed.
            if libc::getppid() != parent as libc::pid_t {
                return Err(io::Error::other("the parent exited during the spawn"));
            }
            Ok(())
        });
    }
    // The handle holds nothing else: dropping it neither waits nor kills.
    let child = command.spawn()?;
    Ok(Child::new(child.id() as libc::pid_t))
}

/// The text, or the failure, for a child that ended with the wait `status`;
/// its files are in `directory`.
fn outcome(directory: &Path, status: libc::c_int) -> Result<String, OcrFailure> {
    if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
        let path = directory.join(OUTPUT_BASE).with_extension("txt");
        let text = read_limited(&path)
            .map_err(|e| OcrFailure::Failed(format!("read the output: {e}")))?
            .ok_or_else(|| OcrFailure::Failed(format!("output over {OUTPUT_LIMIT} bytes")))?;
        let text = text.trim();
        return if text.is_empty() {
            Err(OcrFailure::Empty)
        } else {
            Ok(text.to_owned())
        };
    }
    let stderr = read_head(&directory.join(STDERR_FILE)).unwrap_or_default();
    if stderr.contains(LANGUAGE_MISSING) {
        return Err(OcrFailure::LanguageMissing);
    }
    let how = if libc::WIFEXITED(status) {
        format!("exit status {}", libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        format!("signal {}", libc::WTERMSIG(status))
    } else {
        format!("wait status {status}")
    };
    let first = stderr.lines().map(str::trim).find(|line| !line.is_empty());
    Err(OcrFailure::Failed(match first {
        Some(line) => format!("tesseract {how}: {line}"),
        None => format!("tesseract {how}"),
    }))
}

/// The text of `path`, lossily decoded, or `None` when it is longer than
/// [`OUTPUT_LIMIT`].
fn read_limited(path: &Path) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// The first [`OUTPUT_LIMIT`] bytes of `path`, lossily decoded.
fn read_head(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(OUTPUT_LIMIT)
        .read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// A fresh `0700` directory, removed with all it holds on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn create(root: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let path = root.join(format!(
            "{TEMP_PREFIX}{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // Never an existing directory: creation fails if the name is taken.
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// How a reaped child ended.
struct Exit {
    status: libc::c_int,
    peak_rss_bytes: u64,
}

/// A running child that this module reaps. Dropping it before it was reaped
/// kills it with SIGKILL and waits for it.
struct Child {
    pid: libc::pid_t,
    reaped: bool,
}

impl Child {
    fn new(pid: libc::pid_t) -> Self {
        Self { pid, reaped: false }
    }

    /// Waits, without blocking the thread, until the child exits, and reaps
    /// it.
    async fn exited(&mut self) -> io::Result<Exit> {
        // SAFETY: a plain syscall; a valid return is a new descriptor.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, self.pid, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the descriptor was just opened and is owned here alone.
        let pidfd = unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) };
        let pidfd = AsyncFd::with_interest(pidfd, Interest::READABLE)?;
        loop {
            if let Some(exit) = self.reap()? {
                return Ok(exit);
            }
            // A pidfd becomes readable when the process has exited.
            let mut ready = pidfd.readable().await?;
            ready.clear_ready();
        }
    }

    /// Reaps the child if it has exited.
    fn reap(&mut self) -> io::Result<Option<Exit>> {
        let mut status = 0;
        // SAFETY: zeroed `rusage` is a valid value to be overwritten.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: valid pointers to locals.
        let reaped = unsafe { libc::wait4(self.pid, &mut status, libc::WNOHANG, &mut usage) };
        match reaped {
            0 => Ok(None),
            pid if pid == self.pid => {
                self.reaped = true;
                let kib = u64::try_from(usage.ru_maxrss).unwrap_or(0);
                Ok(Some(Exit {
                    status,
                    peak_rss_bytes: kib * 1024,
                }))
            }
            _ => {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ECHILD) {
                    // Not ours any more: the pid must not be signalled.
                    self.reaped = true;
                }
                Err(error)
            }
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        // SAFETY: the pid is still this child's: it was not reaped yet.
        unsafe {
            libc::kill(self.pid, libc::SIGKILL);
            let mut status = 0;
            while libc::waitpid(self.pid, &mut status, 0) < 0
                && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
            {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::Duration,
    };

    use super::{
        OUTPUT_LIMIT, OcrFailure, PROGRAM, Params, Recognition, TEMP_PREFIX, outcome, run,
    };

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../experiments/exp4/ocr-fixture.png"
    );

    const LIMIT: Duration = Duration::from_secs(15);

    /// A fresh directory for one test, removed at its end.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("addict-ocr-test-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).expect("scratch");
            Self(path)
        }

        /// Whether a temporary directory of a recognition is left in it.
        fn leftovers(&self) -> Vec<PathBuf> {
            fs::read_dir(&self.0)
                .expect("scratch")
                .map(|entry| entry.expect("entry").path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(TEMP_PREFIX))
                })
                .collect()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Recognizes `image` on a runtime of its own; the temporary directory
    /// is made in `scratch`. Gives the pid of the child, if one ran.
    fn recognized(
        scratch: &Scratch,
        image: &Path,
        params: Params,
        limit: Duration,
    ) -> (Recognition, Option<u32>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let mut pid = None;
        let recognition =
            runtime.block_on(run(PROGRAM, image, &params, limit, &scratch.0, &mut |p| {
                pid = Some(p)
            }));
        (recognition, pid)
    }

    /// Whether a process with `pid` exists, a zombie included.
    fn exists(pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    /// A white grey-scale image in the PGM format, which Tesseract reads.
    fn blank_image(scratch: &Scratch) -> PathBuf {
        let path = scratch.0.join("blank.pgm");
        let mut bytes = b"P5\n200 100\n255\n".to_vec();
        bytes.extend(std::iter::repeat_n(255_u8, 200 * 100));
        fs::write(&path, bytes).expect("blank image");
        path
    }

    #[test]
    fn the_fixture_gives_its_sentence() {
        let scratch = Scratch::new("fixture");
        let (recognition, pid) = recognized(&scratch, Path::new(FIXTURE), Params::ENGLISH, LIMIT);
        assert_eq!(
            recognition.result.as_deref(),
            Ok("The bank is beside the river.")
        );
        assert!(recognition.peak_rss_bytes.is_some_and(|bytes| bytes > 0));
        assert!(recognition.elapsed > Duration::ZERO);
        assert!(!exists(pid.expect("pid")));
        assert!(scratch.leftovers().is_empty());
    }

    #[test]
    fn a_missing_image_is_no_image_and_runs_nothing() {
        let scratch = Scratch::new("missing");
        let image = scratch.0.join("absent.png");
        let (recognition, pid) = recognized(&scratch, &image, Params::ENGLISH, LIMIT);
        assert_eq!(recognition.result, Err(OcrFailure::NoImage));
        assert_eq!(recognition.peak_rss_bytes, None);
        assert_eq!(pid, None);
        assert!(scratch.leftovers().is_empty());
    }

    #[test]
    fn a_blank_image_is_empty() {
        let scratch = Scratch::new("blank");
        let image = blank_image(&scratch);
        let (recognition, _) = recognized(&scratch, &image, Params::ENGLISH, LIMIT);
        assert_eq!(recognition.result, Err(OcrFailure::Empty));
        assert!(scratch.leftovers().is_empty());
    }

    #[test]
    fn an_unknown_language_is_language_missing() {
        let scratch = Scratch::new("language");
        let params = Params {
            language: "addict_none",
            page_segmentation: 6,
        };
        let (recognition, _) = recognized(&scratch, Path::new(FIXTURE), params, LIMIT);
        assert_eq!(recognition.result, Err(OcrFailure::LanguageMissing));
        assert!(scratch.leftovers().is_empty());
    }

    #[test]
    fn a_child_over_the_limit_is_killed_and_reaped() {
        let scratch = Scratch::new("limit");
        let (recognition, pid) = recognized(
            &scratch,
            Path::new(FIXTURE),
            Params::ENGLISH,
            Duration::from_millis(1),
        );
        assert_eq!(recognition.result, Err(OcrFailure::TimedOut));
        assert!(!exists(pid.expect("pid")));
        assert!(scratch.leftovers().is_empty());
    }

    #[test]
    fn a_dropped_recognition_kills_and_reaps_its_child() {
        let scratch = Scratch::new("dropped");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let mut pid = None;
        runtime.block_on(async {
            let mut started = |p| pid = Some(p);
            let recognition = run(
                PROGRAM,
                Path::new(FIXTURE),
                &Params::ENGLISH,
                LIMIT,
                &scratch.0,
                &mut started,
            );
            // Polled until the child runs, then dropped.
            let _ = tokio::time::timeout(Duration::from_millis(20), recognition).await;
        });
        assert!(!exists(pid.expect("pid")));
        assert!(scratch.leftovers().is_empty());
    }
    #[test]
    fn a_missing_program_is_tesseract_missing() {
        let scratch = Scratch::new("program");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let recognition = runtime.block_on(run(
            "addict-no-such-tesseract",
            Path::new(FIXTURE),
            &Params::ENGLISH,
            LIMIT,
            &scratch.0,
            &mut |_| {},
        ));
        assert_eq!(recognition.result, Err(OcrFailure::TesseractMissing));
        assert_eq!(recognition.peak_rss_bytes, None);
        assert!(scratch.leftovers().is_empty());
    }

    /// A fake `tesseract` in `scratch`, a shell script that fails the
    /// recognition with `stderr`, verbatim, and runs `listing` for
    /// `--list-langs`. It writes the pid of each of its runs to `pids`. It
    /// is given back once it can be run: another test's fork may still hold
    /// it open for writing for a moment (`ETXTBSY`).
    fn fake(scratch: &Scratch, stderr: &str, listing: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let pids = scratch.0.join("pids");
        let path = scratch.0.join("fake-tesseract");
        let script = format!(
            "#!/bin/sh\n[ \"$1\" = --probe ] && exit 0\necho $$ >> '{pids}'\nif [ \"$1\" = --list-langs ]; then\n{listing}\nfi\ncat >&2 <<'STDERR'\n{stderr}\nSTDERR\nexit 1\n",
            pids = pids.display()
        );
        fs::write(&path, script).expect("script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("mode");
        for _ in 0..100 {
            match std::process::Command::new(&path).arg("--probe").status() {
                Ok(status) => {
                    assert!(status.success(), "probe: {status}");
                    return path;
                }
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("probe: {e}"),
            }
        }
        panic!("the fake stayed busy");
    }

    /// Recognizes the fixture with the program at `program`.
    fn faked(scratch: &Scratch, program: &Path, limit: Duration) -> Recognition {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(run(
            program.to_str().expect("path"),
            Path::new(FIXTURE),
            &Params::ENGLISH,
            limit,
            &scratch.0,
            &mut |_| {},
        ))
    }

    /// The pids the fake wrote; each is gone.
    fn all_gone(scratch: &Scratch, runs: usize) {
        let pids = fs::read_to_string(scratch.0.join("pids")).expect("pids");
        let pids: Vec<u32> = pids.lines().map(|p| p.parse().expect("pid")).collect();
        assert_eq!(pids.len(), runs, "{pids:?}");
        for pid in pids {
            assert!(!exists(pid), "{pid} is left");
        }
    }

    #[test]
    fn a_language_that_is_not_listed_is_language_missing() {
        let scratch = Scratch::new("unlisted");
        let program = fake(
            &scratch,
            "Error opening data file",
            "echo 'List of available languages (1):'\necho osd\nexit 0",
        );
        let recognition = faked(&scratch, &program, LIMIT);
        assert_eq!(recognition.result, Err(OcrFailure::LanguageMissing));
        all_gone(&scratch, 2);
        assert!(scratch.leftovers().is_empty());
    }

    #[test]
    fn a_listed_language_leaves_the_failure_with_its_first_stderr_line() {
        let scratch = Scratch::new("listed");
        let program = fake(
            &scratch,
            "Something else broke",
            "echo 'List of available languages (2):'\necho eng\necho osd\nexit 0",
        );
        let recognition = faked(&scratch, &program, LIMIT);
        assert_eq!(
            recognition.result,
            Err(OcrFailure::Failed(
                "tesseract exit status 1: Something else broke".to_owned()
            ))
        );
        all_gone(&scratch, 2);
        assert!(scratch.leftovers().is_empty());
    }

    #[test]
    fn a_listing_that_fails_leaves_the_failure() {
        let scratch = Scratch::new("listfails");
        let program = fake(&scratch, "Broken", "exit 3");
        let recognition = faked(&scratch, &program, LIMIT);
        assert_eq!(
            recognition.result,
            Err(OcrFailure::Failed(
                "tesseract exit status 1: Broken".to_owned()
            ))
        );
        all_gone(&scratch, 2);
        assert!(scratch.leftovers().is_empty());
    }

    #[test]
    fn a_listing_that_hangs_is_killed_at_the_limit() {
        let scratch = Scratch::new("listhangs");
        let program = fake(&scratch, "Broken", "exec sleep 30");
        let limit = Duration::from_millis(500);
        let recognition = faked(&scratch, &program, limit);
        assert_eq!(
            recognition.result,
            Err(OcrFailure::Failed(
                "tesseract exit status 1: Broken".to_owned()
            ))
        );
        assert!(recognition.elapsed < limit + Duration::from_secs(2));
        all_gone(&scratch, 2);
        assert!(scratch.leftovers().is_empty());
    }

    #[test]
    fn the_stderr_marker_is_language_missing_without_a_listing() {
        let scratch = Scratch::new("marker");
        let program = fake(
            &scratch,
            "Failed loading language 'eng'",
            "echo eng\nexit 0",
        );
        let recognition = faked(&scratch, &program, LIMIT);
        assert_eq!(recognition.result, Err(OcrFailure::LanguageMissing));
        all_gone(&scratch, 1);
    }

    /// The wait status of a child that exited with `code`.
    fn exited(code: i32) -> libc::c_int {
        code << 8
    }

    #[test]
    fn another_exit_is_failed_with_the_status_and_first_stderr_line() {
        let scratch = Scratch::new("failed");
        fs::write(
            scratch.0.join("stderr"),
            "\n  Error during processing.\nmore\n",
        )
        .expect("stderr");
        assert_eq!(
            outcome(&scratch.0, exited(1)),
            Err(OcrFailure::Failed(
                "tesseract exit status 1: Error during processing.".to_owned()
            ))
        );
        fs::write(scratch.0.join("stderr"), "").expect("stderr");
        assert_eq!(
            outcome(&scratch.0, libc::SIGSEGV),
            Err(OcrFailure::Failed(format!(
                "tesseract signal {}",
                libc::SIGSEGV
            )))
        );
    }

    #[test]
    fn an_output_over_the_limit_is_failed() {
        let scratch = Scratch::new("oversize");
        let size = usize::try_from(OUTPUT_LIMIT).expect("limit") + 1;
        fs::write(scratch.0.join("result.txt"), "a".repeat(size)).expect("output");
        assert!(matches!(
            outcome(&scratch.0, exited(0)),
            Err(OcrFailure::Failed(text)) if text.contains("output over")
        ));
        fs::write(scratch.0.join("result.txt"), "a".repeat(size - 1)).expect("output");
        assert!(outcome(&scratch.0, exited(0)).is_ok());
    }
}
