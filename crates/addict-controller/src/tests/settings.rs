//! Scripted-shell tests for settings: files, the key, the cache, the login
//! item and the shortcuts. The shell here only sends events and runs effects
//! on demand; every decision and every text belongs to the controller.

use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use addict_core::{
    dirs::Dirs,
    failure::{
        Ended,
        key_store::KeyStoreFailure,
        login_item::{LoginItemShown, StartupEnd, StartupFailure},
    },
    translation::{English, Request, Source as Language},
};
use addict_deepl::key::{KeyError, KeySource, Secret, WriteError};
use addict_engine::{
    at_once, cache, english, language, my_language,
    view::{
        Binding, CacheState, KeyState, NO_KEY_PLACE, REBINDING, SAVE_UNANSWERED, STARTUP_LEGACY,
        STARTUP_NO_RUNNER, STARTUP_TIMED_OUT, STARTUP_UNREAD, binding_texts, clear_text, save_text,
        startup_unfinished_text,
    },
};

use super::{FakeShell, Silent, marker};
use crate::{
    Effect, Event, Message, RequestId, ViewState,
    settings::{Bindings, SettingsRead, ShortcutState},
};

static TEST_DIR: AtomicU64 = AtomicU64::new(0);

/// A private configuration and cache directory of one test.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let unique = TEST_DIR.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "addict-controller-settings-{tag}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a private settings directory");
        Self(path)
    }

    fn dirs(&self) -> Dirs {
        Dirs {
            config: Some(self.0.join("config")),
            cache: Some(self.0.join("cache")),
            ..Dirs::default()
        }
    }

    fn config(&self) -> PathBuf {
        self.0.join("config")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wait_for(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(
            Instant::now() < deadline,
            "the runner did not answer in time"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn shell(dirs: Dirs) -> FakeShell {
    FakeShell::with_settings(
        dirs,
        Silent,
        false,
        KeySource::new(Err(KeyError::Missing)),
        crate::settings::Limits::default(),
        crate::capture::Capture::default(),
        crate::selection::Selection::default(),
        false,
        None,
    )
}

fn ready(shell: &mut FakeShell) -> ViewState {
    // The constructor has already consumed the startup read; a marker gives
    // a fresh snapshot of the settings as they stand now.
    marker(shell)
}

fn step(shell: &mut FakeShell, event: Event) -> (ViewState, Vec<Effect>) {
    shell.dispatch(event);
    shell.next()
}

/// Injects the four file reads as the runner would report them, in order.
/// Each read gets its own snapshot; the last one carries the combined effects.
fn send_read(
    shell: &mut FakeShell,
    reading: Language,
    my: Language,
    english: English,
    at_once: bool,
) -> (ViewState, Vec<Effect>) {
    let mut effects = Vec::new();
    let mut view = ViewState::default();
    for item in [
        SettingsRead::Reading(reading),
        SettingsRead::My(my),
        SettingsRead::English(english),
        SettingsRead::AtOnce(at_once),
    ] {
        shell
            .controller
            .dispatch
            .0
            .send(Message::SettingsRead(item))
            .unwrap();
        let (rendered, caused) = shell.next();
        view = rendered;
        effects.extend(caused);
    }
    (view, effects)
}

fn cache_request() -> Request {
    Request {
        text: "bank".to_owned(),
        context: None,
        source: Language::En,
        target: Language::Ru,
        english: English::Us,
    }
}

fn key_effect(effects: &[Effect]) -> (RequestId, Secret) {
    let [Effect::StoreKey { id, key }] = effects else {
        panic!("one store effect: {effects:?}");
    };
    (*id, key.clone())
}

#[test]
fn settings_are_read_from_the_files_at_start() {
    let temp = TempDir::new("read");
    let config = temp.config();
    fs::create_dir_all(&config).unwrap();
    language::write(Some(&config.join("language")), Language::Ru).unwrap();
    my_language::write(Some(&config.join("my-language")), Language::En).unwrap();
    english::write(Some(&config.join("english")), English::Gb).unwrap();
    at_once::write(Some(&config.join("translate-sentence")), false).unwrap();

    let mut shell = shell(temp.dirs());
    let view = ready(&mut shell);
    assert!(view.settings.languages_read);
    assert_eq!(view.settings.my_language, Language::En);
    assert_eq!(view.settings.reading, Language::Ru); // Ru is not read while English is mine.
    assert_eq!(view.settings.english, English::Gb);
    assert!(view.settings.english_shown);
    assert!(!view.settings.at_once);
}

#[test]
fn a_choice_before_the_start_read_ends_is_kept() {
    let mut shell = shell(Dirs::default());
    step(&mut shell, Event::MyLanguageChosen(Language::Es));
    step(&mut shell, Event::ReadingChosen(Language::Ca));
    step(&mut shell, Event::EnglishChosen(English::Gb));
    shell.dispatch(Event::AtOnceChosen(false));
    // The reads arrive after all four choices, as off a slow disk.
    send_read(&mut shell, Language::En, Language::Ru, English::Us, true);
    let view = marker(&mut shell);
    assert!(view.settings.languages_read);
    assert_eq!(view.settings.my_language, Language::Es);
    assert_eq!(view.settings.reading, Language::Ca);
    assert_eq!(view.settings.english, English::Gb);
    assert!(!view.settings.at_once);
}

#[test]
fn choices_are_written_with_the_0_5_0_file_names_and_contents() {
    let temp = TempDir::new("write");
    let mut shell = shell(temp.dirs());
    ready(&mut shell);
    step(&mut shell, Event::ReadingChosen(Language::Es));
    step(&mut shell, Event::MyLanguageChosen(Language::Ru));
    step(&mut shell, Event::EnglishChosen(English::Gb));
    shell.dispatch(Event::AtOnceChosen(false));
    let config = temp.config();
    wait_for(|| {
        fs::read_to_string(config.join("language")).unwrap_or_default() == "es\n"
            && fs::read_to_string(config.join("my-language")).unwrap_or_default() == "ru\n"
            && fs::read_to_string(config.join("english")).unwrap_or_default() == "gb\n"
            && fs::read_to_string(config.join("translate-sentence")).unwrap_or_default() == "off\n"
    });
    let view = marker(&mut shell);
    assert_eq!(view.settings.english, English::Gb);
    assert!(!view.settings.at_once);
}

#[test]
fn save_a_key_emits_store_key_and_shows_the_committed_key() {
    let temp = TempDir::new("key");
    let mut shell = shell(temp.dirs());
    ready(&mut shell);
    let key = Secret::new("synthetic-saved-key").unwrap();
    shell.dispatch(Event::SaveKey(Ok(key.clone())));
    let (view, effects) = shell.next();
    assert_eq!(view.settings.key_line, "Saving the key…");
    let (id, sent) = key_effect(&effects);
    assert_eq!(sent, key);

    step(&mut shell, Event::KeyChanged(Ok(key.clone())));
    let (view, _) = step(&mut shell, Event::KeyStored { id, result: Ok(()) });
    assert_eq!(view.settings.key, KeyState::Pro);
    assert_eq!(view.settings.key_line, "");
    assert_eq!(view.settings.key_entry_clears, 1);
}

#[test]
fn an_invalid_key_or_a_missing_place_needs_no_effect() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    shell.dispatch(Event::SaveKey(Err(WriteError::Invalid)));
    let (view, effects) = shell.next();
    assert!(effects.is_empty());
    assert!(view.settings.key_line.starts_with("The key was not saved:"));
    let key = Secret::new("synthetic-key").unwrap();
    shell.dispatch(Event::SaveKey(Ok(key)));
    let (view, effects) = shell.next();
    assert!(effects.is_empty());
    assert!(view.settings.key_line.contains("XDG_CONFIG_HOME"));
}

#[test]
fn a_saved_key_leaves_only_as_store_key() {
    let mut shell = shell(Dirs {
        config: Some(PathBuf::from("/tmp/addict-settings-secret-test")),
        ..Dirs::default()
    });
    ready(&mut shell);
    let key = Secret::new("a-secret-key-that-must-never-be-shown").unwrap();
    shell.dispatch(Event::SaveKey(Ok(key)));
    let (_, effects) = shell.next();
    let view = marker(&mut shell);
    for shown in [
        format!("{view:?}"),
        format!("{effects:?}"),
        format!("{:?}", view.settings),
    ] {
        assert!(!shown.contains("a-secret-key"), "{shown}");
    }
    assert!(matches!(effects.as_slice(), [Effect::StoreKey { .. }]));
}

#[test]
fn a_key_save_deadline_cancels_then_a_late_result_is_ignored() {
    let temp = TempDir::new("key-deadline");
    let mut shell = FakeShell::with_settings(
        temp.dirs(),
        Silent,
        false,
        KeySource::new(Err(KeyError::Missing)),
        crate::settings::Limits {
            key_save: Duration::from_millis(20),
            ..crate::settings::Limits::default()
        },
        crate::capture::Capture::default(),
        crate::selection::Selection::default(),
        false,
        None,
    );
    ready(&mut shell);
    let key = Secret::new("committed-key:fx").unwrap();
    shell.dispatch(Event::SaveKey(Ok(key.clone())));
    let (_, effects) = shell.next();
    let (id, _) = key_effect(&effects);
    // The shell publishes the committed key before the deadline passes.
    step(&mut shell, Event::KeyChanged(Ok(key)));
    let (view, effects) = shell.next();
    assert_eq!(effects, [Effect::Cancel(id)]);
    assert_eq!(view.settings.key, KeyState::Free);
    assert_eq!(view.settings.key_line, SAVE_UNANSWERED);
    assert_eq!(view.settings.key, KeyState::Free);
    shell.dispatch(Event::KeyStored {
        id,
        result: Err(Ended::Shell(KeyStoreFailure::Cancelled)),
    });
    assert_eq!(marker(&mut shell).settings.key_line, SAVE_UNANSWERED);
}

#[test]
fn a_login_switch_runs_the_effect_and_shows_the_answer() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    shell.dispatch(Event::LoginItemToggled(true));
    let (view, effects) = shell.next();
    assert!(!view.settings.login_sensitive);
    assert_eq!(view.settings.login_line, "Asking the desktop…");
    let [Effect::SetLoginItem { id, on }] = effects.as_slice() else {
        panic!("login effect: {effects:?}");
    };
    assert!(*on);
    shell.dispatch(Event::LoginItemSet {
        id: *id,
        result: Ok(StartupEnd::Done(true)),
        runner_missing: false,
    });
    let view = marker(&mut shell);
    assert!(view.settings.login_on);
    assert!(view.settings.login_sensitive);
    assert_eq!(view.settings.login_line, "");
}

#[test]
fn a_login_deadline_cancels_and_an_unfinished_change_says_so() {
    let mut shell = FakeShell::with_settings(
        Dirs::default(),
        Silent,
        false,
        KeySource::new(Err(KeyError::Missing)),
        crate::settings::Limits {
            startup: Duration::from_millis(20),
            ..crate::settings::Limits::default()
        },
        crate::capture::Capture::default(),
        crate::selection::Selection::default(),
        false,
        None,
    );
    ready(&mut shell);
    shell.dispatch(Event::LoginItemToggled(false));
    let (_, effects) = shell.next();
    let [Effect::SetLoginItem { id, .. }] = effects.as_slice() else {
        panic!("login effect: {effects:?}");
    };
    let id = *id;
    let (view, effects) = shell.next();
    assert_eq!(effects, [Effect::Cancel(id)]);
    assert_eq!(view.settings.login_line, STARTUP_TIMED_OUT);
    assert!(view.settings.login_sensitive);
    shell.dispatch(Event::LoginItemSet {
        id,
        result: Ok(StartupEnd::Done(false)),
        runner_missing: false,
    });
    assert_eq!(marker(&mut shell).settings.login_line, STARTUP_TIMED_OUT);

    shell.dispatch(Event::LoginItemToggled(true));
    let (_, effects) = shell.next();
    let [Effect::SetLoginItem { id, .. }] = effects.as_slice() else {
        panic!("second login effect: {effects:?}");
    };
    let (view, _) = step(
        &mut shell,
        Event::LoginItemSet {
            id: *id,
            result: Err(Ended::Unfinished),
            runner_missing: false,
        },
    );
    assert_eq!(
        view.settings.login_line,
        "Startup was not changed: the request did not run."
    );
}

#[test]
fn opening_settings_reads_the_login_item_and_the_cache_again() {
    let temp = TempDir::new("open");
    let mut shell = shell(temp.dirs());
    shell
        .controller
        .cache()
        .put(&cache::key_of(&cache_request()), "answer");
    ready(&mut shell);
    shell.dispatch(Event::SettingsOpened);
    let (mut view, effects) = shell.next();
    assert_eq!(effects, [Effect::ReadLoginItem]);
    assert_eq!(view.settings.cache, CacheState::Checking);
    assert_eq!(view.settings.key_entry_clears, 1);
    assert!(!view.settings.login_sensitive);
    assert!(view.settings.key_line.is_empty() && view.settings.cache_line.is_empty());
    // The cache job renders on its own; consume that snapshot before the
    // login result so the assertion below reads the login render.
    while view.settings.cache == CacheState::Checking {
        view = shell.next().0;
    }
    assert!(matches!(view.settings.cache, CacheState::Read(footprint) if footprint.rows > 0));

    let (view, _) = step(
        &mut shell,
        Event::LoginItemRead {
            shown: Some(LoginItemShown {
                on: true,
                legacy: true,
            }),
            runner_missing: false,
        },
    );
    assert!(view.settings.login_on && view.settings.login_sensitive);
    assert_eq!(
        view.settings.login_line,
        addict_engine::view::STARTUP_LEGACY
    );
    assert!(view.settings.cache_line.is_empty());

    // A login read queued while a change runs is ignored.
    shell.dispatch(Event::LoginItemToggled(false));
    shell.next();
    shell.dispatch(Event::LoginItemRead {
        shown: Some(LoginItemShown {
            on: false,
            legacy: false,
        }),
        runner_missing: false,
    });
    assert_eq!(
        marker(&mut shell).settings.login_line,
        "Asking the desktop…"
    );
}

#[test]
fn clearing_the_cache_clears_the_rows_then_reads_the_size_again() {
    let temp = TempDir::new("clear");
    let mut shell = shell(temp.dirs());
    let store = shell.controller.cache();
    store.put(&cache::key_of(&cache_request()), "answer");
    ready(&mut shell);
    step(&mut shell, Event::ClearCachePressed);
    wait_for(|| store.footprint().rows == 0);
    loop {
        let (view, _) = shell.next();
        if view.settings.cache_line.is_empty()
            && matches!(view.settings.cache, CacheState::Read(footprint) if footprint.rows == 0)
        {
            break;
        }
    }
}

#[test]
fn shortcut_changes_move_the_binding_and_the_line() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    let bindings = Bindings {
        clipboard: Some("Super+Q".into()),
        screenshot: Some("Super+Shift+Q".into()),
    };
    let (view, _) = step(
        &mut shell,
        Event::ShortcutChanged(ShortcutState::Rebinding(bindings.clone())),
    );
    assert_eq!(view.settings.binding, Binding::Bound);
    assert_eq!(view.settings.bindings, bindings);
    assert!(view.settings.rebinding);
    assert_eq!(view.settings.status_line, addict_engine::view::REBINDING);

    let (view, _) = step(
        &mut shell,
        Event::ShortcutChanged(ShortcutState::Bound(bindings.clone())),
    );
    assert_eq!(view.settings.bindings, bindings);
    assert_eq!(view.settings.status_line, addict_engine::view::REBINDING);

    let (view, _) = step(
        &mut shell,
        Event::ShortcutChanged(ShortcutState::ConfigureAnswered(Some("no session".into()))),
    );
    assert_eq!(
        view.settings.status_line,
        addict_engine::view::change_text(Some("no session"))
    );

    let (view, _) = step(
        &mut shell,
        Event::ShortcutChanged(ShortcutState::RebindEnded(None)),
    );
    assert!(!view.settings.rebinding);
    assert_eq!(
        view.settings.status_line,
        addict_engine::view::rebind_text(None)
    );

    let (view, _) = step(
        &mut shell,
        Event::ShortcutChanged(ShortcutState::Failed("broke".into())),
    );
    assert!(matches!(view.settings.binding, Binding::NotBound { .. }));
    assert_eq!(view.settings.status_line, "");
    assert!(binding_texts(&view.settings.binding).unwrap().retry);
}

