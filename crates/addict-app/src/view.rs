//! What the window says about its state: the source label and the status.
//!
//! The texts live here and nowhere else; `addict-core` has none. [`texts`] is
//! pure, so it is tested without a display; [`View`] puts its result into the
//! two labels.

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

/// The source label and the status, one above the other.
pub struct View {
    root: gtk::Box,
    source: gtk::Label,
    status: gtk::Label,
}

impl View {
    /// Builds the labels with the texts of `state`.
    pub fn new(state: WindowState) -> Self {
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
        let view = Self {
            root,
            source,
            status,
        };
        view.render(state);
        view
    }

    /// The box holding the labels; more content goes below them.
    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    /// Shows the texts of `state`.
    pub fn render(&self, state: WindowState) {
        let texts = texts(state);
        self.source.set_text(texts.source);
        self.status.set_text(texts.status);
    }
}

#[cfg(test)]
mod tests {
    use addict_core::input::{Source, WindowState};

    use super::texts;

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
