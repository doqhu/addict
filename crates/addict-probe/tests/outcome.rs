//! Outcome names, exit codes and the mapping from runtime answers to outcomes.

use addict_probe::{
    clipboard::{self, Method, Raw, Source, Status},
    portal::{self, Failure},
    record::Outcome,
    screenshot, shortcut,
};
use ashpd::{desktop::ResponseError, zbus};
use std::{path::PathBuf, time::Duration};

const ALL: &[Outcome] = &[
    Outcome::Reported,
    Outcome::Activated,
    Outcome::NotBound,
    Outcome::Timeout,
    Outcome::Captured,
    Outcome::Cancelled,
    Outcome::Text,
    Outcome::Empty,
    Outcome::Unavailable,
    Outcome::Installed,
    Outcome::Removed,
    Outcome::Refused,
    Outcome::Error,
];

#[test]
fn serialized_name_matches_as_str() {
    for outcome in ALL {
        let json = serde_json::to_value(outcome).unwrap();
        assert_eq!(json.as_str(), Some(outcome.as_str()));
    }
    assert_eq!(Outcome::NotBound.as_str(), "not-bound");
}

#[test]
fn exit_codes_follow_the_matrix() {
    for success in [
        Outcome::Reported,
        Outcome::Activated,
        Outcome::Captured,
        Outcome::Text,
        Outcome::Installed,
        Outcome::Removed,
    ] {
        assert_eq!(success.exit_code(), 0, "{success:?}");
    }
    // Cancelling a capture is not an error.
    assert_eq!(Outcome::Cancelled.exit_code(), 0);
    // Binding refused or empty exits distinct from success.
    assert_ne!(Outcome::NotBound.exit_code(), 0);
    // No portal exits non-zero.
    assert_ne!(Outcome::Unavailable.exit_code(), 0);

    // The failing outcomes are told apart, and none takes the parser's code 2.
    let failing = [
        Outcome::Error,
        Outcome::Timeout,
        Outcome::NotBound,
        Outcome::Unavailable,
        Outcome::Empty,
        Outcome::Refused,
    ];
    for (i, a) in failing.iter().enumerate() {
        assert_ne!(a.exit_code(), 0, "{a:?}");
        assert_ne!(a.exit_code(), 2, "{a:?}");
        for b in &failing[i + 1..] {
            assert_ne!(a.exit_code(), b.exit_code(), "{a:?} vs {b:?}");
        }
    }
}

#[test]
fn portal_response_codes_are_classified() {
    let cancelled = ashpd::Error::Response(ResponseError::Cancelled);
    let other = ashpd::Error::Response(ResponseError::Other);
    assert_eq!(portal::classify(&cancelled), Failure::Cancelled);
    assert_eq!(portal::classify(&other), Failure::Other);
    assert_eq!(portal::finish_for(&cancelled).outcome, Outcome::Cancelled);
    assert_eq!(portal::finish_for(&other).outcome, Outcome::Error);
    assert!(portal::finish_for(&other).error.is_some());
}

#[test]
fn absent_bus_or_interface_is_unavailable() {
    let no_version = ashpd::Error::RequiresVersion(3, 2);
    assert_eq!(
        portal::finish_for(&no_version).outcome,
        Outcome::Unavailable
    );

    let no_bus = ashpd::Error::Zbus(zbus::Error::Address("no bus".into()));
    assert_eq!(portal::finish_for(&no_bus).outcome, Outcome::Unavailable);

    let no_service = zbus::Error::FDO(Box::new(zbus::fdo::Error::ServiceUnknown("x".into())));
    assert_eq!(portal::classify_zbus(&no_service), Failure::Unavailable);

    assert_eq!(
        portal::classify_error_name("org.freedesktop.DBus.Error.UnknownMethod"),
        Failure::Unavailable
    );
    assert_eq!(
        portal::classify_error_name("org.freedesktop.portal.Error.NotAllowed"),
        Failure::Failed
    );
    let denied = ashpd::Error::Portal(ashpd::PortalError::NotAllowed("no".into()));
    assert_eq!(portal::finish_for(&denied).outcome, Outcome::Error);
}