#[test]
fn an_unreadable_file_gives_its_default_and_finishes_the_start_read() {
    let temp = TempDir::new("unreadable");
    let config = temp.config();
    fs::create_dir_all(&config).unwrap();
    // A directory where the reading language file should be: reading it fails.
    fs::create_dir(config.join("language")).unwrap();
    english::write(Some(&config.join("english")), English::Gb).unwrap();
    let mut shell = shell(temp.dirs());
    let view = ready(&mut shell);
    assert!(view.settings.languages_read);
    assert_eq!(view.settings.reading, Language::En);
    assert_eq!(view.settings.english, English::Gb);
    assert_eq!(view.settings.my_language, Language::Ru);
}

#[test]
fn a_late_read_of_one_file_does_not_change_a_chosen_setting_or_the_others() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    step(&mut shell, Event::ReadingChosen(Language::Es));
    shell
        .controller
        .dispatch
        .0
        .send(Message::SettingsRead(SettingsRead::My(Language::En)))
        .unwrap();
    let view = shell.next().0;
    assert_eq!(view.settings.my_language, Language::En);
    assert_eq!(view.settings.reading, Language::Es);
}

#[test]
fn a_second_login_toggle_while_one_runs_is_ignored_and_the_switch_holds() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    let (view, effects) = step(&mut shell, Event::LoginItemToggled(true));
    assert!(view.settings.login_on);
    let [Effect::SetLoginItem { id, on }] = effects.as_slice() else {
        panic!("login effect: {effects:?}");
    };
    assert!(*on);
    let id = *id;
    shell.dispatch(Event::LoginItemToggled(false));
    let view = marker(&mut shell);
    assert!(view.settings.login_on, "the switch shows the asked state");
    let view = step(
        &mut shell,
        Event::LoginItemSet {
            id,
            result: Ok(StartupEnd::Done(true)),
            runner_missing: false,
        },
    )
    .0;
    assert!(view.settings.login_on);
}

