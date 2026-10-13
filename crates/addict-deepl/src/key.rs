//! The key file: where it is, who may read it and what it holds.
//!
//! The key is the first and only thing of the file,
//! `$XDG_CONFIG_HOME/addict/deepl.key`, or `$HOME/.config/addict/deepl.key`
//! when the variable is unset or relative. A file that anyone but its owner
//! can use, or that its owner can execute, is refused before its content is
//! read.

use std::{
    ffi::OsString,
    fmt,
    fs::File,
    io::{self, Read},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
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
    key_path_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

fn key_path_from(config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let config = match config_home.map(PathBuf::from) {
        Some(config) if config.is_absolute() => config,
        // The base directory specification says to ignore a relative value.
        _ => PathBuf::from(home.filter(|home| !home.is_empty())?).join(".config"),
    };
    Some(config.join("addict").join(KEY_FILE))
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
    let key = content.trim();
    // A key is one printable ASCII word; anything else cannot be sent in a
    // header.
    let fits = u64::try_from(content.len()).is_ok_and(|read| read <= KEY_LIMIT);
    if key.is_empty() || !fits || !key.chars().all(|c| c.is_ascii_graphic()) {
        return Err(KeyError::Empty);
    }
    Ok(Key(key.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::PathBuf};

    use super::{Key, key_path_from};

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
