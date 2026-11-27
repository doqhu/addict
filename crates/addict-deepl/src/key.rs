//! The DeepL key source and its Linux file format.
//!
//! The key is the first and only thing in the supplied file. A file that
//! anyone but its owner can use, or that its owner can execute, is refused
//! before its content is read. [`write`] replaces the file with one of mode
//! `0600` that holds a key.

use std::{
    ffi::OsString,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

use tokio::sync::watch;

/// The name of the key file inside the configuration directory.
const KEY_FILE: &str = "deepl.key";

/// The mode bits a key file may have: read and write for its owner.
const ALLOWED_MODE: u32 = 0o600;

/// All the mode bits: the permissions, set-user-id, set-group-id and sticky.
const MODE_BITS: u32 = 0o7777;

/// How many bytes of the file are read. A key is a few dozen characters.
const KEY_LIMIT: usize = 1024;

/// What a key of the Free plan ends with.
const FREE_SUFFIX: &str = ":fx";

/// The key file in the supplied configuration directory.
pub fn key_path(config: Option<&Path>) -> Option<PathBuf> {
    Some(config?.join(KEY_FILE))
}

/// A validated DeepL key. Its `Debug` and `Display` output are redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Validates a key entered by the user, leaving room for the file's newline.
    pub fn new(input: &str) -> Result<Self, WriteError> {
        let key = input.trim();
        key_of(key)
            .filter(|key| key.len() < KEY_LIMIT)
            .map(|key| Self(key.to_owned()))
            .ok_or(WriteError::Invalid)
    }

    /// The key itself, for the `Authorization` header and nothing else.
    pub(crate) fn secret(&self) -> &str {
        &self.0
    }

    /// Whether this is a key of the Free plan, which has its own host.
    pub fn is_free(&self) -> bool {
        self.0.ends_with(FREE_SUFFIX)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<hidden>")
    }
}

/// Why a key could not be loaded. None of these carries the file contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// No configuration directory was available.
    NoDirectory,
    /// There is no such file.
    Missing,
    /// The file is not a regular file or cannot be opened or read.
    Unreadable,
    /// The mode of the file has a bit outside `0600`; its content was not
    /// read.
    TooOpen,
    /// The file holds no key: it is empty or blank, or what it holds cannot
    /// be a key.
    Empty,
}

/// A shared in-memory snapshot. The platform supplies the initial result
/// and installs a new key when it has been saved. Requests wait asynchronously
/// while the initial result is pending.
#[derive(Debug, Clone)]
pub struct KeySource(watch::Sender<Option<Result<Secret, KeyError>>>);

impl KeySource {
    pub fn new(initial: Result<Secret, KeyError>) -> Self {
        Self(watch::Sender::new(Some(initial)))
    }

    /// Starts a source whose initial load is still in progress.
    pub fn pending() -> Self {
        Self(watch::Sender::new(None))
    }

    /// Publishes the startup result if the source is still pending.
    pub fn initialize(&self, initial: Result<Secret, KeyError>) {
        self.0.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(initial);
            true
        });
    }

    /// The current key or refusal, or `None` during the initial load.
    pub fn snapshot(&self) -> Option<Result<Secret, KeyError>> {
        self.0.borrow().clone()
    }

    /// Waits for a snapshot without blocking the caller's thread.
    pub async fn ready(&self) -> Result<Secret, KeyError> {
        let mut updates = self.0.subscribe();
        updates
            .wait_for(Option::is_some)
            .await
            .expect("the source retains its sender")
            .clone()
            .expect("the source is initialized")
    }

    /// Publishes a newly saved key.
    pub fn replace(&self, key: Secret) {
        self.0.send_replace(Some(Ok(key)));
    }

    /// Publishes the outcome of a key change. The result holds no key text
    /// when it is an error.
    pub fn replace_result(&self, result: Result<Secret, KeyError>) {
        self.0.send_replace(Some(result));
    }
}

