//! Why a screen capture gave no snapshot.

/// Why a capture gave no snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFailure {
    /// The application has no permission to capture the screen.
    NoPermission,
    /// The user dismissed the picker or the shell honored a cancellation.
    Cancelled,
    /// The portal answered with code 2: the request ended otherwise. On
    /// GNOME this is also what Esc gives; no cause is claimed.
    Ended,
    /// The bus, the portal or its Screenshot interface is not there.
    Unavailable,
    /// A step failed; the text names it with the portal's own error.
    Failed(String),
    /// The URI is not a local file, or no file is behind it.
    NoImage,
}
