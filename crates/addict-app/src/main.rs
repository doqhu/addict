//! Addict application entry point.
//!
//! `addict install` writes the desktop entry twice: into the applications
//! directory, where the portal needs it, and into the autostart directory,
//! which starts the application at login. `addict uninstall` removes both.
//! With no arguments the process is the resident application: one hidden
//! window, shown each time the global shortcut is pressed. On each press the
//! window shows the source of the input and the state derived from the input
//! contract; Esc hides it, as the title-bar close does.
//!
//! The application is unique: a second launch exits at once and makes the
//! resident instance show its window, as a press of the shortcut does.
//!
//! An attempt to bind the shortcut that ends without one, for any reason or
//! for none within [`BIND_LIMIT`], shows the window by itself with a line
//! saying so and Retry. Retry starts a new attempt; a bound shortcut removes
//! the line.
//!
//! With the hidden `--evidence <FILE> --origin <APPLICATION>` the application
//! appends measurement evidence to the file: the start of the session and, for
//! each press, the activation, the presentation of the frame that follows it
//! and the focus. Without the flag none of that exists.

mod evidence;
mod presentation;
mod view;

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    process::ExitCode,
    rc::{Rc, Weak},
    time::Duration,
};

use addict_core::{APP_ID, input::window_state};
use addict_platform::{
    desktop,
    shortcut::{self, ShortcutEvent, ShortcutHandle},
    work::{Job, Outcome, Work},
};
use clap::{Parser, Subcommand};
use futures_channel::mpsc::UnboundedSender;
use futures_util::StreamExt;
use gtk::{gdk, gio, glib, prelude::*};
use view::{Binding, View};

/// Preferred trigger in the XDG shortcuts syntax; the user may change it.
const TRIGGER: &str = "LOGO+q";

/// How long one attempt to bind the shortcut may take.
const BIND_LIMIT: Duration = Duration::from_secs(60);

#[derive(Parser)]
#[command(name = "addict", version, about = "Addict translator")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Add a test job to the window: slow work with a deadline and a cancel
    #[arg(long, hide = true)]
    work_demo: bool,
    /// Append measurement evidence of each activation to this file
    #[arg(long, hide = true, value_name = "FILE", requires = "origin")]
    evidence: Option<PathBuf>,
    /// The application the shortcut is pressed from, for the evidence
    #[arg(long, hide = true, value_enum, requires = "evidence")]
    origin: Option<evidence::Origin>,
}

#[derive(Subcommand)]
enum Command {
    /// Write the desktop entries: the portal recognises the application and
    /// the session starts it at login
    Install,
    /// Remove the desktop entries that install wrote
    Uninstall,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Install) => report(install_entries()),
        Some(Command::Uninstall) => report(uninstall_entries()),
        None => match open_evidence(cli.evidence.zip(cli.origin)) {
            Ok(writer) => run(cli.work_demo, writer),
            Err(message) => {
                eprintln!("addict: {message}");
                ExitCode::FAILURE
            }
        },
    }
}

/// The evidence file of `--evidence`, opened before anything else starts: a
/// path that cannot be written ends the launch.
fn open_evidence(
    flags: Option<(PathBuf, evidence::Origin)>,
) -> Result<Option<Rc<RefCell<evidence::Writer>>>, String> {
    let Some((path, origin)) = flags else {
        return Ok(None);
    };
    evidence::Writer::open(&path, origin)
        .map(|writer| Some(Rc::new(RefCell::new(writer))))
        .map_err(|e| format!("cannot open the evidence file {}: {e}", path.display()))
}

