//! Measurement evidence of `--evidence`: one JSON line per fact, in the
//! `Evidence` format that `addict-exp4 summarize` reads.
//!
//! The application writes raw values only: the portal timestamp, the receipt
//! time, the `wp_presentation` time. The acceptance latency is computed by the
//! harness from these and a clock certificate.
//!
//! The event names and the fields of `details` are the harness's own. The
//! session start is named `broker-started` because that is the record the
//! harness splits sessions on. The `online` record is the exception: the
//! harness reads its line and judges nothing by it.

use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
    time::{Instant, SystemTime},
};

use addict_core::translation::Refusal;
use addict_platform::work::Outcome;
use serde_json::{Value, json};

use crate::{
    dictionaries::{Answer, Looked},
    online::Subject,
    view::Online,
};

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

#[cfg(test)]
impl Record {
    pub fn event(&self) -> &str {
        self.event
    }

    pub fn details(&self) -> &Value {
        &self.details
    }
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

/// What an online request was about, as the evidence names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requested {
    /// A word, in its sentence or alone.
    Word,
    Sentence,
    /// The definition of the chosen sense.
    Definition,
}

impl Requested {
    fn label(self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Sentence => "sentence",
            Self::Definition => "definition",
        }
    }
}

/// No wildcard arm: a new subject does not compile until it is given its
/// kind.
impl From<Subject> for Requested {
    fn from(subject: Subject) -> Self {
        match subject {
            Subject::Word(_) | Subject::Lone => Self::Word,
            Subject::Sentence => Self::Sentence,
        }
    }
}

/// How an online request ended, as the evidence names it: with a translation
/// or with the refusal it got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Translated,
    Refused(Refusal),
}

impl Ended {
    /// No wildcard arm: a new refusal does not compile until it is given its
    /// label.
    fn label(self) -> &'static str {
        match self {
            Self::Translated => "translated",
            Self::Refused(Refusal::Key) => "no-key",
            Self::Refused(Refusal::UnreadableKey) => "unreadable-key",
            Self::Refused(Refusal::UnsafeKey) => "unsafe-key",
            Self::Refused(Refusal::Network) => "network",
            Self::Refused(Refusal::UnsupportedLanguage) => "unsupported-language",
            Self::Refused(Refusal::RejectedKey) => "rejected-key",
            Self::Refused(Refusal::Quota) => "quota",
            Self::Refused(Refusal::TimedOut) => "timed-out",
            Self::Refused(Refusal::Other) => "other",
        }
    }
}

/// What the evidence says of an online request that stands as `online`. One
/// that still runs has not ended. The translation itself is dropped here.
pub fn ended(online: &Online) -> Option<Ended> {
    match online {
        Online::Translated(_) => Some(Ended::Translated),
        Online::Refused(refusal) => Some(Ended::Refused(*refusal)),
        Online::Pending => None,
    }
}

/// The memory of this process, in bytes; a value that could not be read is
/// `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Memory {
    /// The kernel's high-water mark of the RSS.
    peak_rss_bytes: Option<u64>,
    pss_bytes: Option<u64>,
}

impl Memory {
    /// Reads `VmHWM` of `/proc/self/status` and `Pss` of
    /// `/proc/self/smaps_rollup`.
    fn read() -> Self {
        let field = |path: &str, name: &str| {
            let text = std::fs::read_to_string(path).ok()?;
            proc_bytes(&text, name)
        };
        Self {
            peak_rss_bytes: field("/proc/self/status", "VmHWM:"),
            pss_bytes: field("/proc/self/smaps_rollup", "Pss:"),
        }
    }
}

/// The value of the line of `text` that begins with `name`, which the kernel
/// writes in kB, in bytes.
fn proc_bytes(text: &str, name: &str) -> Option<u64> {
    let mut fields = text
        .lines()
        .find_map(|line| line.strip_prefix(name))?
        .split_whitespace();
    let kilobytes: u64 = fields.next()?.parse().ok()?;
    (fields.next() == Some("kB")).then_some(())?;
    kilobytes.checked_mul(1024)
}

/// The start of an online request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Started(Instant);

impl Started {
    fn now() -> Self {
        Self(Instant::now())
    }

    /// The request ended now.
    fn ended(self) -> Elapsed {
        Elapsed(u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX))
    }
}

/// The milliseconds from the start of an online request to its outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed(u64);

/// What the record of an online request says was measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Measured {
    elapsed_ms: u64,
    memory: Memory,
}

