//! `addict import` on files it refuses, against a private `HOME` and
//! `XDG_DATA_HOME`. The accepted file is not in the repository: its import is
//! tested in `addict-dictionary` on a fixture.

mod common;

use std::path::Path;
use std::process::{Command, Output};

use common::{TempDir, lines, run_with_timeout, text};

/// Runs `addict import <file>` with the given directory variables only.
fn import(file: &Path, home: Option<&Path>, data_home: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_addict"));
    command
        .arg("import")
        .arg(file)
        .env_remove("HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS");
    if let Some(home) = home {
        command.env("HOME", home);
    }
    if let Some(data_home) = data_home {
        command.env("XDG_DATA_HOME", data_home);
    }
    run_with_timeout(command).expect("addict did not exit within the timeout")
}

/// The single stderr line of a refused import that printed nothing else.
fn refusal(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
    let stderr = lines(&output.stderr);
    assert_eq!(stderr.len(), 1, "{stderr:?}");
    stderr[0].clone()
}

#[test]
fn a_file_of_another_size_is_refused_and_nothing_is_written() {
    let temp = TempDir::new("import-size");
    let file = temp.path().join("short.gz");
    std::fs::write(&file, b"not the dictionary").expect("write file");
    let data_home = temp.path().join("data");

    let line = refusal(&import(&file, Some(temp.path()), Some(&data_home)));
    assert!(line.contains("short.gz"), "{line}");
    assert!(line.contains("18 bytes"), "{line}");
    // The sizes of both chosen files.
    assert!(line.contains("11363503 bytes"), "{line}");
    assert!(line.contains("3948540 bytes"), "{line}");
    assert!(!line.contains("SHA-256"), "{line}");
    assert!(!data_home.exists());
}

#[test]
fn a_missing_file_and_a_directory_are_refused() {
    let temp = TempDir::new("import-unreadable");
    let data_home = temp.path().join("data");
    for file in [temp.path().join("absent.gz"), temp.path().to_path_buf()] {
        let line = refusal(&import(&file, Some(temp.path()), Some(&data_home)));
        assert!(line.contains(&file.display().to_string()), "{line}");
        assert!(!data_home.exists());
    }
}

#[test]
fn without_a_data_directory_the_import_is_refused() {
    let temp = TempDir::new("import-nodir");
    let file = temp.path().join("any.gz");
    std::fs::write(&file, b"any").expect("write file");

    let line = refusal(&import(&file, None, None));
    assert!(line.contains("data directory"), "{line}");
    // A relative XDG_DATA_HOME is ignored, so it does not give one either.
    let line = refusal(&import(&file, None, Some(Path::new("relative"))));
    assert!(line.contains("data directory"), "{line}");
    assert_eq!(std::fs::read_dir(temp.path()).expect("list").count(), 1);
}
