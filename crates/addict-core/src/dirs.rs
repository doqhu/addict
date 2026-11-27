//! Product directories supplied by the platform shell.

use std::path::PathBuf;

/// Locations for persistent settings, dictionary indexes and the translation cache.
#[derive(Debug, Clone, Default)]
pub struct Dirs {
    pub config: Option<PathBuf>,
    pub data: Option<PathBuf>,
    pub cache: Option<PathBuf>,
}
