//! The resident application: one hidden window, the attempt to bind the
//! global shortcut and the loop that shows the window for each event.
//!
//! This module owns the GTK application, its uniqueness, the window with its
//! close and Esc handling, and the order of what happens on each event of the
//! shortcut worker, of Retry and of a repeated launch. What the window says is
//! [`view`](crate::view)'s; what is recorded with `--evidence` is
//! [`recording`](crate::recording)'s.

use std::{
    cell::{Cell, RefCell},
    process::ExitCode,
    rc::{Rc, Weak},
    time::Duration,
};

use addict_core::{APP_ID, input::window_state};
use addict_platform::shortcut::{self, ShortcutEvent, ShortcutHandle};
use futures_channel::mpsc::UnboundedSender;
use futures_util::StreamExt;
use gtk::{gdk, gio, glib, prelude::*};

use crate::{
    evidence,
    recording::Recording,
    view::{Binding, View},
};

/// Preferred trigger in the XDG shortcuts syntax; the user may change it.
const TRIGGER: &str = "LOGO+q";

/// How long one attempt to bind the shortcut may take.
const BIND_LIMIT: Duration = Duration::from_secs(60);

/// What lives as long as the resident application.
struct Resident {
    _hold: gio::ApplicationHoldGuard,
    /// The worker of the current attempt; Retry replaces it.
    _shortcut: Rc<RefCell<Option<ShortcutHandle>>>,
    /// Where a repeated launch reports itself.
    events: UnboundedSender<Event>,
}

/// Everything the worker threads tell the UI thread, over one channel.
enum Event {
    /// With the event, the `CLOCK_MONOTONIC` time at which the worker got an
    /// activation; taken only when evidence is recorded.
    Shortcut(ShortcutEvent, Option<u64>),
    /// Retry was pressed.
    Retry,
    /// The application was launched again while this instance is resident.
    Launched,
}

/// The window state of this moment. Nothing acquires input yet, so nothing is
/// pending and nothing has arrived.
fn current_state() -> addict_core::input::WindowState {
    window_state(None, None)
}

/// Starts one attempt to bind the shortcut; its events go to `sender`. With
/// `timed`, an activation carries the time the worker thread received it.
fn start_shortcut(sender: &UnboundedSender<Event>, timed: bool) -> ShortcutHandle {
    let sender = sender.clone();
    shortcut::start(APP_ID, TRIGGER, BIND_LIMIT, move |event| {
        // Here, on the worker: the UI thread may be busy when it arrives.
        let receipt_ns = (timed && matches!(event, ShortcutEvent::Activated { .. }))
            .then(evidence::monotonic_ns);
        let _ = sender.unbounded_send(Event::Shortcut(event, receipt_ns));
    })
}

/// The hidden window around the `view`: closing it hides it, and so does Esc.
fn build_window(application: &gtk::Application, view: &View) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Addict")
        .default_width(480)
        .default_height(240)
        .child(view.root())
        .build();
    // Closing hides; the process and this window stay.
    window.connect_close_request(|window| {
        window.set_visible(false);
        glib::Propagation::Stop
    });
    // Bubble phase: a focused child that uses Esc itself keeps it.
    let key = gtk::EventControllerKey::new();
    let key_window = window.downgrade();
    key.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            if let Some(window) = key_window.upgrade() {
                window.close();
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key);
    window
}

/// The state the event loop keeps between events, on the UI thread.
struct Session {
    binding: Binding,
    view: View,
    window: gtk::ApplicationWindow,
    /// Present only with `--evidence`.
    recording: Option<Recording>,
    /// Whether an activation carries its receipt time.
    timed: bool,
    /// Where the worker of a new attempt sends its events.
    sender: UnboundedSender<Event>,
    /// Weak: the worker must go when the resident state is dropped.
    current: Weak<RefCell<Option<ShortcutHandle>>>,
}

impl Session {
    fn handle(&mut self, event: Event) {
        match event {
            Event::Shortcut(ShortcutEvent::Bound { trigger }, _) => self.bound(&trigger),
            Event::Shortcut(
                ShortcutEvent::Activated {
                    activation_token,
                    timestamp_ms,
                },
                receipt_ns,
            ) => self.activated(activation_token.as_deref(), timestamp_ms, receipt_ns),
            Event::Shortcut(ShortcutEvent::Failed { message }, _) => self.failed(message),
            Event::Launched => self.launched(),
            Event::Retry => self.retry(),
        }
    }

    fn bound(&mut self, trigger: &str) {
        eprintln!("addict: shortcut bound to {trigger}");
        self.binding = Binding::Bound;
        self.view.render_binding(&self.binding);
    }

    /// The shortcut was pressed: the window is shown and takes the focus.
    fn activated(
        &self,
        activation_token: Option<&str>,
        timestamp_ms: u64,
        receipt_ns: Option<u64>,
    ) {
        // Before `present()`: the first frame has the labels.
        self.view.render(current_state());
        // Also before it: the feedback is for the next commit.
        let recorded = self.recording.as_ref().zip(receipt_ns).map(|(r, receipt)| {
            let token = activation_token.is_some();
            (r, r.activated(timestamp_ms, receipt, token))
        });
        // A fresh token per press is what lets the window
        // take focus from the application in front.
        if let Some(token) = activation_token {
            self.window.set_startup_id(token);
        }
        self.window.present();
        if let Some((recording, id)) = recorded {
            // A window that is already visible commits no
            // frame by itself.
            self.window.queue_draw();
            recording.presented(&id, activation_token.is_some());
        }
    }

