use crate::protocol::Backend;
use std::{
    fs::OpenOptions,
    io,
    io::Write,
    path::{Path, PathBuf},
};
pub const MARKER: &str = "X-Addict-Exp4=true";

fn path(dir: &Path, backend: Backend) -> PathBuf {
    dir.join(format!("{}.desktop", backend.app_id()))
}
fn ours(path: &Path) -> io::Result<bool> {
    let metadata = std::fs::symlink_metadata(path)?;
    Ok(metadata.is_file()
        && std::fs::read_to_string(path)?
            .lines()
            .any(|line| line == MARKER))
}
pub fn install(dir: &Path, backend: Backend, executable: &Path) -> io::Result<PathBuf> {
    let executable = executable.canonicalize()?;
    let entry = path(dir, backend);
    if entry.try_exists()? && !ours(&entry)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing an unrelated desktop entry",
        ));
    }
    // Dangling symlinks must also be refused.
    if std::fs::symlink_metadata(&entry).is_ok() && !ours(&entry)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing a symlink/unrelated entry",
        ));
    }
    let base = addict_probe::desktop::entry_text(&executable);
    let text = base.replace("Name=Addict Probe", &format!("Name=Addict EXP-4 {:?}", backend))
        .replace(" caps\n", " --help\n").replace(addict_probe::desktop::MARKER, MARKER)
        .replace("Throwaway entry for the EXP-3 portal probe; remove with addict-probe uninstall-desktop", "EXP-4 window comparison; remove with addict-exp4 uninstall-desktop");
    std::fs::create_dir_all(dir)?;
    // NOFOLLOW protects the final path against symlink substitution.
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&entry)?;
    file.write_all(text.as_bytes())?;
    Ok(entry)
}
pub fn uninstall(dir: &Path, backend: Backend) -> io::Result<PathBuf> {
    let entry = path(dir, backend);
    match std::fs::symlink_metadata(&entry) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(entry),
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    if !ours(&entry)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing an unrelated desktop entry",
        ));
    }
    std::fs::remove_file(&entry)?;
    Ok(entry)
}
