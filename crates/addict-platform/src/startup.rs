//! Login startup through the Background portal: the one way addict turns it
//! on or off, sandboxed or not. Nothing here touches the autostart directory;
//! the portal owns that file.
//!
//! The portal offers no query, so what it last confirmed is kept in a small
//! file of the caller's choice, read with [`read_saved`] and written with
//! [`save`].

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use ashpd::desktop::background::{BackgroundProxy, BackgroundRequestOptions};

use crate::portal::{self, PortalFailure};

/// What the user is told the request is for, when the portal asks them.
const REASON: &str = "Start Addict silently when you log in";

/// The flag that starts addict without its window.
const BACKGROUND_FLAG: &str = "--background";

/// The name of the program on `PATH` inside the sandbox.
const SANDBOX_PROGRAM: &str = "addict";

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

impl From<PortalFailure> for StartupFailure {
    fn from(failure: PortalFailure) -> Self {
        match failure {
            PortalFailure::Cancelled | PortalFailure::Ended => Self::Refused,
            PortalFailure::Unavailable => Self::Unavailable,
            PortalFailure::Failed(text) => Self::Failed(text),
        }
    }
}

/// Whether this process runs in a sandbox, where the autostart directory of
/// the host cannot be read and the sandbox's own path means nothing to it.
pub fn is_sandboxed() -> bool {
    ashpd::is_sandboxed()
}

/// Asks the portal, as `app_id` on a fresh connection, to enable or disable
/// startup, and gives the `autostart` value it answered. `command` is the
/// relaunch command of an enable; a disable sends none, as the portal reads it
/// only for an enable and this binary's path may be gone by then. It has no
/// limit of its own; the caller bounds the wait.
pub async fn request(
    app_id: &str,
    enable: bool,
    command: &[String],
) -> Result<bool, StartupFailure> {
    let connection = portal::registered_connection(app_id).await?;
    let proxy = BackgroundProxy::with_connection(connection)
        .await
        .map_err(|e| portal::classify("Background portal", &e))?;
    let options = BackgroundRequestOptions::default()
        .set_reason(REASON)
        .set_auto_start(enable);
    let options = if enable {
        options.set_command(command)
    } else {
        options
    };
    let answer = proxy
        .request_background(None, options)
        .await
        .and_then(|request| request.response())
        .map_err(|e| portal::classify("background request", &e))?;
    Ok(answer.auto_start())
}

/// Whether an answer of [`request`] confirms what was asked: a success with
/// the other state is a refusal.
pub fn confirmed(enable: bool, answered: bool) -> Result<(), StartupFailure> {
    if answered == enable {
        Ok(())
    } else {
        Err(StartupFailure::Refused)
    }
}

/// [`request`] and [`confirmed`] for a caller with no runtime, within `limit`.
pub fn request_blocking(
    app_id: &str,
    enable: bool,
    command: &[String],
    limit: Duration,
) -> Result<(), StartupFailure> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| StartupFailure::Failed(format!("runtime: {e}")))?;
    let answered = runtime
        .block_on(async { tokio::time::timeout(limit, request(app_id, enable, command)).await })
        .map_err(|_| StartupFailure::NoAnswer)??;
    confirmed(enable, answered)
}

/// The relaunch command of this process, for an enable. Unresolvable is an
/// error, and then the portal is not to be asked.
pub fn relaunch_command() -> Result<Vec<String>, String> {
    if is_sandboxed() {
        return Ok(relaunch_in_sandbox());
    }
    let exe = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|e| format!("cannot resolve the path of this binary: {e}"))?;
    relaunch_of(&exe)
}

fn relaunch_in_sandbox() -> Vec<String> {
    vec![SANDBOX_PROGRAM.to_owned(), BACKGROUND_FLAG.to_owned()]
}

fn relaunch_of(exe: &Path) -> Result<Vec<String>, String> {
    Ok(vec![quote_for_exec(exe)?, BACKGROUND_FLAG.to_owned()])
}

