//! `addict install` against a private `HOME`, `XDG_DATA_HOME` and
//! `XDG_CONFIG_HOME`.

mod common;

use std::path::{Path, PathBuf};

use common::{ENTRY, Homes, TempDir, icon_in, lines, shown, text};

const FOREIGN: &str = "[Desktop Entry]\nName=Someone else\n";
const LEGACY: &str = "[Desktop Entry]\nExec=addict --background\nX-Addict=true\n";

fn binary() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_addict"))
        .canonicalize()
        .expect("canonical binary path")
}

fn place(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
    std::fs::write(path, content).expect("write entry");
}

#[test]
fn writes_a_visible_launcher_and_the_icons_and_nothing_for_login() {
    let temp = TempDir::new("install");
    let homes = Homes::inside(temp.path());
    let applications = homes.applications_entry();

    let output = homes.run("install");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let mut printed = vec![shown(&applications)];
    printed.extend(homes.icons().iter().map(|icon| shown(icon)));
    assert_eq!(lines(&output.stdout), printed);

    let written = std::fs::read_to_string(&applications).expect("read entry");
    let exec = format!("Exec=\"{}\"", binary().display());
    assert!(written.lines().any(|line| line == exec), "{written}");
    assert!(written.lines().any(|line| line == "X-Addict=true"));
    assert!(!written.contains("NoDisplay"), "{written}");
    assert!(!written.contains("--background"), "{written}");
    assert!(
        written
            .lines()
            .any(|line| line == "Icon=io.github.doqhu.Addict"),
        "{written}"
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
    // Nothing else appeared: the entry's directory and the icons' one, and no
    // autostart directory or even the config home it would be in.
    assert_eq!(std::fs::read_dir(temp.path()).expect("list").count(), 1);
    assert_eq!(
        std::fs::read_dir(&homes.data_home).expect("list").count(),
        2
    );
    assert!(!homes.config_home.exists());
}

#[test]
fn reinstall_overwrites_its_own_entry_and_icons() {
    let temp = TempDir::new("reinstall");
    let homes = Homes::inside(temp.path());

    assert!(homes.run("install").status.success());
    let first = std::fs::read_to_string(homes.applications_entry()).expect("read entry");
    let icons: Vec<Vec<u8>> = homes
        .icons()
        .iter()
        .map(|icon| std::fs::read(icon).expect("read icon"))
        .collect();
    std::fs::write(homes.applications_entry(), format!("{first}X-Stale=true\n"))
        .expect("edit entry");
    for icon in homes.icons() {
        let mut stale = std::fs::read(&icon).expect("read icon");
        stale.extend_from_slice(b"stale");
        std::fs::write(&icon, stale).expect("edit icon");
    }

    let output = homes.run("install");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(lines(&output.stdout).len(), 5);
    assert_eq!(
        std::fs::read_to_string(homes.applications_entry()).expect("read entry"),
        first
    );
    for (icon, original) in homes.icons().iter().zip(&icons) {
        assert_eq!(&std::fs::read(icon).expect("read icon"), original);
    }
}

/// A legacy login entry, ours, foreign or a symlink, is not a target: install
/// succeeds and leaves it exactly as it was.
#[test]
fn leaves_a_legacy_autostart_entry_unchanged() {
    for content in [LEGACY, FOREIGN] {
        let temp = TempDir::new("legacy");
        let homes = Homes::inside(temp.path());
        let autostart = homes.autostart_entry();
        place(&autostart, content);

        let output = homes.run("install");
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert_eq!(lines(&output.stdout).len(), 5);
        assert!(!text(&output.stdout).contains("autostart"));
        assert_eq!(std::fs::read_to_string(&autostart).expect("read"), content);
    }

    let temp = TempDir::new("legacy-link");
    let homes = Homes::inside(temp.path());
    let autostart = homes.autostart_entry();
    let target = temp.path().join("target.desktop");
    std::fs::write(&target, FOREIGN).expect("write target");
    std::fs::create_dir_all(autostart.parent().expect("parent")).expect("create dir");
    std::os::unix::fs::symlink(&target, &autostart).expect("symlink");

    let output = homes.run("install");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(std::fs::read_to_string(&target).expect("read"), FOREIGN);
    let metadata = std::fs::symlink_metadata(&autostart).expect("metadata");
    assert!(metadata.is_symlink());
}

/// A foreign file at the entry path: nothing is written, the icons included.
#[test]
fn refuses_a_foreign_entry_and_writes_nothing() {
    let temp = TempDir::new("foreign");
    let homes = Homes::inside(temp.path());
    let entry = homes.applications_entry();
    place(&entry, FOREIGN);

    let output = homes.run("install");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = text(&output.stderr);
    assert!(stderr.contains(&shown(&entry)), "{stderr}");
    assert_eq!(std::fs::read_to_string(&entry).expect("read"), FOREIGN);
    for icon in homes.icons() {
        assert!(!icon.exists(), "{} was written", icon.display());
    }
}

/// A symlink at the entry path, even to a file with the marker.
#[test]
fn refuses_a_symlink_and_writes_nothing() {
    let temp = TempDir::new("symlink");
    let homes = Homes::inside(temp.path());
    let link = homes.applications_entry();
    std::fs::create_dir_all(link.parent().expect("parent")).expect("create dir");
    let target = temp.path().join("target.desktop");
    let own = "[Desktop Entry]\nX-Addict=true\n";
    std::fs::write(&target, own).expect("write target");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");

    let output = homes.run("install");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = text(&output.stderr);
    assert!(stderr.contains(&shown(&link)), "{stderr}");
    assert_eq!(std::fs::read_to_string(&target).expect("read"), own);
    let metadata = std::fs::symlink_metadata(&link).expect("metadata");
    assert!(metadata.is_symlink());
    for icon in homes.icons() {
        assert!(!icon.exists(), "{} was written", icon.display());
    }
}

/// An install of before the icons left the entry alone: this one adds the
/// icons.
#[test]
fn adds_the_icons_to_an_entry_without_them() {
    let temp = TempDir::new("add-icons");
    let homes = Homes::inside(temp.path());
    assert!(homes.run("install").status.success());
    std::fs::remove_dir_all(homes.data_home.join("icons")).expect("remove icons");

    let output = homes.run("install");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(lines(&output.stdout).len(), 5);
    for icon in homes.icons() {
        assert!(icon.is_file(), "{} was not written", icon.display());
    }
}

/// A foreign file or a symlink at an icon path: nothing is written, the
/// entry included.
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
        let entry = homes.applications_entry();
        assert!(!entry.exists(), "{} was written", entry.display());
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
    let icon = icon_in(&temp.path().join(".local/share"), 128);
    let printed = lines(&output.stdout);
    assert_eq!(printed.len(), 5);
    assert_eq!(printed[0], shown(&entry));
    assert_eq!(printed[2], shown(&icon));
    assert!(icon.is_file());
    assert!(entry.is_file());
}

/// Install has no use for a config home, not even the fallback under `HOME`.
#[test]
fn relative_or_empty_config_home_creates_nothing_under_home() {
    for config_home in ["relative/config", ""] {
        let temp = TempDir::new("relative-config");
        let homes = Homes {
            config_home: PathBuf::from(config_home),
            ..Homes::inside(temp.path())
        };
        let output = homes.run("install");
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert!(!temp.path().join(".config").exists());
    }
}
