//! The key file: where it is, who may read it and what it holds.
//!
//! The key is the first and only thing of the file,
//! `$XDG_CONFIG_HOME/addict/deepl.key`, or `$HOME/.config/addict/deepl.key`
//! when the variable is unset or relative. That directory, [`config_dir`],
//! is the product's one configuration directory: the other files of the
//! product are found by the same rule. A file that anyone but its owner
//! can use, or that its owner can execute, is refused before its content is
//! read. [`write`] replaces the file with one of mode `0600` that holds a
//! key, and nothing else of the product writes it.

use std::{
    ffi::OsString,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

/// The name of the key file inside the configuration directory.
const KEY_FILE: &str = "deepl.key";

/// The mode bits a key file may have: read and write for its owner.
const ALLOWED_MODE: u32 = 0o600;

/// All the mode bits: the permissions, set-user-id, set-group-id and sticky.
const MODE_BITS: u32 = 0o7777;

/// How many bytes of the file are read. A key is a few dozen characters.
const KEY_LIMIT: u64 = 1024;

/// What a key of the Free plan ends with.
const FREE_SUFFIX: &str = ":fx";

/// The key file of the product: `$XDG_CONFIG_HOME/addict/deepl.key` when the
/// variable is an absolute path, else `$HOME/.config/addict/deepl.key`.
pub fn key_path() -> Option<PathBuf> {
    Some(config_dir()?.join(KEY_FILE))
}

/// The configuration directory of the product: `$XDG_CONFIG_HOME/addict`
/// when the variable is an absolute path, else `$HOME/.config/addict`;
/// `None` when neither names one. Nothing is read or created here.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

fn config_dir_from(config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let config = match config_home.map(PathBuf::from) {
        Some(config) if config.is_absolute() => config,
        // The base directory specification says to ignore a relative value.
        _ => PathBuf::from(home.filter(|home| !home.is_empty())?).join(".config"),
    };
    Some(config.join("addict"))
}

#[cfg(test)]
fn key_path_from(config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    Some(config_dir_from(config_home, home)?.join(KEY_FILE))
}

/// A DeepL key. Its `Debug` output does not hold it.
#[derive(Clone, PartialEq, Eq)]
pub struct Key(String);

impl Key {
    /// The key itself, for the `Authorization` header and nothing else.
    pub(crate) fn secret(&self) -> &str {
        &self.0
    }

    /// Whether this is a key of the Free plan, which has its own host.
    pub fn is_free(&self) -> bool {
        self.0.ends_with(FREE_SUFFIX)
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key(<hidden>)")
    }
}

/// Why a key file gave no key. None of them holds anything of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
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

/// Reads the key from the file at `path`. The mode is checked on the opened
/// file, before anything is read from it. White space around the key is not
/// part of it.
pub fn read(path: &Path) -> Result<Key, KeyError> {
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
    let mut content = String::new();
    file.take(KEY_LIMIT + 1)
        .read_to_string(&mut content)
        .map_err(|_| KeyError::Unreadable)?;
    let key = key_of(&content).ok_or(KeyError::Empty)?;
    Ok(Key(key.to_owned()))
}

/// The key that the content of a key file holds: the content without the
/// white space around it, when the content fits in [`KEY_LIMIT`] bytes and
/// the key is one printable ASCII word. Anything else cannot be sent in a
/// header. [`read`] and [`write`] share this rule, so that what is written
/// is read back.
fn key_of(content: &str) -> Option<&str> {
    let key = content.trim();
    let fits = u64::try_from(content.len()).is_ok_and(|read| read <= KEY_LIMIT);
    (!key.is_empty() && fits && key.chars().all(|c| c.is_ascii_graphic())).then_some(key)
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

/// Writes `key`, without the white space around it, and a newline to the key
/// file at `path`, which then has mode `0600`. Its directory is made when
/// missing. The key goes to a new file in that directory, which is created
/// with mode `0600`, so that no one else can read it at any moment, and is
/// then renamed over `path`: a reader sees the old file or the new one,
/// never a part of it, and a file that was refused for its mode is replaced.
/// On a failure the temporary file is removed and `path` is as it was.
pub fn write(path: &Path, key: &str) -> Result<(), WriteError> {
    let content = format!("{}\n", key.trim());
    if key_of(&content).is_none() {
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
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    written.map_err(|error| {
        let _ = fs::remove_file(&temp);
        io(error)
    })
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::PathBuf};

    use super::{Key, config_dir_from, key_path_from};

    fn os(text: &str) -> Option<OsString> {
        Some(OsString::from(text))
    }

    #[test]
    fn an_absolute_config_home_holds_the_key_file() {
        assert_eq!(
            key_path_from(os("/x/config"), os("/home/u")),
            Some(PathBuf::from("/x/config/addict/deepl.key"))
        );
    }

    #[test]
    fn an_unset_empty_or_relative_config_home_falls_back_to_home() {
        for config_home in [None, os(""), os("config"), os("./config")] {
            assert_eq!(
                key_path_from(config_home.clone(), os("/home/u")),
                Some(PathBuf::from("/home/u/.config/addict/deepl.key")),
                "{config_home:?}"
            );
        }
    }

    #[test]
    fn without_a_config_home_and_a_home_there_is_no_key_file() {
        for home in [None, os("")] {
            assert_eq!(key_path_from(None, home.clone()), None, "{home:?}");
            assert_eq!(key_path_from(os("relative"), home.clone()), None);
        }
    }

    #[test]
    fn the_key_file_lies_in_the_configuration_directory() {
        for (config_home, home) in [
            (os("/x/config"), os("/home/u")),
            (None, os("/home/u")),
            (os("relative"), os("/home/u")),
        ] {
            let dir = config_dir_from(config_home.clone(), home.clone()).expect("a directory");
            assert!(dir.ends_with("addict"), "{dir:?}");
            assert_eq!(
                key_path_from(config_home, home),
                Some(dir.join("deepl.key"))
            );
        }
        assert_eq!(config_dir_from(None, None), None);
    }

    #[test]
    fn a_free_key_is_known_by_its_suffix() {
        assert!(Key("abc:fx".to_owned()).is_free());
        for pro in ["abc", "abc:fx1", "fx", "abc:FX"] {
            assert!(!Key(pro.to_owned()).is_free(), "{pro}");
        }
    }

    #[test]
    fn the_debug_output_of_a_key_does_not_hold_it() {
        let shown = format!("{:?}", Key("secret-key:fx".to_owned()));
        assert!(!shown.contains("secret"), "{shown}");
        assert!(!shown.contains("fx"), "{shown}");
    }
}
