//! Expensive work lives on bounded workers. Quit cancels and reaps OCR.
use crate::protocol::{Emitter, FrontendEvent};
use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct Workers {
    cancel: Arc<AtomicBool>,
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}
impl Workers {
    pub fn delayed(&self, emitter: Emitter, id: String, duration: Duration) {
        let cancel = self.cancel.clone();
        let handle = std::thread::spawn(move || {
            let start = Instant::now();
            while start.elapsed() < duration.min(Duration::from_secs(15))
                && !cancel.load(Ordering::Relaxed)
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            emitter.send(FrontendEvent::Work {
                id,
                outcome: if cancel.load(Ordering::Relaxed) {
                    "delayed-work-cancelled"
                } else {
                    "delayed-work-completed"
                }
                .into(),
                child_pid: None,
            });
        });
        self.handles.lock().unwrap().push(handle);
    }
    pub fn ocr(&self, fixture: &Path, limit: Duration, emitter: Emitter, id: String) {
        let fixture = fixture.to_owned();
        let cancel = self.cancel.clone();
        let handle = std::thread::spawn(move || {
            let outcome = run_child(&fixture, limit, &cancel, |pid| {
                emitter.send(FrontendEvent::Work {
                    id: id.clone(),
                    outcome: "ocr-started".into(),
                    child_pid: Some(pid),
                })
            });
            emitter.send(FrontendEvent::Work {
                id,
                outcome: match outcome {
                    Ok(text) => format!("ocr-completed: {}", text.trim()),
                    Err(e) => format!("ocr-error: {e}"),
                },
                child_pid: None,
            });
        });
        self.handles.lock().unwrap().push(handle);
    }
    /// Called after the UI loop ends; cancellation is checked every 20 ms.
    pub fn shutdown(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        for handle in self.handles.lock().unwrap().drain(..) {
            let _ = handle.join();
        }
    }
}
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn run_tesseract(
    fixture: &Path,
    limit: Duration,
    started: impl FnOnce(u32),
) -> io::Result<String> {
    run_child(fixture, limit, &AtomicBool::new(false), started)
}
pub fn run_child(
    fixture: &Path,
    limit: Duration,
    cancel: &AtomicBool,
    started: impl FnOnce(u32),
) -> io::Result<String> {
    if limit.is_zero() || limit > Duration::from_secs(30) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "OCR deadline must be 1..=30 s",
        ));
    }
    let directory = Directory(std::env::temp_dir().join(format!(
        "addict-exp4-ocr-{}-{}",
        std::process::id(),
        crate::metrics::monotonic_ns()
    )));
    std::fs::create_dir(&directory.0)?;
    let output = directory.0.join("result");
    let mut command = Command::new("tesseract");
    command
        .arg(fixture)
        .arg(&output)
        .args(["-l", "eng", "--psm", "6"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Even SIGKILL of a frontend cannot orphan a running OCR child. The broker
    // additionally kills the frontend's entire process group on forced cleanup.
    use std::os::unix::process::CommandExt;
    let parent = std::process::id();
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent as i32 {
                return Err(io::Error::other("frontend exited during OCR spawn"));
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    started(child.id());
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(status)) => return Err(io::Error::other(format!("Tesseract exited {status}"))),
            Ok(None) if start.elapsed() < limit && !cancel.load(Ordering::Relaxed) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    if cancel.load(Ordering::Relaxed) {
                        io::ErrorKind::Interrupted
                    } else {
                        io::ErrorKind::TimedOut
                    },
                    "OCR cancelled/deadline exceeded; child killed and reaped",
                ));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        }
    }
    let path = output.with_extension("txt");
    if std::fs::metadata(&path)?.len() > 16_384 {
        return Err(io::Error::other("OCR output exceeds fixture limit"));
    }
    std::fs::read_to_string(path)
}
