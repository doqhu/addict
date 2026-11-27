//! The Read shortcut's selection read through the controller (AD-6, AD-7,
//! AD-12): the effects of a press in the platform's order, the rule that
//! picks the selection's or the clipboard's line, the length limit of each,
//! and the read's supersession, cancellation and deadline.

use addict_core::{failure::selection::SelectionFailure, translation::reading_languages};
use addict_engine::{
    manual::CLIPBOARD_MAX_CHARS,
    view::{Held, NOTHING_SELECTED_TEXT, install_language, install_tesseract, too_long_text},
};

use super::*;
use crate::selection::{Selection, is_offered_command, missed_line};

/// Presses Read with `sequence`; gives the read's id and the effects of
/// the press, which end with the read and, unless the platform reads first,
/// the window shown just before it.
fn press_read(
    shell: &mut FakeShell,
    sequence: u64,
    read_before_show: bool,
) -> (RequestId, Vec<Effect>) {
    shell.dispatch(Event::ReadPressed { sequence });
    let (view, effects) = shell.next();
    assert_eq!(view.read_sequence, sequence);
    assert_eq!(view.reading_source, Some(Source::Clipboard));
    assert_eq!(view.field.text, "");
    assert_eq!(view.source_failure, None);
    let Some(Effect::ReadSelection { id, read_sequence }) = effects.last() else {
        panic!("a read: {effects:?}");
    };
    assert_eq!(*read_sequence, sequence);
    let shown = effects
        .iter()
        .filter(|effect| matches!(effect, Effect::ShowPanel { .. }))
        .count();
    if read_before_show {
        assert_eq!(shown, 0, "{effects:?}");
    } else {
        assert_eq!(
            effects[effects.len() - 2],
            Effect::ShowPanel {
                read_sequence: Some(sequence)
            }
        );
        assert_eq!(shown, 1, "{effects:?}");
    }
    (*id, effects)
}

fn read(shell: &FakeShell, id: RequestId, selection: Option<&str>, clipboard: Option<&str>) {
    shell.dispatch(Event::SelectionRead {
        id,
        result: Ok(SelectionText {
            selection: selection.map(str::to_owned),
            clipboard: clipboard.map(str::to_owned),
        }),
    });
}

/// A result with text for a read that is no longer current.
fn late(id: RequestId) -> Event {
    Event::SelectionRead {
        id,
        result: Ok(SelectionText {
            selection: Some("late".to_owned()),
            clipboard: None,
        }),
    }
}

/// Reads `selection` and `clipboard` after a press; gives the settled view.
fn read_once(selection: Option<&str>, clipboard: Option<&str>) -> ViewState {
    let mut shell = FakeShell::start(None);
    let (id, _) = press_read(&mut shell, 1, false);
    let revision = shell.revision;
    read(&shell, id, selection, clipboard);
    let (view, _) = shell.settled();
    assert!(view.revision > revision);
    assert_eq!(view.reading_source, None);
    view
}

#[test]
fn a_selected_text_fills_the_field_before_the_clipboard() {
    let view = read_once(Some("  I sat on\n the bank. "), Some("copied text"));
    assert_eq!(view.field.text, "I sat on the bank.");
    assert_eq!(view.sentence, "I sat on the bank.");
    assert_eq!(view.source, Source::Clipboard);
    assert_eq!(view.chosen, None);
    assert_eq!(view.source_failure, None);
}

#[test]
fn a_read_text_is_recorded_in_history_and_back_returns_to_it() {
    let mut shell = FakeShell::start(None);
    shell.submit("river");
    let (id, _) = press_read(&mut shell, 1, false);
    read(&shell, id, None, Some("The bank was closed."));
    let (view, _) = shell.settled();
    assert_eq!(view.field.text, "The bank was closed.");
    assert!(view.can_back);
    shell.submit("bank");
    shell.dispatch(Event::Back);
    let (view, _) = shell.next();
    assert_eq!(view.field.text, "The bank was closed.");
}

#[test]
fn a_selected_word_is_chosen_and_looked_up() {
    let view = read_once(Some("bank"), None);
    assert_eq!(view.field.text, "bank");
    assert_eq!(view.chosen, Some(0));
    assert!(view.input.is_some());
}

#[test]
fn the_clipboard_fills_the_field_when_the_selection_gives_no_line() {
    let command = install_tesseract(Language::En);
    for selection in [None, Some("   \n "), Some(command.as_str())] {
        let view = read_once(selection, Some("The bank was closed."));
        assert_eq!(view.field.text, "The bank was closed.", "{selection:?}");
        assert_eq!(view.source, Source::Clipboard);
        assert_eq!(view.source_failure, None);
    }
}