#[test]
fn dismissed_or_empty_binding_is_not_bound() {
    let dismissed = ashpd::Error::Response(ResponseError::Cancelled);
    assert_eq!(
        shortcut::bind_failure(&dismissed).outcome,
        Outcome::NotBound
    );
    assert_eq!(shortcut::bind_outcome(0), Some(Outcome::NotBound));
    assert_eq!(shortcut::bind_outcome(1), None);
    // A missing portal during binding stays unavailable.
    let gone = ashpd::Error::RequiresVersion(2, 1);
    assert_eq!(shortcut::bind_failure(&gone).outcome, Outcome::Unavailable);
}

#[test]
fn screenshot_targets_are_named_and_never_dropped() {
    assert!(portal::target_names(0).is_empty());
    assert_eq!(portal::target_names(5), ["screen", "area"]);
    assert_eq!(portal::target_names(16), ["unknown-bits-0x10"]);
    // The probe sends no option the backend does not advertise.
    assert_eq!(screenshot::SENT_OPTIONS, ["interactive"]);
}

#[test]
fn screenshot_uri_maps_to_a_local_path() {
    assert_eq!(
        screenshot::file_path_from_uri("file:///home/u/Pictures/Screenshot%20From%201.png"),
        Some(PathBuf::from("/home/u/Pictures/Screenshot From 1.png"))
    );
    assert_eq!(
        screenshot::file_path_from_uri("file://localhost/tmp/a.png"),
        Some(PathBuf::from("/tmp/a.png"))
    );
    assert_eq!(
        screenshot::file_path_from_uri("file://otherhost/tmp/a.png"),
        None
    );
    assert_eq!(
        screenshot::file_path_from_uri("https://example.org/a.png"),
        None
    );
    assert_eq!(screenshot::file_path_from_uri("file:///tmp/bad%2"), None);
    assert_eq!(screenshot::file_path_from_uri("file:///tmp/bad%zz"), None);
}

#[test]
fn screenshot_inspect_reports_existence_and_pixel_size() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let missing = screenshot::inspect(&dir.join("no-such-image.png"));
    assert_eq!(missing["exists"], false);
    assert!(missing.get("width").is_none());

    // A 3x2 PNG: signature plus an IHDR chunk is all the size reader needs.
    let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
    png.extend_from_slice(&3u32.to_be_bytes());
    png.extend_from_slice(&2u32.to_be_bytes());
    png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
    let path = dir.join("inspect-3x2.png");
    std::fs::write(&path, &png).unwrap();
    let found = screenshot::inspect(&path);
    assert_eq!(found["exists"], true);
    assert_eq!(found["bytes"], png.len());
    assert_eq!(found["width"], 3);
    assert_eq!(found["height"], 2);
}

fn read(source: Source, method: Method, raw: Raw) -> clipboard::Read1 {
    clipboard::measure(source, method, raw, 5, Duration::from_millis(7))
}

#[test]
fn clipboard_text_is_measured_not_interpreted() {
    let r = read(
        Source::Primary,
        Method::WlPaste,
        Raw::Text("слово\tand more".into()),
    );
    assert_eq!(r.status, Status::Text);
    assert_eq!(r.chars, Some(14));
    assert_eq!(r.bytes, Some(19));
    assert_eq!(r.preview.as_deref(), Some("слово"));
    assert_eq!(r.elapsed_ms, 7);
    assert_eq!(clipboard::preview("a\nb\tc", 10), "a b c");
    assert_eq!(clipboard::preview("secret", 0), "");

    let zero = read(Source::Clipboard, Method::WlPaste, Raw::Text(String::new()));
    assert_eq!(zero.status, Status::Empty);
    assert!(zero.preview.is_none());
}

