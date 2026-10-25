//! The dictionary index: one file per source in the XDG data directory, all
//! in one format.
//!
//! [`format`] owns the layout and the writer, [`index`] reads a written file
//! by positioned reads and answers the `addict_core` dictionary contract,
//! [`combined`] answers it with one article from two open indexes, for the
//! word as written and for its dictionary forms, and gives the same result
//! per headword, [`lookup`] is the per-language hook from a picked word to
//! the headwords to try and the article of one language grouped by kind,
//! [`oewn`], [`freedict`], [`kaikki`], [`wikdict`], [`dbnary`] and
//! [`mythes`] turn a WordNet file (Open English WordNet, Open Multilingual
//! Wordnet), a FreeDict TEI file, a Kaikki JSON Lines file, a WikDict SQLite
//! database, a DBnary Turtle file and a MyThes thesaurus into the writer's
//! input, with what the first two do the same way in `xml` and the SQLite
//! file read by [`sqlite`], [`analysis`] is what a Spanish or Catalan verb
//! form is, from the tags of a Kaikki conjugation table, [`import`] is the
//! logic of `addict import`, and [`sources`] is the one table of dictionary
//! sources, with their licenses and credit lines, that CREDITS, About →
//! Credits and the downloader read.

#![forbid(unsafe_code)]

pub mod analysis;
pub mod combined;
pub mod dbnary;
pub mod format;
pub mod freedict;
pub mod import;
pub mod index;
pub mod kaikki;
pub mod lookup;
pub mod mythes;
pub mod oewn;
pub mod sources;
pub mod sqlite;
pub mod wikdict;
mod xml;

pub use combined::{Combined, Found};
pub use index::{Error, Index};

#[cfg(test)]
mod testdir {
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicU32, Ordering},
    };

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A directory of one test, removed when the test ends.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "addict-dictionary-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir(&path).expect("create temp dir");
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
