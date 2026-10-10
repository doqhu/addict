//! With no reachable display the resident application fails fast: a non-zero
//! exit within the timeout and no portal worker.

mod common;

use std::process::Command;

use common::{TempDir, run_with_timeout, text};

#[test]
fn exits_non_zero_without_a_display() {
    assert_fails_fast("start", &[]);
}

#[test]
fn work_demo_exits_non_zero_without_a_display() {
    assert_fails_fast("start-demo", &["--work-demo"]);
}

#[test]
fn help_does_not_mention_the_work_demo() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_addict"));
    command.arg("--help");
    let output = run_with_timeout(command).expect("addict did not exit within the timeout");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let help = text(&output.stdout);
    // Guards against an empty help that would pass the check below.
    assert!(help.contains("install"), "{help}");
    assert!(!help.contains("work-demo"), "{help}");
}

fn assert_fails_fast(tag: &str, args: &[&str]) {
    let runtime_dir = TempDir::new(tag);
    let mut command = Command::new(env!("CARGO_BIN_EXE_addict"));
    command
        .args(args)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env("XDG_RUNTIME_DIR", runtime_dir.path());
    let output = run_with_timeout(command).expect("addict did not exit within the timeout");
    let stderr = text(&output.stderr);
    assert!(
        !output.status.success(),
        "addict exited with {}; stderr: {stderr}",
        output.status
    );
    assert!(
        output.status.code().is_some(),
        "killed by a signal: {stderr}"
    );
    // Every line the shortcut worker can cause mentions the shortcut.
    assert!(!stderr.contains("shortcut"), "worker started: {stderr}");
}
