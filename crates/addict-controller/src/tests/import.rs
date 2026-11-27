use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use addict_core::{
    dirs::Dirs, failure::file_choice::FileChoiceFailure, translation::Source as Language,
};
use addict_dictionary::{
    Index,
    import::{MIN_KAIKKI_HEADWORDS, Source},
};
use addict_engine::{
    dictionaries::State,
    view::{Lookup, file_choice::ImportState},
};
use flate2::{Compression, write::GzEncoder};

use super::{
    FakeShell,
    downloads::{TestDir, gate, shell, startup, wait_until, write_index},
};
use crate::{
    Effect, Event, RequestId,
    downloads::{Limits, ReadGates},
};

pub(super) fn chooser(shell: &mut FakeShell) -> RequestId {
    shell.dispatch(Event::ImportPressed);
    let (view, effects) = loop {
        let (view, effects) = shell.next();
        if view.downloads.import == ImportState::Choosing {
            break (view, effects);
        }
    };
    assert_eq!(view.downloads.import, ImportState::Choosing);
    assert_eq!(
        view.downloads.import.text(),
        "Choose a downloaded dictionary file…"
    );
    let [Effect::ChooseFile { id }] = effects.as_slice() else {
        panic!("expected one chooser, got {effects:?}");
    };
    *id
}

fn file_chosen(shell: &mut FakeShell, path: PathBuf) {
    let id = chooser(shell);
    shell.dispatch(Event::FileChosen {
        id,
        result: Ok(path),
    });
}

fn failed(shell: &mut FakeShell) -> String {
    failed_within(shell, super::WAIT)
}

fn failed_within(shell: &mut FakeShell, limit: Duration) -> String {
    let view = shell.wait_for_within(limit, |view| {
        matches!(view.downloads.import, ImportState::Failed(_))
    });
    assert!(!view.downloads.import.busy(), "retry is available");
    view.downloads.import.text()
}

#[test]
fn dismissal_is_quiet_writes_nothing_and_allows_retry() {
    let temp = TestDir::new();
    let mut shell = startup(temp.dirs());
    let mut shown = Vec::new();
    for _ in 0..2 {
        let id = chooser(&mut shell);
        shown.push(ImportState::Choosing);
        shell.dispatch(Event::FileChosen {
            id,
            result: Err(FileChoiceFailure::Cancelled),
        });
        let state = shell.next().0.downloads.import;
        assert_eq!(state, ImportState::Idle);
        assert!(state.text().is_empty());
        shown.push(state);
    }
    assert!(!temp.0.join("data").exists());
    assert_eq!(
        shown,
        [
            ImportState::Choosing,
            ImportState::Idle,
            ImportState::Choosing,
            ImportState::Idle
        ]
    );
}

#[test]
fn a_second_activation_while_choosing_is_ignored() {
    let mut shell = startup(Dirs::default());
    let id = chooser(&mut shell);
    shell.quiet(Event::ImportPressed);
    shell.dispatch(Event::FileChosen {
        id,
        result: Err(FileChoiceFailure::Cancelled),
    });
    assert_eq!(shell.next().0.downloads.import, ImportState::Idle);
}

#[test]
fn chooser_and_nonlocal_failures_are_visible_and_retryable() {
    let mut shell = startup(Dirs::default());
    for (result, text) in [
        (
            FileChoiceFailure::Failed("portal unavailable".to_owned()),
            "Cannot choose a dictionary file: portal unavailable",
        ),
        (
            FileChoiceFailure::NotLocal,
            "Cannot import this file: the file chooser did not provide a local or document-portal path. Choose a downloaded file on this computer.",
        ),
        (
            FileChoiceFailure::Unavailable,
            "Cannot choose a dictionary file: the file chooser is unavailable.",
        ),
    ] {
        let id = chooser(&mut shell);
        shell.dispatch(Event::FileChosen {
            id,
            result: Err(result),
        });
        assert_eq!(failed(&mut shell), text);
    }
}

#[test]
fn missing_or_relative_data_directory_is_a_visible_error_without_a_write() {
    let temp = TestDir::new();
    for data in [None, Some(PathBuf::from("relative"))] {
        let mut shell = startup(Dirs {
            data,
            ..Dirs::default()
        });
        file_chosen(&mut shell, temp.0.join("unread-file"));
        assert!(failed(&mut shell).contains("no absolute data directory"));
    }
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 0);
}

