//! The Vulkan drivers the resident lets the loader list, chosen before GTK
//! starts so that creating the renderer does not wake a runtime-suspended
//! discrete GPU.
//!
//! A `VK_LOADER_DRIVERS_SELECT` already in the environment is kept as it is.
//! Otherwise the file `vulkan-drivers` of the configuration directory,
//! [`addict_deepl::key::config_dir`], decides: `all` lets the loader list
//! every driver, another nonblank line is set as the variable, trimmed and
//! verbatim, and a file that is missing, cannot be read, is not a regular
//! file or holds no nonblank line leaves the choice to
//! [`addict_platform::gpu::display_drivers`], which names the drivers of the
//! GPU that drives the display or nothing. Nothing is said about any of it.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

/// The variable of the Vulkan loader that limits the drivers it lists.
const VARIABLE: &str = "VK_LOADER_DRIVERS_SELECT";

/// The name of the file inside the configuration directory.
const DRIVERS_FILE: &str = "vulkan-drivers";

/// How many bytes of the file are read.
const DRIVERS_LIMIT: u64 = 256;

/// The DRM class directory of sysfs.
const DRM: &str = "/sys/class/drm";

/// What the file asks for.
#[derive(Debug, PartialEq, Eq)]
enum Choice {
    /// Every driver: the variable is not set.
    All,
    /// These globs, set verbatim.
    Drivers(String),
    /// The drivers of the GPU that drives the display, if any.
    Automatic,
}

impl Choice {
    /// The choice of the file's `content`: its first nonblank line, trimmed,
    /// `all` or globs; automatic for no content and no nonblank line.
    fn of(content: Option<&str>) -> Self {
        let Some(line) = content
            .and_then(|content| content.lines().map(str::trim).find(|line| !line.is_empty()))
        else {
            return Self::Automatic;
        };
        if line == "all" {
            Self::All
        } else {
            Self::Drivers(line.to_owned())
        }
    }
}

/// Sets `VK_LOADER_DRIVERS_SELECT` for the resident, unless it is set or the
/// rules choose no restriction. It runs in `main`, before GTK starts.
pub fn select_vulkan_drivers() {
    let Some(drivers) = selection(
        std::env::var_os(VARIABLE).is_some(),
        || path().and_then(|path| read(&path).ok()),
        || addict_platform::gpu::display_drivers(Path::new(DRM)).map(str::to_owned),
    ) else {
        return;
    };
    // SAFETY: this runs in `main` before `gtk::init` and before any thread of
    // the process exists, so nothing reads or writes the environment at the
    // same time.
    unsafe { std::env::set_var(VARIABLE, drivers) };
}

/// The value to set: none when the variable is `already_set`, then the
/// choice of the file from `read_file`, which detects with `detect` only
/// when it is automatic.
fn selection(
    already_set: bool,
    read_file: impl FnOnce() -> Option<String>,
    detect: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if already_set {
        return None;
    }
    match Choice::of(read_file().as_deref()) {
        Choice::All => None,
        Choice::Drivers(drivers) => Some(drivers),
        Choice::Automatic => detect(),
    }
}

/// The drivers file of the product, or `None` when the environment names no
/// configuration directory. Nothing is read here.
fn path() -> Option<PathBuf> {
    Some(addict_deepl::key::config_dir()?.join(DRIVERS_FILE))
}

/// The first bytes of the regular file at `path`, as text.
fn read(path: &Path) -> io::Result<String> {
    // Before opening: opening a pipe would wait for its writer.
    if !fs::metadata(path)?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut content = String::new();
    File::open(path)?
        .take(DRIVERS_LIMIT)
        .read_to_string(&mut content)?;
    Ok(content)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, os::unix::fs::PermissionsExt, path::PathBuf};

    use addict_platform::gpu::INTEL_DRIVERS;

    use super::{Choice, read, selection};

    /// A directory of one test, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "addict-vulkan-drivers-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("the directory of the test");
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("vulkan-drivers")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The selection for the file `content` with `detected` as what sysfs
    /// gives, and whether sysfs was read.
    fn select(content: Option<&str>, detected: Option<&str>) -> (Option<String>, bool) {
        let detects = Cell::new(false);
        let chosen = selection(
            false,
            || content.map(str::to_owned),
            || {
                detects.set(true);
                detected.map(str::to_owned)
            },
        );
        (chosen, detects.get())
    }

    #[test]
    fn the_file_chooses_all_globs_or_automatic() {
        assert_eq!(Choice::of(Some("all\n")), Choice::All);
        assert_eq!(Choice::of(Some("  all  \n")), Choice::All);
        assert_eq!(
            Choice::of(Some("\n  nvidia_icd*  \nradeon_icd*\n")),
            Choice::Drivers("nvidia_icd*".to_owned())
        );
        for content in ["", "  \n\t\n", "\n"] {
            assert_eq!(Choice::of(Some(content)), Choice::Automatic, "{content:?}");
        }
        assert_eq!(Choice::of(None), Choice::Automatic);
    }

    #[test]
    fn without_a_file_the_display_gpu_decides() {
        assert_eq!(
            select(None, Some(INTEL_DRIVERS)),
            (Some(INTEL_DRIVERS.to_owned()), true)
        );
        // NVIDIA on the display, one GPU, no or two marks: nothing is set.
        assert_eq!(select(None, None), (None, true));
        assert_eq!(
            select(Some(" \n"), Some(INTEL_DRIVERS)),
            (Some(INTEL_DRIVERS.to_owned()), true)
        );
    }

    #[test]
    fn all_sets_nothing_and_reads_no_sysfs() {
        assert_eq!(select(Some("all\n"), Some(INTEL_DRIVERS)), (None, false));
    }

    #[test]
    fn a_glob_is_set_verbatim_and_reads_no_sysfs() {
        assert_eq!(
            select(Some(" nvidia_icd* \n"), Some(INTEL_DRIVERS)),
            (Some("nvidia_icd*".to_owned()), false)
        );
    }

    #[test]
    fn a_set_variable_is_kept_and_nothing_is_read() {
        let reads = Cell::new(false);
        let detects = Cell::new(false);
        let chosen = selection(
            true,
            || {
                reads.set(true);
                Some("nvidia_icd*".to_owned())
            },
            || {
                detects.set(true);
                Some(INTEL_DRIVERS.to_owned())
            },
        );
        assert_eq!(chosen, None);
        assert!(!reads.get() && !detects.get());
    }

    #[test]
    fn a_missing_unreadable_or_odd_file_reads_as_nothing() {
        let dir = TempDir::new("read");
        let file = dir.file();
        // Missing.
        assert!(read(&file).is_err());
        // A regular file.
        std::fs::write(&file, "intel_icd*\n").expect("write");
        assert_eq!(read(&file).expect("read"), "intel_icd*\n");
        // Longer than the limit: only its first 256 bytes.
        std::fs::write(&file, "a".repeat(1000)).expect("write");
        assert_eq!(read(&file).expect("read").len(), 256);
        // Bytes that are not text.
        std::fs::write(&file, [0xff, 0xfe, b'x']).expect("write");
        assert!(read(&file).is_err());
        // A regular file the process may not read; root reads it anyway.
        std::fs::write(&file, "intel_icd*\n").expect("write");
        std::fs::set_permissions(&file, PermissionsExt::from_mode(0o000)).expect("chmod");
        // SAFETY: geteuid has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } != 0 {
            assert!(read(&file).is_err());
        }
        // A directory in the place of the file.
        std::fs::remove_file(&file).expect("remove");
        std::fs::create_dir(&file).expect("a directory");
        assert!(read(&file).is_err());
    }
}
