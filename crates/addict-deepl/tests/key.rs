//! The key file on real temporary files: what is read and what is refused.

mod common;

use std::os::unix::fs::PermissionsExt;

use addict_deepl::{
    client::host,
    key::{self, KeyError},
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
