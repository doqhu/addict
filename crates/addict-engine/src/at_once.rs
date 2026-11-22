//! Whether a sentence read at a press of the clipboard shortcut is
//! translated online at once, kept across restarts: one line, `on` or `off`,
//! in the file `translate-sentence` of the product's configuration
//! directory, [`addict_deepl::key::config_dir`], the directory of the DeepL
//! key.
//!
//! The file is read once, on the runner, after the start, and written on the
//! runner each time the Translate sentence at once switch of Settings moves;
//! never on the UI thread nor on the way to showing the window. A file that
//! is missing, cannot be read or holds anything but one known word is `on`,
//! the default, and nothing is said about it. A write that fails is written
//! to stderr only: the choice holds until the application ends.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

/// The name of the file inside the configuration directory.
const AT_ONCE_FILE: &str = "translate-sentence";

/// How many bytes of the file are read. A word and its line break are four.
const AT_ONCE_LIMIT: u64 = 64;

/// What the file holds when the sentence is translated at once.
const ON: &str = "on";

/// What the file holds when the sentence waits for a click on a word.
const OFF: &str = "off";

/// The default: a copied sentence is translated at once.
pub const DEFAULT: bool = true;

/// The file of the product, or `None` when the environment names no
/// configuration directory. Nothing is read here.
pub fn path() -> Option<PathBuf> {
    Some(addict_deepl::key::config_dir()?.join(AT_ONCE_FILE))
}

/// Whether the file at `path` says to translate at once; [`DEFAULT`] for no
/// path, a missing or unreadable file and any content but `on` or `off`,
/// white space around it aside. This reads the file, so it runs on the
/// runner.
pub fn read(path: Option<&Path>) -> bool {
    match path
        .and_then(|path| read_word(path).ok())
        .as_deref()
        .map(str::trim)
    {
        Some(ON) => true,
        Some(OFF) => false,
        _ => DEFAULT,
    }
}

/// The first bytes of the regular file at `path`, as text.
fn read_word(path: &Path) -> io::Result<String> {
    // Before opening: opening a pipe would wait for its writer.
    if !fs::metadata(path)?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut content = String::new();
    File::open(path)?
        .take(AT_ONCE_LIMIT)
        .read_to_string(&mut content)?;
    Ok(content)
}

/// Writes `at_once` to the file at `path` as one line, making its directory
/// when there is none. This writes a file, so it runs on the runner.
pub fn write(path: Option<&Path>, at_once: bool) -> io::Result<()> {
    let path = path.ok_or(io::ErrorKind::NotFound)?;
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    fs::write(path, format!("{}\n", if at_once { ON } else { OFF }))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{DEFAULT, read, write};

    /// A directory of one test, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("addict-at-once-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("the directory of the test");
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("addict").join("translate-sentence")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_written_choice_is_read_back() {
        let dir = TempDir::new("round");
        let file = dir.file();
        for (at_once, line) in [(false, "off\n"), (true, "on\n")] {
            write(Some(&file), at_once).expect("written");
            assert_eq!(std::fs::read_to_string(&file).expect("the file"), line);
            assert_eq!(read(Some(&file)), at_once);
        }
    }

    #[test]
    fn a_missing_unreadable_or_unknown_file_is_on() {
        const { assert!(DEFAULT) };
        let dir = TempDir::new("bad");
        let file = dir.file();
        assert!(read(None));
        assert!(read(Some(&file)));
        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        for content in ["OFF\n", "no\n", "", "off on\n", "\u{0}\u{ff}"] {
            std::fs::write(&file, content).expect("write");
            assert!(read(Some(&file)), "{content:?}");
        }
        std::fs::write(&file, "  off \n").expect("write");
        assert!(!read(Some(&file)));
        // A directory in the place of the file.
        std::fs::remove_file(&file).expect("remove");
        std::fs::create_dir(&file).expect("a directory");
        assert!(read(Some(&file)));
    }

    #[test]
    fn a_write_without_a_place_fails_and_changes_nothing() {
        assert!(write(None, false).is_err());
        let dir = TempDir::new("blocked");
        std::fs::write(dir.0.join("addict"), "").expect("a file");
        assert!(write(Some(&dir.file()), false).is_err());
        assert!(read(Some(&dir.file())));
    }
}
