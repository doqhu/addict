//! The reading language kept across restarts: one line, `en`, `es` or `ca`,
//! in the file `language` of the product's configuration directory,
//! [`addict_deepl::key::config_dir`], the directory of the DeepL key.
//!
//! The file is read once, on the runner, after the start, and written on the
//! runner each time the switch of the window changes; never on the UI thread
//! nor on the way to showing the window. A file that is missing, cannot be
//! read or holds anything but one known code is English, and nothing is said
//! about it. A write that fails is written to stderr only: the choice holds
//! until the application ends.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

use addict_core::translation::Source;

/// The name of the file inside the configuration directory.
const LANGUAGE_FILE: &str = "language";

/// How many bytes of the file are read. A code and its line break are three.
const LANGUAGE_LIMIT: u64 = 64;

/// How long a read or a write of the file may take on the runner.
pub const FILE_LIMIT: Duration = Duration::from_secs(2);

/// The language file of the product, or `None` when the environment names no
/// configuration directory. Nothing is read here.
pub fn path() -> Option<PathBuf> {
    Some(addict_deepl::key::config_dir()?.join(LANGUAGE_FILE))
}

/// The language the file at `path` holds; English for no path, a missing or
/// unreadable file and any content but one known code, white space around it
/// aside. This reads the file, so it runs on the runner.
pub fn read(path: Option<&Path>) -> Source {
    path.and_then(|path| read_code(path).ok())
        .and_then(|content| Source::of_code(content.trim()))
        .unwrap_or_default()
}

/// The first bytes of the regular file at `path`, as text.
fn read_code(path: &Path) -> io::Result<String> {
    // Before opening: opening a pipe would wait for its writer.
    if !fs::metadata(path)?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut content = String::new();
    File::open(path)?
        .take(LANGUAGE_LIMIT)
        .read_to_string(&mut content)?;
    Ok(content)
}

/// Writes `source` to the file at `path` as one line, making its directory
/// when there is none. This writes a file, so it runs on the runner.
pub fn write(path: Option<&Path>, source: Source) -> io::Result<()> {
    let path = path.ok_or(io::ErrorKind::NotFound)?;
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    fs::write(path, format!("{}\n", source.code()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use addict_core::translation::Source;

    use super::{read, write};

    /// A directory of one test, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("addict-language-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("the directory of the test");
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("addict").join("language")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_written_language_is_read_back() {
        let dir = TempDir::new("round");
        let file = dir.file();
        for source in Source::ALL {
            write(Some(&file), source).expect("written");
            assert_eq!(
                std::fs::read_to_string(&file).expect("the file"),
                format!("{}\n", source.code())
            );
            assert_eq!(read(Some(&file)), source);
        }
    }

    #[test]
    fn a_missing_unreadable_or_unknown_file_is_english() {
        let dir = TempDir::new("bad");
        let file = dir.file();
        // No path, and no file.
        assert_eq!(read(None), Source::En);
        assert_eq!(read(Some(&file)), Source::En);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        for content in ["fr\n", "", "ES\n", "es ca\n", "en-GB", "\u{0}\u{ff}"] {
            std::fs::write(&file, content).expect("write");
            assert_eq!(read(Some(&file)), Source::En, "{content:?}");
        }
        // White space around a code is not part of it.
        std::fs::write(&file, "  ca \n").expect("write");
        assert_eq!(read(Some(&file)), Source::Ca);
        // A directory in the place of the file.
        std::fs::remove_file(&file).expect("remove");
        std::fs::create_dir(&file).expect("a directory");
        assert_eq!(read(Some(&file)), Source::En);
        // Bytes that are not text.
        std::fs::remove_dir(&file).expect("remove");
        std::fs::write(&file, [0xff, 0xfe, b'e', b's']).expect("write");
        assert_eq!(read(Some(&file)), Source::En);
    }

    #[test]
    fn a_write_without_a_place_fails_and_changes_nothing() {
        assert!(write(None, Source::Es).is_err());
        let dir = TempDir::new("blocked");
        // A file where the directory should be.
        std::fs::write(dir.0.join("addict"), "").expect("a file");
        assert!(write(Some(&dir.file()), Source::Es).is_err());
        assert_eq!(read(Some(&dir.file())), Source::En);
    }
}
