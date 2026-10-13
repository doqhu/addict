//! The recording of `--evidence` on the UI thread. This module owns which
//! activation the window is shown for and the writing of each record when
//! its caller asks for it; the format of the records is [`evidence`]'s and
//! the frame time is [`presentation`]'s.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gtk::prelude::*;

use crate::{evidence, presentation};

/// The recording of `--evidence`, on the UI thread.
pub struct Recording {
    writer: Rc<RefCell<evidence::Writer>>,
    watch: presentation::Watch,
    /// The activation the window is shown for; focus changes belong to it.
    current: Rc<RefCell<Option<String>>>,
    /// Activations of this process.
    count: Cell<u64>,
    /// Whether the window has been shown, by a press or otherwise.
    shown: Cell<bool>,
}

impl Recording {
    /// Writes the session start and attaches to the hidden `window`.
    pub fn start(writer: Rc<RefCell<evidence::Writer>>, window: &gtk::ApplicationWindow) -> Self {
        writer.borrow_mut().write(&evidence::session_start(
            std::process::id(),
            evidence::boot_id().as_deref(),
        ));
        let presented = writer.clone();
        let watch = presentation::Watch::new(window, move |id, frame| {
            presented.borrow_mut().write(&evidence::presentation(
                id,
                frame.time_ns,
                frame.clock_id,
                frame.limitation.as_deref(),
            ));
        });
        let current: Rc<RefCell<Option<String>>> = Rc::default();
        let (focus, focused) = (writer.clone(), current.clone());
        window.connect_is_active_notify(move |window| {
            // A window that is not visible belongs to no press.
            if window.is_visible()
                && let Some(id) = focused.borrow().as_deref()
            {
                focus
                    .borrow_mut()
                    .write(&evidence::focus(id, window.is_active()));
            }
        });
        // Hiding ends the activation: losing the focus to it is not recorded.
        let closed = current.clone();
        window.connect_hide(move |_| {
            closed.borrow_mut().take();
        });
        Self {
            writer,
            watch,
            current,
            count: Cell::new(0),
            shown: Cell::new(false),
        }
    }

    /// Records a press and arms the feedback of its frame; call before
    /// `present()`. Returns the id of the activation.
    pub fn activated(&self, portal_timestamp_ms: u64, receipt_ns: u64, token: bool) -> String {
        let count = self.count.get() + 1;
        self.count.set(count);
        let id = evidence::invocation_id(std::process::id(), receipt_ns, count);
        let cold = !self.shown.replace(true);
        self.writer.borrow_mut().write(&evidence::activation(
            &id,
            cold,
            portal_timestamp_ms,
            receipt_ns,
            token,
        ));
        *self.current.borrow_mut() = Some(id.clone());
        self.watch.arm(id.clone());
        id
    }

    /// Call after the `present()` of the activation `id`.
    pub fn presented(&self, id: &str, token_consumed: bool) {
        self.writer
            .borrow_mut()
            .write(&evidence::token_consumption(id, token_consumed));
    }

    /// A lookup ended as `answered`.
    pub fn looked_up(&self, answered: evidence::Answered) {
        self.writer.borrow_mut().write(&evidence::lookup(answered));
    }

    /// An online request ended; `record` is what the evidence says of it.
    pub fn translated_online(&self, record: &evidence::Record) {
        self.writer.borrow_mut().write(record);
    }

    /// The window was shown with no press behind it.
    pub fn shown_without_press(&self) {
        self.shown.set(true);
    }
}