#[test]
fn reopening_settings_keeps_a_pending_login_change_and_the_rebind_line() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    step(
        &mut shell,
        Event::ShortcutChanged(ShortcutState::Rebinding(Bindings::default())),
    );
    step(&mut shell, Event::LoginItemToggled(true));
    let (view, effects) = step(&mut shell, Event::SettingsOpened);
    let _ = shell.next(); // the cache job's own render
    assert_eq!(view.settings.login_line, "Asking the desktop…");
    assert!(!view.settings.login_sensitive);
    assert!(view.settings.login_on);
    assert!(!effects.contains(&Effect::ReadLoginItem));
    assert_eq!(view.settings.status_line, REBINDING);
    assert_eq!(view.settings.key_entry_clears, 1);
    // Once the rebind ends, a reopening has no line to keep.
    step(
        &mut shell,
        Event::ShortcutChanged(ShortcutState::RebindEnded(None)),
    );
    let view = step(&mut shell, Event::SettingsOpened).0;
    assert_eq!(view.settings.status_line, "");
}

#[test]
fn a_missing_runner_is_said_on_both_login_events() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    let view = step(
        &mut shell,
        Event::LoginItemRead {
            shown: None,
            runner_missing: true,
        },
    )
    .0;
    assert!(view.settings.runner_missing);
    assert!(!view.settings.login_sensitive);
    assert_eq!(view.settings.login_line, STARTUP_NO_RUNNER);

    let (_, effects) = step(&mut shell, Event::LoginItemToggled(true));
    let [Effect::SetLoginItem { id, .. }] = effects.as_slice() else {
        panic!("login effect: {effects:?}");
    };
    let id = *id;
    let view = step(
        &mut shell,
        Event::LoginItemSet {
            id,
            result: Err(Ended::Unfinished),
            runner_missing: true,
        },
    )
    .0;
    assert!(!view.settings.login_on, "the switch keeps its old state");
    assert_eq!(view.settings.login_line, STARTUP_NO_RUNNER);
}

