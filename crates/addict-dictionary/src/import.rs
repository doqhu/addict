//! The logic of `addict import`: a known dictionary file in, its index out.
//!
//! A file is accepted when its byte size and then its SHA-256 equal an entry
//! of [`KNOWN`]; until both match, nothing is written. The file is read into
//! memory once, so the bytes that were hashed are the bytes that are parsed.
//! The index is built in memory, written to a temporary file beside its
//! final place and renamed over it: an import that fails leaves the previous
//! index as it was, and the same file gives the same index bytes.
//!
//! There is no download here: the user brings the file.

use std::{
    ffi::OsString,
    fmt,
    fs::{self, File},
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
};

use flate2::bufread::MultiGzDecoder;
use sha2::{Digest, Sha256};

use crate::{format, oewn};

/// A dictionary source. Each has one index file in the data directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Open English WordNet: gzip of WN-LMF XML.
    Oewn,
}

impl Source {
    /// The name of the index file of the source.
    pub fn index_name(self) -> &'static str {
        match self {
            Self::Oewn => "oewn.index",
        }
    }
}

/// A file the importer accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownFile<'a> {
    /// The name the file is published under; the user's file may be named
    /// otherwise.
    pub name: &'a str,
    /// The source id written into the index.
    pub edition: &'a str,
    pub source: Source,
    /// The size of the file in bytes.
    pub size: u64,
    /// The SHA-256 of the file, in lowercase hexadecimal.
    pub sha256: &'a str,
}

/// The chosen files of the product (the spec's dictionary data).
pub const KNOWN: &[KnownFile<'static>] = &[KnownFile {
    name: "english-wordnet-2025.xml.gz",
    edition: "oewn-2025",
    source: Source::Oewn,
    size: 11_363_503,
    sha256: "9ca6d1dcb75f822fdd66617f7d9da48142ace38dd544d6ad5e2feca1674ad3fe",
}];

/// The data directory of the product: `$XDG_DATA_HOME/addict` when the
/// variable is an absolute path, else `$HOME/.local/share/addict`.
pub fn data_dir() -> Option<PathBuf> {
    data_dir_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

fn data_dir_from(data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let data = match data_home.map(PathBuf::from) {
        Some(data) if data.is_absolute() => data,
        // The base directory specification says to ignore a relative value.
        _ => PathBuf::from(home.filter(|home| !home.is_empty())?).join(".local/share"),
    };
    Some(data.join("addict"))
}

/// Where the index of `source` lives inside the data directory `dir`.
pub fn index_path(dir: &Path, source: Source) -> PathBuf {
    dir.join(source.index_name())
}

/// What an import wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    /// The index file.
    pub path: PathBuf,
    pub headwords: usize,
    pub exception_forms: usize,
}

/// Why an import wrote nothing. The text is one line and names the path.
#[derive(Debug)]
pub enum ImportError {
    /// The file could not be opened or read, or is not a regular file.
    Unreadable { file: PathBuf, cause: io::Error },
    /// No known file has this size; the hash was not computed.
    Size {
        file: PathBuf,
        actual: u64,
        /// Name and size of every known file.
        expected: Vec<(String, u64)>,
    },
    /// A known file has this size and another hash.
    Hash {
        file: PathBuf,
        actual: String,
        /// Name and hash of every known file of this size.
        expected: Vec<(String, String)>,
    },
    /// The file is the known one and its contents could not be turned into
    /// an index.
    Contents { file: PathBuf, cause: String },
    /// The index could not be written.
    Write { path: PathBuf, cause: io::Error },
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { file, cause } => {
                write!(f, "cannot read {}: {cause}", file.display())
            }
            Self::Size {
                file,
                actual,
                expected,
            } => {
                write!(
                    f,
                    "{} is not a known dictionary file: its size is {actual} bytes, expected ",
                    file.display()
                )?;
                list(f, expected, |f, (name, size)| {
                    write!(f, "{size} bytes ({name})")
                })
            }
            Self::Hash {
                file,
                actual,
                expected,
            } => {
                write!(
                    f,
                    "{} is not a known dictionary file: its SHA-256 is {actual}, expected ",
                    file.display()
                )?;
                list(f, expected, |f, (name, hash)| write!(f, "{hash} ({name})"))
            }
            Self::Contents { file, cause } => {
                write!(f, "cannot import {}: {cause}", file.display())
            }
            Self::Write { path, cause } => {
                write!(f, "cannot write the index {}: {cause}", path.display())
            }
        }
    }
}

impl std::error::Error for ImportError {}

