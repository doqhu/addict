//! The controller contract shared by the Linux and macOS shells.

use addict_core::failure::login_item::LoginItemShown;

use super::*;

pub(super) fn linux_platform() -> Platform {
    let install_hint = |code| InstallHint {
        engine: Some(format!(
            "sudo dnf install tesseract tesseract-langpack-{code}"
        )),
        data: Some(format!("sudo dnf install tesseract-langpack-{code}")),
    };
    Platform {
        name: "Linux".into(),
        read_before_show: false,
        has_capture: true,
        ocr_languages: Language::ALL.to_vec(),
        install_hints: InstallHints {
            en: install_hint("eng"),
            es: install_hint("spa"),
            ca: install_hint("cat"),
            ru: install_hint("rus"),
        },
        shortcut_names: ShortcutNames {
            back: "Alt+Left".into(),
            forward: "Alt+Right".into(),
        },
        key_location_hint: "Set an absolute XDG_CONFIG_HOME or HOME to locate addict/deepl.key."
            .into(),
    }
}

fn mac_platform() -> Platform {
    Platform {
        name: "macOS".into(),
        read_before_show: true,
        has_capture: true,
        ocr_languages: Language::ALL.to_vec(),
        install_hints: InstallHints {
            ca: InstallHint {
                engine: None,
                data: Some("Install Catalan data in the app.".into()),
            },
            ..InstallHints::default()
        },
        shortcut_names: ShortcutNames {
            back: "⌥←".into(),
            forward: "⌥→".into(),
        },
        key_location_hint: "Find the DeepL key in Keychain.".into(),
    }
}

pub(super) fn linux_facts() -> StartFacts {
    StartFacts {
        key: None,
        recording: false,
        release: String::new(),
        permissions: Permissions {
            accessibility: Permission::Granted,
            screen_recording: Permission::Granted,
        },
        login_item: None,
        bindings: None,
    }
}

impl FakeShell {
    fn with_contract(dirs: Dirs, platform: Platform, facts: StartFacts) -> Self {
        let (send, renders) = mpsc::channel();
        let controller = start(dirs, platform, facts, move |view, effects| {
            send.send((view, effects)).unwrap();
        })
        .unwrap();
        Self {
            controller,
            renders,
            revision: 0,
        }
    }

    fn contract_ready(platform: Platform) -> (Self, ViewState) {
        let mut shell = Self::with_contract(Dirs::default(), platform, linux_facts());
        let view =
            shell.wait_for(|view| view.downloads.indexes.is_some() && view.settings.languages_read);
        (shell, view)
    }
}

#[test]
fn linux_start_keeps_the_initial_snapshot_and_reports_granted_permissions() {
    let mut shell = FakeShell::with_contract(Dirs::default(), linux_platform(), linux_facts());
    let (view, effects) = shell.next();
    assert!(effects.is_empty());
    assert_eq!(
        view,
        ViewState {
            revision: 1,
            permissions: linux_facts().permissions,
            back_tooltip: "Back (Alt+Left)".into(),
            forward_tooltip: "Forward (Alt+Right)".into(),
            key_location: KeyLocation::Hint(linux_platform().key_location_hint),
            ..ViewState::default()
        }
    );

    let temp = super::downloads::TestDir::new();
    let config = temp.dirs().config;
    let key_file = config.as_ref().unwrap().join("deepl.key");
    let mut shell = FakeShell::with_contract(
        Dirs {
            config,
            ..Dirs::default()
        },
        linux_platform(),
        linux_facts(),
    );
    assert_eq!(shell.next().0.key_location, KeyLocation::File(key_file));
}

