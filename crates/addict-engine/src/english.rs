//! The spelling of English kept across restarts: one line, `us` or `gb`, in
//! the file `english` of the product's configuration directory.
//!
//! The file is read once, on the runner, after the start, and written on the
//! runner each time the English spelling row of Settings changes; never on
//! the UI thread nor on the way to showing the window. A file that is
//! missing, cannot be read or holds anything but one known code is US, and
//! nothing is said about it. A write that fails is written to stderr only:
//! the choice holds until the application ends. The variant is kept apart
//! from my language: the my-language file knows only languages.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use addict_core::translation::English;

/// The name of the file inside the configuration directory.
const ENGLISH_FILE: &str = "english";

/// How many bytes of the file are read. A code and its line break are three.
const ENGLISH_LIMIT: u64 = 64;

/// The English-spelling file of the product, or `None` when no
/// configuration directory was supplied. Nothing is read here.
pub fn path(config: Option<&Path>) -> Option<PathBuf> {
    Some(config?.join(ENGLISH_FILE))
}

/// The variant the file at `path` holds; US for no path, a missing or
/// unreadable file and any content but the code of one variant, white space
/// around it aside. This reads the file, so it runs on the runner.
pub fn read(path: Option<&Path>) -> English {
    path.and_then(|path| read_code(path).ok())
        .and_then(|content| English::of_code(content.trim()))
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
        .take(ENGLISH_LIMIT)
        .read_to_string(&mut content)?;
    Ok(content)
}

/// Writes `english` to the file at `path` as one line, making its directory
/// when there is none. This writes a file, so it runs on the runner.
pub fn write(path: Option<&Path>, english: English) -> io::Result<()> {
    let path = path.ok_or(io::ErrorKind::NotFound)?;
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    fs::write(path, format!("{}\n", english.code()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use addict_core::translation::English;

    use super::{read, write};

    /// A directory of one test, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("addict-english-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("the directory of the test");
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("addict").join("english")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_written_variant_is_read_back() {
        let dir = TempDir::new("round");
        let file = dir.file();
        for english in English::ALL {
            write(Some(&file), english).expect("written");
            assert_eq!(
                std::fs::read_to_string(&file).expect("the file"),
                format!("{}\n", english.code())
            );
            assert_eq!(read(Some(&file)), english);
        }
    }

    #[test]
    fn a_missing_unreadable_or_unknown_file_is_us() {
        let dir = TempDir::new("bad");
        let file = dir.file();
        // No path, and no file.
        assert_eq!(read(None), English::Us);
        assert_eq!(read(Some(&file)), English::Us);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        for content in ["GB\n", "en-GB\n", "uk\n", "", "gb us\n", "\u{0}\u{ff}"] {
            std::fs::write(&file, content).expect("write");
            assert_eq!(read(Some(&file)), English::Us, "{content:?}");
        }
        // White space around a code is not part of it.
        std::fs::write(&file, "  gb \n").expect("write");
        assert_eq!(read(Some(&file)), English::Gb);
        std::fs::write(&file, "\tus\n\n").expect("write");
        assert_eq!(read(Some(&file)), English::Us);
        // A directory in the place of the file.
        std::fs::remove_file(&file).expect("remove");
        std::fs::create_dir(&file).expect("a directory");
        assert_eq!(read(Some(&file)), English::Us);
        // Bytes that are not text.
        std::fs::remove_dir(&file).expect("remove");
        std::fs::write(&file, [0xff, 0xfe, b'g', b'b']).expect("write");
        assert_eq!(read(Some(&file)), English::Us);
    }

    #[test]
    fn a_write_without_a_place_fails_and_changes_nothing() {
        assert!(write(None, English::Gb).is_err());
        let dir = TempDir::new("blocked");
        // A file where the directory should be.
        std::fs::write(dir.0.join("addict"), "").expect("a file");
        assert!(write(Some(&dir.file()), English::Gb).is_err());
        assert_eq!(read(Some(&dir.file())), English::Us);
    }
}
