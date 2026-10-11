//! What the window says about its state: the source label and the status,
//! and below them the binding row when the shortcut is not bound.
//!
//! The texts live here and nowhere else; `addict-core` has none. [`texts`] and
//! [`binding_texts`] are pure, so they are tested without a display; [`View`]
//! puts their results into its widgets.

use addict_core::input::{Source, WindowState};
use gtk::prelude::*;

/// The two texts of one window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Texts {
    /// Where the input comes from.
    pub source: &'static str,
    /// What the window is doing with it.
    pub status: &'static str,
}

/// The texts of `state`. No wildcard arm: a new state or source does not
/// compile until it is given its texts.
pub fn texts(state: WindowState) -> Texts {
    match state {
        // Nothing is being read, so the window waits for the user.
        WindowState::Empty => Texts {
            source: source_label(Source::Manual),
            status: "waiting for input",
        },
        WindowState::Waiting { source } => Texts {
            source: source_label(source),
            status: "reading the source…",
        },
        WindowState::Dictionary { source } => Texts {
            source: source_label(source),
            status: "word without context",
        },
        WindowState::Contextual { source } => Texts {
            source: source_label(source),
            status: "word in its sentence",
        },
    }
}

fn source_label(source: Source) -> &'static str {
    match source {
        Source::Manual => "manual input",
        Source::AtSpi => "accessibility (AT-SPI)",
        Source::Ocr => "screen capture (OCR)",
    }
}

/// What is known about the global shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// An attempt to bind is running; its end is not known yet.
    Attempting,
    /// The shortcut is bound.
    Bound,
    /// The last attempt ended without a bound shortcut. `detail` is the
    /// worker's one-line message, shown as given.
    NotBound { detail: String },
}

/// The binding row of one binding state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingTexts<'a> {
    /// The window's own line. It never names a cause.
    pub text: &'static str,
    /// The worker's message, shown below the line as given.
    pub detail: Option<&'a str>,
    /// Whether Retry can be pressed.
    pub retry: bool,
}

/// The row of `binding`, or `None` when there is nothing to say. No wildcard
/// arm: a new state does not compile until it is given its row.
pub fn binding_texts(binding: &Binding) -> Option<BindingTexts<'_>> {
    match binding {
        Binding::Bound => None,
        Binding::Attempting => Some(BindingTexts {
            text: "Binding the shortcut…",
            detail: None,
            retry: false,
        }),
        Binding::NotBound { detail } => Some(BindingTexts {
            text: "The shortcut is not bound.",
            detail: Some(detail.as_str()).filter(|detail| !detail.trim().is_empty()),
            retry: true,
        }),
    }
}

const RETRY_LABEL: &str = "Retry";

/// The source label and the status, one above the other, then the binding
/// row: its line, the detail and Retry.
pub struct View {
    root: gtk::Box,
    source: gtk::Label,
    status: gtk::Label,
    binding: gtk::Box,
    binding_text: gtk::Label,
    binding_detail: gtk::Label,
    retry: gtk::Button,
}

impl View {
    /// Builds the labels with the texts of `state` and the row of `binding`.
    pub fn new(state: WindowState, binding: &Binding) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        let source = gtk::Label::new(None);
        let status = gtk::Label::new(None);
        root.append(&source);
        root.append(&status);

        let binding_text = gtk::Label::new(None);
        let binding_detail = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .build();
        let retry = gtk::Button::builder()
            .label(RETRY_LABEL)
            .halign(gtk::Align::Center)
            .build();
        let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
        row.append(&binding_text);
        row.append(&binding_detail);
        row.append(&retry);
        root.append(&row);

        let view = Self {
            root,
            source,
            status,
            binding: row,
            binding_text,
            binding_detail,
            retry,
        };
        view.render(state);
        view.render_binding(binding);
        view
    }

    /// The box holding the labels and the binding row; more content goes
    /// below them.
    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    /// Shows the texts of `state`.
    pub fn render(&self, state: WindowState) {
        let texts = texts(state);
        self.source.set_text(texts.source);
        self.status.set_text(texts.status);
    }

    /// Shows the row of `binding`, or hides it when there is none.
    pub fn render_binding(&self, binding: &Binding) {
        let Some(texts) = binding_texts(binding) else {
            self.binding.set_visible(false);
            return;
        };
        self.binding_text.set_text(texts.text);
        self.binding_detail
            .set_text(texts.detail.unwrap_or_default());
        self.binding_detail.set_visible(texts.detail.is_some());
        self.retry.set_sensitive(texts.retry);
        self.binding.set_visible(true);
        if texts.retry {
            // Otherwise the selectable detail, the first focusable widget,
            // takes the focus and selects its text.
            self.retry.grab_focus();
        }
    }

    /// Runs `retry` each time Retry is pressed.
    pub fn connect_retry(&self, retry: impl Fn() + 'static) {
        self.retry.connect_clicked(move |_| retry());
    }
}

#[cfg(test)]
mod tests {
    use addict_core::input::{Source, WindowState};

    use super::{Binding, binding_texts, texts};

    fn not_bound(detail: &str) -> Binding {
        Binding::NotBound {
            detail: detail.to_owned(),
        }
    }

    /// One state of each kind. No wildcard arm: a new state does not compile
    /// here until it is listed.
    fn every_binding() -> Vec<Binding> {
        let all = vec![
            Binding::Attempting,
            Binding::Bound,
            not_bound("binding the shortcut: some portal text"),
        ];
        for binding in &all {
            match binding {
                Binding::Attempting | Binding::Bound | Binding::NotBound { .. } => {}
            }
        }
        all
    }