/// The measuring of one online request. It holds the start of the request
/// only when evidence is recorded: without the flag no clock is read, and
/// then no time is taken at the outcome and no memory is read for a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe(Option<Started>);

impl Probe {
    /// Call it when the request is handed to the runner.
    pub fn start(recording: bool) -> Self {
        Self(recording.then(Started::now))
    }

    /// The request ended now. Call it where the outcome arrives, on the
    /// worker: the time is not the redraw's. It reads the clock and nothing
    /// else, so the worker is not held.
    pub fn ended(self) -> Option<Elapsed> {
        self.0.map(Started::ended)
    }

    /// The request could not start: no time passed.
    pub fn not_started(self) -> Option<Elapsed> {
        self.0.map(|_| Elapsed(0))
    }

    #[cfg(test)]
    pub fn started(self) -> Option<Started> {
        self.0
    }
}

/// The record of an online request for `requested` that stands as `online`
/// and took `elapsed`. None for a request that has not ended, and none
/// without a time: evidence is not recorded then. The memory is read here,
/// off the worker, and only for a record that is made.
pub fn online_record(
    requested: Requested,
    online: &Online,
    elapsed: Option<Elapsed>,
) -> Option<Record> {
    let (ended, Elapsed(elapsed_ms)) = (ended(online)?, elapsed?);
    let measured = Measured {
        elapsed_ms,
        memory: Memory::read(),
    };
    Some(self::online(requested, ended, measured))
}

