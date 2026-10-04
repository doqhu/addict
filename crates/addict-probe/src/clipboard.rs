//! Manual-path fallback evidence: a bounded read of the clipboard and the
//! primary selection, two ways. The content is measured and previewed, never
//! interpreted: a selection is not a word plus its context.

use std::{
    io::Read,
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::record::{Bounded, Finish, Outcome, Progress, bounded};

pub const EXPECTED: &str = "outcome text: at least one read returns text, recorded with its \
     source, length and a short preview; empty or unavailable with the reason when no selection \
     is readable or the protocol is missing; every read ends at the timeout";

/// Upper bound on how much of a selection is read.
pub const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    Clipboard,
    Primary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Method {
    /// `wl-clipboard-rs`: the data-control Wayland protocols, no window.
    WaylandProtocol,
    /// The `wl-paste` program.
    WlPaste,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Text,
    Empty,
    Unavailable,
    Timeout,
    /// The deadline had passed before this read could start.
    NotAttempted,
    Error,
}

/// One attempt to read one selection one way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Read1 {
    pub source: Source,
    pub method: Method,
    pub status: Status,
    pub reason: Option<String>,
    pub chars: Option<usize>,
    pub bytes: Option<usize>,
    pub preview: Option<String>,
    pub elapsed_ms: u64,
}

/// The raw answer of a read, before it is measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Raw {
    Text(String),
    Empty(String),
    Unavailable(String),
    Timeout,
    NotAttempted,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct Args {
    pub preview_chars: usize,
    pub deadline: Instant,
}