/// Prints the paths a command wrote or removed, one per line, or its error.
fn report(result: Result<Vec<PathBuf>, String>) -> ExitCode {
    match result {
        Ok(paths) => {
            for path in paths {
                println!("{}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("addict: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The applications and the autostart directory, in the order the commands
/// act on them. Both targets are checked here, before either is written or
/// removed, so a refusal leaves both as they were.
fn checked_dirs() -> Result<[PathBuf; 2], String> {
    let dirs = [
        desktop::applications_dir()
            .ok_or("cannot find the applications directory: HOME is not set")?,
        desktop::autostart_dir().ok_or("cannot find the autostart directory: HOME is not set")?,
    ];
    for dir in &dirs {
        // The error text names the path.
        desktop::check(dir, APP_ID).map_err(|e| e.to_string())?;
    }
    Ok(dirs)
}

fn install_entries() -> Result<Vec<PathBuf>, String> {
    let exec = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|e| format!("cannot resolve the path of this binary: {e}"))?;
    checked_dirs()?
        .iter()
        .map(|dir| desktop::install(dir, APP_ID, &exec).map_err(|e| e.to_string()))
        .collect()
}

/// The paths that were removed: an entry that was not there is not listed.
fn uninstall_entries() -> Result<Vec<PathBuf>, String> {
    let mut removed = Vec::new();
    for dir in &checked_dirs()? {
        removed.extend(desktop::uninstall(dir, APP_ID).map_err(|e| e.to_string())?);
    }
    Ok(removed)
}

/// What lives as long as the resident application.
struct Resident {
    _hold: gio::ApplicationHoldGuard,
    /// The worker of the current attempt; Retry replaces it.
    _shortcut: Rc<RefCell<Option<ShortcutHandle>>>,
    /// Present only with `--work-demo`.
    _work: Option<Rc<Work>>,
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
    /// The outcome of the demo job number `run`.
    Demo { run: u64, outcome: Outcome<()> },
}

/// The test job of `--work-demo`.
struct Demo {
    work: Rc<Work>,
    panel: gtk::Box,
    status: gtk::Label,
    running: Rc<RefCell<Running>>,
}

/// The demo job in flight. A job replaced by a newer one still reports its
/// cancellation; `last` tells that outcome from the current one.
#[derive(Default)]
struct Running {
    last: u64,
    job: Option<Job>,
}

impl Demo {
    fn build(work: Work, sender: &UnboundedSender<Event>) -> Self {
        let work = Rc::new(work);
        let status = gtk::Label::new(Some("idle"));
        let running: Rc<RefCell<Running>> = Rc::default();

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        // (label, how long the job sleeps, its deadline), in seconds.
        for (name, sleep, deadline) in [("Finish", 2, 10), ("Deadline", 30, 3)] {
            let button = gtk::Button::with_label(name);
            // Weak: the runner must go when the resident state is dropped.
            let work = Rc::downgrade(&work);
            let (status, running, sender) = (status.clone(), running.clone(), sender.clone());
            button.connect_clicked(move |_| {
                start_demo_job(
                    &work,
                    &running,
                    &sender,
                    Duration::from_secs(sleep),
                    Duration::from_secs(deadline),
                );
                status.set_text(&format!("running: {name}"));
            });
            buttons.append(&button);
        }
        let cancel = gtk::Button::with_label("Cancel");
        let job = running.clone();
        cancel.connect_clicked(move |_| {
            // The status changes when the outcome arrives.
            if let Some(job) = &job.borrow().job {
                job.cancel();
            }
        });
        buttons.append(&cancel);

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 6);
        panel.append(&gtk::Entry::new());
        panel.append(&status);
        panel.append(&buttons);
        Self {
            work,
            panel,
            status,
            running,
        }
    }
}

/// Starts a demo job that only sleeps; the one in flight, if any, is replaced.
fn start_demo_job(
    work: &Weak<Work>,
    running: &RefCell<Running>,
    sender: &UnboundedSender<Event>,
    sleep: Duration,
    deadline: Duration,
) {
    let Some(work) = work.upgrade() else {
        return;
    };
    let mut running = running.borrow_mut();
    running.last += 1;
    let run = running.last;
    let sender = sender.clone();
    // Dropping the previous handle cancels the job it stood for.
    running.job = Some(work.run(
        deadline,
        // The timer needs the runtime, so it is created on the worker.
        async move { tokio::time::sleep(sleep).await },
        move |outcome| {
            let _ = sender.unbounded_send(Event::Demo { run, outcome });
        },
    ));
}

fn outcome_text(outcome: Outcome<()>) -> &'static str {
    match outcome {
        Outcome::Finished(()) => "finished",
        Outcome::Cancelled => "cancelled",
        Outcome::TimedOut => "timed out",
        Outcome::Panicked => "panicked",
    }
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

/// The recording of `--evidence`, on the UI thread.
struct Recording {
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
    fn start(writer: Rc<RefCell<evidence::Writer>>, window: &gtk::ApplicationWindow) -> Self {
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
    fn activated(&self, portal_timestamp_ms: u64, receipt_ns: u64, token: bool) -> String {
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
    fn presented(&self, id: &str, token_consumed: bool) {
        self.writer
            .borrow_mut()
            .write(&evidence::token_consumption(id, token_consumed));
    }

    /// The window was shown with no press behind it.
    fn shown_without_press(&self) {
        self.shown.set(true);
    }
}

fn run(work_demo: bool, evidence: Option<Rc<RefCell<evidence::Writer>>>) -> ExitCode {
    // Without a display there is nothing to be resident for: leave before the
    // application or the portal worker exist.
    if let Err(e) = gtk::init() {
        eprintln!("addict: {e}");
        return ExitCode::FAILURE;
    }
    let application = gtk::Application::builder().application_id(APP_ID).build();
    let resident: Rc<RefCell<Option<Resident>>> = Rc::default();
    let recording = evidence.is_some();

    let slot = resident.clone();
    application.connect_startup(move |application| {
        let evidence = &evidence;
        // The worker threads push; the main context wakes only for an event.
        let (sender, mut receiver) = futures_channel::mpsc::unbounded();

        let demo = if work_demo {
            match Work::start() {
                Ok(work) => Some(Demo::build(work, &sender)),
                Err(e) => {
                    eprintln!("addict: work demo unavailable: {e}");
                    None
                }
            }
        } else {
            None
        };
        // The first attempt starts below, with the window still hidden.
        let mut binding = Binding::Attempting;
        let view = View::new(current_state(), &binding);
        let retry = sender.clone();
        view.connect_retry(move || {
            let _ = retry.unbounded_send(Event::Retry);
        });
        if let Some(demo) = &demo {
            view.root().append(&demo.panel);
        }
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

        // Only the primary instance gets here, so a second launch records no
        // session.
        let recording = evidence
            .clone()
            .map(|writer| Recording::start(writer, &window));
        let timed = recording.is_some();
        let handle = Rc::new(RefCell::new(Some(start_shortcut(&sender, timed))));
        let events = sender.clone();
        // Weak: the worker must go when the resident state is dropped.
        let current = Rc::downgrade(&handle);
        let work = demo.as_ref().map(|demo| demo.work.clone());
        let demo = demo.map(|demo| (demo.status, demo.running));
        glib::spawn_future_local(async move {
            while let Some(event) = receiver.next().await {
                match event {
                    Event::Shortcut(ShortcutEvent::Bound { trigger }, _) => {
                        eprintln!("addict: shortcut bound to {trigger}");
                        binding = Binding::Bound;
                        view.render_binding(&binding);
                    }
                    Event::Shortcut(
                        ShortcutEvent::Activated {
                            activation_token,
                            timestamp_ms,
                        },
                        receipt_ns,
                    ) => {
                        // Before `present()`: the first frame has the labels.
                        view.render(current_state());
                        // Also before it: the feedback is for the next commit.
                        let recorded = recording.as_ref().zip(receipt_ns).map(|(r, receipt)| {
                            let token = activation_token.is_some();
                            (r, r.activated(timestamp_ms, receipt, token))
                        });
                        // A fresh token per press is what lets the window
                        // take focus from the application in front.
                        if let Some(token) = activation_token.as_deref() {
                            window.set_startup_id(token);
                        }
                        window.present();
                        if let Some((recording, id)) = recorded {
                            // A window that is already visible commits no
                            // frame by itself.
                            window.queue_draw();
                            recording.presented(&id, activation_token.is_some());
                        }
                    }
                    Event::Shortcut(ShortcutEvent::Failed { message }, _) => {
                        eprintln!("addict: shortcut unavailable: {message}");
                        binding = Binding::NotBound { detail: message };
                        view.render_binding(&binding);
                        // No press stands behind this, so there is no token.
                        if let Some(recording) = &recording {
                            recording.shown_without_press();
                        }
                        window.present();
                    }
                    Event::Launched => {
                        view.render(current_state());
                        // No `set_startup_id`: GTK has already applied the
                        // token of the launcher, when there was one.
                        if let Some(recording) = &recording {
                            recording.shown_without_press();
                        }
                        window.present();
                    }
                    Event::Retry => {
                        // One attempt at a time: a press that was queued
                        // behind another one starts nothing.
                        if !matches!(binding, Binding::NotBound { .. }) {
                            continue;
                        }
                        let Some(current) = current.upgrade() else {
                            continue;
                        };
                        binding = Binding::Attempting;
                        view.render_binding(&binding);
                        // `Failed` is the last act of the old worker, sent
                        // after its session was closed: joining it does not
                        // wait.
                        current.borrow_mut().take();
                        *current.borrow_mut() = Some(start_shortcut(&sender, timed));
                    }
                    Event::Demo { run, outcome } => {
                        if let Some((status, running)) = &demo {
                            let mut running = running.borrow_mut();
                            // Otherwise it is the job that was replaced.
                            if run == running.last {
                                running.job = None;
                                status.set_text(outcome_text(outcome));
                            }
                        }
                    }
                }
            }
        });
        *slot.borrow_mut() = Some(Resident {
            _hold: application.hold(),
            _shortcut: handle,
            _work: work,
            events,
        });
    });
    // `activate` fires here for the launch of this instance and for every
    // later launch, which exits and leaves its activation to this one. The
    // first is the launch at login and shows nothing; a later one shows the
    // window.
    let launched = Cell::new(false);
    let slot = resident.clone();
    application.connect_activate(move |_| {
        if !launched.replace(true) {
            return;
        }
        if let Some(resident) = slot.borrow().as_ref() {
            let _ = resident.events.unbounded_send(Event::Launched);
        }
    });
    application.connect_shutdown(move |_| {
        // Stops the workers; the shortcut one closes the portal session.
        resident.borrow_mut().take();
    });

    // With the flag only: registering here tells whether this launch is the
    // primary instance before it would activate the resident one.
    if recording {
        if let Err(e) = application.register(gio::Cancellable::NONE) {
            eprintln!("addict: {e}");
            return ExitCode::FAILURE;
        }
        if application.is_remote() {
            eprintln!("addict: another instance is running; nothing is recorded");
            // GLib warns on stderr when a registered application that never
            // ran is destroyed; the process ends here anyway.
            std::mem::forget(application);
            return ExitCode::FAILURE;
        }
    }
    let status = application.run_with_args::<&str>(&[]);
    if status == glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
