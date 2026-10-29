//! `addict install` against a private `HOME`, `XDG_DATA_HOME` and
//! `XDG_CONFIG_HOME`.

mod common;

use std::path::{Path, PathBuf};

use common::{ENTRY, Homes, TempDir, icon_in, lines, shown, text};

const FOREIGN: &str = "[Desktop Entry]\nName=Someone else\n";

fn binary() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_addict"))
        .canonicalize()
        .expect("canonical binary path")
}

/// Both entry paths, applications first: the order of the output.
fn entries(homes: &Homes) -> [PathBuf; 2] {
    [homes.applications_entry(), homes.autostart_entry()]
}

fn place(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
    std::fs::write(path, content).expect("write entry");
}

#[test]
fn writes_both_entries_and_prints_their_paths() {
    let temp = TempDir::new("install");
    let homes = Homes::inside(temp.path());
    let [applications, autostart] = entries(&homes);

    let output = homes.run("install");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let mut printed = vec![shown(&applications), shown(&autostart)];
    printed.extend(homes.icons().iter().map(|icon| shown(icon)));
    assert_eq!(lines(&output.stdout), printed);

    let written = std::fs::read_to_string(&applications).expect("read entry");
    let exec = format!("Exec=\"{}\"", binary().display());
    assert!(written.lines().any(|line| line == exec), "{written}");
    assert!(written.lines().any(|line| line == "X-Addict=true"));
    assert!(written.lines().any(|line| line == "NoDisplay=true"));
    assert!(
        written
            .lines()
            .any(|line| line == "Icon=io.github.doqhu.Addict"),
        "{written}"
    );
    // Only the login entry starts silently.
    assert_eq!(
        std::fs::read_to_string(&autostart).expect("read entry"),
        written.replace(&exec, &format!("{exec} --background"))
    );
    // Each icon is a square PNG of its directory's size with the marker.
    for (size, icon) in common::ICON_SIZES.into_iter().zip(homes.icons()) {
        let png = std::fs::read(&icon).expect("read icon");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "{}", icon.display());
        let width = u32::from_be_bytes(png[16..20].try_into().expect("width"));
        let height = u32::from_be_bytes(png[20..24].try_into().expect("height"));
        assert_eq!((width, height), (size, size), "{}", icon.display());
        let marker = b"tEXtX-Addict\0true";
        assert!(png.windows(marker.len()).any(|w| w == marker));
    }
    // Nothing else appeared: the entries' directory and the icons' one.
    assert_eq!(std::fs::read_dir(temp.path()).expect("list").count(), 2);
    assert_eq!(
        std::fs::read_dir(&homes.data_home).expect("list").count(),
        2
    );
    assert_eq!(
        std::fs::read_dir(&homes.config_home).expect("list").count(),
        1
    );
}

#[test]
fn reinstall_overwrites_its_own_entries() {
    let temp = TempDir::new("reinstall");
    let homes = Homes::inside(temp.path());

    assert!(homes.run("install").status.success());
    let first = std::fs::read_to_string(homes.applications_entry()).expect("read entry");
    let icons: Vec<Vec<u8>> = homes
        .icons()
        .iter()
        .map(|icon| std::fs::read(icon).expect("read icon"))
        .collect();
    for entry in entries(&homes) {
        std::fs::write(&entry, format!("{first}X-Stale=true\n")).expect("edit entry");
    }
    for icon in homes.icons() {
        let mut stale = std::fs::read(&icon).expect("read icon");
        stale.extend_from_slice(b"stale");
        std::fs::write(&icon, stale).expect("edit icon");
    }

    let output = homes.run("install");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(lines(&output.stdout).len(), 6);
    assert_eq!(
        std::fs::read_to_string(homes.applications_entry()).expect("read entry"),
        first
    );
    let exec = format!("Exec=\"{}\"", binary().display());
    assert_eq!(
        std::fs::read_to_string(homes.autostart_entry()).expect("read login entry"),
        first.replace(&exec, &format!("{exec} --background"))
    );
    for (icon, original) in homes.icons().iter().zip(&icons) {
        assert_eq!(&std::fs::read(icon).expect("read icon"), original);
    }
}

/// A foreign file at either path: neither file is written or changed.
#[test]
fn refuses_a_foreign_entry_and_writes_neither() {
    for foreign_at in 0..2 {
        let temp = TempDir::new("foreign");
        let homes = Homes::inside(temp.path());
        let entries = entries(&homes);
        let (foreign, other) = (&entries[foreign_at], &entries[1 - foreign_at]);
        place(foreign, FOREIGN);

        let output = homes.run("install");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = text(&output.stderr);
        assert!(stderr.contains(&shown(foreign)), "{stderr}");
        assert_eq!(std::fs::read_to_string(foreign).expect("read"), FOREIGN);
        assert!(!other.exists(), "{} was written", other.display());
    }
}

