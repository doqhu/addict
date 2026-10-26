//! `addict uninstall` against a private `HOME`, `XDG_DATA_HOME` and
//! `XDG_CONFIG_HOME`.

mod common;

use std::path::{Path, PathBuf};

use common::{Homes, TempDir, lines, shown, text};

const FOREIGN: &str = "[Desktop Entry]\nName=Someone else\n";

/// Both entry paths, applications first: the order of the output.
fn entries(homes: &Homes) -> [PathBuf; 2] {
    [homes.applications_entry(), homes.autostart_entry()]
}

fn installed(homes: &Homes) {
    let output = homes.run("install");
    assert!(output.status.success(), "{}", text(&output.stderr));
}

fn replace(path: &Path, content: &str) {
    std::fs::remove_file(path).expect("remove entry");
    std::fs::write(path, content).expect("write entry");
}

#[test]
fn removes_the_entries_and_icons_and_prints_their_paths() {
    let temp = TempDir::new("uninstall");
    let homes = Homes::inside(temp.path());
    let [applications, autostart] = entries(&homes);
    installed(&homes);

    let output = homes.run("uninstall");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let mut printed = vec![shown(&applications), shown(&autostart)];
    printed.extend(homes.icons().iter().map(|icon| shown(icon)));
    assert_eq!(lines(&output.stdout), printed);
    for path in [&applications, &autostart]
        .into_iter()
        .chain(&homes.icons())
    {
        assert!(!path.exists(), "{} is still there", path.display());
        // The directories are not removed.
        assert!(path.parent().expect("parent").is_dir());
    }
}

/// An install of before the icons: its entries are removed, no icon is
/// missed.
#[test]
fn entries_without_icons_are_not_an_error() {
    let temp = TempDir::new("uninstall-no-icons");
    let homes = Homes::inside(temp.path());
    let entries = entries(&homes);
    installed(&homes);
    std::fs::remove_dir_all(homes.data_home.join("icons")).expect("remove icons");

    let output = homes.run("uninstall");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(lines(&output.stdout), entries.map(|entry| shown(&entry)));
}

/// A foreign file or a symlink at an icon path: nothing is removed.
#[test]
fn refuses_a_foreign_or_linked_icon_and_removes_nothing() {
    for link in [false, true] {
        let temp = TempDir::new("uninstall-icon");
        let homes = Homes::inside(temp.path());
        installed(&homes);
        let icon = homes.icon(256);
        let kept: Vec<PathBuf> = entries(&homes)
            .into_iter()
            .chain(homes.icons().into_iter().filter(|other| *other != icon))
            .collect();
        if link {
            let target = temp.path().join("target.png");
            std::fs::rename(&icon, &target).expect("move icon");
            std::os::unix::fs::symlink(&target, &icon).expect("symlink");
        } else {
            std::fs::write(&icon, b"someone else's icon").expect("write foreign");
        }

        let output = homes.run("uninstall");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = text(&output.stderr);
        assert!(stderr.contains(&shown(&icon)), "{stderr}");
        for path in &kept {
            assert!(path.is_file(), "{} was removed", path.display());
        }
        assert_eq!(
            std::fs::symlink_metadata(&icon)
                .expect("metadata")
                .is_symlink(),
            link
        );
    }
}

#[test]
fn nothing_installed_is_not_an_error() {
    let temp = TempDir::new("uninstall-none");
    let homes = Homes::inside(temp.path());

    let output = homes.run("uninstall");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(output.stdout.is_empty());
    // Nothing was created either.
    assert_eq!(std::fs::read_dir(temp.path()).expect("list").count(), 0);
}

/// Only one of the two is present: it is removed.
#[test]
fn removes_the_one_entry_that_is_present() {
    for present_at in 0..2 {
        let temp = TempDir::new("uninstall-one");
        let homes = Homes::inside(temp.path());
        let entries = entries(&homes);
        installed(&homes);
        std::fs::remove_file(&entries[1 - present_at]).expect("remove the other");

        let output = homes.run("uninstall");
        assert!(output.status.success(), "{}", text(&output.stderr));
        // The entry that is present, then the icons.
        let mut printed = vec![shown(&entries[present_at])];
        printed.extend(homes.icons().iter().map(|icon| shown(icon)));
        assert_eq!(lines(&output.stdout), printed);
        assert!(!entries[present_at].exists());
    }
}

/// A foreign file at either path: neither file is removed.
#[test]
fn refuses_a_foreign_entry_and_removes_neither() {
    for foreign_at in 0..2 {
        let temp = TempDir::new("uninstall-foreign");
        let homes = Homes::inside(temp.path());
        let entries = entries(&homes);
        let (foreign, other) = (&entries[foreign_at], &entries[1 - foreign_at]);
        installed(&homes);
        let own = std::fs::read_to_string(other).expect("read entry");
        replace(foreign, FOREIGN);

        let output = homes.run("uninstall");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = text(&output.stderr);
        assert!(stderr.contains(&shown(foreign)), "{stderr}");
        assert_eq!(std::fs::read_to_string(foreign).expect("read"), FOREIGN);
        assert_eq!(std::fs::read_to_string(other).expect("read"), own);
    }
}

/// A symlink at either path, even to a file with the marker.
#[test]
fn refuses_a_symlink_and_removes_neither() {
    for link_at in 0..2 {
        let temp = TempDir::new("uninstall-symlink");
        let homes = Homes::inside(temp.path());
        let entries = entries(&homes);
        let (link, other) = (&entries[link_at], &entries[1 - link_at]);
        installed(&homes);
        let own = std::fs::read_to_string(other).expect("read entry");
        let target = temp.path().join("target.desktop");
        std::fs::rename(link, &target).expect("move entry");
        std::os::unix::fs::symlink(&target, link).expect("symlink");

        let output = homes.run("uninstall");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = text(&output.stderr);
        assert!(stderr.contains(&shown(link)), "{stderr}");
        assert_eq!(std::fs::read_to_string(&target).expect("read"), own);
        let metadata = std::fs::symlink_metadata(link).expect("metadata");
        assert!(metadata.is_symlink());
        assert_eq!(std::fs::read_to_string(other).expect("read"), own);
    }
}

#[test]
fn relative_or_empty_config_home_falls_back_to_home() {
    for config_home in ["relative/config", ""] {
        let temp = TempDir::new("uninstall-relative");
        let homes = Homes {
            config_home: PathBuf::from(config_home),
            ..Homes::inside(temp.path())
        };
        installed(&homes);
        let applications = homes.applications_entry();
        let autostart = temp
            .path()
            .join(".config/autostart/io.github.doqhu.Addict.desktop");
        assert!(applications.is_file());
        assert!(autostart.is_file());

        let output = homes.run("uninstall");
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert_eq!(lines(&output.stdout).len(), 6);
        assert_eq!(
            lines(&output.stdout)[..2],
            [shown(&applications), shown(&autostart)]
        );
        assert!(!applications.exists());
        assert!(!autostart.exists());
    }
}
