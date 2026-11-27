//! What the Settings window says about its state: the startup switch, the
//! cache and the DeepL key, and the shortcut bindings. The controller makes
//! these texts; the shells only put them on screen. Every text here is pure,
//! and none of them holds the key.

use addict_core::failure::{
    Ended,
    key_store::KeyStoreFailure,
    login_item::{StartupEnd, StartupFailure},
};
use addict_deepl::key::{KeyError, WriteError};

use super::{Binding, failure_reason};
use crate::cache::Footprint;

/// The line under a switch that a legacy entry keeps on.
pub const STARTUP_LEGACY: &str = "An older login entry is active and may open the window at \
     login. Turn this off and on again to replace it.";

/// The line when a change passed its limit, which covers the whole job: the
/// portal may have acted already, or may still.
pub const STARTUP_TIMED_OUT: &str = "The Background portal did not answer in time, and the \
     change may still take effect. Turn this off and on again to repair it.";

/// The line when the state could not be read in time.
pub const STARTUP_UNREAD: &str = "The saved startup state could not be read.";

/// The line when the runner of the startup jobs is gone.
pub const STARTUP_NO_RUNNER: &str = "Startup cannot be changed: the runner is not available.";

/// The line for a failure of the portal.
fn startup_failure_text(failure: &StartupFailure) -> String {
    format!("Startup was not changed: {}.", failure_reason(failure))
}

/// What the switch shows, and the line under it, after a change that started
/// from the confirmed state `last`. Never on without a confirmation.
pub fn startup_end_text(end: &StartupEnd, last: bool) -> (bool, String) {
    match end {
        StartupEnd::Done(on) => (*on, String::new()),
        StartupEnd::NotAsked(reason) => (last, format!("Startup was not changed: {reason}.")),
        StartupEnd::Failed(failure) => (last, startup_failure_text(failure)),
        StartupEnd::Revoked { save } => (
            false,
            format!("Startup was turned back off, as its state could not be saved: {save}."),
        ),
        StartupEnd::NotRevoked { save, revoke } => (
            false,
            format!(
                "Startup could not be saved ({save}) and could not be turned back off ({}): \
                 addict may still start at login. Turn this on and off again to repair it.",
                failure_reason(revoke)
            ),
        ),
        StartupEnd::DisabledUnsaved { save } => (
            false,
            format!(
                "Startup is off, but this could not be saved: {save}. This switch may show on \
                 next time."
            ),
        ),
    }
}

/// The line when the portal could not run the change at all.
pub fn startup_unfinished_text() -> String {
    "Startup was not changed: the request did not run.".to_owned()
}

/// Where the size of the cache stands, as the window says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    /// The size is being read.
    Checking,
    /// The cache holds this.
    Read(Footprint),
    /// The size could not be read: no runner, a timeout or a panic.
    Unknown,
}

/// The line of the size of the cache. No wildcard arm: a new state does not
/// compile until it is given its line.
pub fn cache_text(state: CacheState) -> String {
    match state {
        CacheState::Checking => "Checking the cache…".to_owned(),
        CacheState::Read(footprint) if footprint.rows == 0 => "The cache is empty.".to_owned(),
        CacheState::Read(footprint) => format!("The cache holds {}.", size_text(footprint.bytes)),
        CacheState::Unknown => "The size of the cache could not be read.".to_owned(),
    }
}

/// `bytes` in decimal units, one decimal from a kilobyte up: `1.2 MB`.
pub fn size_text(bytes: u64) -> String {
    const KB: u64 = 1_000;
    const MB: u64 = 1_000_000;
    // Integer arithmetic: tenths, rounded half up.
    let tenths = |unit: u64| (bytes * 10 + unit / 2) / unit;
    if bytes < KB {
        format!("{bytes} bytes")
    } else if tenths(KB) < 10_000 {
        let tenths = tenths(KB);
        format!("{}.{} kB", tenths / 10, tenths % 10)
    } else {
        let tenths = tenths(MB);
        format!("{}.{} MB", tenths / 10, tenths % 10)
    }
}

/// The line after a clear of the cache: nothing on success, else why the
/// cache was not cleared.
pub fn clear_text(result: Result<(), &str>) -> String {
    match result {
        Ok(()) => String::new(),
        Err(reason) => format!("The cache was not cleared: {reason}"),
    }
}

/// Where the DeepL key stands, as the window says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// The startup read is still in progress.
    Checking,
    /// The source holds a key of the Free plan.
    Free,
    /// The source holds a key of the Pro plan.
    Pro,
    /// The key could not be loaded.
    Refused(KeyError),
}

impl KeyState {
    /// Projects the active key into its plan or refusal for display.
    pub fn of(source: &addict_deepl::key::KeySource) -> Self {
        match source.snapshot() {
            None => Self::Checking,
            Some(Ok(key)) if key.is_free() => Self::Free,
            Some(Ok(_)) => Self::Pro,
            Some(Err(error)) => Self::Refused(error),
        }
    }
}

