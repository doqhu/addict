//! The entry commands: `addict install` writes the desktop entry into the
//! applications directory and the app icons into the user icon theme, and
//! `addict uninstall` removes them and the login entry an older build wrote
//! into the autostart directory. This module owns the choice of the
//! directories, the icons themselves and the check that precedes any write or
//! removal; the files are `addict_platform::desktop`'s.

use std::path::{Path, PathBuf};

use addict_core::APP_ID;
use addict_platform::desktop;

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
    let exec = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|e| format!("cannot resolve the path of this binary: {e}"))?;
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

/// The paths that were removed: an entry or an icon that was not there is
/// not listed. The login entry of an older build goes too; this only ever
/// removes, and only a file carrying the marker.
pub fn uninstall_entries() -> Result<Vec<PathBuf>, String> {
    let dirs = [
        applications_dir()?,
        desktop::autostart_dir().ok_or("cannot find the autostart directory: HOME is not set")?,
    ];
    for dir in &dirs {
        check_entry(dir)?;
    }
    let icons = checked_icons()?;
    let mut removed = Vec::new();
    for dir in &dirs {
        removed.extend(desktop::uninstall(dir, APP_ID).map_err(|e| e.to_string())?);
    }
    for (size, _) in ICONS {
        removed.extend(desktop::uninstall_icon(&icons, APP_ID, size).map_err(|e| e.to_string())?);
    }
    Ok(removed)
}