#[test]
fn known_os_state_is_in_the_first_settings_snapshot() {
    let bindings = Bindings {
        clipboard: Some("⌘⇧R".into()),
        screenshot: Some("⌘⇧S".into()),
    };
    let shown = LoginItemShown {
        on: true,
        legacy: false,
    };
    let mut shell = FakeShell::with_contract(
        Dirs::default(),
        mac_platform(),
        StartFacts {
            login_item: Some(shown),
            bindings: Some(bindings.clone()),
            ..linux_facts()
        },
    );
    let (first, effects) = shell.next();
    assert_eq!(first.revision, 1);
    assert!(effects.is_empty());
    assert!(first.settings.login_on);
    assert!(first.settings.login_confirmed);
    assert!(first.settings.login_sensitive);
    assert_eq!(first.settings.binding, addict_engine::view::Binding::Bound);
    assert_eq!(first.settings.bindings, bindings);

    let (mut reported, _) = FakeShell::contract_ready(mac_platform());
    reported.dispatch(Event::LoginItemRead {
        shown: Some(shown),
        runner_missing: false,
    });
    reported.next();
    reported.dispatch(Event::ShortcutChanged(ShortcutState::Bound(bindings)));
    let (after, _) = reported.next();
    assert_eq!(first.settings.login_on, after.settings.login_on);
    assert_eq!(
        first.settings.login_confirmed,
        after.settings.login_confirmed
    );
    assert_eq!(
        first.settings.login_sensitive,
        after.settings.login_sensitive
    );
    assert_eq!(first.settings.login_line, after.settings.login_line);
    assert_eq!(first.settings.binding, after.settings.binding);
    assert_eq!(first.settings.bindings, after.settings.bindings);
}

#[test]
fn permissions_change_only_the_snapshot() {
    let (mut shell, before) = FakeShell::contract_ready(mac_platform());
    let permissions = Permissions {
        accessibility: Permission::Denied,
        screen_recording: Permission::NotDetermined,
    };
    shell.dispatch(Event::PermissionChanged(permissions));
    let (after, effects) = shell.next();
    assert!(effects.is_empty());
    assert_eq!(
        after,
        ViewState {
            revision: before.revision + 1,
            permissions,
            ..before
        }
    );
}

#[test]
fn wake_only_advances_the_revision() {
    let (mut shell, before) = FakeShell::contract_ready(mac_platform());
    shell.dispatch(Event::Woke);
    let (after, effects) = shell.next();
    assert!(effects.is_empty());
    assert_eq!(
        after,
        ViewState {
            revision: before.revision + 1,
            ..before
        }
    );
}

fn press_capture(shell: &FakeShell) {
    shell.dispatch(Event::CapturePressed {
        activation_token: None,
    });
}

fn fail_ocr(shell: &mut FakeShell, failure: OcrFailure) -> ViewState {
    press_capture(shell);
    let (_, effects) = shell.next();
    let [Effect::CaptureArea { id, .. }] = effects.as_slice() else {
        panic!("expected capture, got {effects:?}");
    };
    shell.dispatch(Event::AreaCaptured {
        id: *id,
        result: Ok(ImageRef::new(())),
        capture_ms: None,
    });
    let (_, effects) = shell.next();
    let [Effect::PresentCapture { .. }, Effect::Recognize { id, .. }] = effects.as_slice() else {
        panic!("expected recognition, got {effects:?}");
    };
    shell.dispatch(Event::TextRecognized {
        id: *id,
        result: Err(Ended::Shell(failure)),
        timing: Timing::default(),
    });
    shell.next().0
}

#[test]
fn copy_writes_the_displayed_install_hint_only_while_it_is_shown() {
    let (mut shell, _) = FakeShell::contract_ready(linux_platform());
    shell.quiet(Event::CopyPressed);
    let view = fail_ocr(&mut shell, OcrFailure::EngineMissing);
    let text = "sudo dnf install tesseract tesseract-langpack-eng";
    assert_eq!(view.source_copy.as_deref(), Some(text));
    assert_eq!(
        view.source_failure.as_deref(),
        Some(
            "Tesseract is not installed. Install it with: sudo dnf install tesseract tesseract-langpack-eng"
        )
    );
    shell.dispatch(Event::CopyPressed);
    assert_eq!(
        shell.next().1,
        [Effect::WriteClipboard { text: text.into() }]
    );
    shell.dispatch(Event::FieldCleared);
    assert!(shell.next().0.source_copy.is_none());
    shell.quiet(Event::CopyPressed);
}

#[test]
fn only_http_and_https_links_open() {
    let (mut shell, _) = FakeShell::contract_ready(linux_platform());
    for url in [
        "http://example.com/",
        "https://example.com/",
        "HTTPS://example.com/",
    ] {
        shell.dispatch(Event::OpenPressed { url: url.into() });
        assert_eq!(shell.next().1, [Effect::OpenUrl { url: url.into() }]);
    }
    for url in [
        "file:///tmp/a",
        "mailto:user@example.com",
        "https.example.com",
        "https+custom://host/",
    ] {
        shell.quiet(Event::OpenPressed { url: url.into() });
    }
}