    #[test]
    fn only_a_bound_shortcut_has_no_row() {
        for binding in every_binding() {
            assert_eq!(
                binding_texts(&binding).is_none(),
                binding == Binding::Bound,
                "{binding:?}"
            );
        }
    }

    #[test]
    fn a_shown_row_is_never_empty() {
        for binding in every_binding() {
            if let Some(row) = binding_texts(&binding) {
                assert!(!row.text.trim().is_empty(), "{binding:?}");
                assert!(
                    row.detail.is_none_or(|detail| !detail.trim().is_empty()),
                    "{binding:?}"
                );
            }
        }
        // A blank message leaves the row with its own line only.
        let blank = not_bound(" ");
        let row = binding_texts(&blank).expect("row");
        assert!(!row.text.trim().is_empty());
        assert_eq!(row.detail, None);
    }

    #[test]
    fn the_not_bound_line_names_no_cause() {
        // Whatever the worker said, the window's own line stays the same.
        let lines: Vec<_> = [
            "binding the shortcut: cancelled",
            "binding the shortcut: no result within 60s",
            "session bus: I/O error",
        ]
        .into_iter()
        .map(|detail| binding_texts(&not_bound(detail)).expect("row").text)
        .collect();
        assert!(lines.iter().all(|line| *line == lines[0]));
        let lowered = lines[0].to_lowercase();
        for cause in [
            "cancel",
            "dismiss",
            "denied",
            "refus",
            "reject",
            "declin",
            "clos",
            "time",
            "limit",
            "portal",
            "error",
            "fail",
            "unavailable",
            "because",
            "you ",
        ] {
            assert!(!lowered.contains(cause), "{cause:?} in {:?}", lines[0]);
        }
        assert!(lowered.contains("not bound"));
    }

    #[test]
    fn the_detail_is_the_worker_message_as_given() {
        let message = "binding the shortcut: Portal request failed: Other";
        let binding = not_bound(message);
        let row = binding_texts(&binding).expect("row");
        assert_eq!(row.detail, Some(message));
    }

    #[test]
    fn an_attempt_and_a_failure_differ() {
        let attempt = binding_texts(&Binding::Attempting).expect("row");
        let binding = not_bound("x: y");
        let failure = binding_texts(&binding).expect("row");
        assert_ne!(attempt.text, failure.text);
        assert_eq!(attempt.detail, None);
        // Retry cannot start a second attempt while one runs.
        assert!(!attempt.retry);
        assert!(failure.retry);
    }

    /// The states of one kind over `sources`. No wildcard arm: a new state
    /// does not compile here until it is listed.
    fn same_kind(state: WindowState, sources: &[Source]) -> Vec<WindowState> {
        let with = |build: fn(Source) -> WindowState| sources.iter().copied().map(build).collect();
        match state {
            WindowState::Empty => vec![WindowState::Empty],
            WindowState::Waiting { .. } => with(|source| WindowState::Waiting { source }),
            WindowState::Dictionary { .. } => with(|source| WindowState::Dictionary { source }),
            WindowState::Contextual { .. } => with(|source| WindowState::Contextual { source }),
        }
    }

    /// One state of each kind; `same_kind` spreads them over the sources.
    const KINDS: [WindowState; 4] = [
        WindowState::Empty,
        WindowState::Waiting {
            source: Source::Manual,
        },
        WindowState::Dictionary {
            source: Source::Manual,
        },
        WindowState::Contextual {
            source: Source::Manual,
        },
    ];

    fn every_state() -> Vec<WindowState> {
        KINDS
            .into_iter()
            .flat_map(|kind| same_kind(kind, &Source::ALL))
            .collect()
    }

    #[test]
    fn every_state_is_covered() {
        assert_eq!(every_state().len(), 1 + 3 * Source::ALL.len());
    }

    #[test]
    fn both_texts_are_never_empty() {
        for state in every_state() {
            let texts = texts(state);
            assert!(!texts.source.trim().is_empty(), "{state:?}");
            assert!(!texts.status.trim().is_empty(), "{state:?}");
        }
    }

    #[test]
    fn each_source_has_its_own_label() {
        for kind in &KINDS[1..] {
            let labels: Vec<_> = same_kind(*kind, &Source::ALL)
                .into_iter()
                .map(|state| texts(state).source)
                .collect();
            for (index, label) in labels.iter().enumerate() {
                assert!(!labels[..index].contains(label), "{kind:?}: {label}");
            }
        }
    }

    #[test]
    fn the_source_label_does_not_depend_on_the_state() {
        for source in Source::ALL {
            let labels: Vec<_> = KINDS[1..]
                .iter()
                .flat_map(|kind| same_kind(*kind, &[source]))
                .map(|state| texts(state).source)
                .collect();
            assert!(labels.iter().all(|label| *label == labels[0]), "{source:?}");
        }
    }

    #[test]
    fn waiting_status_differs_from_the_other_statuses() {
        for source in Source::ALL {
            let waiting = texts(WindowState::Waiting { source }).status;
            for state in every_state() {
                if !matches!(state, WindowState::Waiting { .. }) {
                    assert_ne!(texts(state).status, waiting, "{state:?}");
                }
            }
        }
    }

    #[test]
    fn empty_asks_for_manual_input() {
        let empty = texts(WindowState::Empty);
        assert_eq!(empty.source, "manual input");
        assert_eq!(empty.status, "waiting for input");
        assert_eq!(
            empty.source,
            texts(WindowState::Waiting {
                source: Source::Manual
            })
            .source
        );
    }
}
