//! `addict install` against a private `HOME`, `XDG_DATA_HOME` and
//! `XDG_CONFIG_HOME`.

mod common;

use std::path::{Path, PathBuf};

use common::{ENTRY, Homes, TempDir, lines, shown, text};

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
    assert_eq!(
        lines(&output.stdout),
        [shown(&applications), shown(&autostart)]
    );

    let written = std::fs::read_to_string(&applications).expect("read entry");
    let exec = format!("Exec=\"{}\"", binary().display());
    assert!(written.lines().any(|line| line == exec), "{written}");
    assert!(written.lines().any(|line| line == "X-Addict=true"));
    assert!(written.lines().any(|line| line == "NoDisplay=true"));
    // The same text in both directories.
    assert_eq!(
        std::fs::read_to_string(&autostart).expect("read entry"),
        written
    );
    // Nothing else appeared.
    assert_eq!(std::fs::read_dir(temp.path()).expect("list").count(), 2);
    for dir in [&homes.data_home, &homes.config_home] {
        assert_eq!(std::fs::read_dir(dir).expect("list").count(), 1);
    }
}

#[test]
fn reinstall_overwrites_its_own_entries() {
    let temp = TempDir::new("reinstall");
    let homes = Homes::inside(temp.path());

    assert!(homes.run("install").status.success());
    let first = std::fs::read_to_string(homes.applications_entry()).expect("read entry");
    for entry in entries(&homes) {
        std::fs::write(&entry, format!("{first}X-Stale=true\n")).expect("edit entry");
    }

    let output = homes.run("install");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(lines(&output.stdout).len(), 2);
    for entry in entries(&homes) {
        assert_eq!(std::fs::read_to_string(&entry).expect("read entry"), first);
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
    assert_eq!(lines(&output.stdout), [shown(&entry), shown(&autostart)]);
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
