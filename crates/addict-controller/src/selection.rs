//! Controller transitions for the read of the primary selection and the
//! clipboard.
//!
//! A Read press clears the field and issues [`Effect::ReadSelection`]; the
//! shell reads both sources raw and returns one [`SelectionText`]. This
//! module owns the 0.5.0 rule: each source gives the field its line only when
//! that line is not a command the window offered to copy, neither is blank
//! and neither is over [`CLIPBOARD_MAX_CHARS`] as one line; the selection's
//! line wins, else the clipboard's.

use std::{future, time::Duration};

use addict_core::{
    failure::{Ended, selection::SelectionFailure},
    input::Source,
};
use addict_engine::{
    manual::{CLIPBOARD_MAX_CHARS, Edit, NoLine, clipboard_line},
    view::{Held, NOTHING_SELECTED_TEXT, command_in, selection_failure_text, too_long_text},
    work::{Job, Outcome},
};

use crate::{Effect, Message, Model, RequestId, SelectionText, deadlines::CLIPBOARD_LIMIT};

/// The read at a press of Read: none runs at first.
pub(crate) struct Selection {
    limit: Duration,
    active: Option<Active>,
    /// Why the last read gave no text; the source line, cleared by the
    /// actions that clear a capture's.
    pub(crate) source_failure: Option<String>,
}

struct Active {
    id: RequestId,
    _deadline: Job,
}

impl Default for Selection {
    fn default() -> Self {
        Self::with_limit(CLIPBOARD_LIMIT)
    }
}

impl Selection {
    pub(crate) fn with_limit(limit: Duration) -> Self {
        Self {
            limit,
            active: None,
            source_failure: None,
        }
    }
}

/// One source's text as the field would take it: its line, or no line and
/// the length of a text that was too long.
enum Line {
    Text(String),
    NoLine(Option<(Held, usize)>),
}

impl Model {
    fn selection_timer(&self, id: RequestId) -> Job {
        let send = self.send.clone();
        self.work.run(
            self.selection.limit,
            future::pending::<()>(),
            move |outcome| {
                if outcome == Outcome::TimedOut {
                    let _ = send.send(Message::SelectionDeadline(id));
                }
            },
        )
    }

    /// Read was pressed: the field is cleared, a read before is given up,
    /// and the selection and the clipboard are read as the new request. The
    /// window is shown before the read or after it, per the platform.
    pub(crate) fn read_pressed(&mut self, sequence: u64, effects: &mut Vec<Effect>) -> bool {
        self.read_sequence = sequence;
        self.field_cleared(effects);
        self.manual.set_reading(Some(Source::Clipboard));
        let id = self.next_request();
        self.selection.active = Some(Active {
            id,
            _deadline: self.selection_timer(id),
        });
        let read_sequence = self.read_sequence;
        if self.read_before_show {
            effects.push(Effect::ReadSelection { id, read_sequence });
        } else {
            effects.push(Effect::ShowPanel {
                read_sequence: Some(read_sequence),
            });
            effects.push(Effect::ReadSelection { id, read_sequence });
        }
        true
    }

    /// Gives up the read in flight, if one is: its id is cancelled and the
    /// window stops waiting for it. Whether one ran.
    pub(crate) fn cancel_read(&mut self, effects: &mut Vec<Effect>) -> bool {
        let Some(active) = self.selection.active.take() else {
            return false;
        };
        effects.push(Effect::Cancel(active.id));
        if self.manual.reading() == Some(Source::Clipboard) {
            self.manual.set_reading(None);
        }
        true
    }

    /// The read in flight is given up and its source line goes, on every
    /// action that gives up a capture and its line. Whether either changed.
    pub(crate) fn clear_read_source(&mut self, effects: &mut Vec<Effect>) -> bool {
        let line = self.selection.source_failure.take().is_some();
        self.cancel_read(effects) || line
    }

