use std::{fs, sync::mpsc::RecvTimeoutError, time::Duration};

use addict_core::{
    failure::login_item::StartupEnd,
    translation::{English, Source as Language},
};
use addict_deepl::key::Secret;
use addict_dictionary::release::Published;
use addict_engine::{at_once, english, language, my_language, view::fatal};

use super::downloads::{TestDir, at_release, listing, startup, stub};
use super::{FakeShell, marker};
use crate::{Effect, Event, FatalView, Message, ViewState};

#[test]
fn reopening_presents_the_current_panel_without_reading() {
    let mut shell = FakeShell::start(None);
    let before = marker(&mut shell);
    shell.dispatch(Event::Reopened);
    let (opened, effects) = shell.next();
    assert_eq!(
        opened,
        ViewState {
            revision: opened.revision,
            ..before
        }
    );
    assert_eq!(
        effects,
        [Effect::ShowPanel {
            read_sequence: None
        }]
    );
}

#[test]
fn closing_the_panel_keeps_its_contents_and_sends_no_effect() {
    let mut shell = FakeShell::start(None);
    shell.edit("one two");
    shell.settled();
    shell.dispatch(Event::Reopened);
    let (before, _) = shell.next();
    shell.quiet(Event::PanelClosed);
    shell.dispatch(Event::Reopened);
    let (after, effects) = shell.next();
    assert_eq!(after.field, before.field);
    assert_eq!(after.shown, before.shown);
    assert_eq!(
        effects,
        [Effect::ShowPanel {
            read_sequence: None
        }]
    );
}

#[test]
fn quit_without_writes_is_immediate_and_emitted_once() {
    let mut shell = FakeShell::start(None);
    shell.dispatch(Event::QuitRequested);
    assert_eq!(shell.next().1, [Effect::Quit]);
    shell.dispatch(Event::QuitRequested);
    assert_eq!(
        shell.renders.recv_timeout(Duration::from_millis(40)),
        Err(RecvTimeoutError::Timeout),
    );
}

#[test]
fn quit_waits_for_all_setting_files_to_be_written() {
    let dir = TestDir::new();
    let dirs = dir.dirs();
    let mut shell = startup(dirs.clone());
    let (release, _gate) = shell.hold_worker();
    for event in [
        Event::ReadingChosen(Language::Ca),
        Event::MyLanguageChosen(Language::En),
        Event::EnglishChosen(English::Gb),
        Event::AtOnceChosen(false),
    ] {
        shell.dispatch(event);
        shell.next();
    }
    shell.quiet(Event::QuitRequested);
    shell.quiet(Event::QuitRequested);
    release.send(()).unwrap();
    loop {
        let (_, effects) = shell.next();
        // Changing my language can also render the download status.
        if !effects.is_empty() {
            assert_eq!(effects, [Effect::Quit]);
            break;
        }
    }
    let config = dirs.config.as_deref();
    assert_eq!(
        language::read(language::path(config).as_deref()),
        Language::Ca
    );
    assert_eq!(
        my_language::read(my_language::path(config).as_deref()),
        Language::En
    );
    assert_eq!(english::read(english::path(config).as_deref()), English::Gb);
    assert!(!at_once::read(at_once::path(config).as_deref()));
}

#[test]
fn quit_waits_for_the_first_run_answer_to_be_written() {
    let dir = TestDir::new();
    let dirs = dir.dirs();
    let server = stub(vec![Published::of("oewn", b"index")], []);
    let mut shell = at_release(dirs.clone(), &server);
    let id = listing(&mut shell).downloads.offer.unwrap().id;
    let (release, _gate) = shell.hold_worker();
    shell.dispatch(Event::OfferNotNowPressed(id));
    assert!(shell.next().0.downloads.offer.is_none());
    shell.quiet(Event::QuitRequested);
    release.send(()).unwrap();
    assert_eq!(shell.next().1, [Effect::Quit]);
    let path = crate::first_run_choice_path(dirs.config.as_deref()).unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "declined\n");
}

#[test]
fn quit_waits_for_both_shell_write_results() {
    let dir = TestDir::new();
    let mut shell = startup(dir.dirs());
    shell.dispatch(Event::SaveKey(Ok(Secret::new("synthetic-key").unwrap())));
    let (_, effects) = shell.next();
    let [Effect::StoreKey { id: key, .. }] = effects.as_slice() else {
        panic!("store key");
    };
    let key = *key;
    shell.dispatch(Event::LoginItemToggled(true));
    let (_, effects) = shell.next();
    let [
        Effect::SetLoginItem {
            id: login,
            on: true,
        },
    ] = effects.as_slice()
    else {
        panic!("set login item");
    };
    let login = *login;
    shell.quiet(Event::QuitRequested);
    shell.dispatch(Event::KeyStored {
        id: key,
        result: Ok(()),
    });
    assert!(shell.next().1.is_empty());
    shell.quiet(Event::QuitRequested);
    shell.dispatch(Event::LoginItemSet {
        id: login,
        result: Ok(StartupEnd::Done(true)),
        runner_missing: false,
    });
    assert_eq!(shell.next().1, [Effect::Quit]);
}

#[test]
fn a_panic_renders_one_fatal_snapshot_and_disconnects_dispatch() {
    let mut shell = FakeShell::start(None);
    let previous = marker(&mut shell);
    shell.controller.dispatch.0.send(Message::Panic).unwrap();
    let (after, effects) = shell.next();
    assert!(effects.is_empty());
    assert_eq!(
        after,
        ViewState {
            revision: previous.revision + 1,
            fatal: Some(FatalView {
                heading: fatal::HEADING.to_owned(),
                text: fatal::TEXT.to_owned(),
                restart: fatal::RESTART.to_owned()
            }),
            ..ViewState::default()
        },
    );
    assert!(!shell.controller.dispatch(Event::Reopened));
    assert_eq!(
        shell.renders.recv_timeout(Duration::from_secs(2)),
        Err(RecvTimeoutError::Disconnected)
    );
}