#[test]
fn an_unreadable_login_item_says_so() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    let view = step(
        &mut shell,
        Event::LoginItemRead {
            shown: None,
            runner_missing: false,
        },
    )
    .0;
    assert_eq!(view.settings.login_line, STARTUP_UNREAD);
    assert!(view.settings.login_sensitive);
}

#[test]
fn a_failed_cache_clear_says_why() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    shell
        .controller
        .dispatch
        .0
        .send(Message::CacheCleared(Some("disk".to_owned())))
        .unwrap();
    let view = shell.next().0;
    assert_eq!(view.settings.cache_line, clear_text(Err("disk")));
}

#[test]
fn retry_and_configure_only_run_in_the_right_binding_state() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    // While the first bind runs, neither action does anything.
    shell.dispatch(Event::RetryPressed);
    let view = step(&mut shell, Event::ConfigurePressed).0;
    assert_eq!(
        view.settings.status_line,
        addict_engine::view::CHANGE_UNBOUND
    );

    let view = step(
        &mut shell,
        Event::ShortcutChanged(ShortcutState::Failed("no portal".into())),
    )
    .0;
    assert!(matches!(view.settings.binding, Binding::NotBound { .. }));
    let (view, effects) = step(&mut shell, Event::RetryPressed);
    assert_eq!(view.settings.binding, Binding::Attempting);
    assert_eq!(effects, [Effect::BindShortcuts]);
    let (view, _) = step(&mut shell, Event::ConfigurePressed);
    assert_eq!(
        view.settings.status_line,
        addict_engine::view::CHANGE_UNBOUND
    );

    step(
        &mut shell,
        Event::ShortcutChanged(ShortcutState::Bound(Default::default())),
    );
    let (_, effects) = step(&mut shell, Event::ConfigurePressed);
    assert_eq!(effects, [Effect::ConfigureShortcuts]);
}

