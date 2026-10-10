//! `addict install` against a private `HOME` and `XDG_DATA_HOME`.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{TempDir, run_with_timeout, text};

const ENTRY: &str = "dev.addict.Addict.desktop";

fn install(home: &Path, data_home: &Path) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_addict"));
    command
        .arg("install")
        .env("HOME", home)
        .env("XDG_DATA_HOME", data_home)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS");
    run_with_timeout(command).expect("addict install did not exit within the timeout")
}

fn binary() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_addict"))
        .canonicalize()
        .expect("canonical binary path")
}

#[test]
fn writes_the_entry_and_prints_its_path() {
    let temp = TempDir::new("install");
    let data_home = temp.path().join("data");
    let entry = data_home.join("applications").join(ENTRY);

    let output = install(temp.path(), &data_home);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout).trim_end(), entry.display().to_string());

    let written = std::fs::read_to_string(&entry).expect("read entry");
    let exec = format!("Exec=\"{}\"", binary().display());
    assert!(written.lines().any(|line| line == exec), "{written}");
    assert!(written.lines().any(|line| line == "X-Addict=true"));
    assert!(written.lines().any(|line| line == "NoDisplay=true"));
    // Nothing else appeared: no autostart entry, nothing under HOME.
    assert_eq!(std::fs::read_dir(temp.path()).expect("list").count(), 1);
    assert_eq!(std::fs::read_dir(&data_home).expect("list").count(), 1);
}

#[test]
fn reinstall_overwrites_its_own_entry() {
    let temp = TempDir::new("reinstall");
    let data_home = temp.path().join("data");
    let entry = data_home.join("applications").join(ENTRY);

    assert!(install(temp.path(), &data_home).status.success());
    let first = std::fs::read_to_string(&entry).expect("read entry");
    std::fs::write(&entry, format!("{first}X-Stale=true\n")).expect("edit entry");

    let output = install(temp.path(), &data_home);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(std::fs::read_to_string(&entry).expect("read entry"), first);
}

#[test]
fn refuses_a_foreign_entry() {
    let temp = TempDir::new("foreign");
    let data_home = temp.path().join("data");
    let entry = data_home.join("applications").join(ENTRY);
    std::fs::create_dir_all(entry.parent().expect("parent")).expect("create dir");
    let foreign = "[Desktop Entry]\nName=Someone else\n";
    std::fs::write(&entry, foreign).expect("write foreign entry");

    let output = install(temp.path(), &data_home);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = text(&output.stderr);
    assert!(stderr.contains(&entry.display().to_string()), "{stderr}");
    assert_eq!(std::fs::read_to_string(&entry).expect("read"), foreign);
}

#[test]
fn refuses_a_symlink() {
    let temp = TempDir::new("symlink");
    let data_home = temp.path().join("data");
    let entry = data_home.join("applications").join(ENTRY);
    std::fs::create_dir_all(entry.parent().expect("parent")).expect("create dir");
    let target = temp.path().join("target.desktop");
    let own = "[Desktop Entry]\nX-Addict=true\n";
    std::fs::write(&target, own).expect("write target");
    std::os::unix::fs::symlink(&target, &entry).expect("symlink");

    let output = install(temp.path(), &data_home);
    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(stderr.contains(&entry.display().to_string()), "{stderr}");
    assert_eq!(std::fs::read_to_string(&target).expect("read"), own);
    let metadata = std::fs::symlink_metadata(&entry).expect("metadata");
    assert!(metadata.is_symlink());
}

#[test]
fn relative_data_home_falls_back_to_home() {
    let temp = TempDir::new("relative");
    let output = install(temp.path(), Path::new("relative/data"));
    assert!(output.status.success(), "{}", text(&output.stderr));
    let entry = temp.path().join(".local/share/applications").join(ENTRY);
    assert_eq!(text(&output.stdout).trim_end(), entry.display().to_string());
    assert!(entry.is_file());
}
