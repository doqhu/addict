//! Outcome names, exit codes and the record line.

use std::{
    collections::HashSet,
    time::{Duration, UNIX_EPOCH},
};

use addict_context::record::{
    Accessibility, EXIT_RECORD_NOT_WRITTEN, Finish, Outcome, Record, SCHEMA, append,
};
use addict_probe::record::Environment;
use serde_json::{Value, json};

#[test]
fn exit_codes_are_fixed() {
    let expected = [
        (Outcome::Reported, "reported", 0),
        (Outcome::Context, "context", 0),
        (Outcome::Error, "error", 1),
        (Outcome::Timeout, "timeout", 3),
        (Outcome::AppNotFound, "app-not-found", 4),
        (Outcome::Unavailable, "unavailable", 5),
        (Outcome::NoText, "no-text", 6),
        (Outcome::NoSelection, "no-selection", 7),
        (Outcome::WordOnly, "word-only", 8),
        (Outcome::Ambiguous, "ambiguous", 10),
    ];
    assert_eq!(expected.len(), Outcome::ALL.len());
    for (outcome, name, code) in expected {
        assert_eq!(outcome.as_str(), name);
        assert_eq!(outcome.exit_code(), code, "{name}");
        assert_eq!(serde_json::to_value(outcome).unwrap(), json!(name));
    }
}

#[test]
fn only_success_exits_zero_and_other_codes_are_distinct() {
    let mut seen = HashSet::new();
    for outcome in Outcome::ALL {
        let code = outcome.exit_code();
        assert_ne!(code, 2, "2 belongs to the argument parser");
        assert_ne!(code, EXIT_RECORD_NOT_WRITTEN);
        if matches!(outcome, Outcome::Reported | Outcome::Context) {
            assert_eq!(code, 0);
        } else {
            assert_ne!(code, 0);
            assert!(
                seen.insert(code),
                "{} shares its exit code",
                outcome.as_str()
            );
        }
    }
}

fn sample(finish: Finish) -> Record {
    Record::new(
        "context",
        Environment::default(),
        Accessibility {
            bus_address: Some("unix:path=/x".to_owned()),
            is_enabled: Some(false),
            ..Accessibility::default()
        },
        json!([{ "name": "App", "toolkit": "GTK", "toolkit_version": "4" }]),
        json!({ "app": "App" }),
        "expected text",
        finish,
        json!({ "phase": "done" }),
        Duration::from_millis(12),
        UNIX_EPOCH + Duration::from_secs(1_791_158_400),
    )
}

#[test]
fn record_is_self_describing() {
    let record = sample(Finish::with_error(Outcome::WordOnly, "GetText: failed"));
    let line = record.to_line();
    assert!(!line.contains('\n'));
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["schema"], json!(SCHEMA));
    assert_eq!(value["tool"], json!("addict-context"));
    assert_eq!(value["tool_version"], json!(env!("CARGO_PKG_VERSION")));
    assert_eq!(value["probe"], json!("context"));
    assert_eq!(value["outcome"], json!("word-only"));
    assert_eq!(value["error"], json!("GetText: failed"));
    assert_eq!(value["accessibility"]["is_enabled"], json!(false));
    assert_eq!(value["accessibility"]["screen_reader_enabled"], Value::Null);
    assert_eq!(value["target"][0]["toolkit"], json!("GTK"));
    assert_eq!(value["elapsed_ms"], json!(12));
    assert!(value["timestamp"].as_str().unwrap().ends_with('Z'));
    for key in ["environment", "inputs", "expected", "details"] {
        assert!(value.get(key).is_some(), "{key}");
    }
}

#[test]
fn append_adds_lines_and_creates_the_directory() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("context-append");
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("nested").join("results.jsonl");
    append(&path, &sample(Finish::ok(Outcome::Context))).unwrap();
    append(&path, &sample(Finish::ok(Outcome::NoText))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let outcomes: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["outcome"].clone())
        .collect();
    assert_eq!(outcomes, vec![json!("context"), json!("no-text")]);
}