/// A shell whose four real file reads wait at `gate`, for tests that act
/// before the start reads land.
fn gated_shell(dirs: Dirs) -> (FakeShell, Arc<crate::settings::ReadGate>) {
    let gate = Arc::new(crate::settings::ReadGate::default());
    let mut shell = FakeShell::with_unread_settings(
        dirs,
        Silent,
        false,
        KeySource::new(Err(KeyError::Missing)),
        crate::settings::Limits::default(),
        crate::capture::Capture::default(),
        crate::selection::Selection::default(),
        false,
        Some(gate.clone()),
    );
    // Consume the initial snapshot; the four reads are still waiting.
    let (view, effects) = shell.next();
    assert!(effects.is_empty() && !view.settings.languages_read);
    shell.wait_for(|view| view.downloads.indexes.is_some());
    (shell, gate)
}

fn until_read(shell: &mut FakeShell) -> ViewState {
    // Fails rather than hangs when a read never reports.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "the start read never finished");
        shell.dispatch(Event::SetPressed(Language::Ca));
        let (view, effects) = shell.next();
        if effects.is_empty() && view.settings.languages_read {
            return view;
        }
    }
}

#[test]
fn a_choice_while_the_files_are_pending_keeps_the_other_files_values() {
    let temp = TempDir::new("gated-choice");
    let config = temp.config();
    fs::create_dir_all(&config).unwrap();
    language::write(Some(&config.join("language")), Language::Ca).unwrap();
    at_once::write(Some(&config.join("translate-sentence")), false).unwrap();
    english::write(Some(&config.join("english")), English::Gb).unwrap();

    let (mut shell, gate) = gated_shell(temp.dirs());
    // The read of my language is still waiting, so the choice wins; the
    // reads of the other three files must run all the same.
    let (chosen, _) = step(&mut shell, Event::MyLanguageChosen(Language::Es));
    assert_eq!(chosen.settings.my_language, Language::Es);
    assert!(!chosen.settings.languages_read);
    gate.release();
    let view = until_read(&mut shell);
    assert_eq!(view.settings.my_language, Language::Es);
    assert_eq!(view.settings.reading, Language::Ca);
    assert_eq!(view.settings.english, English::Gb);
    assert!(!view.settings.at_once);
    wait_for(|| fs::read_to_string(config.join("my-language")).unwrap_or_default() == "es\n");
}

#[test]
fn a_gloss_writes_the_reading_language_file_in_a_real_directory() {
    let temp = TempDir::new("gated-gloss");
    let config = temp.config();
    fs::create_dir_all(&config).unwrap();
    language::write(Some(&config.join("language")), Language::Ru).unwrap();
    let data = super::TestData::with_banco();
    let dirs = Dirs {
        config: Some(config.clone()),
        data: Some(data.0.clone()),
        ..Dirs::default()
    };

    let (mut shell, gate) = gated_shell(dirs);
    step(&mut shell, Event::ReadingChosen(Language::Es));
    shell.edit("banco");
    // The answer's article: the edit's first snapshot may come before the
    // lookup starts, and a gloss of that article would be ignored.
    let shown = loop {
        let (view, _) = shell.next();
        if view.answered == Some(Language::Es) && !view.looking_up {
            break view;
        }
    };
    let article = shown.article_id();
    shell.dispatch(Event::GlossChosen { article, index: 0 });
    loop {
        let (view, _) = shell.next();
        if view.shown.choice() == Some(addict_engine::view::Choice::Gloss(0)) {
            break;
        }
    }
    shell.dispatch(Event::LookUpEnglish {
        article,
        index: 0,
        word: "bank".into(),
    });
    // The lookup's record effects can arrive in either order; the gloss's
    // own render changes the reading language and writes its file.
    loop {
        let (view, _) = shell.next();
        if view.settings.reading == Language::En {
            break;
        }
    }
    gate.release();
    wait_for(|| fs::read_to_string(config.join("language")).unwrap_or_default() == "en\n");
}

