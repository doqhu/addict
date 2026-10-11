//! The product's desktop entry. The host Registry accepts an app id only when
//! a matching `.desktop` file is installed, so registration needs this entry;
//! the same entry in the autostart directory starts the application at login.
//! `install` overwrites and `uninstall` removes only a file `install` wrote,
//! and neither follows a symlink.

use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

/// The line that marks a desktop entry as written by Addict.
pub const MARKER: &str = "X-Addict=true";

/// The user applications directory: `$XDG_DATA_HOME/applications` when the
/// variable is an absolute path, else `$HOME/.local/share/applications`.
pub fn applications_dir() -> Option<PathBuf> {
    applications_dir_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

fn applications_dir_from(data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let data = match data_home.map(PathBuf::from) {
        Some(data) if data.is_absolute() => data,
        // The base directory specification says to ignore a relative value.
        _ => PathBuf::from(home.filter(|home| !home.is_empty())?).join(".local/share"),
    };
    Some(data.join("applications"))
}

/// The user autostart directory: `$XDG_CONFIG_HOME/autostart` when the
/// variable is an absolute path, else `$HOME/.config/autostart`.
pub fn autostart_dir() -> Option<PathBuf> {
    autostart_dir_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

fn autostart_dir_from(config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let config = match config_home.map(PathBuf::from) {
        Some(config) if config.is_absolute() => config,
        // The base directory specification says to ignore a relative value.
        _ => PathBuf::from(home.filter(|home| !home.is_empty())?).join(".config"),
    };
    Some(config.join("autostart"))
}

/// Where the entry for `app_id` lives inside `dir`.
pub fn entry_path(dir: &Path, app_id: &str) -> PathBuf {
    dir.join(format!("{app_id}.desktop"))
}

/// An app id that is safe as a file name and a key-file comment: reverse-DNS,
/// at least two elements of letters, digits, `_` or `-`, none starting with a
/// digit.
fn is_valid_app_id(app_id: &str) -> bool {
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

/// `exec` as one quoted `Exec` argument per the Desktop Entry Specification:
/// a reserved character gets a backslash for the Exec quoting, every backslash
/// is doubled again because Exec is a key-file string, and `%` is doubled so
/// it is not read as a field code.
fn quote_exec(exec: &Path) -> String {
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
    quoted
}

/// The desktop entry text for `app_id`, launching `exec` with no arguments.
pub fn entry_text(app_id: &str, exec: &Path) -> String {
    let exec = quote_exec(exec);
    format!(
        "# Desktop entry of {app_id}, written by `addict install`.\n\
         [Desktop Entry]\n\
         Type=Application\n\
         Name=Addict\n\
         Exec={exec}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         {MARKER}\n"
    )
}

fn refusal(path: &Path, what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{} {what}; left untouched", path.display()),
    )
}

fn with_path(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

/// What stands where the entry for an app id would be, when `install` and
/// `uninstall` may touch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Nothing of that name.
    Absent,
    /// A regular file carrying [`MARKER`].
    Ours,
}

/// Classifies the entry for `app_id` in `dir` without changing anything.
/// Anything of that name that is not a regular file carrying [`MARKER`], a
/// symlink included, is an error: the refusal. Every error names the path.
///
/// `install` and `uninstall` run this themselves; a caller with several
/// targets runs it on all of them first, so that a refusal leaves all as they
/// were.
pub fn check(dir: &Path, app_id: &str) -> io::Result<Target> {
    if !is_valid_app_id(app_id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a valid app id: {app_id:?}"),
        ));
    }
    let path = entry_path(dir, app_id);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() => {
            Err(refusal(&path, "exists and is not a regular file"))
        }
        Ok(_) => {
            // Not readable as text counts as not ours.
            let ours = std::fs::read_to_string(&path)
                .is_ok_and(|text| text.lines().any(|line| line.trim() == MARKER));
            if ours {
                Ok(Target::Ours)
            } else {
                Err(refusal(&path, "exists and was not written by addict"))
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Target::Absent),
        Err(e) => Err(with_path(&path, e)),
    }
}

/// Removes the entry for `app_id` from `dir` and returns its path, or `None`
/// when there was none. Only a regular file carrying [`MARKER`] is removed;
/// anything else of that name, a symlink included, is refused untouched. The
/// directory stays. Every error names the path.
pub fn uninstall(dir: &Path, app_id: &str) -> io::Result<Option<PathBuf>> {
    if check(dir, app_id)? == Target::Absent {
        return Ok(None);
    }
    let path = entry_path(dir, app_id);
    // Unlinking removes the name itself: a symlink swapped in after the check
    // is not followed.
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Some(path)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(with_path(&path, e)),
    }
}