#[test]
fn neither_source_keeps_the_field_empty_and_says_nothing_is_selected() {
    let command = install_language(Language::Es);
    for (selection, clipboard) in [
        (None, None),
        (Some(" "), Some("\n")),
        (None, Some(command.as_str())),
    ] {
        let view = read_once(selection, clipboard);
        assert_eq!(view.field.text, "");
        assert_eq!(view.source_failure.as_deref(), Some(NOTHING_SELECTED_TEXT));
        assert_eq!(view.window_state, WindowState::Empty);
    }
}

#[test]
fn a_too_long_text_is_named_selection_first_and_a_short_clipboard_still_wins() {
    let long_selection = "a".repeat(CLIPBOARD_MAX_CHARS + 1);
    let long_clipboard = "b".repeat(CLIPBOARD_MAX_CHARS + 7);
    let selected = too_long_text(
        Held::Selection,
        CLIPBOARD_MAX_CHARS + 1,
        CLIPBOARD_MAX_CHARS,
    );
    let copied = too_long_text(
        Held::Clipboard,
        CLIPBOARD_MAX_CHARS + 7,
        CLIPBOARD_MAX_CHARS,
    );

    let view = read_once(Some(&long_selection), Some(&long_clipboard));
    assert_eq!(view.source_failure.as_deref(), Some(selected.as_str()));
    let view = read_once(Some(&long_selection), None);
    assert_eq!(view.source_failure.as_deref(), Some(selected.as_str()));
    let view = read_once(None, Some(&long_clipboard));
    assert_eq!(view.source_failure.as_deref(), Some(copied.as_str()));
    assert_eq!(view.field.text, "");

    // Each source has its own limit: a too long selection lets a short
    // clipboard through.
    let view = read_once(Some(&long_selection), Some("river"));
    assert_eq!(view.field.text, "river");
    assert_eq!(view.source_failure, None);
}

#[test]
fn a_cancelled_read_says_nothing_is_selected_and_a_stale_one_is_ignored() {
    let mut shell = FakeShell::start(None);
    let (id, _) = press_read(&mut shell, 1, false);
    shell.dispatch(Event::SelectionRead {
        id,
        result: Err(Ended::Shell(SelectionFailure::Cancelled)),
    });
    let (view, effects) = shell.next();
    assert_eq!(view.source_failure.as_deref(), Some(NOTHING_SELECTED_TEXT));
    assert_eq!(view.reading_source, None);
    assert!(effects.is_empty(), "{effects:?}");

    // The same id again, cancelled or with text, is no longer current.
    shell.quiet(Event::SelectionRead {
        id,
        result: Err(Ended::Shell(SelectionFailure::Cancelled)),
    });
    shell.quiet(late(id));
}

/// Moved from `addict-app`'s resident tests: a new press gives up the read
/// before, whose result is then ignored, and only the new read applies.
#[test]
fn only_the_last_clipboard_read_is_the_one_that_runs() {
    let mut shell = FakeShell::start(None);
    let (first, _) = press_read(&mut shell, 1, false);
    let (second, effects) = press_read(&mut shell, 2, false);
    assert_ne!(first, second);
    assert_eq!(effects[0], Effect::Cancel(first));

    shell.quiet(Event::SelectionRead {
        id: first,
        result: Err(Ended::Shell(SelectionFailure::Cancelled)),
    });
    shell.quiet(late(first));
    read(&shell, second, Some("new text"), None);
    let (view, _) = shell.settled();
    assert_eq!(view.field.text, "new text");
}

#[test]
fn an_edit_back_forward_or_capture_gives_the_read_up() {
    let mut shell = FakeShell::start(None);
    shell.submit("river");
    shell.submit("bank");

    let (id, _) = press_read(&mut shell, 1, false);
    shell.edit("typed");
    let (view, effects) = shell.next();
    assert!(effects.contains(&Effect::Cancel(id)), "{effects:?}");
    assert_eq!(view.reading_source, None);
    if view.looking_up {
        shell.settled();
    }
    shell.quiet(late(id));

    for event in [Event::Back, Event::Forward] {
        let (id, _) = press_read(&mut shell, 2, false);
        shell.dispatch(event);
        let (view, effects) = shell.next();
        assert!(effects.contains(&Effect::Cancel(id)), "{effects:?}");
        assert_eq!(view.reading_source, None);
        if view.looking_up {
            shell.settled();
        }
    }

    let (id, _) = press_read(&mut shell, 3, false);
    shell.dispatch(Event::CapturePressed {
        activation_token: None,
    });
    let (_, effects) = shell.next();
    assert_eq!(effects[0], Effect::Cancel(id));
    assert!(
        matches!(effects.last(), Some(Effect::CaptureArea { .. })),
        "{effects:?}"
    );
}

