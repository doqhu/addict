//! The contract of the published indexes: what the data release of an index
//! format holds and how a downloaded index is installed. There is no network
//! code here; the builder of the release (`examples/build-indexes.rs`) and
//! the application's downloader both read this module, so the two cannot
//! drift apart.
//!
//! The indexes of the format [`format::VERSION`] are assets of the release
//! tagged [`tag`], `indexes-v<VERSION>`: each index compressed with xz as
//! [`asset_name`], `<name>.index.xz`, and [`MANIFEST`], the JSON of a
//! [`Manifest`] that gives the size and the SHA-256 of every asset. A release
//! of an older format is never changed, so an application that reads only
//! that format keeps finding its indexes.
//!
//! [`install`] unpacks a downloaded asset, whose size and hash the caller has
//! checked against the manifest, into a temporary file beside the index,
//! opens it as an index and only then renames it over the old one: an
//! install that fails leaves the previous index as it was and no temporary
//! file behind. [`install_unless`] does the same, and gives up before the
//! rename once its caller says to stop, so an install that was cancelled or
//! ran out of time never replaces the index after the fact. Before it
//! writes, the temporary files of that index left by other processes, which
//! were killed in the middle of an install, are removed.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    format,
    import::{self, Source},
    index::{self, Index},
    sources,
};

/// The name of the manifest asset of the release.
pub const MANIFEST: &str = "manifest.json";

/// The tag of the data release of the indexes of this format.
pub fn tag() -> String {
    format!("indexes-v{}", format::VERSION)
}

/// The name of the asset of the index `name`, such as `oewn.index.xz` for
/// `oewn`.
pub fn asset_name(name: &str) -> String {
    format!("{name}.index.xz")
}

/// What the manifest of a release says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The index format of the release, [`format::VERSION`] for the release
    /// of [`tag`].
    pub format: u32,
    /// The published indexes, each once.
    pub indexes: Vec<Published>,
}

/// One published index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    /// The name of the index without `.index`, as [`sources::Row::index`],
    /// such as `oewn`.
    pub name: String,
    /// The format of the index inside the asset.
    pub format: u32,
    /// The size of the asset, the xz file, in bytes.
    pub size: u64,
    /// The SHA-256 of the asset, in lowercase hexadecimal.
    pub sha256: String,
}

impl Published {
    /// The entry of the index `name` of the current format whose asset is
    /// `xz`.
    pub fn of(name: &str, xz: &[u8]) -> Self {
        Self {
            name: name.to_owned(),
            format: format::VERSION,
            size: xz.len() as u64,
            sha256: sha256(xz),
        }
    }

    /// The name of its asset.
    pub fn asset(&self) -> String {
        asset_name(&self.name)
    }
}

impl Manifest {
    /// The manifest in `json`, which must be of the format this build writes.
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let manifest: Self = serde_json::from_slice(json).map_err(|error| error.to_string())?;
        if manifest.format != format::VERSION {
            return Err(format!(
                "the manifest is of format {}, expected {}",
                manifest.format,
                format::VERSION
            ));
        }
        Ok(manifest)
    }

    /// The manifest as it is published: pretty JSON with a line end.
    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self).expect("a manifest serializes");
        json.push('\n');
        json
    }
}

/// The SHA-256 of `bytes`, in lowercase hexadecimal.
pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The source whose index is named `name`, such as [`Source::Oewn`] for
/// `oewn`; `None` for a name this build does not know.
pub fn source_of(name: &str) -> Option<Source> {
    import::KNOWN
        .iter()
        .map(|known| known.source)
        .find(|&source| sources::row(source).index == name)
}

/// Why an install wrote nothing. The text is one line and names the index.
#[derive(Debug)]
pub enum InstallError {
    /// The asset is not an xz file.
    NotXz { source: Source, cause: String },
    /// The unpacked asset is not an index this build reads.
    NotAnIndex { source: Source, cause: index::Error },
    /// The index could not be written.
    Write {
        source: Source,
        path: PathBuf,
        cause: io::Error,
    },
    /// The caller said to stop before the index was put in place.
    Stopped { source: Source },
}

impl InstallError {
    /// The source of the index that was not installed.
    pub fn source(&self) -> Source {
        match self {
            Self::NotXz { source, .. }
            | Self::NotAnIndex { source, .. }
            | Self::Write { source, .. }
            | Self::Stopped { source } => *source,
        }
    }
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = sources::row(self.source()).index;
        match self {
            Self::NotXz { cause, .. } => {
                write!(f, "cannot install {name}: not an xz file: {cause}")
            }
            Self::NotAnIndex { cause, .. } => {
                write!(f, "cannot install {name}: not a readable index: {cause}")
            }
            Self::Write { path, cause, .. } => write!(
                f,
                "cannot install {name}: cannot write {}: {cause}",
                path.display()
            ),
            Self::Stopped { .. } => write!(f, "cannot install {name}: the install was stopped"),
        }
    }
}

