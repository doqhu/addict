//! Measurement evidence of `--evidence`: one JSON line per fact, in the
//! `Evidence` format that `addict-exp4 summarize` reads.
//!
//! The application writes raw values only: the portal timestamp, the receipt
//! time, the `wp_presentation` time. The acceptance latency is computed by the
//! harness from these and a clock certificate.
//!
//! The event names and the fields of `details` are the harness's own. The
//! session start is named `broker-started` because that is the record the
//! harness splits sessions on.

use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
    time::SystemTime,
};

use addict_platform::work::Outcome;
use serde_json::{Value, json};

use crate::dictionaries::{Answer, Looked};

/// The version of the record format, as the harness checks it.
const SCHEMA: u32 = 1;
/// The product is the GTK4 resident cell of the harness's matrix.
const BACKEND: &str = "gtk4";
const MODE: &str = "resident";

/// The application the presses of a session come from; it is told by the
/// flag, not detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Origin {
    Foliate,
    IntelliJ,
    AndroidStudio,
    Local,
}

impl Origin {
    fn label(self) -> &'static str {
        match self {
            Self::Foliate => "foliate",
            Self::IntelliJ => "intelli-j",
            Self::AndroidStudio => "android-studio",
            Self::Local => "local",
        }
    }
}

/// One fact, before the labels of the session are put on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    invocation_id: Option<String>,
    event: &'static str,
    details: Value,
}

/// The start of a recording process. `boot_id` is what lets the harness tell
/// whether a clock certificate speaks for this session.
pub fn session_start(pid: u32, boot_id: Option<&str>) -> Record {
    Record {
        invocation_id: None,
        event: "broker-started",
        details: json!({
            "pid": pid,
            "boot_id": boot_id,
            "application": "addict",
            "version": env!("CARGO_PKG_VERSION"),
        }),
    }
}

/// A press of the shortcut. `cold` marks the press that shows the window for
/// the first time in this process.
pub fn activation(
    id: &str,
    cold: bool,
    portal_timestamp_ms: u64,
    receipt_monotonic_ns: u64,
    activation_token_present: bool,
) -> Record {
    Record {
        invocation_id: Some(id.to_owned()),
        event: "invocation",
        details: json!({
            "cold": cold,
            "portal_timestamp_ms": portal_timestamp_ms,
            "receipt_monotonic_ns": receipt_monotonic_ns,
            "activation_token_present": activation_token_present,
        }),
    }
}

/// Whether the token of the press was given to the window before it was shown.
pub fn token_consumption(id: &str, token_consumed: bool) -> Record {
    Record {
        invocation_id: Some(id.to_owned()),
        event: "frontend-event",
        details: json!({"event": "activated", "id": id, "token_consumed": token_consumed}),
    }
}

/// The frame that followed the press. A frame with no `wp_presentation` time
/// has no time here and `limitation` says why.
pub fn presentation(
    id: &str,
    time_ns: Option<u64>,
    clock_id: Option<i32>,
    limitation: Option<&str>,
) -> Record {
    Record {
        invocation_id: Some(id.to_owned()),
        event: "presentation",
        details: json!({
            "event": "presented",
            "id": id,
            "time_ns": time_ns,
            "clock_id": clock_id,
            // Only a frame known to have reached the screen showed the UI.
            "usable": time_ns.is_some(),
            "limitation": limitation,
        }),
    }
}

/// The window gained or lost the focus while the press was current.
pub fn focus(id: &str, focused: bool) -> Record {
    Record {
        invocation_id: Some(id.to_owned()),
        event: "frontend-event",
        details: json!({"event": "focus", "id": id, "focused": focused}),
    }
}

/// How a lookup ended, as the evidence names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answered {
    Article,
    NoEntry,
    NotImported,
    Unreadable,
    /// The lookup ran out of its time or panicked.
    Unfinished,
}

impl Answered {
    fn label(self) -> &'static str {
        match self {
            Self::Article => "article",
            Self::NoEntry => "no-entry",
            Self::NotImported => "not-imported",
            Self::Unreadable => "unreadable",
            Self::Unfinished => "unfinished",
        }
    }
}

/// What the evidence says of a lookup that ended with `outcome`. A cancelled
/// lookup was given up for another input and is not one.
pub fn answered(outcome: &Outcome<Looked>) -> Option<Answered> {
    Some(match outcome {
        Outcome::Finished(looked) => match looked.answer {
            Answer::Article(_) => Answered::Article,
            Answer::NoEntry => Answered::NoEntry,
            Answer::NotImported => Answered::NotImported,
            Answer::Unreadable(_) => Answered::Unreadable,
        },
        Outcome::TimedOut | Outcome::Panicked => Answered::Unfinished,
        Outcome::Cancelled => return None,
    })
}