#[test]
fn every_login_end_restores_the_state_confirmed_before_the_toggle() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    let ends: Vec<(Result<StartupEnd, Ended<StartupFailure>>, bool, String)> = vec![
        (Ok(StartupEnd::Done(false)), false, String::new()),
        (
            Ok(StartupEnd::NotAsked("no launcher".into())),
            false,
            "Startup was not changed: no launcher.".to_owned(),
        ),
        (
            Ok(StartupEnd::Failed(StartupFailure::Refused)),
            false,
            "Startup was not changed: the Background portal refused.".to_owned(),
        ),
        (
            Ok(StartupEnd::Revoked {
                save: "disk".into(),
            }),
            false,
            "Startup was turned back off, as its state could not be saved: disk.".to_owned(),
        ),
        (
            Err(Ended::Shell(StartupFailure::Unavailable)),
            false,
            "Startup was not changed: the Background portal is not available.".to_owned(),
        ),
        (
            Err(Ended::DeadlinePassed),
            false,
            STARTUP_TIMED_OUT.to_owned(),
        ),
        (Err(Ended::Unfinished), false, startup_unfinished_text()),
    ];
    for (result, on, line) in ends {
        let (view, effects) = step(&mut shell, Event::LoginItemToggled(true));
        assert!(view.settings.login_on, "the switch shows the asked state");
        let [Effect::SetLoginItem { id, on: true }] = effects.as_slice() else {
            panic!("login effect: {effects:?}");
        };
        let id = *id;
        let (view, _) = step(
            &mut shell,
            Event::LoginItemSet {
                id,
                result,
                runner_missing: false,
            },
        );
        assert_eq!(view.settings.login_on, on, "{line}");
        assert_eq!(view.settings.login_line, line);
        assert!(view.settings.login_sensitive);
    }
}

#[test]
fn a_confirmed_login_change_shows_the_state_the_portal_reported() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    let (_, effects) = step(&mut shell, Event::LoginItemToggled(true));
    let [Effect::SetLoginItem { id, .. }] = effects.as_slice() else {
        panic!("login effect: {effects:?}");
    };
    let view = step(
        &mut shell,
        Event::LoginItemSet {
            id: *id,
            result: Ok(StartupEnd::Done(true)),
            runner_missing: false,
        },
    )
    .0;
    assert!(view.settings.login_on);
    assert!(view.settings.login_line.is_empty());
}

#[test]
fn a_controller_login_deadline_cancels_and_restores_the_switch() {
    let mut shell = FakeShell::with_settings(
        Dirs::default(),
        Silent,
        false,
        KeySource::new(Err(KeyError::Missing)),
        crate::settings::Limits {
            startup: Duration::from_millis(20),
            ..crate::settings::Limits::default()
        },
        crate::capture::Capture::default(),
        crate::selection::Selection::default(),
        false,
        None,
    );
    ready(&mut shell);
    let (_, effects) = step(&mut shell, Event::LoginItemToggled(true));
    let [Effect::SetLoginItem { id, .. }] = effects.as_slice() else {
        panic!("login effect: {effects:?}");
    };
    let id = *id;
    let (view, effects) = shell.next();
    assert_eq!(effects, [Effect::Cancel(id)]);
    assert_eq!(view.settings.login_line, STARTUP_TIMED_OUT);
    assert!(!view.settings.login_on, "no change was confirmed");
    shell.dispatch(Event::LoginItemSet {
        id,
        result: Ok(StartupEnd::Done(true)),
        runner_missing: false,
    });
    assert_eq!(marker(&mut shell).settings.login_line, STARTUP_TIMED_OUT);
}

#[test]
fn a_no_place_key_save_ignores_the_invalid_key() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    let (view, effects) = step(&mut shell, Event::SaveKey(Err(WriteError::Invalid)));
    assert!(effects.is_empty());
    assert_eq!(view.settings.key_line, save_text(Err(NO_KEY_PLACE)));
}