#[test]
fn clipboard_outcome_is_the_most_informative_read() {
    let unavailable = read(
        Source::Clipboard,
        Method::WaylandProtocol,
        Raw::Unavailable("protocol missing".into()),
    );
    let empty = read(
        Source::Clipboard,
        Method::WlPaste,
        Raw::Empty("Nothing is copied".into()),
    );
    let timeout = read(Source::Primary, Method::WlPaste, Raw::Timeout);
    let text = read(Source::Primary, Method::WlPaste, Raw::Text("x".into()));
    let error = read(Source::Primary, Method::WlPaste, Raw::Error("boom".into()));

    let all = [unavailable.clone(), empty.clone(), timeout.clone(), text];
    assert_eq!(clipboard::summarize(&all).outcome, Outcome::Text);
    assert_eq!(clipboard::summarize(&all).error, None);

    // Clipboard blocked: a definite answer names the outcome; the timeout
    // only ended the wait. Every non-text reason stays in the error text.
    let no_text = [unavailable.clone(), empty.clone(), timeout.clone()];
    let finish = clipboard::summarize(&no_text);
    assert_eq!(finish.outcome, Outcome::Empty);
    let reasons = finish.error.unwrap();
    assert!(
        reasons.starts_with("Clipboard/WlPaste: Nothing is copied"),
        "{reasons}"
    );
    assert!(reasons.contains("protocol missing"), "{reasons}");
    assert!(
        reasons.contains("no answer before the deadline"),
        "{reasons}"
    );

    let missing_then_late = [unavailable.clone(), timeout.clone(), error.clone()];
    let finish = clipboard::summarize(&missing_then_late);
    assert_eq!(finish.outcome, Outcome::Unavailable);
    let reasons = finish.error.unwrap();
    assert!(
        reasons.contains("no answer before the deadline"),
        "{reasons}"
    );
    assert!(reasons.contains("boom"), "{reasons}");

    // Timeout only when no read gave a definite answer.
    let not_attempted = read(Source::Primary, Method::WlPaste, Raw::NotAttempted);
    assert_eq!(not_attempted.status, Status::NotAttempted);
    let only_late = [timeout.clone(), error.clone(), not_attempted.clone()];
    let finish = clipboard::summarize(&only_late);
    assert_eq!(finish.outcome, Outcome::Timeout);
    let reasons = finish.error.unwrap();
    assert!(
        reasons.starts_with("Primary/WlPaste: no answer before the deadline"),
        "{reasons}"
    );
    assert!(reasons.contains("not attempted"), "{reasons}");
    // A read that never started is not itself a definite answer.
    let finish = clipboard::summarize(&[unavailable.clone(), not_attempted]);
    assert_eq!(finish.outcome, Outcome::Unavailable);

    let blocked = [unavailable.clone(), empty];
    let finish = clipboard::summarize(&blocked);
    assert_eq!(finish.outcome, Outcome::Empty);
    assert!(finish.error.unwrap().contains("Nothing is copied"));

    let missing = [unavailable.clone(), error.clone()];
    let finish = clipboard::summarize(&missing);
    assert_eq!(finish.outcome, Outcome::Unavailable);
    assert!(finish.error.unwrap().contains("protocol missing"));

    assert_eq!(clipboard::summarize(&[error]).outcome, Outcome::Error);
    assert_eq!(clipboard::summarize(&[]).outcome, Outcome::Unavailable);
}

#[test]
fn wl_paste_messages_are_named() {
    assert!(matches!(
        clipboard::wl_paste_failure("Nothing is copied", Some(1)),
        Raw::Empty(_)
    ));
    assert!(matches!(
        clipboard::wl_paste_failure("Failed to connect to a Wayland server", Some(1)),
        Raw::Unavailable(_)
    ));
    assert!(matches!(
        clipboard::wl_paste_failure("", Some(1)),
        Raw::Error(_)
    ));
}