/// Reads the key from the file at `path`. The mode is checked on the opened
/// file, before anything is read from it. White space around the key is not
/// part of it.
pub fn read(path: &Path) -> Result<Secret, KeyError> {
    let missing = |error: io::Error| match error.kind() {
        io::ErrorKind::NotFound => KeyError::Missing,
        _ => KeyError::Unreadable,
    };
    // Before opening: opening a pipe would wait for its writer.
    if !std::fs::metadata(path).map_err(missing)?.is_file() {
        return Err(KeyError::Unreadable);
    }
    let file = File::open(path).map_err(missing)?;
    let metadata = file.metadata().map_err(|_| KeyError::Unreadable)?;
    if !metadata.is_file() {
        return Err(KeyError::Unreadable);
    }
    if metadata.permissions().mode() & MODE_BITS & !ALLOWED_MODE != 0 {
        return Err(KeyError::TooOpen);
    }
    let mut bytes = Vec::new();
    file.take((KEY_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| KeyError::Unreadable)?;
    if bytes.len() > KEY_LIMIT {
        return Err(KeyError::Empty);
    }
    let content = std::str::from_utf8(&bytes).map_err(|_| KeyError::Unreadable)?;
    let key = key_of(content).ok_or(KeyError::Empty)?;
    Ok(Secret(key.to_owned()))
}

/// The key that the content of a key file holds: the content without the
/// white space around it, when the content fits in [`KEY_LIMIT`] bytes and
/// the key is one printable ASCII word. Anything else cannot be sent in a
/// header. [`read`] and [`Secret::new`] share this rule.
fn key_of(content: &str) -> Option<&str> {
    let key = content.trim();
    (!key.is_empty() && content.len() <= KEY_LIMIT && key.bytes().all(|c| c.is_ascii_graphic()))
        .then_some(key)
}

/// Why a key was not written. Neither holds anything of the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// What was given is not a key: blank, longer than a key file may be, or
    /// not one printable ASCII word. Nothing was written.
    Invalid,
    /// The directory or the file could not be made; the file is as it was.
    Io(io::ErrorKind),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => write!(
                f,
                "a DeepL key is one word of printable ASCII, at most {} characters",
                KEY_LIMIT - 1
            ),
            Self::Io(kind) => write!(f, "{kind}"),
        }
    }
}

impl std::error::Error for WriteError {}

/// Tells apart the temporary files of the writes of one process.
static NEXT_TEMP: AtomicU32 = AtomicU32::new(0);

/// Writes a normalized `key` and a newline to the key
/// file at `path`, which then has mode `0600`. Its directory is made when
/// missing. The key goes to a new file in that directory, which is created
/// with mode `0600`, so that no one else can read it at any moment, and is
/// then renamed over `path`: a reader sees the old file or the new one,
/// never a part of it, and a file that was refused for its mode is replaced.
/// Returns the persisted secret. On a failure the temporary file is removed
/// and `path` is as it was.
pub fn write(path: &Path, key: Secret) -> Result<Secret, WriteError> {
    // An existing file without a newline can hold one extra character.
    if key.0.len() >= KEY_LIMIT {
        return Err(WriteError::Invalid);
    }
    let io = |error: io::Error| WriteError::Io(error.kind());
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .ok_or(WriteError::Io(io::ErrorKind::InvalidInput))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(io)?;
    let mut temp_name = OsString::from(".");
    temp_name.push(name);
    temp_name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = dir.join(temp_name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(ALLOWED_MODE)
        .open(&temp)
        .map_err(io)?;
    let written = (|| {
        // The umask may have taken bits away; the mode is exactly 0600.
        file.set_permissions(fs::Permissions::from_mode(ALLOWED_MODE))?;
        file.write_all(key.0.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    written.map_err(|error| {
        let _ = fs::remove_file(&temp);
        io(error)
    })?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn a_free_key_is_known_by_its_suffix() {
        assert!(Secret::new("abc:fx").unwrap().is_free());
        for pro in ["abc", "abc:fx1", "fx", "abc:FX"] {
            assert!(!Secret::new(pro).unwrap().is_free(), "{pro}");
        }
    }

    #[test]
    fn the_debug_output_of_a_key_does_not_hold_it() {
        let key = Secret::new("secret-key:fx").unwrap();
        let shown = format!("{key:?} {key}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(!shown.contains("fx"), "{shown}");
    }
}
