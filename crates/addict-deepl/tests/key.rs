//! The key file on real temporary files: what is read and what is refused.

mod common;

use std::os::unix::fs::PermissionsExt;

use addict_deepl::{
    client::host,
    key::{self, KeyError, WriteError},
};

use common::{KEY, TempDir};

#[test]
fn a_file_of_its_owner_alone_gives_the_key_without_the_white_space_around_it() {
    let dir = TempDir::new("key-read");
    for content in [KEY.to_owned(), format!("{KEY}\n"), format!("  {KEY}\r\n\n")] {
        for mode in [0o600, 0o400] {
            let path = dir.key_file(&content, mode);
            let key = key::read(&path).expect("key");
            assert!(!key.is_free(), "{content:?}");
            assert_eq!(host(&key), "https://api.deepl.com");
        }
    }
}

#[test]
fn the_host_is_chosen_by_the_suffix_of_the_key() {
    let dir = TempDir::new("key-host");
    let free = key::read(&dir.key_file(&format!("{KEY}:fx\n"), 0o600)).expect("key");
    assert!(free.is_free());
    assert_eq!(host(&free), "https://api-free.deepl.com");
    let pro = key::read(&dir.key_file(KEY, 0o600)).expect("key");
    assert_eq!(host(&pro), "https://api.deepl.com");
}

#[test]
fn a_mode_with_any_bit_outside_0600_is_refused() {
    let dir = TempDir::new("key-mode");
    for mode in [
        0o644, 0o640, 0o604, 0o620, 0o602, 0o660, 0o666, 0o700, 0o610, 0o601, 0o444, 0o1600,
    ] {
        let path = dir.key_file(KEY, mode);
        // The file system may drop a bit it does not keep for a file.
        let kept = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o7777;
        if kept & !0o600 == 0 {
            continue;
        }
        assert_eq!(key::read(&path), Err(KeyError::TooOpen), "{mode:o}");
    }
}

#[test]
fn a_file_that_is_not_there_is_missing() {
    let dir = TempDir::new("key-missing");
    assert_eq!(key::read(&dir.key_path()), Err(KeyError::Missing));
    assert_eq!(
        key::read(&dir.path().join("no-such-directory/deepl.key")),
        Err(KeyError::Missing)
    );
}

#[test]
fn a_file_without_a_key_is_empty() {
    let dir = TempDir::new("key-empty");
    let long = "k".repeat(5000);
    for content in [
        "",
        " ",
        "\n\n",
        "two words",
        "line\nline",
        "ключ",
        long.as_str(),
    ] {
        let path = dir.key_file(content, 0o600);
        assert_eq!(key::read(&path), Err(KeyError::Empty), "{content:?}");
    }
}

#[test]
fn what_cannot_be_read_as_a_file_is_unreadable() {
    let dir = TempDir::new("key-unreadable");
    // A directory in the place of the file.
    assert_eq!(key::read(dir.path()), Err(KeyError::Unreadable));
    // Bytes that are not text.
    let path = dir.key_path();
    std::fs::write(&path, [0xff, 0xfe, 0x00]).expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("mode");
    assert_eq!(key::read(&path), Err(KeyError::Unreadable));
    // A file its owner may not read; the superuser reads it anyway.
    let path = dir.key_file(KEY, 0o000);
    if std::fs::read(&path).is_err() {
        assert_eq!(key::read(&path), Err(KeyError::Unreadable));
    }
}

#[test]
fn no_error_and_no_debug_output_holds_the_key() {
    let dir = TempDir::new("key-debug");
    let key = key::read(&dir.key_file(KEY, 0o600)).expect("key");
    let refused = key::read(&dir.key_file(KEY, 0o644));
    for shown in [format!("{key:?}"), format!("{refused:?}")] {
        assert!(!shown.contains(KEY), "{shown}");
        assert!(!shown.contains("test-key"), "{shown}");
    }
}

/// The mode bits of the file at `path`.
fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o7777
}

/// The names in `dir`, sorted.
fn names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .expect("read the directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn a_written_key_is_read_back_from_a_file_of_mode_0600() {
    let dir = TempDir::new("key-write");
    let path = dir.key_path();
    for (given, free) in [(format!("{KEY}:fx"), true), (format!("  {KEY}\n"), false)] {
        key::write(&path, &given).expect("write");
        assert_eq!(mode(&path), 0o600, "{given:?}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            format!("{}\n", given.trim())
        );
        assert_eq!(key::read(&path).expect("key").is_free(), free);
    }
    assert_eq!(names(dir.path()), ["deepl.key"]);
}

#[test]
fn a_write_replaces_a_key_and_a_file_that_was_too_open() {
    let dir = TempDir::new("key-replace");
    for mode_before in [0o600, 0o644, 0o666] {
        let path = dir.key_file("old-key", mode_before);
        key::write(&path, KEY).expect("write");
        assert_eq!(mode(&path), 0o600, "{mode_before:o}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            format!("{KEY}\n")
        );
        assert!(key::read(&path).is_ok());
    }
    assert_eq!(names(dir.path()), ["deepl.key"]);
}

#[test]
fn a_missing_directory_is_made() {
    let dir = TempDir::new("key-dir");
    let path = dir.path().join("config/addict/deepl.key");
    key::write(&path, KEY).expect("write");
    assert_eq!(mode(&path), 0o600);
    assert!(key::read(&path).is_ok());
}

#[test]
fn what_is_not_a_key_is_not_written() {
    let dir = TempDir::new("key-invalid");
    let long = "k".repeat(1024);
    for given in [
        "",
        "   ",
        "\n",
        "two words",
        "line\nline",
        "ключ",
        "tab\tkey",
        long.as_str(),
    ] {
        assert_eq!(
            key::write(&dir.key_path(), given),
            Err(WriteError::Invalid),
            "{given:?}"
        );
        assert!(names(dir.path()).is_empty(), "{given:?}");
    }
    // An old key stays when the new one is refused.
    let path = dir.key_file("old-key", 0o600);
    assert_eq!(key::write(&path, "two words"), Err(WriteError::Invalid));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "old-key");
}

#[test]
fn the_longest_key_that_is_written_is_read_back() {
    let dir = TempDir::new("key-long");
    let path = dir.key_path();
    key::write(&path, &"k".repeat(1023)).expect("write");
    assert!(key::read(&path).is_ok());
}

#[test]
fn a_failed_write_leaves_the_old_file_and_no_temporary_file() {
    let dir = TempDir::new("key-unwritable");
    let path = dir.key_file("old-key", 0o600);
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).expect("mode");
    // The superuser writes anyway.
    let probe = dir.path().join("probe");
    if std::fs::write(&probe, "").is_ok() {
        let _ = std::fs::remove_file(&probe);
    } else {
        let error = key::write(&path, KEY).expect_err("an unwritable directory");
        assert!(matches!(error, WriteError::Io(_)), "{error:?}");
        assert!(!format!("{error:?}{error}").contains(KEY));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "old-key");
        assert_eq!(names(dir.path()), ["deepl.key"]);
    }
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).expect("mode");
}

#[test]
fn a_failed_rename_removes_the_temporary_file() {
    let dir = TempDir::new("key-rename");
    // A directory that is not empty in the place of the file.
    let path = dir.key_path();
    std::fs::create_dir(&path).expect("dir");
    std::fs::write(path.join("inside"), "").expect("write");
    assert!(matches!(key::write(&path, KEY), Err(WriteError::Io(_))));
    assert_eq!(names(dir.path()), ["deepl.key"]);
}
