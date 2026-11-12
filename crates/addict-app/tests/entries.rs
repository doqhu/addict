//! AppImage installation resolves the outer file before touching the profile.
mod common;

use common::{Homes, TempDir, run_with_timeout, text};
use std::{
    ffi::OsString,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::Path,
};

fn executable(path: &Path) {
    std::fs::write(path, b"image fixture").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn installs_the_canonical_outer_image_and_explicit_reinstall_repairs_a_move() {
    let temp = TempDir::new("image-install");
    let homes = Homes::inside(&temp.path().join("profile"));
    let first = temp.path().join("outer image.AppImage");
    let moved = temp.path().join("moved image.AppImage");
    let alias = temp.path().join("alias.AppImage");
    executable(&first);
    std::os::unix::fs::symlink(&first, &alias).unwrap();
    for image in [&alias, &moved] {
        if image == &moved {
            std::fs::rename(&first, &moved).unwrap();
        }
        let mut command = homes.command("install");
        command.env("APPIMAGE", image).env("APPDIR", temp.path());
        // The extraction is a sibling of the outer file, never its parent.
        let appdir = temp.path().join("extracted");
        std::fs::create_dir_all(&appdir).unwrap();
        command.env("APPDIR", &appdir);
        let output = run_with_timeout(command).unwrap();
        assert!(output.status.success(), "{}", text(&output.stderr));
        let written = std::fs::read_to_string(homes.applications_entry()).unwrap();
        let target = image.canonicalize().unwrap();
        assert!(
            written
                .lines()
                .any(|line| line == format!("Exec=\"{}\"", target.display())),
            "{written}"
        );
        assert!(!written.contains("--background"));
        assert!(!homes.autostart_entry().exists());
    }
}

#[test]
fn invalid_runtime_signals_preserve_every_installed_file() {
    let temp = TempDir::new("image-refusal");
    let homes = Homes::inside(&temp.path().join("profile"));
    assert!(homes.run("install").status.success());
    let files: Vec<_> = std::iter::once(homes.applications_entry())
        .chain(homes.icons())
        .collect();
    let before: Vec<_> = files.iter().map(|p| std::fs::read(p).unwrap()).collect();
    let extracted = temp.path().join("extracted");
    std::fs::create_dir(&extracted).unwrap();
    let inner = extracted.join("addict");
    executable(&inner);
    let noexec = temp.path().join("noexec");
    std::fs::write(&noexec, b"no").unwrap();
    let owner_noexec = temp.path().join("owner-noexec");
    std::fs::write(&owner_noexec, b"image fixture").unwrap();
    std::fs::set_permissions(&owner_noexec, std::fs::Permissions::from_mode(0o641)).unwrap();
    let control = temp.path().join("bad\nimage");
    executable(&control);
    let nonutf8 = temp
        .path()
        .join(OsString::from_vec(b"bad\xffimage".to_vec()));
    executable(&nonutf8);
    let link = temp.path().join("extracted-link");
    std::os::unix::fs::symlink(&inner, &link).unwrap();
    let cases = [
        None,
        Some(OsString::new()),
        Some("relative".into()),
        Some(temp.path().join("missing").into_os_string()),
        Some(extracted.clone().into_os_string()),
        Some(noexec.into_os_string()),
        Some(owner_noexec.into_os_string()),
        Some(control.into_os_string()),
        Some(nonutf8.into_os_string()),
        Some(inner.into_os_string()),
        Some(link.into_os_string()),
    ];
    for signal in cases {
        let mut command = homes.command("install");
        command.env("APPDIR", &extracted);
        if let Some(signal) = signal {
            command.env("APPIMAGE", signal);
        }
        let output = run_with_timeout(command).unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = text(&output.stderr);
        assert!(
            error.contains("persistent outer AppImage") && error.contains("run install again"),
            "{error}"
        );
        for (path, content) in files.iter().zip(&before) {
            assert_eq!(&std::fs::read(path).unwrap(), content);
        }
        assert!(!homes.config_home.exists());
    }
}
