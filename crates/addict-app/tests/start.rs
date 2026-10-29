//! With no reachable display the resident application fails fast: a non-zero
//! exit within the timeout and no portal worker.

mod common;

use std::process::Command;

use common::{TempDir, run_with_timeout, text};

#[test]
fn exits_non_zero_without_a_display() {
    assert_fails_fast("start", &[]);
    assert_fails_fast("start-background", &["--background"]);
}

#[test]
fn help_does_not_mention_the_evidence_flags() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_addict"));
    command.arg("--help");
    let output = run_with_timeout(command).expect("addict did not exit within the timeout");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let help = text(&output.stdout);
    // Guards against an empty help that would pass the checks below.
    for command in ["  install", "  uninstall"] {
        assert!(help.contains(command), "{help}");
    }
    assert!(help.contains("--background"), "{help}");
    assert!(!help.contains("evidence"), "{help}");
    assert!(!help.contains("origin"), "{help}");
}

#[test]
fn the_evidence_flags_are_required_together() {
    let dir = TempDir::new("start-evidence-pair");
    let file = dir.path().join("evidence.jsonl");
    for args in [
        vec!["--evidence", file.to_str().expect("utf-8 path")],
        vec!["--origin", "foliate"],
    ] {
        let output = run_without_display(&dir, &args);
        assert!(!output.status.success(), "{args:?}");
        assert!(!file.exists(), "{args:?}");
    }
}

#[test]
fn an_unwritable_evidence_path_fails_with_one_line() {
    let dir = TempDir::new("start-evidence-unwritable");
    // The directory of the file does not exist and is not created.
    let file = dir.path().join("missing").join("evidence.jsonl");
    let path = file.to_str().expect("utf-8 path");
    let output = run_without_display(&dir, &["--evidence", path, "--origin", "foliate"]);
    let stderr = text(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(output.status.code().is_some(), "killed: {stderr}");
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.starts_with("addict: cannot open the evidence file "));
    assert!(stderr.contains(path), "{stderr}");
    assert!(!dir.path().join("missing").exists());
}

#[test]
fn without_the_flag_no_file_is_created() {
    let dir = TempDir::new("start-no-evidence");
    let work = dir.path().join("work");
    std::fs::create_dir(&work).expect("create the working directory");
    let mut command = Command::new(env!("CARGO_BIN_EXE_addict"));
    command
        .current_dir(&work)
        .env("HOME", &work)
        .env("TMPDIR", &work)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env("XDG_RUNTIME_DIR", dir.path().join("runtime"));
    let output = run_with_timeout(command).expect("addict did not exit within the timeout");
    assert!(!output.status.success());
    let left: Vec<_> = std::fs::read_dir(&work)
        .expect("list the working directory")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// With the flag the file is opened before the display is: it exists, and
/// what it held is still there.
#[test]
fn the_evidence_file_is_opened_at_start_and_not_truncated() {
    let dir = TempDir::new("start-evidence-kept");
    let file = dir.path().join("evidence.jsonl");
    std::fs::write(&file, "earlier\n").expect("write the earlier content");
    let path = file.to_str().expect("utf-8 path");
    let output = run_without_display(&dir, &["--evidence", path, "--origin", "local"]);
    // No display: the launch still fails, after the file was opened.
    assert!(!output.status.success());
    let kept = std::fs::read_to_string(&file).expect("read the evidence file");
    // No session started, so no record either.
    assert_eq!(kept, "earlier\n");
}

fn run_without_display(runtime_dir: &TempDir, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_addict"));
    command
        .args(args)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env("XDG_RUNTIME_DIR", runtime_dir.path());
    run_with_timeout(command).expect("addict did not exit within the timeout")
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