#[test]
fn a_stale_login_read_after_a_change_is_ignored_until_settings_opens_again() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    let (_, effects) = step(&mut shell, Event::LoginItemToggled(true));
    let [Effect::SetLoginItem { id, .. }] = effects.as_slice() else {
        panic!("login effect: {effects:?}");
    };
    let id = *id;
    shell.dispatch(Event::LoginItemRead {
        shown: Some(LoginItemShown {
            on: false,
            legacy: false,
        }),
        runner_missing: false,
    });
    assert_eq!(
        marker(&mut shell).settings.login_line,
        "Asking the desktop…"
    );
    step(
        &mut shell,
        Event::LoginItemSet {
            id,
            result: Ok(StartupEnd::Done(true)),
            runner_missing: false,
        },
    );
    // A read that was asked before the toggle and ends after its answer is
    // stale: it would show the state from before the change.
    shell.dispatch(Event::LoginItemRead {
        shown: Some(LoginItemShown {
            on: false,
            legacy: false,
        }),
        runner_missing: false,
    });
    let view = marker(&mut shell);
    assert!(view.settings.login_on, "the confirmed change stays");
    assert_eq!(view.settings.login_line, "");
    // Opening Settings again asks anew, and that read is applied.
    let (_, effects) = step(&mut shell, Event::SettingsOpened);
    assert!(effects.contains(&Effect::ReadLoginItem), "{effects:?}");
    shell.dispatch(Event::LoginItemRead {
        shown: Some(LoginItemShown {
            on: true,
            legacy: true,
        }),
        runner_missing: false,
    });
    // The cache refresh from opening Settings can render before the login read.
    let view = shell.wait_for(|view| view.settings.login_sensitive);
    assert!(view.settings.login_on);
    assert_eq!(view.settings.login_line, STARTUP_LEGACY);
}

#[test]
fn every_choice_renders_a_full_snapshot() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    let (view, _) = step(&mut shell, Event::ReadingChosen(Language::Ca));
    assert_eq!(view.settings.reading, Language::Ca);
    let (view, _) = step(&mut shell, Event::MyLanguageChosen(Language::Es));
    assert_eq!(view.settings.my_language, Language::Es);
    let (view, _) = step(&mut shell, Event::EnglishChosen(English::Gb));
    assert_eq!(view.settings.english, English::Gb);
    let (view, _) = step(&mut shell, Event::AtOnceChosen(false));
    assert!(!view.settings.at_once);
}

#[test]
fn a_read_that_timed_out_or_panicked_gives_its_default_and_finishes_the_wait() {
    let temp = TempDir::new("timed-out-read");
    let config = temp.config();
    fs::create_dir_all(&config).unwrap();
    // Files that hold other values, so a default can only come from the
    // limit: the gate is never opened, and every read must time out.
    fs::write(config.join("language"), "es\n").unwrap();
    fs::write(config.join("my-language"), "en\n").unwrap();
    fs::write(config.join("english"), "gb\n").unwrap();
    fs::write(config.join("translate-sentence"), "off\n").unwrap();
    let gate = Arc::new(crate::settings::ReadGate::default());
    let mut shell = FakeShell::with_unread_settings(
        temp.dirs(),
        Silent,
        false,
        KeySource::new(Err(KeyError::Missing)),
        crate::settings::Limits {
            language_file: Duration::from_millis(20),
            ..crate::settings::Limits::default()
        },
        crate::capture::Capture::default(),
        crate::selection::Selection::default(),
        false,
        Some(gate.clone()),
    );
    let view = until_read(&mut shell);
    assert_eq!(view.settings.reading, Language::En);
    assert_eq!(view.settings.my_language, Language::Ru);
    assert_eq!(view.settings.english, English::Us);
    assert!(view.settings.at_once);
}

#[test]
fn a_newer_key_save_cancels_the_pending_one_and_its_late_answer_is_ignored() {
    let temp = TempDir::new("key-superseded");
    let mut shell = shell(temp.dirs());
    ready(&mut shell);
    let (_, effects) = step(
        &mut shell,
        Event::SaveKey(Ok(Secret::new("synthetic-first-key").unwrap())),
    );
    let (first, _) = key_effect(&effects);
    let (_, effects) = step(
        &mut shell,
        Event::SaveKey(Ok(Secret::new("synthetic-second-key").unwrap())),
    );
    let [
        Effect::Cancel(cancelled),
        Effect::StoreKey { id: second, .. },
    ] = effects.as_slice()
    else {
        panic!("cancel then store: {effects:?}");
    };
    assert_eq!(*cancelled, first);
    // The first save's answer belongs to a retired id.
    shell.dispatch(Event::KeyStored {
        id: first,
        result: Err(Ended::Shell(KeyStoreFailure::Failed(
            "disk full".to_owned(),
        ))),
    });
    assert_eq!(marker(&mut shell).settings.key_line, "Saving the key…");
    let (view, _) = step(
        &mut shell,
        Event::KeyStored {
            id: *second,
            result: Ok(()),
        },
    );
    assert_eq!(view.settings.key_line, "");
}

#[test]
fn the_switch_state_stays_confirmed_while_a_change_runs() {
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    let (view, effects) = step(&mut shell, Event::LoginItemToggled(true));
    // The knob follows the user; the confirmed state does not move until the
    // portal answers, as 0.5.0's stopped `state-set` kept it.
    assert!(view.settings.login_on);
    assert!(!view.settings.login_confirmed);
    let [Effect::SetLoginItem { id, .. }] = effects.as_slice() else {
        panic!("login effect: {effects:?}");
    };
    let (view, _) = step(
        &mut shell,
        Event::LoginItemSet {
            id: *id,
            result: Ok(StartupEnd::Done(true)),
            runner_missing: false,
        },
    );
    assert!(view.settings.login_on && view.settings.login_confirmed);
}

