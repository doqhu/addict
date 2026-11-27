//! The outcomes a native file chooser can report.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChoiceFailure {
    /// The chooser supplied no local or document-portal path.
    NotLocal,
    /// The shell cannot run the chooser on this platform.
    Unavailable,
    /// The user dismissed the work or the shell honoured `Cancel`.
    Cancelled,
    /// Everything else, with the shell's detail.
    Failed(String),
}