/// An online request ended. The record says what kind of text was asked for,
/// how it ended and what that took, and nothing of the text, of the answer
/// or of the key. It names no press, and its event is its own: the harness
/// counts it nowhere.
fn online(requested: Requested, ended: Ended, measured: Measured) -> Record {
    Record {
        invocation_id: None,
        event: "online",
        details: json!({
            "subject": requested.label(),
            "outcome": ended.label(),
            "elapsed_ms": measured.elapsed_ms,
            "peak_rss_bytes": measured.memory.peak_rss_bytes,
            "pss_bytes": measured.memory.pss_bytes,
        }),
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

    const MEMORY: Memory = Memory {
        peak_rss_bytes: Some(141_234_176),
        pss_bytes: Some(63_012_864),
    };

    /// Every way an online request ends, with its label.
    fn endings() -> [(Online, &'static str); 10] {
        let refused = |refusal| Online::Refused(refusal);
        [
            (Online::Translated("берег реки".to_owned()), "translated"),
            (refused(Refusal::Key), "no-key"),
            (refused(Refusal::UnreadableKey), "unreadable-key"),
            (refused(Refusal::UnsafeKey), "unsafe-key"),
            (refused(Refusal::Network), "network"),
            (
                refused(Refusal::UnsupportedLanguage),
                "unsupported-language",
            ),
            (refused(Refusal::RejectedKey), "rejected-key"),
            (refused(Refusal::Quota), "quota"),
            (refused(Refusal::TimedOut), "timed-out"),
            (refused(Refusal::Other), "other"),
        ]
    }

    #[test]
    fn an_online_request_is_recorded_by_its_kind_outcome_delay_and_memory() {
        let file = TempFile::new("online");
        let mut writer = start(&file.0, Origin::Foliate);
        let kinds = [
            (Requested::from(Subject::Word(3)), "word"),
            (Requested::from(Subject::Lone), "word"),
            (Requested::from(Subject::Sentence), "sentence"),
            (Requested::Definition, "definition"),
        ];
        let mut expected = Vec::new();
        for (requested, subject) in kinds {
            for (elapsed_ms, (online, outcome)) in (400..).zip(endings()) {
                let measured = Measured {
                    elapsed_ms,
                    memory: MEMORY,
                };
                let ended = ended(&online).expect("a request that ended");
                writer.write(&super::online(requested, ended, measured));
                expected.push(json!({
                    "subject": subject,
                    "outcome": outcome,
                    "elapsed_ms": elapsed_ms,
                    "peak_rss_bytes": 141_234_176,
                    "pss_bytes": 63_012_864,
                }));
            }
        }
        // A request that still runs has no outcome to record.
        assert_eq!(ended(&Online::Pending), None);

        let text = std::fs::read_to_string(&file.0).expect("read the evidence");
        // Nothing of the answer is in the file.
        assert!(!text.contains("берег"));
        let rows = read(&file.0);
        assert_eq!(rows.len(), 1 + expected.len());
        for (row, details) in rows[1..].iter().zip(&expected) {
            assert_eq!(row.event, "online");
            assert_eq!(row.invocation_id, None);
            assert_eq!(
                (row.backend, row.mode, row.origin),
                (Backend::Gtk4, Mode::Resident, HarnessOrigin::Foliate)
            );
            // These five fields and nothing else.
            assert_eq!(&row.details, details);
        }
    }

    #[test]
    fn memory_that_cannot_be_read_is_null_in_the_record() {
        let file = TempFile::new("online-null");
        let mut writer = start(&file.0, Origin::Foliate);
        let measured = Measured {
            elapsed_ms: 0,
            memory: Memory {
                peak_rss_bytes: None,
                pss_bytes: None,
            },
        };
        let refused = Ended::Refused(Refusal::Other);
        writer.write(&online(Requested::Definition, refused, measured));
        assert_eq!(
            read(&file.0)[1].details,
            json!({
                "subject": "definition",
                "outcome": "other",
                "elapsed_ms": 0,
                "peak_rss_bytes": null,
                "pss_bytes": null,
            })
        );
    }

    #[test]
    fn the_memory_fields_of_proc_are_read_in_bytes() {
        let status =
            "Name:\taddict\nVmPeak:\t  900000 kB\nVmHWM:\t  137924 kB\nVmRSS:\t  120000 kB\n";
        assert_eq!(proc_bytes(status, "VmHWM:"), Some(137_924 * 1024));
        let rollup = "55d0-7ffd ---p 00000000 00:00 0 [rollup]\nRss:  120000 kB\n\
                      Pss:   61536 kB\nPss_Dirty:  100 kB\nPss_Anon:  200 kB\n";
        assert_eq!(proc_bytes(rollup, "Pss:"), Some(61_536 * 1024));
        // A missing field, another unit and a value that is no number.
        assert_eq!(proc_bytes(rollup, "VmHWM:"), None);
        assert_eq!(proc_bytes("VmHWM:\t12 MB\n", "VmHWM:"), None);
        assert_eq!(proc_bytes("VmHWM:\tmany kB\n", "VmHWM:"), None);
        assert_eq!(proc_bytes("VmHWM:\t12\n", "VmHWM:"), None);
    }

    #[test]
    fn the_memory_of_this_process_is_read_when_the_record_is_made() {
        let probe = Probe::start(true);
        let elapsed = probe.ended().expect("a recorded request is timed");
        assert!(elapsed.0 < 60_000);
        assert_eq!(probe.not_started(), Some(Elapsed(0)));
        let translated = Online::Translated("берег".to_owned());
        let record = online_record(Requested::Word, &translated, Some(elapsed)).expect("a record");
        assert_eq!(record.details["elapsed_ms"], elapsed.0);
        for field in ["peak_rss_bytes", "pss_bytes"] {
            assert!(
                record.details[field]
                    .as_u64()
                    .is_some_and(|bytes| bytes > 0)
            );
        }
        // No record, and so no read, of a request that still runs or was not
        // timed.
        assert_eq!(
            online_record(Requested::Word, &Online::Pending, Some(elapsed)),
            None
        );
        assert_eq!(online_record(Requested::Word, &translated, None), None);
    }

    #[test]
    fn a_session_with_online_records_summarises_as_one_without() {
        let session = |with_online: bool| {
            let file = TempFile::new("online-summary");
            let mut writer = start(&file.0, Origin::Foliate);
            let measured = Measured {
                elapsed_ms: 412,
                memory: MEMORY,
            };
            let online = |writer: &mut Writer, requested| {
                if with_online {
                    writer.write(&super::online(requested, Ended::Translated, measured));
                }
            };
            for count in 1..=5 {
                press(&mut writer, count, true, true);
                online(&mut writer, Requested::Word);
            }
            for _ in 0..3 {
                writer.write(&lookup(Answered::Article));
                online(&mut writer, Requested::Sentence);
                online(&mut writer, Requested::Definition);
            }
            read(&file.0)
        };
        let (without, with) = (session(false), session(true));
        assert_eq!(with.len(), without.len() + 5 + 6);
        let value = |summary| serde_json::to_value(summary).expect("a summary is JSON");
        for mapping in [None, Some(&mapping())] {
            assert_eq!(
                value(foliate(&with, mapping)),
                value(foliate(&without, mapping))
            );
        }
        let product = |rows: &[Evidence]| {
            let all = metrics::summarize_product(rows, Some(&mapping()));
            serde_json::to_value(all).expect("a summary is JSON")
        };
        let row = product(&with);
        assert_eq!(row, product(&without));
        assert_eq!(row[0]["lookups"], 3);
        assert_eq!(row[0]["warm_attempts"], 4);
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
