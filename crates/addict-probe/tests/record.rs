//! Record serialization, JSON-lines append, capability values and the
//! desktop entry rules.

use std::{
    path::PathBuf,
    time::{Duration, UNIX_EPOCH},
};

use addict_probe::{
    caps::{self, Global},
    desktop,
    record::{self, Environment, Finish, Outcome, Progress, Record},
};
use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample(outcome: Finish) -> Record {
    Record::new(
        "caps",
        Environment {
            os: Some("Fedora Linux 44".into()),
            ..Environment::default()
        },
        json!({ "timeout_s": 10.0 }),
        "expected text",
        outcome,
        json!({ "phase": "done" }),
        Duration::from_millis(58),
        UNIX_EPOCH + Duration::from_millis(1_791_144_707_470),
    )
}

#[test]
fn timestamps_are_utc_rfc3339() {
    assert_eq!(record::rfc3339_utc(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    assert_eq!(
        record::rfc3339_utc(UNIX_EPOCH + Duration::from_millis(951_827_696_789)),
        "2000-02-29T12:34:56.789Z"
    );
    assert_eq!(
        record::rfc3339_utc(UNIX_EPOCH + Duration::from_secs(1_791_144_707)),
        "2026-10-04T20:11:47.000Z"
    );
}

#[test]
fn record_is_self_describing() {
    let line = sample(Finish::with_error(Outcome::Unavailable, "bus gone")).to_line();
    assert!(!line.contains('\n'));
    let value: Value = serde_json::from_str(&line).unwrap();
    for key in [
        "schema",
        "timestamp",
        "tool",
        "tool_version",
        "probe",
        "environment",
        "inputs",
        "expected",
        "outcome",
        "error",
        "details",
        "elapsed_ms",
    ] {
        assert!(value.get(key).is_some(), "missing {key}");
    }
    assert_eq!(value["schema"], record::SCHEMA);
    assert_eq!(value["tool"], "addict-probe");
    assert_eq!(value["tool_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(value["outcome"], "unavailable");
    assert_eq!(value["error"], "bus gone");
    assert_eq!(value["elapsed_ms"], 58);
    assert_eq!(value["timestamp"], "2026-10-04T20:11:47.470Z");
    assert_eq!(value["environment"]["os"], "Fedora Linux 44");
    // What the machine did not report is null, not invented.
    assert_eq!(value["environment"]["gnome_shell"], Value::Null);

    let ok: Value = serde_json::from_str(&sample(Finish::ok(Outcome::Reported)).to_line()).unwrap();
    assert_eq!(ok["error"], Value::Null);
}

#[test]
fn append_adds_one_line_per_run_and_keeps_earlier_ones() {
    let path = scratch("append").join("nested/results.jsonl");
    record::append(&path, &sample(Finish::ok(Outcome::Reported))).unwrap();
    record::append(&path, &sample(Finish::with_error(Outcome::Timeout, "late"))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.ends_with('\n'));
    let outcomes: Vec<String> = text
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["outcome"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(outcomes, ["reported", "timeout"]);
}

#[test]
fn append_reports_an_unwritable_path() {
    let dir = scratch("unwritable");
    std::fs::write(dir.join("file"), "x").unwrap();
    let under_a_file = dir.join("file/results.jsonl");
    assert!(record::append(&under_a_file, &sample(Finish::ok(Outcome::Reported))).is_err());
}

#[test]
fn progress_keeps_the_last_phase_and_all_details() {
    let progress = Progress::new();
    progress.phase("bind-dialog");
    progress.put("bind_ms", 12);
    progress.phase("wait-for-press");
    assert_eq!(
        progress.snapshot(),
        json!({ "phase": "wait-for-press", "bind_ms": 12 })
    );
}

#[test]
fn os_release_pretty_name_is_unquoted() {
    let text = "NAME=\"Fedora Linux\"\nPRETTY_NAME=\"Fedora Linux 44 (Workstation Edition)\"\n";
    assert_eq!(
        record::parse_os_release(text).as_deref(),
        Some("Fedora Linux 44 (Workstation Edition)")
    );
    assert_eq!(record::parse_os_release("NAME=x\n"), None);
}

#[test]
fn missing_wayland_globals_are_recorded_as_missing() {
    let globals = Ok(vec![
        Global {
            interface: "wl_data_device_manager".into(),
            version: 3,
        },
        Global {
            interface: "wl_compositor".into(),
            version: 6,
        },
    ]);
    let value = caps::watched(&globals);
    assert_eq!(value["reachable"], true);
    assert_eq!(value["global_count"], 2);
    assert_eq!(
        value["globals"]["wl_data_device_manager"],
        json!({ "present": true, "version": 3 })
    );
    assert_eq!(
        value["globals"]["ext_data_control_manager_v1"],
        json!({ "present": false })
    );
    for name in caps::WATCHED_GLOBALS {
        assert!(value["globals"].get(name).is_some(), "{name}");
    }

    let unreachable = caps::watched(&Err("connect: no compositor".into()));
    assert_eq!(unreachable["reachable"], false);
    assert_eq!(unreachable["error"], "connect: no compositor");
}

#[test]
fn desktop_entry_is_installed_replaced_and_removed() {
    let dir = scratch("desktop-own").join("applications");
    let exec = PathBuf::from("/opt/my probe/addict-probe");
    let progress = Progress::new();

    let finish = desktop::install(&dir, "dev.addict.Probe", &exec, &progress);
    assert_eq!(finish, Finish::ok(Outcome::Installed));
    let path = dir.join("dev.addict.Probe.desktop");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("[Desktop Entry]\n"));
    assert!(text.contains("Exec=\"/opt/my probe/addict-probe\" caps\n"));
    assert!(text.lines().any(|l| l == desktop::MARKER));
    assert_eq!(progress.snapshot()["existed"], false);

    // Reserved characters: a backslash for the Exec quoting, doubled because
    // Exec is a key-file string.
    assert!(
        desktop::entry_text(&PathBuf::from("/opt/pro$be/a\\b"))
            .contains("Exec=\"/opt/pro\\\\$be/a\\\\\\\\b\" caps\n")
    );

    // Its own file may be rewritten, e.g. after the binary moved.
    let moved = PathBuf::from("/elsewhere/addict-probe");
    assert_eq!(
        desktop::install(&dir, "dev.addict.Probe", &moved, &progress).outcome,
        Outcome::Installed
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("/elsewhere/")
    );

    assert_eq!(
        desktop::uninstall(&dir, "dev.addict.Probe", &progress),
        Finish::ok(Outcome::Removed)
    );
    assert!(!path.exists());
    // Removing what is already gone is still "removed".
    assert_eq!(
        desktop::uninstall(&dir, "dev.addict.Probe", &progress).outcome,
        Outcome::Removed
    );
    assert_eq!(progress.snapshot()["existed"], false);
}

#[test]
fn foreign_desktop_entry_is_never_touched() {
    let dir = scratch("desktop-foreign");
    let path = dir.join("org.example.App.desktop");
    let foreign = "[Desktop Entry]\nType=Application\nName=Someone else\nExec=other\n";
    std::fs::write(&path, foreign).unwrap();
    let progress = Progress::new();

    let install = desktop::install(&dir, "org.example.App", &PathBuf::from("/x"), &progress);
    assert_eq!(install.outcome, Outcome::Refused);
    let uninstall = desktop::uninstall(&dir, "org.example.App", &progress);
    assert_eq!(uninstall.outcome, Outcome::Refused);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), foreign);

    // A directory or symlink of that name is not the probe's file either.
    let link = dir.join("org.example.Link.desktop");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert_eq!(
        desktop::uninstall(&dir, "org.example.Link", &progress).outcome,
        Outcome::Refused
    );
    assert!(link.symlink_metadata().is_ok());
}

#[test]
fn app_id_cannot_escape_the_applications_directory() {
    assert!(desktop::is_valid_app_id("dev.addict.Probe"));
    for bad in [
        "",
        "probe",
        "../evil.x",
        "dev/addict.Probe",
        "dev..Probe",
        "dev.1addict",
        "a.b c",
    ] {
        assert!(!desktop::is_valid_app_id(bad), "{bad:?}");
        let progress = Progress::new();
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("never-created");
        assert_eq!(
            desktop::install(&dir, bad, &PathBuf::from("/x"), &progress).outcome,
            Outcome::Error
        );
        assert_eq!(
            desktop::uninstall(&dir, bad, &progress).outcome,
            Outcome::Error
        );
        assert!(!dir.exists());
    }
}
