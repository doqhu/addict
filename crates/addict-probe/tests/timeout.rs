//! Timeout expiry: the helper itself, and the built binary against a bus and
//! a compositor that accept a connection and never answer.

use std::{
    os::unix::net::UnixListener,
    path::PathBuf,
    process::{Command, Output},
    time::{Duration, Instant},
};

use addict_probe::record::{Bounded, bounded};
use serde_json::Value;

#[tokio::test(start_paused = true)]
async fn bounded_ends_at_the_limit() {
    let started = tokio::time::Instant::now();
    let never = std::future::pending::<()>();
    assert_eq!(
        bounded(Duration::from_secs(30), never).await,
        Bounded::TimedOut
    );
    assert_eq!(started.elapsed(), Duration::from_secs(30));
}

#[tokio::test(start_paused = true)]
async fn bounded_returns_a_result_that_arrives_in_time() {
    let quick = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        7
    };
    assert_eq!(
        bounded(Duration::from_secs(30), quick).await,
        Bounded::Done(7)
    );
}

struct Run {
    output: Output,
    elapsed: Duration,
    records: Vec<Value>,
}

/// Runs the binary with a session bus and a Wayland display that are silent
/// sockets unless `bus` says otherwise.
fn probe(name: &str, bus: Option<&str>, args: &[&str]) -> Run {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("run-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("results.jsonl");

    // Listening but never accepting: a client connects and waits forever.
    let bus_socket = dir.join("bus");
    let wayland_socket = dir.join("wayland");
    let _bus = UnixListener::bind(&bus_socket).unwrap();
    let _wayland = UnixListener::bind(&wayland_socket).unwrap();
    let silent_bus = format!("unix:path={}", bus_socket.display());

    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_addict-probe"))
        .args(args)
        .arg("--out")
        .arg(&out)
        .env("DBUS_SESSION_BUS_ADDRESS", bus.unwrap_or(&silent_bus))
        .env("WAYLAND_DISPLAY", &wayland_socket)
        .env("HOME", &dir)
        .env_remove("ADDICT_PROBE_OUT")
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    let records = std::fs::read_to_string(&out)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    Run {
        output,
        elapsed,
        records,
    }
}

fn assert_timed_out(run: &Run, timeout: Duration) {
    assert_eq!(run.output.status.code(), Some(3), "{:?}", run.output);
    assert_eq!(run.records.len(), 1);
    assert_eq!(run.records[0]["outcome"], "timeout");
    assert!(run.records[0]["error"].is_string());
    assert!(run.elapsed >= timeout, "ended early: {:?}", run.elapsed);
    assert!(
        run.elapsed < timeout + Duration::from_secs(1),
        "ended more than a second late: {:?}",
        run.elapsed
    );
}

#[test]
fn shortcut_times_out_on_a_silent_bus() {
    let run = probe("shortcut", None, &["shortcut", "--timeout", "1"]);
    assert_timed_out(&run, Duration::from_secs(1));
    assert_eq!(run.records[0]["probe"], "shortcut");
    assert_eq!(run.records[0]["details"]["phase"], "connect-session-bus");
    assert_eq!(run.records[0]["inputs"]["preferred_trigger"], "CTRL+ALT+d");
}

#[test]
fn screenshot_times_out_on_a_silent_bus() {
    let run = probe("screenshot", None, &["screenshot", "--timeout", "0.5"]);
    assert_timed_out(&run, Duration::from_millis(500));
    assert_eq!(run.records[0]["inputs"]["options_sent"][0], "interactive");
}

#[test]
fn caps_times_out_on_a_silent_compositor_and_bus() {
    let run = probe("caps", None, &["caps", "--timeout", "0.5"]);
    assert_timed_out(&run, Duration::from_millis(500));
}

#[test]
fn clipboard_times_out_on_a_silent_compositor() {
    let run = probe("clipboard", None, &["clipboard", "--timeout", "1"]);
    assert_timed_out(&run, Duration::from_secs(1));
    let reads = run.records[0]["details"]["reads"].as_array().unwrap();
    assert_eq!(reads.len(), 4);
    // Only the read that actually waited timed out; the deadline had passed
    // before the others could start.
    assert_eq!(reads[0]["status"], "timeout");
    for read in &reads[1..] {
        assert_eq!(read["status"], "not-attempted");
        assert!(read["reason"].as_str().unwrap().contains("not attempted"));
    }
    let error = run.records[0]["error"].as_str().unwrap();
    assert_eq!(
        error.matches("no answer before the deadline").count(),
        1,
        "{error}"
    );
}

#[test]
fn missing_bus_is_unavailable_and_still_recorded() {
    let run = probe(
        "no-bus",
        Some("unix:path=/nonexistent/addict-probe-test-bus"),
        &["shortcut", "--timeout", "5"],
    );
    assert_eq!(run.output.status.code(), Some(5), "{:?}", run.output);
    assert_eq!(run.records.len(), 1);
    assert_eq!(run.records[0]["outcome"], "unavailable");
    assert!(
        run.records[0]["error"]
            .as_str()
            .unwrap()
            .contains("session bus"),
        "{}",
        run.records[0]["error"]
    );
    assert!(run.elapsed < Duration::from_secs(4));
}

#[test]
fn desktop_entry_round_trip_stays_inside_home() {
    let run = probe("desktop", None, &["install-desktop"]);
    assert_eq!(run.output.status.code(), Some(0), "{:?}", run.output);
    assert_eq!(run.records[0]["outcome"], "installed");
    assert_eq!(run.records[0]["inputs"]["app_id"], "dev.addict.Probe");
    let entry = PathBuf::from(run.records[0]["details"]["path"].as_str().unwrap());
    assert!(entry.ends_with(".local/share/applications/dev.addict.Probe.desktop"));
    assert!(entry.starts_with(env!("CARGO_TARGET_TMPDIR")));
    let text = std::fs::read_to_string(&entry).unwrap();
    assert!(text.contains(env!("CARGO_BIN_EXE_addict-probe")), "{text}");

    // Removal through the built binary, in the same redirected home.
    let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("run-desktop");
    let out = home.join("results.jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_addict-probe"))
        .args(["uninstall-desktop", "--out"])
        .arg(&out)
        .env("HOME", &home)
        .env_remove("ADDICT_PROBE_OUT")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let records = std::fs::read_to_string(&out).unwrap();
    let last: Value = serde_json::from_str(records.lines().last().unwrap()).unwrap();
    assert_eq!(records.lines().count(), 2);
    assert_eq!(last["outcome"], "removed");
    assert_eq!(last["details"]["existed"], true);
    assert!(!entry.exists());
}

#[test]
fn desktop_subcommands_refuse_any_other_app_id() {
    for command in ["install-desktop", "uninstall-desktop"] {
        let run = probe(
            &format!("desktop-other-{command}"),
            None,
            &[command, "--app-id", "org.example.App"],
        );
        assert_eq!(run.output.status.code(), Some(7), "{:?}", run.output);
        assert_eq!(run.records[0]["outcome"], "refused");
        assert_eq!(run.records[0]["inputs"]["app_id"], "org.example.App");
        // Nothing was touched: no path was even resolved, no directory made.
        assert!(run.records[0]["details"].get("path").is_none());
        let home =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("run-desktop-other-{command}"));
        assert!(!home.join(".local").exists());
    }
}

#[test]
fn unwritable_record_path_has_its_own_exit_code() {
    let output = Command::new(env!("CARGO_BIN_EXE_addict-probe"))
        .args([
            "uninstall-desktop",
            "--out",
            "/proc/addict-probe/none.jsonl",
        ])
        .env("HOME", env!("CARGO_TARGET_TMPDIR"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(9), "{output:?}");
}

#[test]
fn bad_timeout_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_addict-probe"))
        .args(["caps", "--timeout", "0"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
