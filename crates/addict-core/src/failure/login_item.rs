//! Why turning login startup on or off did not happen, and what the switch
//! shows after the answer.

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

/// What the Start at login section shows when the window opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginItemShown {
    /// Whether the switch is on.
    pub on: bool,
    /// Whether an older build's login entry is what makes it on.
    pub legacy: bool,
}

/// How a change of the switch ended. It says what the switch then shows; a
/// state saved or not is part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupEnd {
    /// The portal confirmed it and it is saved.
    Done(bool),
    /// The portal was not asked: why, naming a path if there is one.
    NotAsked(String),
    /// The portal did not do it; nothing is saved.
    Failed(StartupFailure),
    /// Enabled, the save failed, and the portal took it back.
    Revoked { save: String },
    /// Enabled, the save failed, and the portal did not take it back.
    NotRevoked {
        save: String,
        revoke: StartupFailure,
    },
    /// Disabled, but the save failed.
    DisabledUnsaved { save: String },
}