/// The first `limit` characters; control characters become spaces so a
/// preview stays on one line.
pub fn preview(text: &str, limit: usize) -> String {
    text.chars()
        .take(limit)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub fn measure(
    source: Source,
    method: Method,
    raw: Raw,
    preview_chars: usize,
    elapsed: Duration,
) -> Read1 {
    let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let base = |status, reason| Read1 {
        source,
        method,
        status,
        reason,
        chars: None,
        bytes: None,
        preview: None,
        elapsed_ms,
    };
    match raw {
        Raw::Text(text) if text.is_empty() => base(
            Status::Empty,
            Some("the selection is zero bytes".to_owned()),
        ),
        Raw::Text(text) => Read1 {
            chars: Some(text.chars().count()),
            bytes: Some(text.len()),
            preview: Some(preview(&text, preview_chars)),
            ..base(Status::Text, None)
        },
        Raw::Empty(reason) => base(Status::Empty, Some(reason)),
        Raw::Unavailable(reason) => base(Status::Unavailable, Some(reason)),
        Raw::Timeout => base(
            Status::Timeout,
            Some("no answer before the deadline".to_owned()),
        ),
        Raw::NotAttempted => base(
            Status::NotAttempted,
            Some("not attempted: the deadline had passed before this read started".to_owned()),
        ),
        Raw::Error(reason) => base(Status::Error, Some(reason)),
    }
}

/// The run's outcome from its reads. Text anywhere beats everything. Without
/// text, a definite answer names the outcome: an empty selection, then
/// unavailability. `timeout` is the outcome only when no read gave a definite
/// answer; an unexplained failure is last. The error text carries the reason
/// of every read, those behind the outcome first.
pub fn summarize(reads: &[Read1]) -> Finish {
    if reads.iter().any(|r| r.status == Status::Text) {
        return Finish::ok(Outcome::Text);
    }
    let reasons = |status: Status| -> String {
        let (named, rest): (Vec<&Read1>, Vec<&Read1>) =
            reads.iter().partition(|r| r.status == status);
        named
            .into_iter()
            .chain(rest)
            .map(|r| {
                format!(
                    "{:?}/{:?}: {}",
                    r.source,
                    r.method,
                    r.reason.as_deref().unwrap_or("no reason given")
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    for (status, outcome) in [
        (Status::Empty, Outcome::Empty),
        (Status::Unavailable, Outcome::Unavailable),
        (Status::Timeout, Outcome::Timeout),
        (Status::Error, Outcome::Error),
        (Status::NotAttempted, Outcome::Timeout),
    ] {
        if reads.iter().any(|r| r.status == status) {
            return Finish::with_error(outcome, reasons(status));
        }
    }
    Finish::with_error(Outcome::Unavailable, "no read was attempted")
}

pub async fn run(args: &Args, progress: &Progress) -> Finish {
    progress.put(
        "note",
        "content is measured and previewed only; it is not treated as a word or its context",
    );
    let mut reads = Vec::new();
    // In sequence: `wl-paste` may need to take focus for an instant, and two
    // readers doing that at once would disturb each other's answer.
    for (source, method) in [
        (Source::Clipboard, Method::WaylandProtocol),
        (Source::Clipboard, Method::WlPaste),
        (Source::Primary, Method::WaylandProtocol),
        (Source::Primary, Method::WlPaste),
    ] {
        progress.phase(&format!("read-{source:?}-{method:?}").to_lowercase());
        let started = Instant::now();
        let remaining = args.deadline.saturating_duration_since(started);
        let raw = if remaining.is_zero() {
            Raw::NotAttempted
        } else {
            let read = async {
                match method {
                    Method::WaylandProtocol => protocol_read(source).await,
                    Method::WlPaste => wl_paste_read(source).await,
                }
            };
            match bounded(remaining, read).await {
                Bounded::Done(raw) => raw,
                Bounded::TimedOut => Raw::Timeout,
            }
        };
        reads.push(measure(
            source,
            method,
            raw,
            args.preview_chars,
            started.elapsed(),
        ));
        progress.put("reads", &reads);
    }
    progress.phase("done");

    let finish = summarize(&reads);
    if let Some(first) = reads.iter().find(|r| r.status == Status::Text) {
        progress.put("text_source", first.source);
        progress.put("text_method", first.method);
        progress.put("text_chars", first.chars);
        progress.put("text_preview", &first.preview);
    }
    finish
}

async fn protocol_read(source: Source) -> Raw {
    let (tx, rx) = tokio::sync::oneshot::channel();
    // A helper thread: the library blocks, and a blocked thread must not be
    // able to hold the run past its deadline.
    std::thread::spawn(move || {
        let _ = tx.send(protocol_read_blocking(source));
    });
    rx.await
        .unwrap_or_else(|_| Raw::Error("the helper thread ended without an answer".to_owned()))
}

fn protocol_read_blocking(source: Source) -> Raw {
    use wl_clipboard_rs::paste::{ClipboardType, Error, MimeType, Seat, get_contents};

    let clipboard = match source {
        Source::Clipboard => ClipboardType::Regular,
        Source::Primary => ClipboardType::Primary,
    };
    match get_contents(clipboard, Seat::Unspecified, MimeType::Text) {
        Ok((pipe, _mime)) => {
            let mut bytes = Vec::new();
            match pipe.take(MAX_BYTES).read_to_end(&mut bytes) {
                Ok(_) => Raw::Text(String::from_utf8_lossy(&bytes).into_owned()),
                Err(e) => Raw::Error(format!("reading the selection pipe: {e}")),
            }
        }
        Err(e @ (Error::ClipboardEmpty | Error::NoMimeType)) => Raw::Empty(e.to_string()),
        Err(
            e @ (Error::MissingProtocol { .. }
            | Error::PrimarySelectionUnsupported
            | Error::NoSeats
            | Error::SeatNotFound
            | Error::SocketOpenError(_)
            | Error::WaylandConnection(_)),
        ) => Raw::Unavailable(e.to_string()),
        Err(e) => Raw::Error(e.to_string()),
    }
}

async fn wl_paste_read(source: Source) -> Raw {
    let mut command = tokio::process::Command::new("wl-paste");
    command.arg("--no-newline").args(["--type", "text"]);
    if source == Source::Primary {
        command.arg("--primary");
    }
    command
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    match command.output().await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Raw::Unavailable("wl-paste is not installed".to_owned())
        }
        Err(e) => Raw::Error(format!("starting wl-paste: {e}")),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            if output.status.success() {
                let mut bytes = output.stdout;
                bytes.truncate(MAX_BYTES as usize);
                Raw::Text(String::from_utf8_lossy(&bytes).into_owned())
            } else {
                wl_paste_failure(&stderr, output.status.code())
            }
        }
    }
}

/// Names a `wl-paste` failure from what it printed.
pub fn wl_paste_failure(stderr: &str, code: Option<i32>) -> Raw {
    let lower = stderr.to_lowercase();
    let text = if stderr.is_empty() {
        format!("wl-paste exited with {code:?} and no message")
    } else {
        stderr.to_owned()
    };
    if lower.contains("nothing is copied")
        || lower.contains("no selection")
        || lower.contains("no suitable type")
    {
        Raw::Empty(text)
    } else if lower.contains("failed to connect")
        || lower.contains("missing")
        || lower.contains("not supported")
        || lower.contains("unsupported")
    {
        Raw::Unavailable(text)
    } else {
        Raw::Error(text)
    }
}
