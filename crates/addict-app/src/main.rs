//! Addict application entry point.
//!
//! `addict install` writes the desktop entry the portal needs. With no
//! arguments the process is the resident application: one hidden window, shown
//! each time the global shortcut is pressed. On each press the window shows
//! the source of the input and the state derived from the input contract;
//! Esc hides it, as the title-bar close does.
//!
//! An attempt to bind the shortcut that ends without one, for any reason or
//! for none within [`BIND_LIMIT`], shows the window by itself with a line
//! saying so and Retry. Retry starts a new attempt; a bound shortcut removes
//! the line.

mod view;

use std::{
    cell::RefCell,
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
}

#[derive(Subcommand)]
enum Command {
    /// Write the desktop entry that lets the portal recognise the application
    Install,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Install) => install(),
        None => run(cli.work_demo),
    }
}

fn install() -> ExitCode {
    match install_entry() {
        Ok(path) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("addict: {message}");
            ExitCode::FAILURE
        }
    }
}

fn install_entry() -> Result<PathBuf, String> {
    let exec = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|e| format!("cannot resolve the path of this binary: {e}"))?;
    let dir = desktop::applications_dir()
        .ok_or("cannot find the applications directory: HOME is not set")?;
    // The error text of `install` names the path.
    desktop::install(&dir, APP_ID, &exec).map_err(|e| e.to_string())
}

/// What lives as long as the resident application.
struct Resident {
    _hold: gio::ApplicationHoldGuard,
    /// The worker of the current attempt; Retry replaces it.
    _shortcut: Rc<RefCell<Option<ShortcutHandle>>>,
    /// Present only with `--work-demo`.
    _work: Option<Rc<Work>>,
}

/// Everything the worker threads tell the UI thread, over one channel.
enum Event {
    Shortcut(ShortcutEvent),
    /// Retry was pressed.
    Retry,
    /// The outcome of the demo job number `run`.
    Demo {
        run: u64,
        outcome: Outcome<()>,
    },
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

/// Starts one attempt to bind the shortcut; its events go to `sender`.
fn start_shortcut(sender: &UnboundedSender<Event>) -> ShortcutHandle {
    let sender = sender.clone();
    shortcut::start(APP_ID, TRIGGER, BIND_LIMIT, move |event| {
        let _ = sender.unbounded_send(Event::Shortcut(event));
    })
}

fn run(work_demo: bool) -> ExitCode {
    // Without a display there is nothing to be resident for: leave before the
    // application or the portal worker exist.
    if let Err(e) = gtk::init() {
        eprintln!("addict: {e}");
        return ExitCode::FAILURE;
    }
    let application = gtk::Application::builder().application_id(APP_ID).build();
    let resident: Rc<RefCell<Option<Resident>>> = Rc::default();

    let slot = resident.clone();
    application.connect_startup(move |application| {
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

        let handle = Rc::new(RefCell::new(Some(start_shortcut(&sender))));
        // Weak: the worker must go when the resident state is dropped.
        let current = Rc::downgrade(&handle);
        let work = demo.as_ref().map(|demo| demo.work.clone());
        let demo = demo.map(|demo| (demo.status, demo.running));
        glib::spawn_future_local(async move {
            while let Some(event) = receiver.next().await {
                match event {
                    Event::Shortcut(ShortcutEvent::Bound { trigger }) => {
                        eprintln!("addict: shortcut bound to {trigger}");
                        binding = Binding::Bound;
                        view.render_binding(&binding);
                    }
                    Event::Shortcut(ShortcutEvent::Activated {
                        activation_token, ..
                    }) => {
                        // Before `present()`: the first frame has the labels.
                        view.render(current_state());
                        // A fresh token per press is what lets the window
                        // take focus from the application in front.
                        if let Some(token) = activation_token.as_deref() {
                            window.set_startup_id(token);
                        }
                        window.present();
                    }
                    Event::Shortcut(ShortcutEvent::Failed { message }) => {
                        eprintln!("addict: shortcut unavailable: {message}");
                        binding = Binding::NotBound { detail: message };
                        view.render_binding(&binding);
                        // No press stands behind this, so there is no token.
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
                        *current.borrow_mut() = Some(start_shortcut(&sender));
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
        });
    });
    // A launch, the first or a repeated one, shows nothing.
    application.connect_activate(|_| {});
    application.connect_shutdown(move |_| {
        // Stops the workers; the shortcut one closes the portal session.
        resident.borrow_mut().take();
    });

    let status = application.run_with_args::<&str>(&[]);
    if status == glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