/// Writes the entry for `app_id` into `dir` and returns its path. An existing
/// entry is overwritten only when it is a regular file carrying [`MARKER`];
/// anything else of that name, a symlink included, is refused untouched. Every
/// error names the path.
pub fn install(dir: &Path, app_id: &str, exec: &Path) -> io::Result<PathBuf> {
    check(dir, app_id)?;
    let path = entry_path(dir, app_id);
    std::fs::create_dir_all(dir).map_err(|e| with_path(dir, e))?;
    // O_NOFOLLOW: a symlink swapped in after the check above is not followed.
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .and_then(|mut file| file.write_all(entry_text(app_id, exec).as_bytes()))
        .map_err(|e| with_path(&path, e))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_ID: &str = "dev.addict.Test";

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "addict-platform-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            ));
            std::fs::create_dir(&path).expect("create temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn exec_line(text: &str) -> &str {
        text.lines()
            .find_map(|line| line.strip_prefix("Exec="))
            .expect("Exec line")
    }

    #[test]
    fn applications_dir_prefers_absolute_data_home() {
        assert_eq!(
            applications_dir_from(Some("/data".into()), Some("/home/u".into())),
            Some(PathBuf::from("/data/applications"))
        );
    }

    #[test]
    fn applications_dir_ignores_relative_or_empty_data_home() {
        for data_home in [Some("relative/data".into()), Some("".into()), None] {
            assert_eq!(
                applications_dir_from(data_home, Some("/home/u".into())),
                Some(PathBuf::from("/home/u/.local/share/applications"))
            );
        }
    }

    #[test]
    fn applications_dir_needs_home_without_data_home() {
        assert_eq!(applications_dir_from(None, None), None);
        assert_eq!(applications_dir_from(None, Some("".into())), None);
    }

    #[test]
    fn autostart_dir_prefers_absolute_config_home() {
        assert_eq!(
            autostart_dir_from(Some("/config".into()), Some("/home/u".into())),
            Some(PathBuf::from("/config/autostart"))
        );
    }

    #[test]
    fn autostart_dir_ignores_relative_or_empty_config_home() {
        for config_home in [Some("relative/config".into()), Some("".into()), None] {
            assert_eq!(
                autostart_dir_from(config_home, Some("/home/u".into())),
                Some(PathBuf::from("/home/u/.config/autostart"))
            );
        }
    }

    #[test]
    fn autostart_dir_needs_home_without_config_home() {
        assert_eq!(autostart_dir_from(None, None), None);
        assert_eq!(autostart_dir_from(None, Some("".into())), None);
    }

    #[test]
    fn entry_has_the_agreed_keys_and_marker() {
        let text = entry_text(APP_ID, Path::new("/usr/bin/addict"));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[1..],
            [
                "[Desktop Entry]",
                "Type=Application",
                "Name=Addict",
                "Exec=\"/usr/bin/addict\"",
                "Terminal=false",
                "NoDisplay=true",
                MARKER,
            ]
        );
        assert!(lines[0].starts_with('#') && lines[0].contains(APP_ID));
    }

    #[test]
    fn exec_keeps_a_path_with_spaces_as_one_argument() {
        let text = entry_text(APP_ID, Path::new("/opt/my apps/addict"));
        assert_eq!(exec_line(&text), "\"/opt/my apps/addict\"");
    }

    #[test]
    fn exec_escapes_reserved_characters() {
        let text = entry_text(APP_ID, Path::new("/o\"p`t$/a\\b/100%/addict"));
        assert_eq!(
            exec_line(&text),
            "\"/o\\\\\"p\\\\`t\\\\$/a\\\\\\\\b/100%%/addict\""
        );
    }

    #[test]
    fn install_writes_a_new_entry_and_creates_the_directory() {
        let temp = TempDir::new("new");
        let dir = temp.0.join("share/applications");
        let path = install(&dir, APP_ID, Path::new("/usr/bin/addict")).expect("install");
        assert_eq!(path, dir.join("dev.addict.Test.desktop"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read entry"),
            entry_text(APP_ID, Path::new("/usr/bin/addict"))
        );
    }

    #[test]
    fn install_overwrites_its_own_entry() {
        let temp = TempDir::new("own");
        install(&temp.0, APP_ID, Path::new("/old/addict-with-a-longer-path")).expect("first");
        let path = install(&temp.0, APP_ID, Path::new("/new/addict")).expect("second");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read entry"),
            entry_text(APP_ID, Path::new("/new/addict"))
        );
    }

    #[test]
    fn install_refuses_a_foreign_entry() {
        let temp = TempDir::new("foreign");
        let path = entry_path(&temp.0, APP_ID);
        let foreign = "[Desktop Entry]\nName=Someone else\n";
        std::fs::write(&path, foreign).expect("write foreign entry");
        let error = install(&temp.0, APP_ID, Path::new("/usr/bin/addict")).expect_err("refused");
        assert!(error.to_string().contains(&path.display().to_string()));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), foreign);
    }

    #[test]
    fn install_refuses_a_symlink_even_to_its_own_entry() {
        let temp = TempDir::new("symlink");
        let target = temp.0.join("target.desktop");
        let ours = entry_text(APP_ID, Path::new("/old/addict"));
        std::fs::write(&target, &ours).expect("write target");
        let path = entry_path(&temp.0, APP_ID);
        std::os::unix::fs::symlink(&target, &path).expect("symlink");
        let error = install(&temp.0, APP_ID, Path::new("/new/addict")).expect_err("refused");
        assert!(error.to_string().contains(&path.display().to_string()));
        assert_eq!(std::fs::read_to_string(&target).expect("read"), ours);
        assert!(
            std::fs::symlink_metadata(&path)
                .expect("metadata")
                .is_symlink()
        );
    }

    #[test]
    fn install_refuses_a_dangling_symlink() {
        let temp = TempDir::new("dangling");
        let target = temp.0.join("missing.desktop");
        let path = entry_path(&temp.0, APP_ID);
        std::os::unix::fs::symlink(&target, &path).expect("symlink");
        install(&temp.0, APP_ID, Path::new("/usr/bin/addict")).expect_err("refused");
        assert!(!target.exists());
    }

    #[test]
    fn install_rejects_an_app_id_that_is_not_a_file_name() {
        let temp = TempDir::new("id");
        for app_id in ["../evil", "single", "a..b", "dev.addict/x", "dev.1addict"] {
            let error = install(&temp.0, app_id, Path::new("/usr/bin/addict")).expect_err(app_id);
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
        assert_eq!(std::fs::read_dir(&temp.0).expect("list").count(), 0);
    }

    #[test]
    fn check_tells_absent_from_ours() {
        let temp = TempDir::new("check");
        assert_eq!(check(&temp.0, APP_ID).expect("absent"), Target::Absent);
        // A directory that does not exist holds nothing either.
        let missing = temp.0.join("missing");
        assert_eq!(check(&missing, APP_ID).expect("no dir"), Target::Absent);
        assert!(!missing.exists());
        install(&temp.0, APP_ID, Path::new("/usr/bin/addict")).expect("install");
        assert_eq!(check(&temp.0, APP_ID).expect("ours"), Target::Ours);
    }

    #[test]
    fn check_refuses_a_directory_of_that_name() {
        let temp = TempDir::new("check-dir");
        let path = entry_path(&temp.0, APP_ID);
        std::fs::create_dir(&path).expect("create dir");
        let error = check(&temp.0, APP_ID).expect_err("refused");
        assert!(error.to_string().contains(&path.display().to_string()));
    }

    #[test]
    fn uninstall_removes_its_own_entry_and_keeps_the_directory() {
        let temp = TempDir::new("remove");
        let dir = temp.0.join("autostart");
        let path = install(&dir, APP_ID, Path::new("/usr/bin/addict")).expect("install");
        assert_eq!(
            uninstall(&dir, APP_ID).expect("uninstall"),
            Some(path.clone())
        );
        assert!(!path.exists());
        assert!(dir.is_dir());
    }

    #[test]
    fn uninstall_of_an_absent_entry_is_not_an_error() {
        let temp = TempDir::new("remove-absent");
        assert_eq!(uninstall(&temp.0, APP_ID).expect("absent"), None);
        let missing = temp.0.join("missing");
        assert_eq!(uninstall(&missing, APP_ID).expect("no dir"), None);
        assert!(!missing.exists());
    }

    #[test]
    fn uninstall_refuses_a_foreign_entry() {
        let temp = TempDir::new("remove-foreign");
        let path = entry_path(&temp.0, APP_ID);
        let foreign = "[Desktop Entry]\nName=Someone else\n";
        std::fs::write(&path, foreign).expect("write foreign entry");
        let error = uninstall(&temp.0, APP_ID).expect_err("refused");
        assert!(error.to_string().contains(&path.display().to_string()));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), foreign);
    }

    #[test]
    fn uninstall_refuses_a_symlink_even_to_its_own_entry() {
        let temp = TempDir::new("remove-symlink");
        let target = temp.0.join("target.desktop");
        let ours = entry_text(APP_ID, Path::new("/usr/bin/addict"));
        std::fs::write(&target, &ours).expect("write target");
        let path = entry_path(&temp.0, APP_ID);
        std::os::unix::fs::symlink(&target, &path).expect("symlink");
        let error = uninstall(&temp.0, APP_ID).expect_err("refused");
        assert!(error.to_string().contains(&path.display().to_string()));
        assert_eq!(std::fs::read_to_string(&target).expect("read"), ours);
        assert!(
            std::fs::symlink_metadata(&path)
                .expect("metadata")
                .is_symlink()
        );
    }

    #[test]
    fn uninstall_rejects_an_app_id_that_is_not_a_file_name() {
        let temp = TempDir::new("remove-id");
        let error = uninstall(&temp.0, "../evil").expect_err("rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