impl std::error::Error for InstallError {}

/// Installs the index of `source` from `xz`, its downloaded asset, into the
/// data directory `dir`, which is created when missing, and gives the path
/// of the index. The asset is unpacked into a temporary file in `dir`, which
/// must open as an index before it is renamed over the previous one; on any
/// error the temporary file is removed and the previous index stays.
pub fn install(xz: &[u8], source: Source, dir: &Path) -> Result<PathBuf, InstallError> {
    install_unless(xz, source, dir, || false)
}

/// As [`install`], but `stopped` is asked before the asset is unpacked and
/// once more right before the rename: when it says `true`, the temporary
/// file is removed, the previous index stays and the error is
/// [`InstallError::Stopped`]. The temporary files of this index that other
/// processes left in `dir` are removed first.
pub fn install_unless(
    xz: &[u8],
    source: Source,
    dir: &Path,
    stopped: impl Fn() -> bool,
) -> Result<PathBuf, InstallError> {
    if stopped() {
        return Err(InstallError::Stopped { source });
    }
    let path = import::index_path(dir, source);
    sweep(dir, &path);
    let mut packed = xz;
    let mut unpacked = Vec::new();
    lzma_rs::xz_decompress(&mut packed, &mut unpacked).map_err(|error| InstallError::NotXz {
        source,
        cause: one_line(&error),
    })?;
    let mut unreadable = None;
    let mut halted = false;
    let written = import::replace_checked(dir, &path, &unpacked, |temporary| {
        Index::open(temporary).map(drop).map_err(|error| {
            let cause = io::Error::other(error.to_string());
            unreadable = Some(error);
            cause
        })?;
        if stopped() {
            halted = true;
            return Err(io::Error::other("the install was stopped"));
        }
        Ok(())
    });
    match (written, unreadable) {
        (Ok(()), _) => Ok(path),
        (Err(_), _) if halted => Err(InstallError::Stopped { source }),
        (Err(_), Some(cause)) => Err(InstallError::NotAnIndex { source, cause }),
        (Err(cause), None) => Err(InstallError::Write {
            source,
            path,
            cause,
        }),
    }
}

/// Removes the temporary files of the index at `path` that processes other
/// than this one left in `dir`, `.<file>.<pid>.tmp`; one of this process is
/// its own install's and is left alone. Nothing is reported: a file that
/// cannot be removed is in the way of nothing.
fn sweep(dir: &Path, path: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let own = import::temporary_path(dir, path, std::process::id());
    let Some(file) = path.file_name().and_then(|file| file.to_str()) else {
        return;
    };
    let prefix = format!(".{file}.");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix(&prefix))
            .and_then(|rest| rest.strip_suffix(".tmp"))
        else {
            continue;
        };
        if pid.is_empty() || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let stale = entry.path();
        if stale != own && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = fs::remove_file(stale);
        }
    }
}

