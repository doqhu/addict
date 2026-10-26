//! With no reachable session bus the worker reports one failure and stops; it
//! never waits for a press.
//!
//! The unreachable bus address is given to a child process at spawn: the test
//! re-runs this test binary for its one test, so no process changes its own
//! environment.

mod common;

use std::ffi::OsStr;
use std::sync::mpsc;
use std::time::Duration;

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
    common::run_child(
        TEST_NAME,
        CHILD_TIMEOUT,
        &[
            (CHILD_MARKER, OsStr::new("1")),
            ("DBUS_SESSION_BUS_ADDRESS", OsStr::new(BUS_ADDRESS)),
        ],
    );
}

fn child_body() {
    let (sender, receiver) = mpsc::channel();
    let handle = shortcut::start("io.github.doqhu.Test", "LOGO+q", BIND_LIMIT, move |event| {
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
