//! The logic of `addict import`: a known dictionary file in, its index out.
//!
//! A file is accepted when its byte size and then its SHA-256 equal an entry
//! of [`KNOWN`]; until both match, nothing is written. The file is read into
//! memory once, so the bytes that were hashed are the bytes that are parsed.
//! The index is built in memory, written to a temporary file beside its
//! final place and renamed over it: an import that fails leaves the previous
//! index as it was, and the same file gives the same index bytes.
//!
//! Each source has its own index file, and an import touches no other. The
//! WordNet file is a gzip of its XML, the Kaikki files gzips of JSON Lines.
//! The FreeDict and the Open Multilingual Wordnet files are xz of a tar
//! archive, unpacked in memory; of its members only the dictionary,
//! [`FREEDICT_MEMBER`] or [`OMW_ES_MEMBER`], is read. The WikDict files are
//! SQLite databases, the DBnary file a bzip2 of Turtle, and the thesaurus a
//! MyThes `.dat` file.
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

use addict_core::translation::Source as Language;

use crate::{dbnary, format, freedict, kaikki, mythes, oewn, wikdict};

/// A dictionary source. Each has one index file in the data directory, in
/// the one index format; what the file holds and in which language is the
/// source's, not the file's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Open English WordNet: gzip of WN-LMF XML.
    Oewn,
    /// FreeDict: xz of a tar archive with the dictionary in TEI XML.
    Freedict,
    /// The Spanish Wiktionary's Spanish entries from Kaikki: gzip of JSON
    /// Lines.
    KaikkiEs,
    /// WikDict Spanish-Russian: an SQLite database.
    WikdictEsRu,
    /// The Spanish wordnet of Open Multilingual Wordnet 2.0: xz of a tar
    /// archive with WN-LMF XML.
    OmwEs,
    /// The English Wiktionary's Spanish entries from Kaikki: gzip of JSON
    /// Lines.
    KaikkiEnSpanish,
    /// WikDict Catalan-Russian: an SQLite database.
    WikdictCaRu,
    /// DBnary's Catalan Wiktionary in Ontolex: bzip2 of Turtle.
    DbnaryCa,
    /// Softcatalà's Catalan thesaurus: a MyThes `.dat` file.
    SinonimsCat,
    /// The English Wiktionary's Catalan entries from Kaikki: gzip of JSON
    /// Lines.
    KaikkiEnCatalan,
}

/// What the senses or the translations of an index are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Definitions in the language of the words, with synonyms per sense.
    Definitions,
    /// Russian translations of the words.
    Russian,
    /// Synonyms of the words in their own language, from the members of the
    /// senses; the definitions of the senses are not shown.
    Synonyms,
    /// English glosses of the words: the definitions of the senses are
    /// English phrases, not definitions in the language of the words.
    English,
}

