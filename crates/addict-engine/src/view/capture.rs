//! Source messages for the screen picker.

use std::borrow::Cow;

use addict_core::failure::{Ended, capture::CaptureFailure};

/// What the source line says when the snapshot file is not there, whether
/// the capture or the recognition found it missing.
pub(super) const NO_SNAPSHOT_FILE: &str = "The snapshot file was not found.";

/// What ends the line of a capture that left no snapshot: the way back to the
/// region picker, on the button that has the keyboard focus then.
pub(super) const CAPTURE_RETRY: &str = "Press Capture to try again.";

/// `line`, what happened to a capture that left no snapshot, and the way back.
pub(super) fn with_retry(line: &str) -> Cow<'static, str> {
    format!("{line} {CAPTURE_RETRY}").into()
}

/// The source line when the picker gave no snapshot.
pub fn capture_failure_text(failure: &Ended<CaptureFailure>) -> Cow<'static, str> {
    match failure {
        Ended::Shell(CaptureFailure::NoPermission) => {
            with_retry("Screen capture permission was denied.")
        }
        Ended::Shell(CaptureFailure::Cancelled) => with_retry("The capture was cancelled."),
        Ended::Shell(CaptureFailure::Ended) => with_retry("No snapshot was taken."),
        Ended::Shell(CaptureFailure::Unavailable) => {
            with_retry("Screen capture is not available here.")
        }
        Ended::Shell(CaptureFailure::Failed(detail)) => {
            let detail = detail.trim_end_matches('.');
            with_retry(&format!("The screen capture failed: {detail}."))
        }
        Ended::Shell(CaptureFailure::NoImage) => with_retry(NO_SNAPSHOT_FILE),
        Ended::DeadlinePassed => with_retry("No area was chosen in time."),
        Ended::Unfinished => with_retry("The capture did not finish."),
    }
}
