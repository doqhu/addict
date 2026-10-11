//! The entry commands: `addict install` writes the desktop entry into the
//! applications and the autostart directory, `addict uninstall` removes both.
//! This module owns the choice of the two directories and the check that
//! precedes any write or removal; the files themselves are
//! `addict_platform::desktop`'s.

use std::path::PathBuf;

use addict_core::APP_ID;
use addict_platform::desktop;

/// The applications and the autostart directory, in the order the commands
/// act on them. Both targets are checked here, before either is written or
/// removed, so a refusal leaves both as they were.
fn checked_dirs() -> Result<[PathBuf; 2], String> {
    let dirs = [
        desktop::applications_dir()
            .ok_or("cannot find the applications directory: HOME is not set")?,
        desktop::autostart_dir().ok_or("cannot find the autostart directory: HOME is not set")?,
    ];
    for dir in &dirs {
        // The error text names the path.
        desktop::check(dir, APP_ID).map_err(|e| e.to_string())?;
    }
    Ok(dirs)
}

pub fn install_entries() -> Result<Vec<PathBuf>, String> {
    let exec = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|e| format!("cannot resolve the path of this binary: {e}"))?;
    checked_dirs()?
        .iter()
        .map(|dir| desktop::install(dir, APP_ID, &exec).map_err(|e| e.to_string()))
        .collect()
}

/// The paths that were removed: an entry that was not there is not listed.
pub fn uninstall_entries() -> Result<Vec<PathBuf>, String> {
    let mut removed = Vec::new();
    for dir in &checked_dirs()? {
        removed.extend(desktop::uninstall(dir, APP_ID).map_err(|e| e.to_string())?);
    }
    Ok(removed)
}