impl Source {
    /// The name of the index file of the source.
    pub fn index_name(self) -> &'static str {
        match self {
            Self::Oewn => "oewn.index",
            Self::Freedict => "freedict.index",
            Self::KaikkiEs => "kaikki-es-Espanol.index",
            Self::WikdictEsRu => "wikdict-es-ru.index",
            Self::OmwEs => "omw-es.index",
            Self::KaikkiEnSpanish => "kaikki-en-Spanish.index",
            Self::WikdictCaRu => "wikdict-ca-ru.index",
            Self::DbnaryCa => "dbnary-ca.index",
            Self::SinonimsCat => "sinonims-cat.index",
            Self::KaikkiEnCatalan => "kaikki-en-Catalan.index",
        }
    }

    /// The language of the headwords of the source.
    pub fn language(self) -> Language {
        match self {
            Self::Oewn | Self::Freedict => Language::En,
            Self::KaikkiEs | Self::WikdictEsRu | Self::OmwEs | Self::KaikkiEnSpanish => {
                Language::Es
            }
            Self::WikdictCaRu | Self::DbnaryCa | Self::SinonimsCat | Self::KaikkiEnCatalan => {
                Language::Ca
            }
        }
    }

    /// What the source gives for its headwords.
    pub fn kind(self) -> Kind {
        match self {
            Self::Oewn | Self::KaikkiEs | Self::DbnaryCa => Kind::Definitions,
            Self::Freedict | Self::WikdictEsRu | Self::WikdictCaRu => Kind::Russian,
            Self::OmwEs | Self::SinonimsCat => Kind::Synonyms,
            Self::KaikkiEnSpanish | Self::KaikkiEnCatalan => Kind::English,
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

/// The chosen files of the product (the spec's dictionary data): English,
/// then Spanish, then Catalan, each language in the order Russian,
/// definitions, synonyms, English. An article shows the kinds of a language
/// in this order.
pub const KNOWN: &[KnownFile<'static>] = &[
    KnownFile {
        name: "english-wordnet-2025.xml.gz",
        edition: "oewn-2025",
        source: Source::Oewn,
        size: 11_363_503,
        sha256: "9ca6d1dcb75f822fdd66617f7d9da48142ace38dd544d6ad5e2feca1674ad3fe",
    },
    KnownFile {
        name: "freedict-eng-rus-2025.11.23.src.tar.xz",
        edition: "freedict-eng-rus-2025.11.23",
        source: Source::Freedict,
        size: 3_948_540,
        sha256: "f3dadcc29d7df6e77edb13b98a2ba2c3fac002852eddd8b557637f0b13d39c39",
    },
    // `wikdict-es-ru` of EXP-9: WikDict 2_2026-06.
    KnownFile {
        name: "es-ru.sqlite3",
        edition: "wikdict-es-ru-2026-06",
        source: Source::WikdictEsRu,
        size: 5_963_776,
        sha256: "4c941b5df1809575e47d5e0524a683eb03a3a1941b731b7434be10a74b6e78f6",
    },
    // `kaikki-es-Espanol` of EXP-9: the eswiktionary dump of 2026-10-01,
    // extracted on 2026-10-16. Kaikki replaces the file about weekly under
    // the same URL; only this copy is known.
    KnownFile {
        name: "kaikki.org-dictionary-Español.jsonl.gz",
        edition: "kaikki-es-Espanol-2026-10-16",
        source: Source::KaikkiEs,
        size: 108_490_455,
        sha256: "fee9c37580717d7faa69e951515a94fa0aaa77b816d2f9f96a0fb8d44b87b9bc",
    },
    // `omw-es-2.0` of EXP-9: Open Multilingual Wordnet 2.0.
    KnownFile {
        name: "omw-es-2.0.tar.xz",
        edition: "omw-es-2.0",
        source: Source::OmwEs,
        size: 3_232_924,
        sha256: "d8450d42885cd51f3db39fe64219a7a003eeb432b4caa00428285fe6ab224303",
    },
    // `kaikki-en-Spanish` of EXP-9: the enwiktionary dump of 2026-09-02,
    // extracted on 2026-10-03.
    KnownFile {
        name: "kaikki.org-dictionary-Spanish.jsonl.gz",
        edition: "kaikki-en-Spanish-2026-10-03",
        source: Source::KaikkiEnSpanish,
        size: 95_164_223,
        sha256: "c805cfb9f13c1769400d08e4a01bc4aeefb67775cbb36e2e4deb199e0af57b38",
    },
    // `wikdict-ca-ru` of EXP-9: WikDict 2_2026-06.
    KnownFile {
        name: "ca-ru.sqlite3",
        edition: "wikdict-ca-ru-2026-06",
        source: Source::WikdictCaRu,
        size: 3_076_096,
        sha256: "01cf2923abad3086781da0638245cd86d2edb70f23bcea7eb149caabb7ef8421",
    },
    // `dbnary-ca-ontolex` of EXP-9: the DBnary dump of 2026-10-01.
    KnownFile {
        name: "ca_dbnary_ontolex_2026-10-01.ttl.bz2",
        edition: "dbnary-ca-ontolex-2026-10-01",
        source: Source::DbnaryCa,
        size: 20_657_385,
        sha256: "bb52ce1aee0164ea276a74586cf24a38ef6b118047eb9f609d6da2c3eddf4142",
    },
    // `sinonims-cat-2.3.2` of EXP-9: Softcatalà's thesaurus 2.3.2.
    KnownFile {
        name: "th_ca_ES.dat",
        edition: "sinonims-cat-2.3.2",
        source: Source::SinonimsCat,
        size: 12_078_115,
        sha256: "ab637e5a780e89b3a10b91f307b7d6811e8f335a259102f1d329ad9d2a4a090e",
    },
    // `kaikki-en-Catalan` of EXP-9: the enwiktionary dump of 2026-09-02,
    // extracted on 2026-10-03.
    KnownFile {
        name: "kaikki.org-dictionary-Catalan.jsonl.gz",
        edition: "kaikki-en-Catalan-2026-10-03",
        source: Source::KaikkiEnCatalan,
        size: 23_331_032,
        sha256: "7cc434210347de5d15723d299509c21f49a68a5029fb1145a59f0f582bdf12a5",
    },
];

/// The member of the FreeDict archive that is the dictionary.
pub const FREEDICT_MEMBER: &str = "eng-rus/eng-rus.tei";

/// The member of the Open Multilingual Wordnet archive that is the wordnet.
pub const OMW_ES_MEMBER: &str = "omw-es/omw-es.xml";

/// The `lexvo:` code of the language of the DBnary entries read.
const DBNARY_CA_LANGUAGE: &str = "cat";

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
        Source::Freedict => archive_member(&bytes, FREEDICT_MEMBER, |member| {
            freedict::parse(member, known.edition).map_err(|error| error.to_string())
        }),
        Source::OmwEs => archive_member(&bytes, OMW_ES_MEMBER, |member| {
            oewn::parse(member, known.edition).map_err(|error| error.to_string())
        }),
        Source::KaikkiEs | Source::KaikkiEnSpanish | Source::KaikkiEnCatalan => kaikki::parse(
            BufReader::new(MultiGzDecoder::new(bytes.as_slice())),
            known.source.language().code(),
            known.edition,
        )
        .map_err(|error| error.to_string()),
        Source::WikdictEsRu | Source::WikdictCaRu => {
            wikdict::parse(&bytes, known.edition).map_err(|error| error.to_string())
        }
        Source::DbnaryCa => dbnary_file(&bytes, known.edition),
        Source::SinonimsCat => {
            mythes::parse(bytes.as_slice(), known.edition).map_err(|error| error.to_string())
        }
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

/// The contents of the index of an xz of a tar archive: the xz stream is
/// unpacked, and `parse` reads the member `wanted` of the tar archive
/// inside it.
fn archive_member(
    archive: &[u8],
    wanted: &str,
    parse: impl FnOnce(BufReader<tar::Entry<'_, &[u8]>>) -> Result<format::Contents, String>,
) -> Result<format::Contents, String> {
    let mut packed = archive;
    let mut unpacked = Vec::new();
    lzma_rs::xz_decompress(&mut packed, &mut unpacked)
        .map_err(|error| format!("not an xz archive: {}", one_line(&error)))?;
    let not_tar = |error: io::Error| format!("not a tar archive: {}", one_line(&error));
    let mut members = tar::Archive::new(unpacked.as_slice());
    for member in members.entries().map_err(not_tar)? {
        let member = member.map_err(not_tar)?;
        if member.header().entry_type().is_file()
            && member.path_bytes().as_ref() == wanted.as_bytes()
        {
            return parse(BufReader::new(member));
        }
    }
    Err(format!("the archive has no member {wanted}"))
}

/// The contents of the index of the DBnary file: a bzip2 stream of Turtle.
fn dbnary_file(file: &[u8], edition: &str) -> Result<format::Contents, String> {
    if !file.starts_with(b"BZh") {
        return Err("not a bzip2 file".to_owned());
    }
    let turtle = BufReader::new(bzip2::bufread::MultiBzDecoder::new(file));
    dbnary::parse(turtle, DBNARY_CA_LANGUAGE, edition).map_err(|error| one_line(&error))
}

/// The text of a foreign error on one line.
fn one_line(error: &dyn fmt::Display) -> String {
    error
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
    use crate::{
        dbnary::tests::FIXTURE as TURTLE,
        freedict::tests::FIXTURE as TEI,
        index::Index,
        kaikki::tests::FIXTURE as KAIKKI,
        mythes::tests::FIXTURE as THESAURUS,
        oewn::tests::{FIXTURE, OMW},
        testdir::TempDir,
        wikdict::tests::database as wikdict,
    };

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
    fn the_known_files_are_the_chosen_wordnet_freedict_and_spanish_kaikki() {
        assert_eq!(KNOWN.len(), 10);
        assert_eq!(KNOWN[1].name, "freedict-eng-rus-2025.11.23.src.tar.xz");
        assert_eq!(KNOWN[1].edition, "freedict-eng-rus-2025.11.23");
        assert_eq!(KNOWN[1].size, 3948540);
        assert_eq!(
            KNOWN[1].sha256,
            "f3dadcc29d7df6e77edb13b98a2ba2c3fac002852eddd8b557637f0b13d39c39"
        );
        assert_eq!(KNOWN[1].source, Source::Freedict);
        assert_eq!(
            index_path(Path::new("/d"), Source::Freedict),
            Path::new("/d/freedict.index")
        );
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

    #[test]
    fn the_spanish_source_is_the_chosen_kaikki_file_with_its_kind_and_language() {
        let known = KNOWN[3];
        assert_eq!(known.source, Source::KaikkiEs);
        assert_eq!(known.size, 108490455);
        assert_eq!(
            known.sha256,
            "fee9c37580717d7faa69e951515a94fa0aaa77b816d2f9f96a0fb8d44b87b9bc"
        );
        assert_eq!(
            index_path(Path::new("/d"), Source::KaikkiEs),
            Path::new("/d/kaikki-es-Espanol.index")
        );
        assert_eq!(Source::KaikkiEs.language(), Language::Es);
        assert_eq!(Source::KaikkiEs.kind(), Kind::Definitions);
        assert_eq!(Source::Oewn.language(), Language::En);
        assert_eq!(Source::Oewn.kind(), Kind::Definitions);
        assert_eq!(Source::Freedict.language(), Language::En);
        assert_eq!(Source::Freedict.kind(), Kind::Russian);
        // One index file per source.
        let mut names: Vec<_> = KNOWN.iter().map(|k| k.source.index_name()).collect();
        names.dedup();
        assert_eq!(names.len(), KNOWN.len());
    }

    #[test]
    fn the_kaikki_file_is_imported_into_its_own_readable_index() {
        let fixture = Fixture::new("kaikki");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(KAIKKI.as_bytes()).expect("compress");
        let gz = encoder.finish().expect("compress");
        let archive = Archive::new(&fixture, "kaikki.jsonl.gz", &gz);
        let [wordnet] = fixture.known();
        let known = [
            wordnet,
            KnownFile {
                name: "kaikki.jsonl.gz",
                edition: "fixture-es-1",
                source: Source::KaikkiEs,
                size: archive.size,
                sha256: &archive.sha256,
            },
        ];
        let data = fixture.data();
        let oewn = import_known(&fixture.file(), &data, &known).expect("wordnet");
        let oewn_bytes = fs::read(&oewn.path).expect("index");

        let imported = import_known(&archive.file, &data, &known).expect("import");
        assert_eq!(imported.path, data.join("kaikki-es-Espanol.index"));
        assert_eq!(imported.headwords, 3);
        assert_eq!(imported.exception_forms, 3);
        assert_eq!(fs::read(&oewn.path).expect("index"), oewn_bytes);
        let bytes = fs::read(&imported.path).expect("index");
        let again = import_known(&archive.file, &data, &known).expect("again");
        assert_eq!(again, imported);
        assert_eq!(fs::read(&imported.path).expect("index"), bytes);

        let index = Index::open(&imported.path).expect("open");
        assert_eq!(index.source(), "fixture-es-1");
        let banco = index.find("banco").expect("find").expect("banco");
        assert_eq!(banco.senses.len(), 4);
        assert_eq!(banco.senses[1].synonyms, ["banca"]);
        assert_eq!(banco.senses[2].synonyms, ["banquillo"]);
        assert!(banco.translations.is_empty());
        assert_eq!(index.find("zzz").expect("find"), None);
        assert_eq!(index.exception_lemmas("cerró").expect("form"), ["cerrar"]);

        // Another size or hash writes nothing, as for the other sources.
        let short = Archive::new(&fixture, "short.jsonl.gz", &gz[..gz.len() - 3]);
        assert!(matches!(
            import_known(&short.file, &data, &known),
            Err(ImportError::Size { .. })
        ));
        assert_eq!(fs::read(&imported.path).expect("index"), bytes);
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

    /// A tar archive of `members`, each a path and its bytes.
    fn tar_of(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, bytes) in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            builder
                .append_data(&mut header, path, *bytes)
                .expect("append");
        }
        builder.into_inner().expect("tar")
    }

    fn xz_of(bytes: &[u8]) -> Vec<u8> {
        let mut packed = Vec::new();
        lzma_rs::xz_compress(&mut &bytes[..], &mut packed).expect("compress");
        packed
    }

    /// A file that stands for the FreeDict archive, in the directory of an
    /// OEWN fixture, and the hash of that file.
    struct Archive {
        file: PathBuf,
        size: u64,
        sha256: String,
    }

    impl Archive {
        fn new(fixture: &Fixture, name: &str, bytes: &[u8]) -> Self {
            let file = fixture.dir.path().join(name);
            fs::write(&file, bytes).expect("write archive");
            Self {
                file,
                size: bytes.len() as u64,
                sha256: hex(bytes),
            }
        }

        /// The archive as `freedict-eng-rus` is published: the dictionary
        /// among other members.
        fn whole(fixture: &Fixture) -> Self {
            let tar = tar_of(&[
                ("eng-rus/COPYING", b"a licence"),
                ("eng-rus/README", b""),
                (FREEDICT_MEMBER, TEI.as_bytes()),
                ("eng-rus/freedict-P5.xml", b"<TEI>not the dictionary</TEI>"),
            ]);
            Self::new(fixture, "freedict.tar.xz", &xz_of(&tar))
        }

        fn known_as(&self, source: Source) -> KnownFile<'_> {
            KnownFile {
                name: "fixture.file",
                edition: "fixture-1",
                source,
                size: self.size,
                sha256: &self.sha256,
            }
        }

        fn known(&self) -> KnownFile<'_> {
            KnownFile {
                name: "fixture.src.tar.xz",
                edition: "fixture-ru-1",
                source: Source::Freedict,
                size: self.size,
                sha256: &self.sha256,
            }
        }

        /// Imports the archive where both the WordNet fixture and it are
        /// known.
        fn import(&self, fixture: &Fixture) -> Result<Imported, ImportError> {
            let [wordnet] = fixture.known();
            import_known(&self.file, &fixture.data(), &[wordnet, self.known()])
        }
    }

    #[test]
    fn the_freedict_archive_is_imported_into_a_readable_index() {
        let fixture = Fixture::new("freedict");
        let archive = Archive::whole(&fixture);
        let imported = archive.import(&fixture).expect("import");
        assert_eq!(imported.path, fixture.data().join("freedict.index"));
        assert_eq!(imported.headwords, 4);
        assert_eq!(imported.exception_forms, 0);

        let index = Index::open(&imported.path).expect("open");
        assert_eq!(index.source(), "fixture-ru-1");
        assert_eq!(index.headwords(), 4);
        assert_eq!(index.exception_forms(), 0);
        let bank = index.find("bank").expect("find").expect("bank");
        assert_eq!(bank.senses.len(), 4);
        assert!(bank.senses.iter().all(|sense| sense.synonyms.is_empty()));
        assert_eq!(
            bank.translations,
            ["бе́рег", "брег", "банк", "ряд", "накреня́ться"]
        );
        let unit = index.find("R&D").expect("find").expect("R&D");
        assert!(unit.senses.is_empty());
        assert_eq!(unit.translations, ["НИОКР"]);
        assert_eq!(index.find("Bank").expect("find"), None);
        let left: Vec<_> = fs::read_dir(fixture.data())
            .expect("data directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(left, ["freedict.index"]);
    }

    #[test]
    fn a_second_freedict_import_replaces_the_index_with_the_same_bytes() {
        let fixture = Fixture::new("freedict-again");
        let archive = Archive::whole(&fixture);
        let first = archive.import(&fixture).expect("import");
        let bytes = fs::read(&first.path).expect("index");
        let second = archive.import(&fixture).expect("import again");
        assert_eq!(first, second);
        assert_eq!(fs::read(&second.path).expect("index"), bytes);
    }

    #[test]
    fn an_import_leaves_the_index_of_the_other_source_as_it_was() {
        let fixture = Fixture::new("two-sources");
        let archive = Archive::whole(&fixture);
        let [wordnet] = fixture.known();
        let known = [wordnet, archive.known()];
        let data = fixture.data();

        let oewn = import_known(&fixture.file(), &data, &known).expect("wordnet");
        let oewn_bytes = fs::read(&oewn.path).expect("index");
        let freedict = import_known(&archive.file, &data, &known).expect("freedict");
        assert_ne!(oewn.path, freedict.path);
        assert_eq!(fs::read(&oewn.path).expect("index"), oewn_bytes);

        let freedict_bytes = fs::read(&freedict.path).expect("index");
        import_known(&fixture.file(), &data, &known).expect("wordnet again");
        assert_eq!(fs::read(&freedict.path).expect("index"), freedict_bytes);
        assert_eq!(fs::read(&oewn.path).expect("index"), oewn_bytes);

        // A refused file touches neither.
        let short = Archive::new(&fixture, "short.xz", b"short");
        assert!(import_known(&short.file, &data, &known).is_err());
        assert_eq!(fs::read(&freedict.path).expect("index"), freedict_bytes);
        assert_eq!(fs::read(&oewn.path).expect("index"), oewn_bytes);
        let mut left: Vec<_> = fs::read_dir(&data)
            .expect("data directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["freedict.index", "oewn.index"]);
    }

    #[test]
    fn another_size_or_hash_of_the_archive_writes_nothing() {
        let fixture = Fixture::new("freedict-refused");
        let archive = Archive::whole(&fixture);
        let whole = fs::read(&archive.file).expect("archive");
        let [wordnet] = fixture.known();
        let known = [wordnet, archive.known()];

        let truncated = fixture.dir.path().join("truncated.xz");
        fs::write(&truncated, &whole[..whole.len() - 9]).expect("write");
        let error = import_known(&truncated, &fixture.data(), &known).expect_err("size");
        assert!(matches!(error, ImportError::Size { .. }), "{error}");
        let text = one_line(&error);
        assert!(text.contains("truncated.xz"), "{text}");
        // The sizes of both known files.
        assert!(text.contains(&format!("{} bytes", whole.len())), "{text}");
        assert!(
            text.contains(&format!("{} bytes", fixture.gz.len())),
            "{text}"
        );
        assert!(!text.contains("SHA-256"), "{text}");

        let mut changed = whole.clone();
        changed[whole.len() / 2] ^= 1;
        let file = fixture.dir.path().join("changed.xz");
        fs::write(&file, &changed).expect("write");
        let error = import_known(&file, &fixture.data(), &known).expect_err("hash");
        assert!(matches!(error, ImportError::Hash { .. }), "{error}");
        let text = one_line(&error);
        assert!(text.contains("changed.xz"), "{text}");
        assert!(text.contains(&archive.sha256), "{text}");
        assert!(text.contains(&hex(&changed)), "{text}");
        // Only the known file of this size applies.
        assert!(!text.contains(&fixture.sha256), "{text}");
        assert!(!fixture.dir.path().join("data").exists());
    }

    #[test]
    fn a_known_archive_with_bad_contents_writes_nothing() {
        let fixture = Fixture::new("freedict-contents");
        let tei = TEI.as_bytes();
        let without_orth = TEI.replace("<orth>shore</orth>", "");
        let without_entries = "<TEI><text><body></body></text></TEI>";
        let cases: [(&str, Vec<u8>, &str); 8] = [
            ("not xz", b"this is not xz at all".to_vec(), "not an xz"),
            ("xz of no tar", xz_of(tei), "not a tar archive"),
            ("an empty archive", xz_of(&tar_of(&[])), FREEDICT_MEMBER),
            (
                "no dictionary member",
                xz_of(&tar_of(&[
                    ("eng-rus/COPYING", b"a licence"),
                    ("eng-rus.tei", tei),
                    ("eng-rus/eng-rus.tei.bak", tei),
                ])),
                FREEDICT_MEMBER,
            ),
            (
                "the member is not TEI",
                xz_of(&tar_of(&[(FREEDICT_MEMBER, FIXTURE.as_bytes())])),
                "not a FreeDict TEI file",
            ),
            (
                "the member is not XML",
                xz_of(&tar_of(&[(FREEDICT_MEMBER, b"plain text")])),
                "no entry",
            ),
            (
                "an entry without orth",
                xz_of(&tar_of(&[(FREEDICT_MEMBER, without_orth.as_bytes())])),
                "no orth",
            ),
            (
                "no entry",
                xz_of(&tar_of(&[(FREEDICT_MEMBER, without_entries.as_bytes())])),
                "no entry",
            ),
        ];
        for (name, bytes, cause) in cases {
            let archive = Archive::new(&fixture, "bad.tar.xz", &bytes);
            let error = archive.import(&fixture).expect_err(name);
            assert!(
                matches!(error, ImportError::Contents { .. }),
                "{name}: {error}"
            );
            let text = one_line(&error);
            assert!(text.contains("bad.tar.xz"), "{name}: {text}");
            assert!(text.contains(cause), "{name}: {text}");
            assert!(!fixture.dir.path().join("data").exists(), "{name}");
        }

        // The index of an earlier import stays.
        let imported = Archive::whole(&fixture).import(&fixture).expect("import");
        let before = fs::read(&imported.path).expect("index");
        let archive = Archive::new(&fixture, "bad.tar.xz", &xz_of(&tar_of(&[])));
        assert!(archive.import(&fixture).is_err());
        assert_eq!(fs::read(&imported.path).expect("index"), before);
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

    #[test]
    fn the_chosen_spanish_and_catalan_files_are_known_with_their_kinds() {
        let rows: Vec<_> = KNOWN
            .iter()
            .map(|known| {
                let source = known.source;
                (
                    source.index_name(),
                    source.language(),
                    source.kind(),
                    known.name,
                )
            })
            .collect();
        assert_eq!(
            rows[2..],
            [
                (
                    "wikdict-es-ru.index",
                    Language::Es,
                    Kind::Russian,
                    "es-ru.sqlite3"
                ),
                (
                    "kaikki-es-Espanol.index",
                    Language::Es,
                    Kind::Definitions,
                    "kaikki.org-dictionary-Español.jsonl.gz"
                ),
                (
                    "omw-es.index",
                    Language::Es,
                    Kind::Synonyms,
                    "omw-es-2.0.tar.xz"
                ),
                (
                    "kaikki-en-Spanish.index",
                    Language::Es,
                    Kind::English,
                    "kaikki.org-dictionary-Spanish.jsonl.gz"
                ),
                (
                    "wikdict-ca-ru.index",
                    Language::Ca,
                    Kind::Russian,
                    "ca-ru.sqlite3"
                ),
                (
                    "dbnary-ca.index",
                    Language::Ca,
                    Kind::Definitions,
                    "ca_dbnary_ontolex_2026-10-01.ttl.bz2"
                ),
                (
                    "sinonims-cat.index",
                    Language::Ca,
                    Kind::Synonyms,
                    "th_ca_ES.dat"
                ),
                (
                    "kaikki-en-Catalan.index",
                    Language::Ca,
                    Kind::English,
                    "kaikki.org-dictionary-Catalan.jsonl.gz"
                ),
            ]
        );
        let pins: Vec<_> = KNOWN[2..]
            .iter()
            .map(|known| (known.size, &known.sha256[..8], known.edition))
            .collect();
        assert_eq!(
            pins,
            [
                (5963776, "4c941b5d", "wikdict-es-ru-2026-06"),
                (108490455, "fee9c375", "kaikki-es-Espanol-2026-10-16"),
                (3232924, "d8450d42", "omw-es-2.0"),
                (95164223, "c805cfb9", "kaikki-en-Spanish-2026-10-03"),
                (3076096, "01cf2923", "wikdict-ca-ru-2026-06"),
                (20657385, "bb52ce1a", "dbnary-ca-ontolex-2026-10-01"),
                (12078115, "ab637e5a", "sinonims-cat-2.3.2"),
                (23331032, "7cc43421", "kaikki-en-Catalan-2026-10-03"),
            ]
        );
        assert!(KNOWN.iter().all(|known| known.sha256.len() == 64));
        // One index file per source.
        let mut names: Vec<_> = KNOWN.iter().map(|k| k.source.index_name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), KNOWN.len());
    }

    fn gz_of(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).expect("compress");
        encoder.finish().expect("compress")
    }

    fn bz2_of(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        encoder.write_all(bytes).expect("compress");
        encoder.finish().expect("compress")
    }

    /// English Wiktionary lines in the shape of `kaikki-en-Spanish` and
    /// `kaikki-en-Catalan`: English glosses, a form-of entry and a `forms`
    /// list, and an entry of the other language.
    const KAIKKI_EN: &str = concat!(
        r#"{"word": "banco", "lang_code": "es", "senses": [{"glosses": ["bank (financial institution)"]}, {"glosses": ["bench"]}], "forms": [{"form": "bancos", "tags": ["plural"]}]}"#,
        "\n",
        r#"{"word": "bancos", "lang_code": "es", "senses": [{"glosses": ["plural of banco"], "form_of": [{"word": "banco"}]}]}"#,
        "\n",
        r#"{"word": "col·lecció", "lang_code": "ca", "senses": [{"glosses": ["collection"]}], "forms": [{"form": "col·leccions", "tags": ["plural"]}]}"#,
        "\n",
        r#"{"word": "banc", "lang_code": "ca", "senses": [{"glosses": ["bench"]}, {"glosses": ["bank"]}], "synonyms": [{"word": "seient"}]}"#,
        "\n",
    );

    /// The file of `source` in the shape of its chosen file.
    fn file_of(source: Source) -> Vec<u8> {
        match source {
            Source::WikdictEsRu => wikdict(false),
            Source::WikdictCaRu => wikdict(true),
            Source::OmwEs => xz_of(&tar_of(&[
                ("omw-es/LICENSE", b"a licence"),
                (OMW_ES_MEMBER, OMW.as_bytes()),
            ])),
            Source::KaikkiEnSpanish => gz_of(KAIKKI_EN.as_bytes()),
            // Another file of the same lines: a file is known by its hash.
            Source::KaikkiEnCatalan => gz_of(format!("{KAIKKI_EN}\n").as_bytes()),
            Source::DbnaryCa => bz2_of(TURTLE.as_bytes()),
            Source::SinonimsCat => THESAURUS.as_bytes().to_vec(),
            Source::KaikkiEs => gz_of(KAIKKI.as_bytes()),
            Source::Oewn | Source::Freedict => unreachable!("an English source"),
        }
    }

    /// What the article of `word` in an index gives under its kind: the
    /// definitions, the Russian translations, or the synonyms.
    fn article(index: &Index, word: &str) -> (Vec<String>, Vec<Vec<String>>, Vec<String>) {
        let article = index
            .find(word)
            .expect("find")
            .unwrap_or_else(|| panic!("{word}"));
        (
            article
                .senses
                .iter()
                .map(|sense| sense.definition.clone())
                .collect(),
            article
                .senses
                .iter()
                .map(|sense| sense.synonyms.clone())
                .collect(),
            article.translations,
        )
    }

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn each_new_source_is_imported_into_its_own_index_with_the_same_bytes_again() {
        let new = [
            Source::WikdictEsRu,
            Source::OmwEs,
            Source::KaikkiEnSpanish,
            Source::WikdictCaRu,
            Source::DbnaryCa,
            Source::SinonimsCat,
            Source::KaikkiEnCatalan,
        ];
        let fixture = Fixture::new("every-source");
        let files: Vec<Archive> = new
            .iter()
            .map(|source| Archive::new(&fixture, source.index_name(), &file_of(*source)))
            .collect();
        let [wordnet] = fixture.known();
        let mut known = vec![wordnet];
        known.extend(new.iter().zip(&files).map(|(source, file)| KnownFile {
            name: source.index_name(),
            edition: "fixture-2",
            source: *source,
            size: file.size,
            sha256: &file.sha256,
        }));
        let data = fixture.data();
        let oewn = import_known(&fixture.file(), &data, &known).expect("wordnet");
        let oewn_bytes = fs::read(&oewn.path).expect("index");

        let mut written: Vec<(PathBuf, Vec<u8>)> = vec![(oewn.path.clone(), oewn_bytes)];
        for (source, file) in new.iter().zip(&files) {
            let imported = import_known(&file.file, &data, &known).expect("import");
            assert_eq!(imported.path, data.join(source.index_name()), "{source:?}");
            // Every index written before is as it was.
            for (path, bytes) in &written {
                assert_eq!(&fs::read(path).expect("index"), bytes, "{source:?}");
            }
            let bytes = fs::read(&imported.path).expect("index");
            let again = import_known(&file.file, &data, &known).expect("again");
            assert_eq!(again, imported, "{source:?}");
            assert_eq!(
                fs::read(&imported.path).expect("index"),
                bytes,
                "{source:?}"
            );
            written.push((imported.path, bytes));
        }
        let mut left: Vec<_> = fs::read_dir(&data)
            .expect("data directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("name")
            })
            .collect();
        left.sort();
        let mut expected: Vec<_> = new.iter().map(|source| source.index_name()).collect();
        expected.push("oewn.index");
        expected.sort_unstable();
        assert_eq!(left, expected);

        let open = |source: Source| {
            let index = Index::open(&data.join(source.index_name())).expect("open");
            assert_eq!(index.source(), "fixture-2");
            index
        };
        // Russian: the translations, no senses.
        let russian = open(Source::WikdictEsRu);
        assert_eq!(
            article(&russian, "banco"),
            (vec![], vec![], strings(&["банк", "скамья", "лавка"]))
        );
        let russian = open(Source::WikdictCaRu);
        assert_eq!(
            article(&russian, "col·lecció"),
            (vec![], vec![], strings(&["колле́кция", "коллекция"]))
        );
        assert_eq!(russian.exception_forms(), 0);
        // Synonyms: the members of the senses, without the word itself.
        let omw = open(Source::OmwEs);
        assert_eq!(
            article(&omw, "banco").1,
            [
                strings(&["entidad bancaria", "banca"]),
                strings(&["escaño"])
            ]
        );
        let thesaurus = open(Source::SinonimsCat);
        let (definitions, synonyms, translations) = article(&thesaurus, "col·lecció");
        assert!(definitions.iter().all(String::is_empty));
        assert_eq!(synonyms[1], strings(&["sèrie", "lot (m)", "joc de peces"]));
        assert!(translations.is_empty());
        // Definitions: the Catalan senses.
        let dbnary = open(Source::DbnaryCa);
        let (definitions, synonyms, _) = article(&dbnary, "col·lecció");
        assert_eq!(definitions.len(), 3);
        assert_eq!(definitions[0], "Conjunt d'objectes\nreunits.");
        assert!(synonyms.iter().all(Vec::is_empty));
        // English: the glosses of the language's entries, and the form table.
        let spanish = open(Source::KaikkiEnSpanish);
        assert_eq!(
            article(&spanish, "banco").0,
            strings(&["bank (financial institution)", "bench"])
        );
        assert_eq!(spanish.find("banc").expect("find"), None);
        assert_eq!(spanish.exception_lemmas("bancos").expect("form"), ["banco"]);
        let catalan = open(Source::KaikkiEnCatalan);
        assert_eq!(article(&catalan, "col·lecció").0, strings(&["collection"]));
        assert_eq!(catalan.find("banco").expect("find"), None);
        assert_eq!(
            catalan.exception_lemmas("col·leccions").expect("form"),
            ["col·lecció"]
        );
        assert_eq!(catalan.exception_forms(), 1);
    }

    #[test]
    fn a_known_file_of_a_new_source_with_bad_contents_writes_nothing() {
        let fixture = Fixture::new("new-contents");
        let mut builder = crate::sqlite::tests::Builder::new(512);
        let other = builder.leaf(&[]);
        let no_table = builder.finish(&[(
            "table",
            "simple_translation",
            other,
            "CREATE TABLE simple_translation(written_rep TEXT, trans_list)",
        )]);
        let cases: [(&str, Source, Vec<u8>, &str); 7] = [
            (
                "not SQLite",
                Source::WikdictEsRu,
                b"not a database".to_vec(),
                "not an SQLite database",
            ),
            (
                "no translation table",
                Source::WikdictCaRu,
                no_table,
                "no table translation",
            ),
            (
                "not bz2",
                Source::DbnaryCa,
                TURTLE.as_bytes().to_vec(),
                "not a bzip2 file",
            ),
            (
                "no CA entry",
                Source::DbnaryCa,
                bz2_of(KAIKKI_EN.as_bytes()),
                "no lexical entry",
            ),
            (
                "a cut bz2",
                Source::DbnaryCa,
                bz2_of(TURTLE.as_bytes())[..40].to_vec(),
                "cannot read",
            ),
            (
                "no member",
                Source::OmwEs,
                xz_of(&tar_of(&[("omw-es/LICENSE", b"a licence")])),
                OMW_ES_MEMBER,
            ),
            ("empty .dat", Source::SinonimsCat, Vec::new(), "empty"),
        ];
        for (name, source, bytes, cause) in cases {
            let file = Archive::new(&fixture, "bad.file", &bytes);
            let known = [KnownFile {
                name: "bad.file",
                edition: "bad-1",
                source,
                size: file.size,
                sha256: &file.sha256,
            }];
            let error = import_known(&file.file, &fixture.data(), &known).expect_err(name);
            assert!(
                matches!(error, ImportError::Contents { .. }),
                "{name}: {error}"
            );
            let text = one_line(&error);
            assert!(text.contains("bad.file"), "{name}: {text}");
            assert!(text.contains(cause), "{name}: {text}");
            assert!(!fixture.data().exists(), "{name}");
        }

        // The index of an earlier import stays.
        let good = Archive::new(&fixture, "good.dat", THESAURUS.as_bytes());
        let empty = Archive::new(&fixture, "empty.dat", b"");
        let known = [
            good.known_as(Source::SinonimsCat),
            empty.known_as(Source::SinonimsCat),
        ];
        let imported = import_known(&good.file, &fixture.data(), &known).expect("import");
        let before = fs::read(&imported.path).expect("index");
        assert!(import_known(&empty.file, &fixture.data(), &known).is_err());
        assert_eq!(fs::read(&imported.path).expect("index"), before);
    }
}
