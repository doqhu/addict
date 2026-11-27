//! Timings measured by the capture and recognition adapters.

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
