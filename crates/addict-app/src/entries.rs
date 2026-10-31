//! The entry commands: `addict install` writes the desktop entry into the
//! applications directory and the app icons into the user icon theme, and
//! `addict uninstall` asks the Background portal to turn off login startup,
//! and then removes them and the saved startup state. It never touches the
//! autostart directory itself: the portal removes that entry, while the
//! launcher that gives it the identity is still there. This module owns the
//! choice of the directories, the icons themselves and the check that precedes
//! any write or removal; the files are `addict_platform::desktop`'s.

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use addict_core::APP_ID;
use addict_deepl::key;
use addict_platform::{
    desktop::{self, LoginEntry, Target},
    startup::{self, StartupFailure},
};

/// The file of the last startup state the portal confirmed, in the
/// configuration directory.
const STARTUP_FILE: &str = "login-startup";

/// How long the portal may take over one request. It can be waiting for the
/// user to answer its dialog.
pub const STARTUP_LIMIT: Duration = Duration::from_secs(60);

/// Where the last confirmed startup state is kept, or `None` when the
/// environment names no configuration directory.
pub fn startup_file() -> Option<PathBuf> {
    Some(key::config_dir()?.join(STARTUP_FILE))
}

/// What stands at the login entry, outside the sandbox. A foreign file or a
/// symlink is the refusal, naming the path.
pub fn login_entry() -> Result<LoginEntry, String> {
    desktop::login_entry(&autostart_dir()?, APP_ID).map_err(|e| e.to_string())
}

/// Refuses, outside the sandbox, when the launcher is not installed: the
/// portal knows a host app only by its desktop entry, and would not register
/// it.
pub fn check_launcher_installed() -> Result<(), String> {
    match desktop::check_launcher(&applications_dir()?, APP_ID).map_err(|e| e.to_string())? {
        Target::Ours => Ok(()),
        Target::Absent => {
            Err("the launcher is not installed: run `addict install` first".to_owned())
        }
    }
}

/// Why the portal did not change startup, as a clause.
pub fn failure_reason(failure: &StartupFailure) -> String {
    match failure {
        StartupFailure::Refused => "the Background portal refused".to_owned(),
        StartupFailure::Unavailable => "the Background portal is not available".to_owned(),
        StartupFailure::NoAnswer => "the Background portal did not answer in time".to_owned(),
        StartupFailure::Failed(text) => text.clone(),
    }
}

/// The app icon by size in pixels, from `icons/make-icons.py`. The binary
/// carries them so that the install works from wherever it is run.
const ICONS: [(u32, &[u8]); 4] = [
    (64, include_bytes!("../icons/64.png")),
    (128, include_bytes!("../icons/128.png")),
    (256, include_bytes!("../icons/256.png")),
    (512, include_bytes!("../icons/512.png")),
];

fn applications_dir() -> Result<PathBuf, String> {
    desktop::applications_dir()
        .ok_or_else(|| "cannot find the applications directory: HOME is not set".to_owned())
}

fn autostart_dir() -> Result<PathBuf, String> {
    desktop::autostart_dir()
        .ok_or_else(|| "cannot find the autostart directory: HOME is not set".to_owned())
}

/// Checks the entry in `dir`. The error text names the path.
fn check_entry(dir: &Path) -> Result<(), String> {
    desktop::check(dir, APP_ID)
        .map(drop)
        .map_err(|e| e.to_string())
}

/// The icons directory, with every icon checked. Every target of a command is
/// checked before any is written or removed, so a refusal leaves all as they
/// were.
fn checked_icons() -> Result<PathBuf, String> {
    let icons = desktop::icons_dir().ok_or("cannot find the icons directory: HOME is not set")?;
    for (size, _) in ICONS {
        desktop::check_icon(&icons, APP_ID, size).map_err(|e| e.to_string())?;
    }
    Ok(icons)
}

/// The paths that were written: the entry, then the icons by size. The
/// autostart directory is not a target: only the Background portal may manage
/// it.
pub fn install_entries() -> Result<Vec<PathBuf>, String> {
    let exec = startup::canonical_exe()?;
    let applications = applications_dir()?;
    check_entry(&applications)?;
    let icons = checked_icons()?;
    let mut written = Vec::new();
    written.push(desktop::install(&applications, APP_ID, &exec).map_err(|e| e.to_string())?);
    for (size, png) in ICONS {
        written.push(desktop::install_icon(&icons, APP_ID, size, png).map_err(|e| e.to_string())?);
    }
    Ok(written)
}