/// The other entry is ours and stale: a refusal leaves it as it was.
#[test]
fn a_refusal_leaves_its_own_other_entry_unchanged() {
    for foreign_at in 0..2 {
        let temp = TempDir::new("foreign-own");
        let homes = Homes::inside(temp.path());
        let entries = entries(&homes);
        let (foreign, other) = (&entries[foreign_at], &entries[1 - foreign_at]);
        place(foreign, FOREIGN);
        let stale = "[Desktop Entry]\nX-Addict=true\nX-Stale=true\n";
        place(other, stale);

        let output = homes.run("install");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(std::fs::read_to_string(foreign).expect("read"), FOREIGN);
        assert_eq!(std::fs::read_to_string(other).expect("read"), stale);
    }
}

/// A symlink at either path, even to a file with the marker.
#[test]
fn refuses_a_symlink_and_writes_neither() {
    for link_at in 0..2 {
        let temp = TempDir::new("symlink");
        let homes = Homes::inside(temp.path());
        let entries = entries(&homes);
        let (link, other) = (&entries[link_at], &entries[1 - link_at]);
        std::fs::create_dir_all(link.parent().expect("parent")).expect("create dir");
        let target = temp.path().join("target.desktop");
        let own = "[Desktop Entry]\nX-Addict=true\n";
        std::fs::write(&target, own).expect("write target");
        std::os::unix::fs::symlink(&target, link).expect("symlink");

        let output = homes.run("install");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = text(&output.stderr);
        assert!(stderr.contains(&shown(link)), "{stderr}");
        assert_eq!(std::fs::read_to_string(&target).expect("read"), own);
        let metadata = std::fs::symlink_metadata(link).expect("metadata");
        assert!(metadata.is_symlink());
        assert!(!other.exists(), "{} was written", other.display());
    }
}

/// An install of before the icons left the entries alone: this one adds the
/// icons.
#[test]
fn adds_the_icons_to_entries_without_them() {
    let temp = TempDir::new("add-icons");
    let homes = Homes::inside(temp.path());
    assert!(homes.run("install").status.success());
    std::fs::remove_dir_all(homes.data_home.join("icons")).expect("remove icons");

    let output = homes.run("install");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(lines(&output.stdout).len(), 6);
    for icon in homes.icons() {
        assert!(icon.is_file(), "{} was not written", icon.display());
    }
}

/// A foreign file or a symlink at an icon path: nothing is written, the
/// entries included.
#[test]
fn refuses_a_foreign_or_linked_icon_and_writes_nothing() {
    for (size_at, link) in [(0, false), (3, false), (0, true), (3, true)] {
        let temp = TempDir::new("icon-refusal");
        let homes = Homes::inside(temp.path());
        let icon = homes.icons()[size_at].clone();
        let target = temp.path().join("target.png");
        std::fs::create_dir_all(icon.parent().expect("parent")).expect("create dir");
        if link {
            // Even a link to a file that carries the marker.
            std::fs::write(&target, b"tEXtX-Addict\0true").expect("write target");
            std::os::unix::fs::symlink(&target, &icon).expect("symlink");
        } else {
            std::fs::write(&icon, b"someone else's icon").expect("write foreign");
        }

        let output = homes.run("install");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = text(&output.stderr);
        assert!(stderr.contains(&shown(&icon)), "{stderr}");
        for entry in entries(&homes) {
            assert!(!entry.exists(), "{} was written", entry.display());
        }
        for other in homes.icons().iter().filter(|other| **other != icon) {
            assert!(!other.exists(), "{} was written", other.display());
        }
        if link {
            assert!(
                std::fs::symlink_metadata(&icon)
                    .expect("metadata")
                    .is_symlink()
            );
            assert_eq!(std::fs::read(&target).expect("read"), b"tEXtX-Addict\0true");
        } else {
            assert_eq!(std::fs::read(&icon).expect("read"), b"someone else's icon");
        }
    }
}

#[test]
fn relative_data_home_falls_back_to_home() {
    let temp = TempDir::new("relative-data");
    let homes = Homes {
        data_home: PathBuf::from("relative/data"),
        ..Homes::inside(temp.path())
    };
    let output = homes.run("install");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let entry = temp.path().join(".local/share/applications").join(ENTRY);
    let autostart = homes.autostart_entry();
    let icon = icon_in(&temp.path().join(".local/share"), 128);
    let printed = lines(&output.stdout);
    assert_eq!(printed.len(), 6);
    assert_eq!(printed[..2], [shown(&entry), shown(&autostart)]);
    assert_eq!(printed[3], shown(&icon));
    assert!(icon.is_file());
    assert!(entry.is_file());
    assert!(autostart.starts_with(&homes.config_home));
    assert!(autostart.is_file());
}

#[test]
fn relative_or_empty_config_home_falls_back_to_home() {
    for config_home in ["relative/config", ""] {
        let temp = TempDir::new("relative-config");
        let homes = Homes {
            config_home: PathBuf::from(config_home),
            ..Homes::inside(temp.path())
        };
        let output = homes.run("install");
        assert!(output.status.success(), "{}", text(&output.stderr));
        let entry = temp.path().join(".config/autostart").join(ENTRY);
        assert_eq!(lines(&output.stdout)[1], shown(&entry));
        assert!(entry.is_file());
    }
}
