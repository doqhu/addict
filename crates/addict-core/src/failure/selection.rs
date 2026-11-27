//! Why a read of the selection or the clipboard gave no text.

/// Why a read of the selection or the clipboard gave no text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionFailure {
    /// The application has no permission to read the selection.
    NoPermission,
    /// The application in front changed during the read, so the text read
    /// may belong to another one.
    WrongApp,
    /// The selection or the clipboard cannot be read here.
    Unavailable,
    /// The user dismissed the read or the shell honored a cancellation.
    Cancelled,
}
