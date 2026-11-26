//! Why turning login startup on or off did not happen.

/// Why the portal did not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupFailure {
    /// The portal answered with code 1 or 2, or answered success with the
    /// other state than the one asked for.
    Refused,
    /// The bus, the portal or its Background interface is not there.
    Unavailable,
    /// The limit passed with no answer.
    NoAnswer,
    /// A step failed; the text names it with the portal's own error.
    Failed(String),
}
