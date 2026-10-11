//! With no reachable session bus the worker reports one failure and stops; it
//! never waits for a press.
//!
//! The unreachable bus address is given to a child process at spawn: the test
//! re-runs this test binary for its one test, so no process changes its own
//! environment.

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use addict_platform::shortcut::{self, ShortcutEvent};

const TEST_NAME: &str = "unreachable_bus_reports_one_failure_and_stops";
/// Set only on the child; tells the test to run its body.
const CHILD_MARKER: &str = "ADDICT_TEST_UNREACHABLE_BUS_CHILD";
const BUS_ADDRESS: &str = "unix:path=/nonexistent/addict-test-bus";

/// Limit for each wait inside the child.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);
/// The limit of the attempt; this failure arrives long before it.
const BIND_LIMIT: Duration = Duration::from_secs(30);
/// Limit for the whole child run.
const CHILD_TIMEOUT: Duration = Duration::from_secs(40);

#[test]
fn unreachable_bus_reports_one_failure_and_stops() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        child_body();
    } else {
        run_child();
    }
}

fn run_child() {
    let mut child = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
        .env(CHILD_MARKER, "1")
        .env("DBUS_SESSION_BUS_ADDRESS", BUS_ADDRESS)
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

fn child_body() {
    let (sender, receiver) = mpsc::channel();
    let handle = shortcut::start("dev.addict.Test", "LOGO+q", BIND_LIMIT, move |event| {
        let _ = sender.send(event);
    });

    match receiver
        .recv_timeout(EVENT_TIMEOUT)
        .expect("no event in time")
    {
        ShortcutEvent::Failed { message } => {
            assert!(message.starts_with("session bus:"), "{message}");
            assert!(!message.contains('\n'), "{message}");
        }
        other => panic!("expected a failure, got {other:?}"),
    }
    drop(handle);
    // The worker is gone and took the callback with it: the channel is
    // disconnected with nothing left in it.
    assert_eq!(
        receiver.recv_timeout(EVENT_TIMEOUT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}