/// A lookup ended. The record says how and nothing of the word: the harness
/// needs to know that the indexes were asked, not what the user reads.
pub fn lookup(answered: Answered) -> Record {
    Record {
        invocation_id: None,
        event: "lookup",
        details: json!({"answer": answered.label()}),
    }
}

/// The id of the activation number `count` of this process.
pub fn invocation_id(pid: u32, receipt_monotonic_ns: u64, count: u64) -> String {
    format!("{pid}-{receipt_monotonic_ns}-{count}")
}

/// The line of `record`, without the newline. `timestamp_ms` is wall time and
/// only tells a reader when the line was written.
fn line(origin: Origin, timestamp_ms: u128, record: &Record) -> String {
    json!({
        "schema": SCHEMA,
        "timestamp": timestamp_ms.to_string(),
        "backend": BACKEND,
        "mode": MODE,
        "origin": origin.label(),
        "invocation_id": record.invocation_id,
        "event": record.event,
        "details": record.details,
    })
    .to_string()
}

/// `CLOCK_MONOTONIC` in nanoseconds: the clock of the receipt time.
pub fn monotonic_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is a valid timespec for the call to fill.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
    assert_eq!(status, 0, "CLOCK_MONOTONIC is always available on Linux");
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

/// The boot id of the running kernel, when it can be read.
pub fn boot_id() -> Option<String> {
    let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let id = text.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// Appends records to the evidence file. The first failed write is reported
/// on stderr and ends the recording; the application goes on without it.
#[derive(Debug)]
pub struct Writer {
    /// `None` after a failed write.
    file: Option<File>,
    origin: Origin,
}

impl Writer {
    /// Opens `path` for appending and creates it when it is missing; what the
    /// file already holds stays.
    pub fn open(path: &Path, origin: Origin) -> io::Result<Self> {
        let file = OpenOptions::new().append(true).create(true).open(path)?;
        Ok(Self {
            file: Some(file),
            origin,
        })
    }

    pub fn write(&mut self, record: &Record) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let timestamp_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let mut text = line(self.origin, timestamp_ms, record);
        text.push('\n');
        // One write per line: an appended line is never interleaved.
        if let Err(e) = file.write_all(text.as_bytes()) {
            eprintln!("addict: evidence recording stopped: {e}");
            self.file = None;
        }
    }

    #[cfg(test)]
    fn is_recording(&self) -> bool {
        self.file.is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use addict_exp4::{
        Evidence,
        metrics::{self, ClockMapping, Summary},
        protocol::{Backend, Mode, Origin as HarnessOrigin},
    };

    use addict_core::dictionary::Article;

    use super::*;
    use crate::dictionaries::{IndexFile, Indexes};

    const BOOT: &str = "test-boot";

    /// A file path that is private to one test and removed with it.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(tag: &str) -> Self {
            Self(std::env::temp_dir().join(format!(
                "addict-evidence-{tag}-{}-{}.jsonl",
                std::process::id(),
                monotonic_ns()
            )))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A certificate of `BOOT` with no offset and no uncertainty.
    fn mapping() -> ClockMapping {
        ClockMapping {
            portal_backend: "gnome".into(),
            backend_version: "50".into(),
            evidence_path: "proof".into(),
            evidence_sha256: "a".repeat(64),
            verified: true,
            portal_clock_id: 1,
            presentation_clock_id: 1,
            portal_to_presentation_offset_ns: 0,
            uncertainty_ns: 0,
            boot_id: BOOT.into(),
            session_type: "wayland".into(),
            shell_version: "50".into(),
            portal_version: "1".into(),
            portal_timestamp_bits: 64,
        }
    }

    /// The records of press number `count`, one second apart, shown 100 ms
    /// after the portal timestamp unless the frame has no time.
    fn press(writer: &mut Writer, count: u64, token: bool, presented: bool) -> String {
        let portal_ms = 1000 * count;
        let receipt_ns = portal_ms * 1_000_000 + 2_000_000;
        let id = invocation_id(1, receipt_ns, count);
        writer.write(&activation(&id, count == 1, portal_ms, receipt_ns, token));
        writer.write(&token_consumption(&id, token));
        if presented {
            let time_ns = portal_ms * 1_000_000 + 100_000_000;
            writer.write(&presentation(&id, Some(time_ns), Some(1), None));
        } else {
            let limitation = "wp_presentation frame discarded";
            writer.write(&presentation(&id, None, None, Some(limitation)));
        }
        writer.write(&focus(&id, true));
        id
    }

    fn start(path: &Path, origin: Origin) -> Writer {
        let mut writer = Writer::open(path, origin).expect("open the evidence file");
        writer.write(&session_start(1, Some(BOOT)));
        writer
    }

    fn read(path: &Path) -> Vec<Evidence> {
        metrics::read_evidence(path).expect("every line parses as harness evidence")
    }

    fn foliate(rows: &[Evidence], mapping: Option<&ClockMapping>) -> Summary {
        metrics::summarize(rows, mapping)
            .into_iter()
            .find(|s| {
                s.backend == Backend::Gtk4
                    && s.mode == Mode::Resident
                    && s.origin == HarnessOrigin::Foliate
            })
            .expect("the GTK4 resident Foliate row")
    }

    #[test]
    fn every_line_of_a_session_parses_as_harness_evidence() {
        let file = TempFile::new("parse");
        let mut writer = start(&file.0, Origin::Foliate);
        let cold = press(&mut writer, 1, true, true);
        press(&mut writer, 2, false, false);
        let rows = read(&file.0);
        let events: Vec<_> = rows.iter().map(|r| r.event.as_str()).collect();
        assert_eq!(
            events,
            [
                "broker-started",
                "invocation",
                "frontend-event",
                "presentation",
                "frontend-event",
                "invocation",
                "frontend-event",
                "presentation",
                "frontend-event",
            ]
        );
        for row in &rows {
            assert_eq!(row.schema, addict_exp4::SCHEMA);
            assert_eq!(
                (row.backend, row.mode, row.origin),
                (Backend::Gtk4, Mode::Resident, HarnessOrigin::Foliate)
            );
        }
        assert_eq!(rows[0].details["pid"], 1);
        assert_eq!(rows[0].details["boot_id"], BOOT);
        assert_eq!(rows[1].invocation_id.as_deref(), Some(cold.as_str()));
        assert_eq!(
            rows[1].details,
            json!({
                "cold": true,
                "portal_timestamp_ms": 1000,
                "receipt_monotonic_ns": 1_002_000_000u64,
                "activation_token_present": true,
            })
        );
        assert_eq!(rows[3].details["time_ns"], 1_100_000_000u64);
        assert_eq!(rows[3].details["clock_id"], 1);
        // The application never writes a latency: the harness computes it.
        assert!(rows[3].details.get("acceptance_latency_ns").is_none());
        assert_eq!(rows[5].details["cold"], false);
        assert_eq!(rows[5].details["activation_token_present"], false);
        assert_eq!(rows[7].details["time_ns"], Value::Null);
        assert_eq!(rows[7].details["usable"], false);
        assert_eq!(
            rows[7].details["limitation"],
            "wp_presentation frame discarded"
        );
    }

    #[test]
    fn a_lookup_is_recorded_by_its_answer_and_without_its_word() {
        let file = TempFile::new("lookup");
        let mut writer = start(&file.0, Origin::Foliate);
        let article = Article {
            senses: Vec::new(),
            translations: vec!["банк".to_owned()],
        };
        let oewn = IndexFile {
            index: "oewn.index",
            published: "english-wordnet-2025.xml.gz",
        };
        let looked = |answer| {
            Outcome::Finished(Looked {
                answer,
                indexes: Indexes::OPEN,
            })
        };
        let outcomes = [
            (looked(Answer::Article(article)), "article"),
            (looked(Answer::NoEntry), "no-entry"),
            (looked(Answer::NotImported), "not-imported"),
            (looked(Answer::Unreadable(oewn)), "unreadable"),
            (Outcome::TimedOut, "unfinished"),
            (Outcome::Panicked, "unfinished"),
        ];
        for (outcome, _) in &outcomes {
            writer.write(&lookup(answered(outcome).expect("a lookup that ended")));
        }
        // A lookup given up for another input is not recorded.
        assert_eq!(answered(&Outcome::Cancelled), None);

        let rows = read(&file.0);
        assert_eq!(rows.len(), 1 + outcomes.len());
        for (row, (_, label)) in rows[1..].iter().zip(&outcomes) {
            assert_eq!(row.event, "lookup");
            assert_eq!(row.invocation_id, None);
            assert_eq!(
                (row.backend, row.mode, row.origin),
                (Backend::Gtk4, Mode::Resident, HarnessOrigin::Foliate)
            );
            // The answer and nothing else: no word, no article.
            assert_eq!(row.details, json!({"answer": label}));
        }
    }

    #[test]
    fn every_origin_is_one_the_harness_knows() {
        let file = TempFile::new("origins");
        let origins = [
            (Origin::Foliate, HarnessOrigin::Foliate),
            (Origin::IntelliJ, HarnessOrigin::IntelliJ),
            (Origin::AndroidStudio, HarnessOrigin::AndroidStudio),
            (Origin::Local, HarnessOrigin::Local),
        ];
        for (origin, _) in origins {
            start(&file.0, origin);
        }
        let read: Vec<_> = read(&file.0).iter().map(|r| r.origin).collect();
        assert_eq!(read, origins.map(|(_, harness)| harness));
    }

    #[test]
    fn a_warm_activation_is_an_attempt_and_a_timed_frame_is_a_sample() {
        let file = TempFile::new("warm");
        let mut writer = start(&file.0, Origin::Foliate);
        for count in 1..=5 {
            press(&mut writer, count, true, true);
        }
        let rows = read(&file.0);
        // The cold press is not an attempt.
        let s = foliate(&rows, None);
        assert_eq!((s.warm_attempts, s.valid_latency_samples), (4, 0));
        let s = foliate(&rows, Some(&mapping()));
        assert_eq!((s.warm_attempts, s.valid_latency_samples), (4, 4));
        // 100 ms to the frame, plus 1 ms of timestamp quantization.
        assert_eq!(s.max_ms, Some(101.0));
    }

    #[test]
    fn a_presentation_without_a_time_is_not_a_sample() {
        let file = TempFile::new("untimed");
        let mut writer = start(&file.0, Origin::Foliate);
        press(&mut writer, 1, true, true);
        press(&mut writer, 2, true, false);
        press(&mut writer, 3, true, true);
        // A timed frame of a press with no token is not a sample either.
        press(&mut writer, 4, false, true);
        let s = foliate(&read(&file.0), Some(&mapping()));
        assert_eq!((s.warm_attempts, s.valid_latency_samples), (3, 1));
    }

    #[test]
    fn a_second_session_supersedes_the_first_and_truncates_nothing() {
        let file = TempFile::new("sessions");
        let mut first = start(&file.0, Origin::Foliate);
        for count in 1..=4 {
            press(&mut first, count, true, true);
        }
        drop(first);
        let before = std::fs::read(&file.0).expect("read the first session");
        assert_eq!(foliate(&read(&file.0), None).warm_attempts, 3);

        let mut second = start(&file.0, Origin::Foliate);
        press(&mut second, 1, true, true);
        press(&mut second, 2, true, true);
        let after = std::fs::read(&file.0).expect("read both sessions");
        assert!(after.len() > before.len());
        assert_eq!(&after[..before.len()], &before[..]);
        let rows = read(&file.0);
        assert_eq!(rows.len(), 17 + 9);
        assert_eq!(foliate(&rows, None).warm_attempts, 1);
    }

    #[test]
    fn records_of_android_studio_are_written_and_left_unsummarised() {
        let file = TempFile::new("studio");
        let mut writer = start(&file.0, Origin::AndroidStudio);
        press(&mut writer, 1, true, true);
        press(&mut writer, 2, true, true);
        let rows = read(&file.0);
        assert_eq!(rows.len(), 9);
        assert!(
            metrics::summarize(&rows, Some(&mapping()))
                .iter()
                .all(|s| s.warm_attempts == 0)
        );
    }

    #[test]
    fn an_unopenable_path_is_an_error() {
        let file = TempFile::new("missing-dir");
        let path = file.0.join("evidence.jsonl");
        assert!(Writer::open(&path, Origin::Local).is_err());
        assert!(!file.0.exists());
    }

    #[test]
    fn a_failed_write_ends_the_recording() {
        // Every write to /dev/full fails with "no space left on device".
        let mut writer = Writer::open(Path::new("/dev/full"), Origin::Local).expect("open");
        assert!(writer.is_recording());
        writer.write(&session_start(1, None));
        assert!(!writer.is_recording());
        // Later records are dropped without another attempt.
        writer.write(&focus("id", true));
        assert!(!writer.is_recording());
    }

    #[test]
    fn the_receipt_clock_does_not_go_back() {
        let first = monotonic_ns();
        assert!(first > 0);
        assert!(monotonic_ns() >= first);
    }
}
