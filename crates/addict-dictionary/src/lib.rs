//! The dictionary index: one file per source in the XDG data directory, all
//! in one format.
//!
//! [`format`] owns the layout and the writer, [`index`] reads a written file
//! by positioned reads and answers the `addict_core` dictionary contract,
//! [`oewn`] turns an Open English WordNet file into the writer's input and
//! [`import`] is the logic of `addict import`.

#![forbid(unsafe_code)]

pub mod format;
pub mod import;
pub mod index;
pub mod oewn;

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