#[test]
fn refused_and_unreadable_files_preserve_the_existing_index() {
    let temp = TestDir::new();
    let data = temp.0.join("data");
    fs::create_dir(&data).unwrap();
    let index = data.join("oewn.index");
    fs::write(&index, b"previous index").unwrap();
    let corrupt = temp.0.join("english-wordnet-2025.xml.gz");
    fs::write(&corrupt, b"corrupt input").unwrap();
    let mut shell = startup(temp.dirs());
    for path in [corrupt, temp.0.join("missing-file")] {
        file_chosen(&mut shell, path.clone());
        assert!(failed(&mut shell).contains(&path.display().to_string()));
        assert_eq!(fs::read(&index).unwrap(), b"previous index");
        assert_eq!(fs::read_dir(&data).unwrap().count(), 1);
    }
}

#[test]
#[ignore = "imports the real pinned OEWN file; run explicitly"]
fn the_real_chooser_action_imports_oewn_and_preserves_it_on_refusal() {
    let fixture = std::env::var_os("ADDICT_OEWN_IMPORT_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../experiments/exp7/data/english-wordnet-2025.xml.gz")
        });
    assert!(
        fixture.is_file(),
        "set ADDICT_OEWN_IMPORT_FILE to the pinned file"
    );
    let temp = TestDir::new();
    let data = temp.0.join("data");
    let refresh = gate();
    let mut shell = shell(
        temp.dirs(),
        String::new(),
        None,
        ReadGates {
            refresh: Some(refresh.clone()),
            ..Default::default()
        },
        Limits {
            check: Duration::from_secs(15),
            ..Limits::default()
        },
    );
    shell.wait_for(|view| view.settings.languages_read && view.downloads.indexes.is_some());
    file_chosen(&mut shell, fixture.clone());
    let state = shell.next().0.downloads.import;
    assert_eq!(state, ImportState::Importing);
    wait_until(Duration::from_secs(60), || data.join("oewn.index").exists());
    shell.quiet(Event::ImportPressed);
    let index = Index::open(&data.join("oewn.index")).expect("installed index opens");
    assert!(index.headwords() > 100_000);
    assert!(index.find("dog").expect("English lookup").is_some());
    refresh.release();
    let view = shell.wait_for_within(Duration::from_secs(30), |view| {
        matches!(view.downloads.import, ImportState::Imported(_))
    });
    let ImportState::Imported(ref imported) = view.downloads.import else {
        unreachable!()
    };
    assert_eq!(imported.headwords as u64, index.headwords());
    let completion = view.downloads.import.text();
    assert!(completion.contains("Imported oewn.index"));
    assert!(completion.contains("already in use, restart Addict to use the new edition"));
    assert_eq!(view.downloads.indexes.unwrap().definitions, State::Open);
    let prior = fs::read(&imported.path).unwrap();
    let corrupt = temp.0.join("corrupt.gz");
    fs::write(&corrupt, b"not an upstream dictionary").unwrap();
    file_chosen(&mut shell, corrupt);
    assert!(failed(&mut shell).contains("corrupt.gz"));
    assert_eq!(fs::read(&imported.path).unwrap(), prior);

    // A regular file cannot be the data directory, even as root.
    let unwritable = temp.0.join("not-a-directory");
    fs::write(&unwritable, b"leave this intact").unwrap();
    let mut other = startup(Dirs {
        data: Some(unwritable.clone()),
        ..Dirs::default()
    });
    file_chosen(&mut other, fixture);
    assert!(
        failed_within(&mut other, Duration::from_secs(60))
            .contains(&unwritable.display().to_string())
    );
    assert_eq!(fs::read(&unwritable).unwrap(), b"leave this intact");
    assert_eq!(fs::read(&imported.path).unwrap(), prior);
}

pub(super) fn kaikki_fixture(root: &Path) -> PathBuf {
    let path = root.join("es-extract.jsonl.gz");
    let mut gzip = GzEncoder::new(fs::File::create(&path).unwrap(), Compression::fast());
    for word in ["banco".to_owned()]
        .into_iter()
        .chain((0..MIN_KAIKKI_HEADWORDS).map(|n| format!("palabra{n}")))
    {
        writeln!(
            gzip,
            r#"{{"word":"{word}","lang_code":"es","senses":[{{"glosses":["asiento"]}}]}}"#
        )
        .unwrap();
    }
    gzip.finish().unwrap();
    path
}

