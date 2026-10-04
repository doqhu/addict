//! One evidence format for every probe: outcome, run record, environment
//! capture and JSON-lines append.

use std::{
    fs::OpenOptions,
    future::Future,
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::{Map, Value};

/// Version of the record layout. Bump when a field changes meaning.
pub const SCHEMA: u32 = 1;

/// Exit code used when the record itself could not be written.
pub const EXIT_RECORD_NOT_WRITTEN: u8 = 9;

/// The named end of a run. Cancellation and unavailability are outcomes, not
/// errors; only `Error` means the probe met something it has no name for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// `caps`: the runtime answered and the answer was recorded.
    Reported,
    /// `shortcut`: the bound shortcut fired.
    Activated,
    /// `shortcut`: the binding was dismissed or zero shortcuts were bound.
    NotBound,
    /// A bounded wait expired.
    Timeout,
    /// `screenshot`: the portal returned an image URI.
    Captured,
    /// The user cancelled the interaction.
    Cancelled,
    /// `clipboard`: text was read.
    Text,
    /// `clipboard`: a selection was reachable and held no text.
    Empty,
    /// The capability is missing or unreachable at runtime.
    Unavailable,
    /// `install-desktop`: the desktop entry was written.
    Installed,
    /// `uninstall-desktop`: the desktop entry is gone.
    Removed,
    /// `install-desktop` / `uninstall-desktop`: the file is not the probe's.
    Refused,
    /// Anything the probe has no named outcome for.
    Error,
}

impl Outcome {
    /// Process exit code. `2` is left to the argument parser.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Reported
            | Self::Activated
            | Self::Captured
            | Self::Text
            | Self::Installed
            | Self::Removed
            | Self::Cancelled => 0,
            Self::Error => 1,
            Self::Timeout => 3,
            Self::NotBound => 4,
            Self::Unavailable => 5,
            Self::Empty => 6,
            Self::Refused => 7,
        }
    }

    /// The name written to the record.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Activated => "activated",
            Self::NotBound => "not-bound",
            Self::Timeout => "timeout",
            Self::Captured => "captured",
            Self::Cancelled => "cancelled",
            Self::Text => "text",
            Self::Empty => "empty",
            Self::Unavailable => "unavailable",
            Self::Installed => "installed",
            Self::Removed => "removed",
            Self::Refused => "refused",
            Self::Error => "error",
        }
    }
}

/// How a probe ended: the outcome and, when there is one, the error text
/// exactly as the runtime gave it.
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

/// Details a probe gathers while it runs. Shared with the runner so that a
/// run cut short by the timeout still records how far it got.
#[derive(Debug, Clone, Default)]
pub struct Progress(Arc<Mutex<Map<String, Value>>>);

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Names the wait the probe is in; a timeout record carries the last one.
    pub fn phase(&self, phase: &str) {
        self.put("phase", phase);
    }

    pub fn put(&self, key: &str, value: impl Serialize) {
        let value = serde_json::to_value(value).unwrap_or(Value::Null);
        self.lock().insert(key.to_owned(), value);
    }

    pub fn snapshot(&self) -> Value {
        Value::Object(self.lock().clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Map<String, Value>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// What the machine reported about itself. A value the machine did not give
/// is `null`, never a guess.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Environment {
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub session_type: Option<String>,
    pub current_desktop: Option<String>,
    pub wayland_display: Option<String>,
    pub gnome_shell: Option<String>,
    pub xdg_desktop_portal: Option<String>,
    pub xdg_desktop_portal_gnome: Option<String>,
    pub wl_paste: Option<String>,
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

/// Result of a bounded wait.
#[derive(Debug, PartialEq, Eq)]
pub enum Bounded<T> {
    Done(T),
    TimedOut,
}

/// Runs `future` for at most `limit`. The future is dropped when the limit
/// passes, so nothing keeps waiting behind the caller's back.
pub async fn bounded<F: Future>(limit: Duration, future: F) -> Bounded<F::Output> {
    match tokio::time::timeout(limit, future).await {
        Ok(value) => Bounded::Done(value),
        Err(_) => Bounded::TimedOut,
    }
}

/// UTC timestamp, `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn rfc3339_utc(time: SystemTime) -> String {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since_epoch.as_secs();
    let millis = since_epoch.subsec_millis();
    let (year, month, day) = civil_from_days(secs / 86_400);
    let rest = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// algorithm, restricted to dates on or after the epoch).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// Reads the environment. Every external command is bounded by `limit`; a
/// command that is missing, fails or overruns leaves its field `null`.
pub async fn capture_environment(limit: Duration) -> Environment {
    let (gnome_shell, portal, portal_gnome, wl_paste) = tokio::join!(
        command_line(limit, "gnome-shell", &["--version"]),
        command_line(
            limit,
            "rpm",
            &["-q", "--qf", "%{VERSION}-%{RELEASE}", "xdg-desktop-portal"]
        ),
        command_line(
            limit,
            "rpm",
            &[
                "-q",
                "--qf",
                "%{VERSION}-%{RELEASE}",
                "xdg-desktop-portal-gnome"
            ]
        ),
        command_line(limit, "wl-paste", &["--version"]),
    );
    Environment {
        os: os_pretty_name(),
        kernel: std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_owned()),
        session_type: env_var("XDG_SESSION_TYPE"),
        current_desktop: env_var("XDG_CURRENT_DESKTOP"),
        wayland_display: env_var("WAYLAND_DISPLAY"),
        gnome_shell,
        xdg_desktop_portal: portal,
        xdg_desktop_portal_gnome: portal_gnome,
        wl_paste,
    }
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn os_pretty_name() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    parse_os_release(&text)
}

/// `PRETTY_NAME` from os-release text, unquoted.
pub fn parse_os_release(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim().trim_matches('"').to_owned())
        .filter(|value| !value.is_empty())
}

/// First line of a successful command's stdout.
async fn command_line(limit: Duration, program: &str, args: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match bounded(limit, command.output()).await {
        Bounded::Done(Ok(output)) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(|line| line.trim().to_owned())
                .filter(|line| !line.is_empty())
        }
        _ => None,
    }
}
