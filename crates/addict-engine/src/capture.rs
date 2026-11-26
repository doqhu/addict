//! What a capture ended with, without the job that makes it: what was
//! [`Captured`], why there is no text ([`Failure`]) and what the capture
//! took ([`Timing`]). The job that drives the portal and Tesseract is the
//! Linux shell's; the window texts of these are `view`'s.

use addict_core::failure::{capture::CaptureFailure, recognize::OcrFailure};

/// What a capture ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// The recognized text, as Tesseract gave it, trimmed.
    Text(String),
    /// No text.
    Failed(Failure),
}

/// Why a capture gave no text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The screenshot gave no snapshot.
    Capture(CaptureFailure),
    /// The user took longer than the capture's limit.
    CaptureTimedOut,
    /// The recognition gave no text.
    Ocr(OcrFailure),
    /// The job could not start, panicked or was stopped at its deadline.
    Unfinished,
}

/// What a capture took, measured on the worker; what was not reached is
/// `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timing {
    /// From the start of the job to the snapshot or to the capture's failure:
    /// the wait for the area is in it.
    pub capture_ms: Option<u64>,
    /// The recognition alone, as the adapter measured it; `None` also when
    /// the adapter found no image to recognize.
    pub recognition_ms: Option<u64>,
    /// The peak RSS of the Tesseract child, as the adapter read it.
    pub child_peak_rss_bytes: Option<u64>,
}