#[test]
fn import_completion_refreshes_missing_languages_before_the_next_lookup() {
    let temp = TestDir::new();
    let data = temp.0.join("data");
    fs::create_dir(&data).unwrap();
    let refresh = gate();
    let (checked, checks) = std::sync::mpsc::channel();
    let mut shell = shell(
        temp.dirs(),
        String::new(),
        None,
        ReadGates {
            refresh: Some(Arc::clone(&refresh)),
            checked: Some(checked),
            ..ReadGates::default()
        },
        Limits::default(),
    );
    let missing = shell
        .wait_for(|view| view.settings.languages_read && view.downloads.indexes.is_some())
        .downloads
        .indexes
        .unwrap();
    checks.recv_timeout(super::WAIT).unwrap();
    assert_eq!(missing.spanish, State::Missing);
    assert_eq!(missing.catalan, State::Missing);

    for (language, word) in [(Language::Es, "banco"), (Language::Ca, "banc")] {
        shell.dispatch(Event::ReadingChosen(language));
        shell.edit(word);
        let view = shell.wait_for(|view| view.sentence == word);
        assert_eq!(
            view.shown.lookup(),
            Some(&Lookup::NoDictionary(language)),
            "initially gated by Missing"
        );
        checks.recv_timeout(super::WAIT).unwrap();
        shell.edit("");
        shell.wait_for(|view| view.sentence.is_empty());
    }
    // The tiny pair fixtures preserve the translation assertions of the
    // original regression. A real manual import triggers their refresh.
    write_index(&data, Source::WikdictEsRu, "banco");
    write_index(&data, Source::WikdictCaRu, "banc");
    file_chosen(&mut shell, kaikki_fixture(&temp.0));
    let pending = shell.next().0;
    assert_eq!(pending.downloads.import, ImportState::Importing);
    assert_eq!(pending.downloads.import.text(), "Importing dictionary…");
    wait_until(Duration::from_secs(4), || {
        data.join("kaikki-es-Espanol.index").exists()
    });
    let pending = super::downloads::mark(&mut shell);
    assert_eq!(pending.downloads.indexes, Some(missing));
    assert_eq!(
        pending.downloads.import,
        ImportState::Importing,
        "not ready before controller delivery"
    );

    refresh.release();
    let completed =
        shell.wait_for(|view| matches!(view.downloads.import, ImportState::Imported(_)));
    assert_ne!(completed.downloads.indexes, Some(missing));
    super::downloads::stale_lookup(&mut shell, missing);
    let ready = super::downloads::mark(&mut shell);
    assert_eq!(
        ready.downloads.indexes, completed.downloads.indexes,
        "late pre-import events are ignored"
    );
    for (language, word) in [(Language::Es, "banco"), (Language::Ca, "banc")] {
        super::downloads::assert_pair_lookup(&mut shell, language, word);
    }
}

#[test]
fn stale_file_choice_cannot_complete_a_new_chooser() {
    let mut shell = startup(Dirs::default());
    let old = chooser(&mut shell);
    shell.dispatch(Event::FileChosen {
        id: old,
        result: Err(FileChoiceFailure::Cancelled),
    });
    assert_eq!(shell.next().0.downloads.import, ImportState::Idle);
    let id = chooser(&mut shell);
    assert_ne!(id, old);
    shell.quiet(Event::FileChosen {
        id: old,
        result: Ok(PathBuf::from("/tmp/old-file")),
    });
    shell.dispatch(Event::FileChosen {
        id,
        result: Err(FileChoiceFailure::Cancelled),
    });
    assert_eq!(shell.next().0.downloads.import, ImportState::Idle);
}

#[test]
fn import_refresh_timeout_reports_that_the_file_was_imported() {
    let temp = TestDir::new();
    let mut shell = shell(
        temp.dirs(),
        String::new(),
        None,
        ReadGates {
            refresh: Some(gate()),
            ..ReadGates::default()
        },
        Limits {
            check: super::WAIT * 2,
            download: Duration::from_millis(100),
            ..Limits::default()
        },
    );
    shell.wait_for(|view| view.settings.languages_read && view.downloads.indexes.is_some());
    file_chosen(&mut shell, kaikki_fixture(&temp.0));
    assert!(
        failed(&mut shell)
            .starts_with("The file was imported, but dictionary readiness could not be refreshed:")
    );
    assert!(temp.0.join("data/kaikki-es-Espanol.index").is_file());
}

#[test]
fn import_deadline_expires_while_waiting_to_import() {
    let temp = TestDir::new();
    let mut shell = shell(
        temp.dirs(),
        String::new(),
        None,
        ReadGates::default(),
        Limits {
            import: Duration::ZERO,
            ..Limits::default()
        },
    );
    shell.wait_for(|view| view.settings.languages_read && view.downloads.indexes.is_some());
    file_chosen(&mut shell, kaikki_fixture(&temp.0));
    assert_eq!(
        failed(&mut shell),
        addict_engine::view::first_run::late_text(Duration::ZERO)
    );
}