/// `exe` as one quoted argument of an `Exec` value. The portal joins the items
/// of the command with spaces, unquoted, and stores the result as a key-file
/// string, which does the key-file escaping: only the Exec level is done here.
fn quote_for_exec(exe: &Path) -> Result<String, String> {
    let text = exe
        .to_str()
        .filter(|text| !text.contains(char::is_control))
        .ok_or_else(|| {
            format!(
                "the path of this binary cannot stand in a desktop entry: {}",
                exe.display()
            )
        })?;
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

/// The content of the saved state.
const ENABLED: &str = "enabled";
const DISABLED: &str = "disabled";

/// What was last confirmed: `None` with no file, or with one that is not a
/// regular file holding a state.
pub fn read_saved(path: &Path) -> Option<bool> {
    // Before opening: opening a pipe would wait for its writer.
    if !std::fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    match std::fs::read_to_string(path).ok()?.trim() {
        ENABLED => Some(true),
        DISABLED => Some(false),
        _ => None,
    }
}

/// Saves what the portal confirmed: a whole new file that replaces the name,
/// so a symlink there is not followed. The directory is created. Every error
/// names the path.
pub fn save(path: &Path, enabled: bool) -> io::Result<()> {
    let named = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", path.display()));
    let dir = path.parent().ok_or_else(|| {
        named(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path has no directory",
        ))
    })?;
    std::fs::create_dir_all(dir).map_err(named)?;
    let temporary = temporary_beside(path);
    let content = if enabled { ENABLED } else { DISABLED };
    let written = std::fs::write(&temporary, format!("{content}\n"))
        .and_then(|()| std::fs::rename(&temporary, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(named(e));
    }
    Ok(())
}

fn temporary_beside(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "addict-startup-{tag}-{}-{}",
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

    #[test]
    fn the_sandbox_command_is_the_program_on_path() {
        assert_eq!(relaunch_in_sandbox(), ["addict", "--background"]);
    }

    #[test]
    fn a_native_path_is_one_quoted_argument() {
        assert_eq!(
            relaunch_of(Path::new("/a b/addict")).expect("command"),
            ["\"/a b/addict\"", "--background"]
        );
        assert_eq!(
            relaunch_of(Path::new("/usr/bin/addict")).expect("command"),
            ["\"/usr/bin/addict\"", "--background"]
        );
    }

    #[test]
    fn reserved_characters_get_the_exec_quoting_only() {
        assert_eq!(
            quote_for_exec(Path::new("/o\"p`t$/a\\b/100%/addict")).expect("quoted"),
            "\"/o\\\"p\\`t\\$/a\\\\b/100%%/addict\""
        );
    }

    #[test]
    fn a_path_a_desktop_entry_cannot_hold_is_refused() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        let error = relaunch_of(Path::new("/a\nb/addict")).expect_err("newline");
        assert!(error.contains("cannot stand"), "{error}");
        relaunch_of(Path::new(OsStr::from_bytes(b"/a\xff/addict"))).expect_err("not UTF-8");
    }

    #[test]
    fn only_the_state_asked_for_is_confirmed() {
        assert_eq!(confirmed(true, true), Ok(()));
        assert_eq!(confirmed(false, false), Ok(()));
        assert_eq!(confirmed(true, false), Err(StartupFailure::Refused));
        assert_eq!(confirmed(false, true), Err(StartupFailure::Refused));
    }

    #[test]
    fn both_refusal_codes_are_refusals() {
        assert_eq!(
            StartupFailure::from(PortalFailure::Cancelled),
            StartupFailure::Refused
        );
        assert_eq!(
            StartupFailure::from(PortalFailure::Ended),
            StartupFailure::Refused
        );
    }

    #[test]
    fn the_saved_state_reads_back_and_creates_its_directory() {
        let temp = TempDir::new("save");
        let path = temp.0.join("addict/login-startup");
        assert_eq!(read_saved(&path), None);
        save(&path, true).expect("save");
        assert_eq!(read_saved(&path), Some(true));
        save(&path, false).expect("save");
        assert_eq!(read_saved(&path), Some(false));
        assert_eq!(
            std::fs::read_dir(path.parent().expect("parent"))
                .expect("list")
                .count(),
            1,
            "no temporary file stays"
        );
    }

    #[test]
    fn unknown_content_or_a_directory_is_no_state() {
        let temp = TempDir::new("unknown");
        let path = temp.0.join("login-startup");
        std::fs::write(&path, "perhaps\n").expect("write");
        assert_eq!(read_saved(&path), None);
        std::fs::remove_file(&path).expect("remove");
        std::fs::create_dir(&path).expect("dir");
        assert_eq!(read_saved(&path), None);
    }

    #[test]
    fn a_failed_save_names_the_path_and_leaves_no_temporary() {
        let temp = TempDir::new("fail");
        let path = temp.0.join("login-startup");
        std::fs::create_dir(&path).expect("a directory in its place");
        let error = save(&path, true).expect_err("refused");
        assert!(error.to_string().contains(&path.display().to_string()));
        assert_eq!(std::fs::read_dir(&temp.0).expect("list").count(), 1);
    }

    #[test]
    fn a_save_replaces_a_symlink_without_following_it() {
        let temp = TempDir::new("link");
        let target = temp.0.join("target");
        std::fs::write(&target, "kept").expect("write");
        let path = temp.0.join("login-startup");
        std::os::unix::fs::symlink(&target, &path).expect("symlink");
        assert_eq!(read_saved(&path), None);
        save(&path, true).expect("save");
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "kept");
        assert_eq!(read_saved(&path), Some(true));
    }
}