fn list<T>(
    f: &mut fmt::Formatter<'_>,
    items: &[T],
    mut one: impl FnMut(&mut fmt::Formatter<'_>, &T) -> fmt::Result,
) -> fmt::Result {
    for (at, item) in items.iter().enumerate() {
        if at > 0 {
            write!(f, " or ")?;
        }
        one(f, item)?;
    }
    Ok(())
}

/// Imports the dictionary file `file` into the data directory `dir`, which
/// is created when missing. The index of the file's source is replaced.
pub fn import(file: &Path, dir: &Path) -> Result<Imported, ImportError> {
    import_known(file, dir, KNOWN)
}

fn import_known(file: &Path, dir: &Path, known: &[KnownFile<'_>]) -> Result<Imported, ImportError> {
    let (bytes, known) = recognise(file, known)?;
    let contents = match known.source {
        Source::Oewn => oewn::parse(
            BufReader::new(MultiGzDecoder::new(bytes.as_slice())),
            known.edition,
        )
        .map_err(|error| error.to_string()),
    }
    .and_then(|contents| {
        let index = format::encode(&contents).map_err(|error| error.to_string())?;
        Ok((contents, index))
    });
    let (contents, index) = contents.map_err(|cause| ImportError::Contents {
        file: file.to_owned(),
        cause,
    })?;
    drop(bytes);

    let path = index_path(dir, known.source);
    replace(dir, &path, &index).map_err(|cause| ImportError::Write {
        path: path.clone(),
        cause,
    })?;
    Ok(Imported {
        path,
        headwords: contents.headwords.len(),
        exception_forms: contents.forms.len(),
    })
}

/// The bytes of `file` and the known file they are: the size is compared
/// first and the hash is computed only for a size that is known.
fn recognise<'a>(
    file: &Path,
    known: &'a [KnownFile<'a>],
) -> Result<(Vec<u8>, &'a KnownFile<'a>), ImportError> {
    let unreadable = |cause| ImportError::Unreadable {
        file: file.to_owned(),
        cause,
    };
    let wrong_size = |actual| ImportError::Size {
        file: file.to_owned(),
        actual,
        expected: known
            .iter()
            .map(|known| (known.name.to_owned(), known.size))
            .collect(),
    };
    // The type is checked before the open: opening a FIFO would block.
    if !fs::metadata(file).map_err(unreadable)?.is_file() {
        return Err(unreadable(io::Error::other("not a regular file")));
    }
    let handle = File::open(file).map_err(unreadable)?;
    let metadata = handle.metadata().map_err(unreadable)?;
    if !metadata.is_file() {
        return Err(unreadable(io::Error::other("not a regular file")));
    }
    let size = metadata.len();
    if !known.iter().any(|known| known.size == size) {
        return Err(wrong_size(size));
    }
    // One byte more than expected shows a file that grew since.
    let mut bytes = Vec::new();
    handle
        .take(size + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 != size {
        return Err(wrong_size(bytes.len() as u64));
    }
    let actual: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let same_size = || known.iter().filter(|known| known.size == size);
    match same_size().find(|known| known.sha256.eq_ignore_ascii_case(&actual)) {
        Some(known) => Ok((bytes, known)),
        None => Err(ImportError::Hash {
            file: file.to_owned(),
            actual,
            expected: same_size()
                .map(|known| (known.name.to_owned(), known.sha256.to_owned()))
                .collect(),
        }),
    }
}

/// Puts `index` at `path` inside `dir` through a temporary file in `dir`
/// and a rename; on an error the temporary file is removed and `path` is as
/// it was.
fn replace(dir: &Path, path: &Path, index: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".{}.tmp", std::process::id()));
    let temporary = dir.join(name);
    let written = File::create(&temporary)
        .and_then(|mut file| {
            file.write_all(index)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

#[cfg(test)]
mod tests {
    use flate2::{Compression, write::GzEncoder};

    use super::*;
    use crate::{index::Index, oewn::tests::FIXTURE, testdir::TempDir};

    fn os(text: &str) -> Option<OsString> {
        Some(OsString::from(text))
    }

    #[test]
    fn the_data_directory_follows_the_base_directory_rules() {
        assert_eq!(
            data_dir_from(os("/data"), os("/home/u")),
            Some(PathBuf::from("/data/addict"))
        );
        assert_eq!(
            data_dir_from(None, os("/home/u")),
            Some(PathBuf::from("/home/u/.local/share/addict"))
        );
        // A relative or an empty value is ignored.
        for ignored in ["relative/data", ""] {
            assert_eq!(
                data_dir_from(os(ignored), os("/home/u")),
                Some(PathBuf::from("/home/u/.local/share/addict"))
            );
        }
        assert_eq!(data_dir_from(None, None), None);
        assert_eq!(data_dir_from(None, os("")), None);
        assert_eq!(data_dir_from(os("relative"), None), None);
        assert_eq!(
            data_dir_from(os("/data"), None),
            Some(PathBuf::from("/data/addict"))
        );
    }

    #[test]
    fn the_known_file_is_the_chosen_wordnet() {
        assert_eq!(KNOWN.len(), 1);
        assert_eq!(KNOWN[0].size, 11363503);
        assert_eq!(
            KNOWN[0].sha256,
            "9ca6d1dcb75f822fdd66617f7d9da48142ace38dd544d6ad5e2feca1674ad3fe"
        );
        assert_eq!(KNOWN[0].source, Source::Oewn);
        assert_eq!(
            index_path(Path::new("/d"), Source::Oewn),
            Path::new("/d/oewn.index")
        );
    }

    /// A gzip of the WordNet fixture in a file, and the hash of that file.
    struct Fixture {
        dir: TempDir,
        gz: Vec<u8>,
        sha256: String,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(FIXTURE.as_bytes()).expect("compress");
            let gz = encoder.finish().expect("compress");
            let sha256 = hex(&gz);
            let fixture = Self {
                dir: TempDir::new(tag),
                gz,
                sha256,
            };
            fs::write(fixture.file(), &fixture.gz).expect("write fixture");
            fixture
        }

        fn file(&self) -> PathBuf {
            self.dir.path().join("wordnet.xml.gz")
        }

        /// The data directory; it does not exist until an import creates it.
        fn data(&self) -> PathBuf {
            self.dir.path().join("data/addict")
        }

        fn known(&self) -> [KnownFile<'_>; 1] {
            [KnownFile {
                name: "fixture.xml.gz",
                edition: "fixture-1",
                source: Source::Oewn,
                size: self.gz.len() as u64,
                sha256: &self.sha256,
            }]
        }

        fn import(&self, file: &Path) -> Result<Imported, ImportError> {
            import_known(file, &self.data(), &self.known())
        }
    }

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn one_line(error: &ImportError) -> String {
        let text = error.to_string();
        assert!(!text.contains('\n'), "{text}");
        text
    }

    #[test]
    fn the_known_file_is_imported_into_a_readable_index() {
        let fixture = Fixture::new("import");
        let imported = fixture.import(&fixture.file()).expect("import");
        assert_eq!(imported.path, fixture.data().join("oewn.index"));
        assert_eq!(imported.headwords, 6);
        assert_eq!(imported.exception_forms, 4);

        let index = Index::open(&imported.path).expect("open");
        assert_eq!(index.source(), "fixture-1");
        assert_eq!(index.headwords(), 6);
        assert_eq!(index.exception_forms(), 4);
        let bank = index.find("bank").expect("find").expect("bank");
        assert_eq!(bank.senses.len(), 3);
        assert_eq!(bank.senses[0].synonyms, ["shore"]);
        assert_eq!(bank.senses[1].synonyms, ["Bank"]);
        assert!(bank.senses[2].synonyms.is_empty());
        assert!(bank.translations.is_empty());
        assert!(index.find("'hood").expect("find").is_some());
        assert_eq!(index.find("banks").expect("find"), None);
        assert_eq!(index.exception_lemmas("lain").expect("form"), ["lie"]);
        // Nothing but the index is left in the data directory.
        let left: Vec<_> = fs::read_dir(fixture.data())
            .expect("data directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(left, ["oewn.index"]);
    }

    #[test]
    fn a_second_import_replaces_the_index_with_the_same_bytes() {
        let fixture = Fixture::new("again");
        let first = fixture.import(&fixture.file()).expect("import");
        let bytes = fs::read(&first.path).expect("index");
        // An index opened before the second import keeps reading its file.
        let open = Index::open(&first.path).expect("open");
        let second = fixture.import(&fixture.file()).expect("import again");
        assert_eq!(first, second);
        assert_eq!(fs::read(&second.path).expect("index"), bytes);
        assert!(open.find("bank").expect("find").is_some());
    }

    #[test]
    fn another_hash_of_the_same_size_writes_nothing() {
        let fixture = Fixture::new("hash");
        let mut changed = fixture.gz.clone();
        *changed.last_mut().expect("bytes") ^= 1;
        let file = fixture.dir.path().join("changed.gz");
        fs::write(&file, &changed).expect("write");

        let error = fixture.import(&file).expect_err("another hash");
        assert!(matches!(error, ImportError::Hash { .. }), "{error}");
        let text = one_line(&error);
        assert!(text.contains("changed.gz"), "{text}");
        assert!(text.contains(&fixture.sha256), "{text}");
        assert!(text.contains(&hex(&changed)), "{text}");
        assert!(!fixture.data().exists());
        assert!(!fixture.dir.path().join("data").exists());
    }

    #[test]
    fn another_size_writes_nothing_and_claims_no_hash() {
        let fixture = Fixture::new("size");
        let file = fixture.dir.path().join("short.gz");
        let short = &fixture.gz[..fixture.gz.len() - 7];
        fs::write(&file, short).expect("write");

        let error = fixture.import(&file).expect_err("another size");
        assert!(matches!(error, ImportError::Size { .. }), "{error}");
        let text = one_line(&error);
        assert!(text.contains("short.gz"), "{text}");
        assert!(text.contains(&format!("{} bytes", short.len())), "{text}");
        assert!(
            text.contains(&format!("{} bytes", fixture.gz.len())),
            "{text}"
        );
        assert!(!text.contains("SHA-256"), "{text}");
        assert!(!text.contains(&hex(short)), "{text}");
        assert!(!fixture.dir.path().join("data").exists());

        let empty = fixture.dir.path().join("empty.gz");
        fs::write(&empty, b"").expect("write");
        let error = fixture.import(&empty).expect_err("empty");
        assert!(
            matches!(error, ImportError::Size { actual: 0, .. }),
            "{error}"
        );
    }

    #[test]
    fn a_missing_file_and_a_directory_write_nothing() {
        let fixture = Fixture::new("unreadable");
        for file in [
            fixture.dir.path().join("absent.gz"),
            fixture.dir.path().to_owned(),
        ] {
            let error = fixture.import(&file).expect_err("unreadable");
            assert!(matches!(error, ImportError::Unreadable { .. }), "{error}");
            let text = one_line(&error);
            assert!(text.contains(&file.display().to_string()), "{text}");
            assert!(!fixture.dir.path().join("data").exists());
        }
    }

    #[test]
    fn a_data_directory_that_cannot_be_created_is_an_error() {
        let fixture = Fixture::new("nodir");
        // A regular file where a parent of the data directory should be.
        fs::write(fixture.dir.path().join("data"), b"in the way").expect("write");
        let error = fixture.import(&fixture.file()).expect_err("no directory");
        assert!(matches!(error, ImportError::Write { .. }), "{error}");
        let text = one_line(&error);
        assert!(text.contains("oewn.index"), "{text}");
        assert_eq!(
            fs::read(fixture.dir.path().join("data")).expect("file"),
            b"in the way"
        );
    }

    #[test]
    fn a_failed_import_leaves_the_previous_index_as_it_was() {
        let fixture = Fixture::new("previous");
        let imported = fixture.import(&fixture.file()).expect("import");
        let bytes = fs::read(&imported.path).expect("index");

        let mut changed = fixture.gz.clone();
        changed[fixture.gz.len() / 2] ^= 0x10;
        let file = fixture.dir.path().join("changed.gz");
        fs::write(&file, &changed).expect("write");
        assert!(fixture.import(&file).is_err());
        assert!(fixture.import(&fixture.dir.path().join("absent")).is_err());

        // The index cannot be replaced: a directory is where the temporary
        // file would be written.
        let temporary = fixture
            .data()
            .join(format!(".oewn.index.{}.tmp", std::process::id()));
        fs::create_dir(&temporary).expect("block the temporary file");
        let error = fixture.import(&fixture.file()).expect_err("blocked");
        assert!(matches!(error, ImportError::Write { .. }), "{error}");
        fs::remove_dir(&temporary).expect("unblock");

        assert_eq!(fs::read(&imported.path).expect("index"), bytes);
    }

    #[test]
    fn a_known_file_that_is_not_a_wordnet_writes_nothing() {
        let fixture = Fixture::new("contents");
        let file = fixture.dir.path().join("text.gz");
        let text = b"this is not gzip at all";
        fs::write(&file, text).expect("write");
        let sha256 = hex(text);
        let known = [KnownFile {
            name: "text.gz",
            edition: "text-1",
            source: Source::Oewn,
            size: text.len() as u64,
            sha256: &sha256,
        }];
        let error = import_known(&file, &fixture.data(), &known).expect_err("not a wordnet");
        assert!(matches!(error, ImportError::Contents { .. }), "{error}");
        assert!(one_line(&error).contains("text.gz"));
        assert!(!fixture.data().exists());
    }
}
