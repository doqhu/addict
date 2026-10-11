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
fn removes_both_entries_and_prints_their_paths() {
    let temp = TempDir::new("uninstall");
    let homes = Homes::inside(temp.path());
    let [applications, autostart] = entries(&homes);
    installed(&homes);

    let output = homes.run("uninstall");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        lines(&output.stdout),
        [shown(&applications), shown(&autostart)]
    );
    for entry in [&applications, &autostart] {
        assert!(!entry.exists(), "{} is still there", entry.display());
        // The directories are not removed.
        assert!(entry.parent().expect("parent").is_dir());
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
        assert_eq!(lines(&output.stdout), [shown(&entries[present_at])]);
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
            .join(".config/autostart/dev.addict.Addict.desktop");
        assert!(applications.is_file());
        assert!(autostart.is_file());

        let output = homes.run("uninstall");
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert_eq!(
            lines(&output.stdout),
            [shown(&applications), shown(&autostart)]
        );
        assert!(!applications.exists());
        assert!(!autostart.exists());
    }
}
