//! The entry commands: `addict install` writes the desktop entry into the
//! applications and the autostart directory and the app icons into the user
//! icon theme, `addict uninstall` removes all of them. This module owns the
//! choice of the directories, the icons themselves and the check that
//! precedes any write or removal; the files are `addict_platform::desktop`'s.

use std::path::PathBuf;

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

/// The applications and the autostart directory, in the order the commands
/// act on them, and the icons directory. Every target is checked here, before
/// any is written or removed, so a refusal leaves all as they were.
fn checked_dirs() -> Result<([PathBuf; 2], PathBuf), String> {
    let dirs = [
        desktop::applications_dir()
            .ok_or("cannot find the applications directory: HOME is not set")?,
        desktop::autostart_dir().ok_or("cannot find the autostart directory: HOME is not set")?,
    ];
    let icons = desktop::icons_dir().ok_or("cannot find the icons directory: HOME is not set")?;
    for dir in &dirs {
        // The error text names the path.
        desktop::check(dir, APP_ID).map_err(|e| e.to_string())?;
    }
    for (size, _) in ICONS {
        desktop::check_icon(&icons, APP_ID, size).map_err(|e| e.to_string())?;
    }
    Ok((dirs, icons))
}

/// The paths that were written: the entries, then the icons by size.
pub fn install_entries() -> Result<Vec<PathBuf>, String> {
    let exec = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|e| format!("cannot resolve the path of this binary: {e}"))?;
    let (dirs, icons) = checked_dirs()?;
    let mut written = Vec::new();
    for dir in &dirs {
        written.push(desktop::install(dir, APP_ID, &exec).map_err(|e| e.to_string())?);
    }
    for (size, png) in ICONS {
        written.push(desktop::install_icon(&icons, APP_ID, size, png).map_err(|e| e.to_string())?);
    }
    Ok(written)
}

/// The paths that were removed: an entry or an icon that was not there is
/// not listed.
pub fn uninstall_entries() -> Result<Vec<PathBuf>, String> {
    let (dirs, icons) = checked_dirs()?;
    let mut removed = Vec::new();
    for dir in &dirs {
        removed.extend(desktop::uninstall(dir, APP_ID).map_err(|e| e.to_string())?);
    }
    for (size, _) in ICONS {
        removed.extend(desktop::uninstall_icon(&icons, APP_ID, size).map_err(|e| e.to_string())?);
    }
    Ok(removed)
}
