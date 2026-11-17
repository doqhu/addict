//! The reading language kept across restarts: one line, `en`, `es`, `ca`
//! or `ru`, in the file `language` of the product's configuration directory,
//! [`addict_deepl::key::config_dir`], the directory of the DeepL key.
//!
//! The file is read once, on the runner, after the start, and written on the
//! runner at each choice of the user, a press in the switch or a click on a
//! gloss's English word; a change of the switch caused by my language writes
//! nothing. Neither is done on the UI thread
//! nor on the way to showing the window. A file that is missing, cannot be
//! read or holds anything but one known code is English, and nothing is said
//! about it. A write that fails is written to stderr only: the choice holds
//! until the application ends.
//!
//! The language kept is the saved one; the window reads in
//! [`reading`] of it and my language, which never is my language itself.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

use addict_core::translation::{Source, reading_languages};

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
/// unreadable file and any content but the code of one language, white
/// space around it aside. This reads the file, so it runs on the runner.
pub fn read(path: Option<&Path>) -> Source {
    path.and_then(|path| read_code(path).ok())
        .and_then(|content| Source::of_code(content.trim()))
        .unwrap_or_default()
}

/// The language the window reads in when `saved` is the saved reading
/// language and `my` is my language: `saved`, unless it is `my`, and then
/// the first of [`reading_languages`] of `my`. Nothing is written by it: a
/// saved language that falls back stays saved.
pub fn reading(saved: Source, my: Source) -> Source {
    if saved == my {
        reading_languages(my)[0]
    } else {
        saved
    }
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

    use addict_core::translation::{Source, reading_languages};

    use super::{read, reading, write};

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
        for content in [
            "fr\n",
            "",
            "ES\n",
            "es ca\n",
            "en-GB",
            "\u{0}\u{ff}",
            "RU\n",
        ] {
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

    /// The saved language holds unless it is my language, which falls back
    /// to the first reading language of my language; with Russian mine,
    /// every saved language of 0.2.0 holds.
    #[test]
    fn the_reading_language_is_the_saved_one_unless_it_is_mine() {
        for my in Source::ALL {
            for saved in Source::ALL {
                let expected = if saved == my {
                    reading_languages(my)[0]
                } else {
                    saved
                };
                let effective = reading(saved, my);
                assert_eq!(effective, expected, "{saved:?} with {my:?}");
                assert_ne!(effective, my, "{saved:?} with {my:?}");
                assert!(reading_languages(my).contains(&effective));
            }
        }
        assert_eq!(reading(Source::En, Source::En), Source::Es);
        assert_eq!(reading(Source::Ru, Source::Ru), Source::En);
        for saved in reading_languages(Source::Ru) {
            assert_eq!(reading(saved, Source::Ru), saved);
        }
    }

    /// A saved Russian falls back to English while Russian is mine and is
    /// Russian again with English mine.
    #[test]
    fn a_saved_russian_comes_back_with_english_mine() {
        assert_eq!(reading(Source::Ru, Source::Ru), Source::En);
        assert_eq!(reading(Source::Ru, Source::En), Source::Ru);
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