/// The text of a foreign error on one line.
fn one_line(error: &dyn fmt::Display) -> String {
    error
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::{
        format::{Contents, Headword, SenseRecord},
        testdir::TempDir,
    };

    /// A small index of one headword, `bank`.
    fn index_bytes() -> Vec<u8> {
        let contents = Contents {
            source: "oewn-fixture".to_owned(),
            senses: vec![SenseRecord {
                part_of_speech: None,
                definition: "sloping land beside a body of water".to_owned(),
                members: vec!["bank".to_owned(), "shore".to_owned()],
                metadata: None,
            }],
            headwords: vec![Headword {
                entries: Vec::new(),
                word: "bank".to_owned(),
                senses: vec![0],
                translations: Vec::new(),
            }],
            forms: Vec::new(),
            analyses: Vec::new(),
        };
        format::encode(&contents).expect("encode")
    }

    fn xz_of(bytes: &[u8]) -> Vec<u8> {
        let mut packed = Vec::new();
        lzma_rs::xz_compress(&mut &bytes[..], &mut packed).expect("compress");
        packed
    }

    /// The names of the entries of `dir`, sorted.
    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("the directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("utf-8")
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_tag_and_the_asset_follow_the_format_version() {
        assert_eq!(format::VERSION, 3);
        assert_eq!(tag(), "indexes-v3");
        assert_eq!(asset_name("oewn"), "oewn.index.xz");
        assert_eq!(MANIFEST, "manifest.json");
    }

    #[test]
    fn a_name_is_the_source_of_its_row() {
        assert_eq!(source_of("oewn"), Some(Source::Oewn));
        assert_eq!(source_of("freedict"), Some(Source::Freedict));
        assert_eq!(source_of("wikdict-ru-ca"), Some(Source::WikdictRuCa));
        assert_eq!(source_of("oewn.index"), None);
        assert_eq!(source_of("tessdata"), None);
        assert_eq!(source_of(""), None);
    }

    #[test]
    fn a_manifest_reads_back_what_it_writes() {
        let xz = xz_of(&index_bytes());
        let manifest = Manifest {
            format: format::VERSION,
            indexes: vec![Published::of("oewn", &xz)],
        };
        let published = &manifest.indexes[0];
        assert_eq!(published.size, xz.len() as u64);
        assert_eq!(published.sha256, sha256(&xz));
        assert_eq!(published.sha256.len(), 64);
        assert_eq!(published.asset(), "oewn.index.xz");
        let json = manifest.to_json();
        assert!(json.ends_with("}\n"), "{json}");
        assert_eq!(Manifest::parse(json.as_bytes()).expect("parse"), manifest);
    }

    #[test]
    fn a_garbled_manifest_or_one_of_another_format_is_refused() {
        assert!(Manifest::parse(b"").is_err());
        assert!(Manifest::parse(b"<html>Not Found</html>").is_err());
        let without_indexes = format!(r#"{{"format":{}}}"#, format::VERSION);
        assert!(Manifest::parse(without_indexes.as_bytes()).is_err());
        for older in 1..format::VERSION {
            let json = format!(r#"{{"format":{older},"indexes":[]}}"#);
            let error = Manifest::parse(json.as_bytes()).expect_err("an older format");
            assert!(error.contains(&format!("format {older}")), "{error}");
        }
        let empty = format!(r#"{{"format":{},"indexes":[]}}"#, format::VERSION);
        let empty = Manifest::parse(empty.as_bytes()).expect("parse");
        assert!(empty.indexes.is_empty());
    }

    #[test]
    fn an_asset_is_installed_into_a_readable_index() {
        let dir = TempDir::new("install");
        let data = dir.path().join("data/addict");
        let path = install(&xz_of(&index_bytes()), Source::Oewn, &data).expect("install");
        assert_eq!(path, data.join("oewn.index"));
        let index = Index::open(&path).expect("open");
        assert_eq!(index.source(), "oewn-fixture");
        assert!(index.find("bank").expect("find").is_some());
        assert_eq!(entries(&data), ["oewn.index"]);
    }

    #[test]
    fn an_install_replaces_the_previous_index() {
        let dir = TempDir::new("install-again");
        let path = import::index_path(dir.path(), Source::Oewn);
        fs::write(&path, b"an older index").expect("write");
        install(&xz_of(&index_bytes()), Source::Oewn, dir.path()).expect("install");
        assert_eq!(fs::read(&path).expect("index"), index_bytes());
        assert_eq!(entries(dir.path()), ["oewn.index"]);
    }

    #[test]
    fn an_asset_that_is_not_xz_writes_nothing() {
        let dir = TempDir::new("not-xz");
        let path = import::index_path(dir.path(), Source::Oewn);
        fs::write(&path, b"the old index").expect("write");
        let error = install(&index_bytes(), Source::Oewn, dir.path()).expect_err("not xz");
        assert!(matches!(error, InstallError::NotXz { .. }), "{error:?}");
        assert_eq!(error.source(), Source::Oewn);
        let text = error.to_string();
        assert!(text.contains("oewn") && !text.contains('\n'), "{text}");
        assert_eq!(fs::read(&path).expect("index"), b"the old index");
        assert_eq!(entries(dir.path()), ["oewn.index"]);
    }

    #[test]
    fn an_asset_that_is_not_an_index_writes_nothing() {
        let dir = TempDir::new("not-index");
        let path = import::index_path(dir.path(), Source::Oewn);
        fs::write(&path, b"the old index").expect("write");
        let mut damaged = index_bytes();
        damaged.truncate(damaged.len() / 2);
        for bytes in [b"not an index at all".to_vec(), damaged] {
            let error = install(&xz_of(&bytes), Source::Oewn, dir.path()).expect_err("bad index");
            assert!(
                matches!(error, InstallError::NotAnIndex { .. }),
                "{error:?}"
            );
            let text = error.to_string();
            assert!(text.contains("oewn") && !text.contains('\n'), "{text}");
            assert_eq!(fs::read(&path).expect("index"), b"the old index");
            assert_eq!(entries(dir.path()), ["oewn.index"]);
        }
        // Nothing at all when there was no index before.
        fs::remove_file(&path).expect("remove");
        assert!(install(&xz_of(b"nothing"), Source::Oewn, dir.path()).is_err());
        assert!(entries(dir.path()).is_empty());
    }

    #[test]
    fn an_asset_whose_notice_is_damaged_writes_nothing() {
        let dir = TempDir::new("notice-damaged");
        let path = import::index_path(dir.path(), Source::Oewn);
        fs::write(&path, b"the old index").expect("write");
        let contents = format::tests::sample();
        let mut damaged =
            format::encode_with_notice(&contents, &format::tests::notice()).expect("encode");
        // The last byte of the file is the last of the notice, its hash.
        damaged.push(0);
        let at = 16 + format::SectionId::Notice as usize * 16 + 8;
        let len = u64::from_le_bytes(damaged[at..at + 8].try_into().expect("length"));
        damaged[at..at + 8].copy_from_slice(&(len + 1).to_le_bytes());
        let error = install(&xz_of(&damaged), Source::Oewn, dir.path()).expect_err("bad notice");
        assert!(
            matches!(error, InstallError::NotAnIndex { .. }),
            "{error:?}"
        );
        assert_eq!(fs::read(&path).expect("index"), b"the old index");
        assert_eq!(entries(dir.path()), ["oewn.index"]);
        // The same file with its notice whole installs.
        damaged.pop();
        damaged[at..at + 8].copy_from_slice(&len.to_le_bytes());
        install(&xz_of(&damaged), Source::Oewn, dir.path()).expect("install");
        let index = Index::open(&path).expect("open");
        assert_eq!(index.notice(), Some(&format::tests::notice()));
    }

    #[test]
    fn a_stop_before_the_rename_leaves_the_old_index_and_no_temporary_file() {
        use std::cell::Cell;

        let dir = TempDir::new("stopped");
        let path = import::index_path(dir.path(), Source::Oewn);
        fs::write(&path, b"the old index").expect("write");
        let xz = xz_of(&index_bytes());
        // Not stopped before the unpacking, stopped right before the rename.
        let asked = Cell::new(0);
        let error = install_unless(&xz, Source::Oewn, dir.path(), || {
            asked.set(asked.get() + 1);
            asked.get() > 1
        })
        .expect_err("stopped");
        assert_eq!(asked.get(), 2);
        assert!(matches!(error, InstallError::Stopped { .. }), "{error:?}");
        assert_eq!(error.source(), Source::Oewn);
        let text = error.to_string();
        assert!(text.contains("oewn") && !text.contains('\n'), "{text}");
        assert_eq!(fs::read(&path).expect("index"), b"the old index");
        assert_eq!(entries(dir.path()), ["oewn.index"]);
        // Stopped from the start: nothing is unpacked or written.
        let error = install_unless(&xz, Source::Oewn, dir.path(), || true).expect_err("stopped");
        assert!(matches!(error, InstallError::Stopped { .. }), "{error:?}");
        assert_eq!(entries(dir.path()), ["oewn.index"]);
        // Never stopped: installed.
        install_unless(&xz, Source::Oewn, dir.path(), || false).expect("install");
        assert_eq!(fs::read(&path).expect("index"), index_bytes());
    }

    #[test]
    fn the_temporary_files_other_processes_left_are_swept() {
        let dir = TempDir::new("sweep");
        // This process's own temporary file is its install's, which writes
        // it anew and renames it, so it is not among them.
        let foreign = std::process::id().wrapping_add(1);
        let kept = [
            format!(".freedict.index.{foreign}.tmp"),
            ".oewn.index.tmp".to_owned(),
            ".oewn.index.12a.tmp".to_owned(),
            "notes.txt".to_owned(),
        ];
        for name in kept.iter().chain([&format!(".oewn.index.{foreign}.tmp")]) {
            fs::write(dir.path().join(name), b"left").expect("write");
        }
        install(&xz_of(&index_bytes()), Source::Oewn, dir.path()).expect("install");
        let mut expected: Vec<String> = kept.to_vec();
        expected.push("oewn.index".to_owned());
        expected.sort();
        assert_eq!(entries(dir.path()), expected);
    }

    #[test]
    fn an_install_into_a_directory_that_cannot_be_made_writes_nothing() {
        let dir = TempDir::new("blocked");
        let blocked = dir.path().join("file");
        fs::write(&blocked, b"").expect("a file");
        let error = install(
            &xz_of(&index_bytes()),
            Source::Oewn,
            &blocked.join("addict"),
        )
        .expect_err("");
        assert!(matches!(error, InstallError::Write { .. }), "{error:?}");
        assert!(error.to_string().contains("oewn"), "{error}");
        assert_eq!(entries(dir.path()), ["file"]);
    }
}