    /// The raw texts came back. The current read takes the selection's line,
    /// else the clipboard's, as a new text with the source clipboard and no
    /// word chosen unless it is one word; with neither, the source line says
    /// why. A result for a read that was given up is ignored.
    pub(crate) fn selection_read(
        &mut self,
        id: RequestId,
        result: Result<SelectionText, Ended<SelectionFailure>>,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let Some(active) = self.selection.active.take() else {
            return false;
        };
        if active.id != id {
            self.selection.active = Some(active);
            return false;
        }
        // Dropping the job ends the deadline.
        self.manual.set_reading(None);
        let line = match result {
            Ok(text) => rule(&text),
            Err(failure) => {
                eprintln!("addict: no text from the selection or the clipboard: {failure:?}");
                Err(selection_failure_text(&failure))
            }
        };
        match line {
            Ok(line) => {
                let edit = self.manual.copied(&line);
                self.set_field(self.manual.sentence().to_owned());
                if edit != Edit::Same {
                    self.history.record(self.manual.sentence());
                }
                self.text_replaced(edit);
                // With Translate sentence at once on, a text of more words
                // is asked for online as a whole sentence, and nothing else.
                if self.online.at_once {
                    self.ask_whole(effects);
                }
            }
            Err(message) => self.selection.source_failure = Some(message),
        }
        if self.read_before_show {
            effects.push(Effect::ShowPanel {
                read_sequence: Some(self.read_sequence),
            });
        }
        true
    }

    /// No result came within [`CLIPBOARD_LIMIT`]: the read is cancelled and
    /// says that nothing is selected or copied. A later result is ignored.
    pub(crate) fn selection_deadline(&mut self, id: RequestId, effects: &mut Vec<Effect>) -> bool {
        let Some(active) = self.selection.active.take() else {
            return false;
        };
        if active.id != id {
            self.selection.active = Some(active);
            return false;
        }
        eprintln!(
            "addict: no text from the selection or the clipboard: no answer within {:?}",
            self.selection.limit
        );
        effects.push(Effect::Cancel(active.id));
        self.manual.set_reading(None);
        self.selection.source_failure = Some(NOTHING_SELECTED_TEXT.to_owned());
        if self.read_before_show {
            effects.push(Effect::ShowPanel {
                read_sequence: Some(self.read_sequence),
            });
        }
        true
    }
}

/// The field's text from the raw texts: the selection's line, else the
/// clipboard's, else why neither gave one.
fn rule(text: &SelectionText) -> Result<String, String> {
    let selection = line(Held::Selection, text.selection.as_deref());
    let clipboard = line(Held::Clipboard, text.clipboard.as_deref());
    match (selection, clipboard) {
        (Line::Text(line), _) => Ok(line),
        (Line::NoLine(_), Line::Text(line)) => Ok(line),
        (Line::NoLine(selection), Line::NoLine(clipboard)) => {
            Err(missed_line(selection, clipboard))
        }
    }
}

/// `raw`, the text `held`, as the field would take it: its line of at most
/// [`CLIPBOARD_MAX_CHARS`] characters, unless that is a command the window
/// offered to copy.
fn line(held: Held, raw: Option<&str>) -> Line {
    let Some(raw) = raw else {
        return Line::NoLine(None);
    };
    match clipboard_line(raw) {
        Ok(line) if !is_offered_command(&line) => Line::Text(line),
        Ok(_) | Err(NoLine::Blank) => Line::NoLine(None),
        Err(NoLine::TooLong(chars)) => Line::NoLine(Some((held, chars))),
    }
}

/// Whether `line` is exactly a command the window offers to copy: such a
/// line is the user's Copy, not a text to look up.
pub(crate) fn is_offered_command(line: &str) -> bool {
    command_in(line).as_deref() == Some(line)
}

/// The source line when neither source gave a line: that the selection was
/// too long, when it was, else that the clipboard was, else that nothing is
/// selected or copied.
pub(crate) fn missed_line(
    selection: Option<(Held, usize)>,
    clipboard: Option<(Held, usize)>,
) -> String {
    selection.or(clipboard).map_or_else(
        || NOTHING_SELECTED_TEXT.to_owned(),
        |(held, chars)| too_long_text(held, chars, CLIPBOARD_MAX_CHARS),
    )
}