#[test]
fn a_start_read_asks_the_shown_word_again_only_where_0_5_0_did() {
    let data = super::TestData::with_banco();
    let (mut shell, _gate) = gated_shell(Dirs {
        data: Some(data.0.clone()),
        ..Dirs::default()
    });
    step(&mut shell, Event::ReadingChosen(Language::Es));
    shell.edit("banco");
    // Wait for the lookup's answer, not just a snapshot without a lookup:
    // the edit's first snapshot may come before the lookup starts.
    loop {
        let (view, _) = shell.next();
        if view.answered == Some(Language::Es) && !view.looking_up {
            break;
        }
    }
    // Each of these changes a value but not what the shown word is asked in.
    // The read's own snapshot: earlier renders may still be queued.
    let mut read_one = |read: SettingsRead, done: &dyn Fn(&ViewState) -> bool| {
        let (mut view, mut effects) = step_read(&mut shell, read);
        while !done(&view) {
            (view, effects) = shell.next();
        }
        (view, effects)
    };
    let (view, effects) = read_one(SettingsRead::AtOnce(false), &|view| !view.settings.at_once);
    assert!(!view.looking_up, "an at-once read re-asked");
    assert!(effects.is_empty(), "{effects:?}");
    let (view, effects) = read_one(SettingsRead::English(English::Gb), &|view| {
        view.settings.english == English::Gb
    });
    assert!(
        !view.looking_up,
        "a spelling read re-asked while Russian is mine"
    );
    assert!(effects.is_empty(), "{effects:?}");
    // My language asks it again, as `set_my_language` did.
    let (view, _) = read_one(SettingsRead::My(Language::Ca), &|view| {
        view.settings.my_language == Language::Ca
    });
    assert!(view.looking_up, "my language re-asks the word");
}

/// Delivers one start read to the controller as its runner would.
fn step_read(shell: &mut FakeShell, read: SettingsRead) -> (ViewState, Vec<Effect>) {
    shell
        .controller
        .dispatch
        .0
        .send(Message::SettingsRead(read))
        .unwrap();
    shell.next()
}

/// 0.5.0's `startup_outcome`, now the controller's login ends: a timeout or
/// a job that did not run leaves the switch at the confirmed state, from
/// either state; a cancelled job says nothing; only `Done` moves it.
#[test]
fn only_a_finished_confirmation_moves_the_switch() {
    // Moves the switch to `on` and answers with `result`.
    let toggle = |shell: &mut FakeShell,
                  on: bool,
                  result: Result<StartupEnd, Ended<StartupFailure>>|
     -> (ViewState, RequestId) {
        let (_, effects) = step(shell, Event::LoginItemToggled(on));
        let [Effect::SetLoginItem { id, .. }] = effects.as_slice() else {
            panic!("login effect: {effects:?}");
        };
        let id = *id;
        let (view, _) = step(
            shell,
            Event::LoginItemSet {
                id,
                result,
                runner_missing: false,
            },
        );
        (view, id)
    };
    for timed_out in [true, false] {
        for last in [false, true] {
            let mut shell = shell(Dirs::default());
            ready(&mut shell);
            if last {
                toggle(&mut shell, true, Ok(StartupEnd::Done(true)));
            }
            let ended = if timed_out {
                Err(Ended::DeadlinePassed)
            } else {
                Err(Ended::Unfinished)
            };
            let (view, _) = toggle(&mut shell, !last, ended);
            assert_eq!(view.settings.login_on, last);
            assert_eq!(view.settings.login_confirmed, last);
            assert!(!view.settings.login_line.is_empty());
        }
    }
    let mut shell = shell(Dirs::default());
    ready(&mut shell);
    let (view, _) = toggle(&mut shell, true, Err(Ended::DeadlinePassed));
    let line = view.settings.login_line;
    assert!(!line.contains("not changed"), "{line}");
    assert!(line.contains("did not answer in time"), "{line}");
    assert!(line.contains("may still take effect"), "{line}");
    assert!(line.contains("off and on"), "{line}");
    // A cancelled job's answer belongs to an id the controller retired: it
    // shows nothing.
    let (_, effects) = step(&mut shell, Event::LoginItemToggled(true));
    let [Effect::SetLoginItem { id, .. }] = effects.as_slice() else {
        panic!("login effect: {effects:?}");
    };
    let current = *id;
    let before = marker(&mut shell).settings;
    shell.dispatch(Event::LoginItemSet {
        id: RequestId(current.0 - 1),
        result: Err(Ended::Unfinished),
        runner_missing: false,
    });
    assert_eq!(marker(&mut shell).settings, before);
    let (view, _) = step(
        &mut shell,
        Event::LoginItemSet {
            id: current,
            result: Ok(StartupEnd::Done(true)),
            runner_missing: false,
        },
    );
    assert!(view.settings.login_on);
    assert_eq!(view.settings.login_line, "");
}