    /// The attempt ended without a shortcut: the window shows itself to say so.
    fn failed(&mut self, message: String) {
        eprintln!("addict: shortcut unavailable: {message}");
        self.binding = Binding::NotBound { detail: message };
        self.view.render_binding(&self.binding);
        // No press stands behind this, so there is no token.
        if let Some(recording) = &self.recording {
            recording.shown_without_press();
        }
        self.window.present();
    }

    fn launched(&self) {
        self.view.render(current_state());
        // No `set_startup_id`: GTK has already applied the
        // token of the launcher, when there was one.
        if let Some(recording) = &self.recording {
            recording.shown_without_press();
        }
        self.window.present();
    }

    fn retry(&mut self) {
        // One attempt at a time: a press that was queued
        // behind another one starts nothing.
        if !matches!(self.binding, Binding::NotBound { .. }) {
            return;
        }
        let Some(current) = self.current.upgrade() else {
            return;
        };
        self.binding = Binding::Attempting;
        self.view.render_binding(&self.binding);
        // `Failed` is the last act of the old worker, sent
        // after its session was closed: joining it does not
        // wait.
        current.borrow_mut().take();
        *current.borrow_mut() = Some(start_shortcut(&self.sender, self.timed));
    }
}

/// Builds the hidden window, starts the first attempt to bind the shortcut
/// and the event loop. Runs once, in the primary instance.
fn start(
    application: &gtk::Application,
    evidence: Option<Rc<RefCell<evidence::Writer>>>,
) -> Resident {
    // The worker threads push; the main context wakes only for an event.
    let (sender, mut receiver) = futures_channel::mpsc::unbounded();

    // The first attempt starts below, with the window still hidden.
    let binding = Binding::Attempting;
    let view = View::new(current_state(), &binding);
    let retry = sender.clone();
    view.connect_retry(move || {
        let _ = retry.unbounded_send(Event::Retry);
    });
    let window = build_window(application, &view);

    // Only the primary instance gets here, so a second launch records no
    // session.
    let recording = evidence.map(|writer| Recording::start(writer, &window));
    let timed = recording.is_some();
    let handle = Rc::new(RefCell::new(Some(start_shortcut(&sender, timed))));
    let events = sender.clone();
    let mut session = Session {
        binding,
        view,
        window,
        recording,
        timed,
        sender,
        current: Rc::downgrade(&handle),
    };
    glib::spawn_future_local(async move {
        while let Some(event) = receiver.next().await {
            session.handle(event);
        }
    });
    Resident {
        _hold: application.hold(),
        _shortcut: handle,
        events,
    }
}

/// `activate` fires for the launch of this instance and for every later
/// launch, which exits and leaves its activation to this one. The first is
/// the launch at login and shows nothing; a later one shows the window.
fn connect_launches(application: &gtk::Application, resident: Rc<RefCell<Option<Resident>>>) {
    let launched = Cell::new(false);
    application.connect_activate(move |_| {
        if !launched.replace(true) {
            return;
        }
        if let Some(resident) = resident.borrow().as_ref() {
            let _ = resident.events.unbounded_send(Event::Launched);
        }
    });
}

/// Registers the application and gives it back when this launch is the
/// primary instance; otherwise the launch ends with the returned code.
fn primary(application: gtk::Application) -> Result<gtk::Application, ExitCode> {
    if let Err(e) = application.register(gio::Cancellable::NONE) {
        eprintln!("addict: {e}");
        return Err(ExitCode::FAILURE);
    }
    if application.is_remote() {
        eprintln!("addict: another instance is running; nothing is recorded");
        // GLib warns on stderr when a registered application that never
        // ran is destroyed; the process ends here anyway.
        std::mem::forget(application);
        return Err(ExitCode::FAILURE);
    }
    Ok(application)
}

/// Runs the resident application until it quits. With `evidence`, each
/// activation is recorded to it.
pub fn run(evidence: Option<Rc<RefCell<evidence::Writer>>>) -> ExitCode {
    // Without a display there is nothing to be resident for: leave before the
    // application or the portal worker exist.
    if let Err(e) = gtk::init() {
        eprintln!("addict: {e}");
        return ExitCode::FAILURE;
    }
    let mut application = gtk::Application::builder().application_id(APP_ID).build();
    let resident: Rc<RefCell<Option<Resident>>> = Rc::default();
    let recording = evidence.is_some();

    let slot = resident.clone();
    application.connect_startup(move |application| {
        let resident = start(application, evidence.clone());
        *slot.borrow_mut() = Some(resident);
    });
    connect_launches(&application, resident.clone());
    application.connect_shutdown(move |_| {
        // Stops the workers; the shortcut one closes the portal session.
        resident.borrow_mut().take();
    });

    // With the flag only: registering here tells whether this launch is the
    // primary instance before it would activate the resident one.
    if recording {
        application = match primary(application) {
            Ok(application) => application,
            Err(code) => return code,
        };
    }
    let status = application.run_with_args::<&str>(&[]);
    if status == glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
