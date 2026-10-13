//! Shared helpers of the client tests: a private temporary directory and a
//! key file with a given content and mode in it.
#![allow(dead_code)]

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

/// A key no service knows; the tests look for it where it must not be.
pub const KEY: &str = "0a1b2c3d-test-key-of-the-stub";

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory of one test, removed when the test ends.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "addict-deepl-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).expect("create the temporary directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Where the key file of this directory is, written or not.
    pub fn key_path(&self) -> PathBuf {
        self.0.join("deepl.key")
    }

    /// Writes the key file with `content` and `mode` and gives its path.
    pub fn key_file(&self, content: &str, mode: u32) -> PathBuf {
        let path = self.key_path();
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, content).expect("write the key file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("set the mode of the key file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