/// Moved from `addict-app`'s resident tests: a read the shell never
/// answers, as one that waits for a window that never becomes active, ends
/// at the deadline with the read cancelled and nothing selected.
#[test]
fn a_read_that_waits_for_the_window_is_ended_without_a_cancellable() {
    for read_before_show in [false, true] {
        let mut shell = FakeShell::with_selection(
            Selection::with_limit(Duration::from_millis(15)),
            read_before_show,
        );
        let (id, _) = press_read(&mut shell, 1, read_before_show);
        let (view, effects) = shell.next();
        assert_eq!(view.source_failure.as_deref(), Some(NOTHING_SELECTED_TEXT));
        assert_eq!(view.reading_source, None);
        let mut expected = vec![Effect::Cancel(id)];
        if read_before_show {
            expected.push(Effect::ShowPanel {
                read_sequence: Some(1),
            });
        }
        assert_eq!(effects, expected);
        // The shell's acknowledgement and a late text change nothing.
        shell.quiet(Event::SelectionRead {
            id,
            result: Err(Ended::Shell(SelectionFailure::Cancelled)),
        });
        shell.quiet(late(id));
    }
}

#[test]
fn a_platform_that_reads_first_shows_the_window_after_the_result() {
    let mut shell = FakeShell::with_selection(Selection::default(), true);
    let (id, effects) = press_read(&mut shell, 1, true);
    assert_eq!(
        effects.last(),
        Some(&Effect::ReadSelection {
            id,
            read_sequence: 1
        })
    );
    read(&shell, id, Some("The bank was closed."), None);
    let (view, effects) = shell.next();
    assert_eq!(view.field.text, "The bank was closed.");
    assert_eq!(
        effects.last(),
        Some(&Effect::ShowPanel {
            read_sequence: Some(1)
        })
    );

    // A failure shows the window too, with its line.
    let (id, _) = press_read(&mut shell, 2, true);
    shell.dispatch(Event::SelectionRead {
        id,
        result: Err(Ended::Shell(SelectionFailure::NoPermission)),
    });
    let (view, effects) = shell.next();
    assert_eq!(view.source_failure.as_deref(), Some(NOTHING_SELECTED_TEXT));
    assert_eq!(
        effects,
        [Effect::ShowPanel {
            read_sequence: Some(2)
        }]
    );
}

/// Moved from `addict-app`'s resident tests.
#[test]
fn a_too_long_selection_is_named_before_the_clipboard() {
    let selection = Some((Held::Selection, 1200));
    let clipboard = Some((Held::Clipboard, 54828));
    let selected = too_long_text(Held::Selection, 1200, CLIPBOARD_MAX_CHARS);
    let copied = too_long_text(Held::Clipboard, 54828, CLIPBOARD_MAX_CHARS);
    assert_eq!(missed_line(selection, clipboard), selected);
    assert_eq!(missed_line(selection, None), selected);
    // The selection was blank.
    assert_eq!(missed_line(None, clipboard), copied);
    // Neither was too long: nothing is selected or copied.
    assert_eq!(missed_line(None, None), NOTHING_SELECTED_TEXT);
}

/// Moved from `addict-app`'s resident tests.
#[test]
fn a_line_that_is_an_offered_command_is_not_a_text() {
    for language in reading_languages(Language::Ru) {
        assert!(is_offered_command(&install_tesseract(language)));
        assert!(is_offered_command(&install_language(language)));
    }
    assert!(!is_offered_command("bank"));
    assert!(!is_offered_command(&format!(
        "run {}",
        install_tesseract(Language::En)
    )));
    assert!(!is_offered_command(
        "Catalan OCR data is not installed. Install it with: \
         sudo dnf install tesseract-langpack-cat"
    ));
}

/// Moved from `addict-app`'s resident tests.
#[test]
fn the_clipboard_limit_is_finite_and_short() {
    assert!(!deadlines::CLIPBOARD_LIMIT.is_zero());
    assert!(deadlines::CLIPBOARD_LIMIT.as_secs() <= 1);
}