/// The line of a key state. No wildcard arm: a new state does not compile
/// until it is given its line.
pub fn key_text(state: KeyState) -> &'static str {
    match state {
        KeyState::Checking => "Checking…",
        KeyState::Refused(KeyError::NoDirectory) => {
            "No place for the key file: set an absolute XDG_CONFIG_HOME or HOME."
        }
        KeyState::Free => "A DeepL key of the Free plan is set.",
        KeyState::Pro => "A DeepL key of the Pro plan is set.",
        KeyState::Refused(KeyError::Missing) => "There is no key file.",
        KeyState::Refused(KeyError::Unreadable) => "The key file cannot be read.",
        KeyState::Refused(KeyError::TooOpen) => {
            "The key file must have permissions 0600; it was not read."
        }
        KeyState::Refused(KeyError::Empty) => "The key file holds no key.",
    }
}

/// The line after a save of the key: nothing on success, else why the key
/// was not saved. The reason never holds the key.
pub fn save_text(result: Result<(), &str>) -> String {
    match result {
        Ok(()) => String::new(),
        Err(reason) => format!("The key was not saved: {reason}."),
    }
}

/// Why a key is not saved when the environment names no configuration
/// directory.
pub const NO_KEY_PLACE: &str =
    "there is no place for the key file: set an absolute XDG_CONFIG_HOME or HOME";

/// The key's line when the write of the key file did not answer in time:
/// it may still have been written, so nothing is claimed.
pub const SAVE_UNANSWERED: &str =
    "The save did not answer in time; the line above shows the active key state.";

/// The hint line that names where the key is saved.
pub fn saved_in_text(key_file: Option<&std::path::Path>) -> String {
    key_file.map_or_else(
        || "There is no place to save the key.".to_owned(),
        |path| format!("Saved in {}", path.display()),
    )
}

/// The line for one outcome of a store of the key. No key text is in it.
pub fn key_store_text(failure: &Ended<KeyStoreFailure>) -> String {
    match failure {
        Ended::Shell(KeyStoreFailure::Invalid) => {
            let detail = WriteError::Invalid.to_string();
            save_text(Err(&detail))
        }
        Ended::Shell(KeyStoreFailure::NoPlace) => save_text(Err(NO_KEY_PLACE)),
        Ended::Shell(KeyStoreFailure::Unavailable) => save_text(Err("the runner is not available")),
        Ended::Shell(KeyStoreFailure::Cancelled) => String::new(),
        Ended::Shell(KeyStoreFailure::Failed(detail)) => save_text(Err(detail)),
        Ended::DeadlinePassed => SAVE_UNANSWERED.to_owned(),
        Ended::Unfinished => save_text(Err("the write did not run")),
    }
}

/// What the window says about the shortcut: the trigger the portal bound, or
/// where the attempt stands.
pub fn trigger_text(binding: &Binding, trigger: Option<&str>) -> String {
    match (binding, trigger) {
        (Binding::Bound, Some(trigger)) if !trigger.trim().is_empty() => trigger.to_owned(),
        (Binding::Bound, _) => "Unassigned. Choose a key in Configure shortcuts…".to_owned(),
        (Binding::Attempting, _) => "Binding the shortcut…".to_owned(),
        (Binding::NotBound { detail }, _) => format!("Not bound: {detail}"),
    }
}

/// The line after the portal answered the request for its dialog.
pub fn change_text(answer: Option<&str>) -> String {
    match answer {
        None => {
            "The system's dialog configures both shortcuts. Assignments update here immediately."
                .to_owned()
        }
        Some(message) => format!("The shortcut cannot be changed from here: {message}"),
    }
}

/// Configure shortcuts needs a session, and no new binding running.
pub fn change_sensitive(bound: bool, rebinding: bool) -> bool {
    bound && !rebinding
}

/// The line while a new binding has the system's dialog open.
pub const REBINDING: &str = "Choose the keys in the system's dialog.";

/// The line when the window is shown again: empty, unless a new binding is
/// running, which is why the button is off.
pub fn reopened_status(rebinding: bool) -> &'static str {
    if rebinding { REBINDING } else { "" }
}

/// The line after a new binding ended: what the worker said, or that the keys
/// that were chosen are in place.
pub fn rebind_text(message: Option<&str>) -> String {
    message.map_or_else(|| "The new keys are set.".to_owned(), str::to_owned)
}

/// The line when Configure shortcuts was pressed without a bound session.
pub const CHANGE_UNBOUND: &str =
    "Shortcut configuration needs a portal session. Retry the connection first.";

/// The class of the block of a key state.
pub fn key_tone(state: KeyState) -> &'static str {
    match state {
        KeyState::Checking => "pending",
        KeyState::Free | KeyState::Pro => "good",
        KeyState::Refused(_) => "bad",
    }
}

/// What the empty key field asks for: a set key is replaced, not entered.
pub fn key_placeholder(state: KeyState) -> &'static str {
    match key_tone(state) {
        "good" => "Paste a new key to replace it",
        _ => "Paste a DeepL key",
    }
}

/// What takes the keyboard focus when a capture ended without text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailedEndFocus {
    Capture,
    Retry,
}

/// The focus after a capture without text under `binding`: Capture, for the
/// retry, or Retry when it is offered. No wildcard arm: a new state does not
/// compile until it is given its focus.
pub fn failed_end_focus(binding: &Binding) -> FailedEndFocus {
    match binding {
        Binding::NotBound { .. } => FailedEndFocus::Retry,
        Binding::Attempting | Binding::Bound => FailedEndFocus::Capture,
    }
}
