//! The probe's own desktop entry. The host Registry accepts an app id only
//! when a matching `.desktop` file exists, so the registered run needs one.
//! The probe writes it, removes it, and touches no file it did not write.

use std::{
    io,
    path::{Path, PathBuf},
};

use crate::record::{Finish, Outcome, Progress};

pub const EXPECTED_INSTALL: &str = "outcome installed: the desktop entry for the app id exists \
     and points at this binary; refused when a file of that name was not written by the probe";
pub const EXPECTED_UNINSTALL: &str = "outcome removed: no desktop entry of the probe remains; \
     refused when the file of that name was not written by the probe";

/// The line that marks a desktop entry as the probe's own.
pub const MARKER: &str = "X-Addict-Probe=true";

/// `~/.local/share/applications`, from `$HOME`.
pub fn applications_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".local/share/applications"))
}

/// An app id that is safe to use as a file name: reverse-DNS, at least two
/// elements, each of letters, digits, `_` or `-`, none starting with a digit.
pub fn is_valid_app_id(app_id: &str) -> bool {
    app_id.len() <= 255
        && app_id.split('.').count() >= 2
        && app_id.split('.').all(|element| {
            !element.is_empty()
                && !element.starts_with(|c: char| c.is_ascii_digit())
                && element
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

pub fn entry_path(dir: &Path, app_id: &str) -> PathBuf {
    dir.join(format!("{app_id}.desktop"))
}

/// The desktop entry text. `exec` is quoted per the Desktop Entry
/// Specification so a path with spaces stays one argument: a reserved
/// character gets a backslash for the Exec quoting, and every backslash is
/// doubled again because Exec is a key-file string.
pub fn entry_text(exec: &Path) -> String {
    let exec = exec.to_string_lossy();
    let mut quoted = String::with_capacity(exec.len() + 2);
    quoted.push('"');
    for c in exec.chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Addict Probe\n\
         Comment=Throwaway entry for the EXP-3 portal probe; remove with addict-probe uninstall-desktop\n\
         Exec={quoted} caps\n\
         Terminal=true\n\
         NoDisplay=true\n\
         {MARKER}\n"
    )
}

fn is_ours(path: &Path) -> io::Result<bool> {
    Ok(std::fs::read_to_string(path)?
        .lines()
        .any(|line| line.trim() == MARKER))
}

pub fn install(dir: &Path, app_id: &str, exec: &Path, progress: &Progress) -> Finish {
    if !is_valid_app_id(app_id) {
        return Finish::with_error(Outcome::Error, format!("not a valid app id: {app_id:?}"));
    }
    let path = entry_path(dir, app_id);
    progress.put("path", &path);
    progress.put("exec", exec);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            let ours = metadata.is_file() && is_ours(&path).unwrap_or(false);
            progress.put("existed", true);
            if !ours {
                return Finish::with_error(
                    Outcome::Refused,
                    format!(
                        "{} exists and was not written by the probe; left untouched",
                        path.display()
                    ),
                );
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => progress.put("existed", false),
        Err(e) => return Finish::with_error(Outcome::Error, format!("{}: {e}", path.display())),
    }
    let written =
        std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, entry_text(exec)));
    match written {
        Ok(()) => Finish::ok(Outcome::Installed),
        Err(e) => Finish::with_error(Outcome::Error, format!("{}: {e}", path.display())),
    }
}

pub fn uninstall(dir: &Path, app_id: &str, progress: &Progress) -> Finish {
    if !is_valid_app_id(app_id) {
        return Finish::with_error(Outcome::Error, format!("not a valid app id: {app_id:?}"));
    }
    let path = entry_path(dir, app_id);
    progress.put("path", &path);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            progress.put("existed", false);
            Finish::ok(Outcome::Removed)
        }
        Err(e) => Finish::with_error(Outcome::Error, format!("{}: {e}", path.display())),
        Ok(metadata) => {
            progress.put("existed", true);
            if !(metadata.is_file() && is_ours(&path).unwrap_or(false)) {
                return Finish::with_error(
                    Outcome::Refused,
                    format!(
                        "{} was not written by the probe; left untouched",
                        path.display()
                    ),
                );
            }
            match std::fs::remove_file(&path) {
                Ok(()) => Finish::ok(Outcome::Removed),
                Err(e) => Finish::with_error(Outcome::Error, format!("{}: {e}", path.display())),
            }
        }
    }
}
