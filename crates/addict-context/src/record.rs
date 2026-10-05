//! EXP-1/2 evidence format: outcome, run record with an accessibility-status
//! block, JSON-lines append. Environment capture, timestamps and the bounded
//! wait come from `addict_probe`; its EXP-3 outcome set is left alone.

use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::Path,
    time::{Duration, SystemTime},
};

use addict_probe::record::{Environment, rfc3339_utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version of the record layout. Bump when a field changes meaning.
pub const SCHEMA: u32 = 1;

/// Exit code used when the record itself could not be written.
pub const EXIT_RECORD_NOT_WRITTEN: u8 = 9;

/// The named end of a run. A missing bus, a missing application, no text and
/// no selection are outcomes, not errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// `status` / `tree`: the bus answered and the answer was recorded.
    Reported,
    /// `context`: the selected word and a sentence were read.
    Context,
    /// `context`: the selection was read, no sentence from either source.
    WordOnly,
    /// `context`: the application has no Text object with characters.
    NoText,
    /// `context`: Text objects exist, none reports a selection.
    NoSelection,
    /// `context`: more than one object reports a selection.
    Ambiguous,
    /// `tree` / `context`: no registered application matches `--app`.
    AppNotFound,
    /// The accessibility bus is absent or unreachable.
    Unavailable,
    /// A bounded wait expired.
    Timeout,
    /// Anything the probe has no named outcome for.
    Error,
}

impl Outcome {
    pub const ALL: [Self; 10] = [
        Self::Reported,
        Self::Context,
        Self::WordOnly,
        Self::NoText,
        Self::NoSelection,
        Self::Ambiguous,
        Self::AppNotFound,
        Self::Unavailable,
        Self::Timeout,
        Self::Error,
    ];

    /// Process exit code. `2` is left to the argument parser and `9` to a
    /// record that could not be written.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Reported | Self::Context => 0,
            Self::Error => 1,
            Self::Timeout => 3,
            Self::AppNotFound => 4,
            Self::Unavailable => 5,
            Self::NoText => 6,
            Self::NoSelection => 7,
            Self::WordOnly => 8,
            Self::Ambiguous => 10,
        }
    }

    /// The name written to the record.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Context => "context",
            Self::WordOnly => "word-only",
            Self::NoText => "no-text",
            Self::NoSelection => "no-selection",
            Self::Ambiguous => "ambiguous",
            Self::AppNotFound => "app-not-found",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Error => "error",
        }
    }
}

/// How a run ended: the outcome and, when there is one, the error text as
/// the runtime gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finish {
    pub outcome: Outcome,
    pub error: Option<String>,
}

impl Finish {
    pub fn ok(outcome: Outcome) -> Self {
        Self {
            outcome,
            error: None,
        }
    }

    pub fn with_error(outcome: Outcome, error: impl Into<String>) -> Self {
        Self {
            outcome,
            error: Some(error.into()),
        }
    }
}

/// What the session said about accessibility when the run was taken. A value
/// the machine did not give is `null`, never a guess.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accessibility {
    /// Address of the accessibility bus the probe used.
    pub bus_address: Option<String>,
    /// `AT_SPI_BUS_ADDRESS` or `org.a11y.Bus`.
    pub bus_address_source: Option<String>,
    /// `org.a11y.Status.IsEnabled`.
    pub is_enabled: Option<bool>,
    /// `org.a11y.Status.ScreenReaderEnabled`.
    pub screen_reader_enabled: Option<bool>,
    /// Why a status value above is `null`.
    pub status_error: Option<String>,
    /// Installed `at-spi2-core` package version.
    pub at_spi2_core: Option<String>,
    /// `gsettings get org.gnome.desktop.interface toolkit-accessibility`.
    pub toolkit_accessibility_setting: Option<String>,
}

/// One line of evidence.
#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub schema: u32,
    pub timestamp: String,
    pub tool: String,
    pub tool_version: String,
    pub probe: String,
    pub environment: Environment,
    pub accessibility: Accessibility,
    /// The matched application(s): name, toolkit, version, pid. `null` for
    /// `status` and when no application was matched.
    pub target: Value,
    pub inputs: Value,
    pub expected: String,
    pub outcome: Outcome,
    pub error: Option<String>,
    pub details: Value,
    pub elapsed_ms: u64,
}

impl Record {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        probe: &str,
        environment: Environment,
        accessibility: Accessibility,
        target: Value,
        inputs: Value,
        expected: &str,
        finish: Finish,
        details: Value,
        elapsed: Duration,
        now: SystemTime,
    ) -> Self {
        Self {
            schema: SCHEMA,
            timestamp: rfc3339_utc(now),
            tool: env!("CARGO_PKG_NAME").to_owned(),
            tool_version: env!("CARGO_PKG_VERSION").to_owned(),
            probe: probe.to_owned(),
            environment,
            accessibility,
            target,
            inputs,
            expected: expected.to_owned(),
            outcome: finish.outcome,
            error: finish.error,
            details,
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        }
    }

    /// The record as one JSON line, without the trailing newline.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("a record holds only JSON-representable values")
    }
}

/// Appends one record as one line, creating the file and its parent
/// directory if needed. Earlier lines are never rewritten.
pub fn append(path: &Path, record: &Record) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut line = record.to_line();
    line.push('\n');
    file.write_all(line.as_bytes())?;
    file.flush()
}