/// Whether `uninstall` must ask the portal to turn off startup. In the sandbox
/// the entry cannot be seen, so the saved state decides; outside it, the entry
/// does.
fn must_disable(sandboxed: bool, saved: Option<bool>, entry: LoginEntry) -> bool {
    if sandboxed {
        saved == Some(true)
    } else {
        entry != LoginEntry::None
    }
}

/// Asks the portal to turn off startup, on behalf of `uninstall`: anything but
/// a confirmed answer is the refusal of the whole command. `entry` names what
/// stays behind.
fn disable_startup(entry: &str) -> Result<(), String> {
    let refused = |reason: &str| {
        format!("{reason}; nothing was removed, and the login entry is still at {entry}")
    };
    startup::request_blocking(APP_ID, false, &[], STARTUP_LIMIT)
        .map_err(|failure| refused(&failure_reason(&failure)))
}

/// Refuses a state file that a plain unlink would not remove.
fn check_startup_file(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Err(format!(
            "{} exists and is not a regular file; left untouched",
            path.display()
        )),
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
        _ => Ok(()),
    }
}

fn remove_startup_file(path: &Path) -> Result<Option<PathBuf>, String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(Some(path.to_owned())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The paths that were removed: an entry or an icon that was not there is not
/// listed. Everything is checked first, and a login entry of ours goes through
/// the portal before any file goes, as the portal needs the launcher to know
/// this application. If the portal does not confirm, nothing is removed.
pub fn uninstall_entries() -> Result<Vec<PathBuf>, String> {
    let applications = applications_dir()?;
    let launcher = desktop::check_launcher(&applications, APP_ID).map_err(|e| e.to_string())?;
    let sandboxed = startup::is_sandboxed();
    // The sandbox has its own, empty, autostart directory.
    let entry = if sandboxed {
        LoginEntry::None
    } else {
        login_entry()?
    };
    let icons = checked_icons()?;
    let state = startup_file();
    if let Some(state) = &state {
        check_startup_file(state)?;
    }
    let saved = state.as_deref().and_then(startup::read_saved);

    let mut login = None;
    if must_disable(sandboxed, saved, entry) {
        if sandboxed {
            disable_startup(&format!("~/.config/autostart/{APP_ID}.desktop"))?;
        } else {
            let path = desktop::entry_path(&autostart_dir()?, APP_ID);
            if launcher == Target::Absent {
                return Err(format!(
                    "the Background portal needs the launcher, which is not installed; nothing \
                     was removed, and the login entry is still at {}: run `addict install` and \
                     then `addict uninstall`",
                    path.display()
                ));
            }
            disable_startup(&path.display().to_string())?;
            // A success with the file still there would be a silent start at
            // the next login.
            if login_entry()? != LoginEntry::None {
                return Err(format!(
                    "the Background portal answered success but left the entry; nothing was \
                     removed, and the login entry is still at {}",
                    path.display()
                ));
            }
            login = Some(path);
        }
    }

    let mut removed = Vec::new();
    removed.extend(desktop::uninstall(&applications, APP_ID).map_err(|e| e.to_string())?);
    removed.extend(login);
    for (size, _) in ICONS {
        removed.extend(desktop::uninstall_icon(&icons, APP_ID, size).map_err(|e| e.to_string())?);
    }
    if let Some(state) = &state {
        removed.extend(remove_startup_file(state)?);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use addict_platform::desktop::LoginEntry;

    use super::must_disable;

    #[test]
    fn outside_the_sandbox_the_entry_decides() {
        assert!(!must_disable(false, Some(true), LoginEntry::None));
        assert!(must_disable(false, Some(false), LoginEntry::Legacy));
        assert!(must_disable(false, None, LoginEntry::Portal));
    }

    #[test]
    fn in_the_sandbox_the_saved_state_decides() {
        assert!(must_disable(true, Some(true), LoginEntry::None));
        assert!(!must_disable(true, Some(false), LoginEntry::None));
        assert!(!must_disable(true, None, LoginEntry::None));
    }
}