#[test]
fn dismissal_hides_the_panel_and_close_needs_no_effect() {
    let (mut shell, _) = FakeShell::contract_ready(linux_platform());
    shell.dispatch(Event::DismissPressed);
    assert_eq!(shell.next().1, [Effect::HidePanel]);
    shell.quiet(Event::PanelClosed);
}

#[test]
fn platforms_without_capture_disable_the_control_and_ignore_the_press() {
    let (mut shell, view) = FakeShell::contract_ready(Platform {
        has_capture: false,
        ..linux_platform()
    });
    assert!(!view.capture_sensitive);
    shell.quiet(Event::CapturePressed {
        activation_token: None,
    });
}

#[test]
fn unoffered_language_says_which_data_is_missing_without_capturing() {
    let (mut shell, _) = FakeShell::contract_ready(Platform {
        ocr_languages: vec![Language::En],
        ..mac_platform()
    });
    shell.dispatch(Event::ReadingChosen(Language::Ca));
    shell.wait_for(|view| view.settings.reading == Language::Ca);
    press_capture(&shell);
    let (view, effects) = shell.next();
    assert!(matches!(
        effects.as_slice(),
        [Effect::PresentCapture { retry: true, .. }]
    ));
    assert!(view.capture_sensitive);
    assert_eq!(
        view.source_failure.as_deref(),
        Some(
            "Catalan OCR data is not installed. Install it with: Install Catalan data in the app."
        )
    );
    assert_eq!(
        view.source_copy.as_deref(),
        Some("Install Catalan data in the app.")
    );
}

#[test]
fn identical_ocr_events_only_vary_the_platform_wording() {
    let (mut linux, _) = FakeShell::contract_ready(linux_platform());
    let (mut mac, _) = FakeShell::contract_ready(mac_platform());
    for shell in [&mut linux, &mut mac] {
        shell.dispatch(Event::ReadingChosen(Language::Ca));
        shell.wait_for(|view| view.settings.reading == Language::Ca);
    }
    let linux = fail_ocr(&mut linux, OcrFailure::LanguageMissing);
    let mac = fail_ocr(&mut mac, OcrFailure::LanguageMissing);
    assert_eq!(
        linux.source_failure.as_deref(),
        Some(
            "Catalan OCR data is not installed. Install it with: sudo dnf install tesseract-langpack-cat"
        )
    );
    assert_eq!(
        linux.source_copy.as_deref(),
        Some("sudo dnf install tesseract-langpack-cat")
    );
    assert_eq!(
        mac.source_failure.as_deref(),
        Some(
            "Catalan OCR data is not installed. Install it with: Install Catalan data in the app."
        )
    );
    assert_eq!(
        mac.source_copy.as_deref(),
        Some("Install Catalan data in the app.")
    );
    assert_eq!(
        linux.key_location,
        KeyLocation::Hint(linux_platform().key_location_hint)
    );
    assert_eq!(
        mac.key_location,
        KeyLocation::Hint(mac_platform().key_location_hint)
    );
    assert_eq!(linux.back_tooltip, "Back (Alt+Left)");
    assert_eq!(linux.forward_tooltip, "Forward (Alt+Right)");
    assert_eq!(mac.back_tooltip, "Back (⌥←)");
    assert_eq!(mac.forward_tooltip, "Forward (⌥→)");
    assert_eq!(
        mac,
        ViewState {
            source_failure: mac.source_failure.clone(),
            source_copy: mac.source_copy.clone(),
            key_location: mac.key_location.clone(),
            back_tooltip: mac.back_tooltip.clone(),
            forward_tooltip: mac.forward_tooltip.clone(),
            ..linux
        }
    );
}

#[test]
fn a_bundled_engine_or_language_has_no_command_to_copy() {
    let (mut shell, _) = FakeShell::contract_ready(mac_platform());
    for (failure, message) in [
        (OcrFailure::EngineMissing, "Tesseract is not installed."),
        (
            OcrFailure::LanguageMissing,
            "English OCR data is not installed.",
        ),
    ] {
        let view = fail_ocr(&mut shell, failure);
        assert_eq!(view.source_failure.as_deref(), Some(message));
        assert!(view.source_copy.is_none());
        shell.quiet(Event::CopyPressed);
    }
}
