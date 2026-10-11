//! With a session bus that accepts the connection and never answers, the
//! worker reports one failure when the limit of the attempt runs out, not
//! before, and stops; it never waits for a press.
//!
//! The bus address is given to a child process at spawn: the test re-runs this
//! test binary for its one test, so no process changes its own environment.
//! The child owns the silent socket.

use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use addict_platform::shortcut::{self, ShortcutEvent};

const TEST_NAME: &str = "silent_bus_reports_one_failure_at_the_limit_and_stops";
/// Set only on the child: the path of the socket it must listen on.
const CHILD_SOCKET: &str = "ADDICT_TEST_SILENT_BUS_SOCKET";

/// The limit of the attempt.
const BIND_LIMIT: Duration = Duration::from_millis(500);
/// Limit for each wait inside the child.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);
/// Limit for the whole child run.
const CHILD_TIMEOUT: Duration = Duration::from_secs(40);

#[test]
fn silent_bus_reports_one_failure_at_the_limit_and_stops() {
    match std::env::var_os(CHILD_SOCKET) {
        Some(socket) => child_body(Path::new(&socket)),
        None => run_child(),
    }
}

/// A directory of this test run; removed with everything in it on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn create() -> Self {
        let dir = std::env::temp_dir().join(format!("addict-silent-bus-{}", std::process::id()));
        std::fs::create_dir(&dir).expect("create the socket directory");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_child() {
    let scratch = Scratch::create();
    let socket = scratch.0.join("bus");
    let mut child = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
        .env(CHILD_SOCKET, &socket)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}", socket.display()),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the test binary");
    let deadline = Instant::now() + CHILD_TIMEOUT;
    loop {
        match child.try_wait().expect("poll the child") {
            Some(_) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the child did not exit within {CHILD_TIMEOUT:?}");
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    // The output of one test fits a pipe buffer.
    let output = child.wait_with_output().expect("collect the child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "child failed with {}\nstdout: {stdout}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    // Guards against a filter that matched nothing.
    assert!(stdout.contains("1 passed"), "child ran no test: {stdout}");
}

fn child_body(socket: &Path) {
    let listener = UnixListener::bind(socket).expect("listen on the silent socket");
    // Accepts every connection and keeps it open without a byte in reply. The
    // thread ends with the process.
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    });

    let (sender, receiver) = mpsc::channel();
    let started = Instant::now();
    let handle = shortcut::start("dev.addict.Test", "LOGO+q", BIND_LIMIT, move |event| {
        let _ = sender.send(event);
    });

    match receiver
        .recv_timeout(EVENT_TIMEOUT)
        .expect("no event in time")
    {
        ShortcutEvent::Failed { message } => {
            let waited = started.elapsed();
            assert!(waited >= BIND_LIMIT, "failed after {waited:?}: {message}");
            assert!(message.contains(&format!("{BIND_LIMIT:?}")), "{message}");
            assert!(!message.contains('\n'), "{message}");
        }
        other => panic!("expected a failure, got {other:?}"),
    }
    // The worker stops by itself and takes the callback with it: with the
    // handle still held, the channel is disconnected with nothing left in it.
    assert_eq!(
        receiver.recv_timeout(EVENT_TIMEOUT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
    drop(handle);
}
