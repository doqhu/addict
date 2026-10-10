//! Addict application entry point.
//!
//! `addict install` writes the desktop entry the portal needs. With no
//! arguments the process is the resident application: one hidden window, shown
//! each time the global shortcut is pressed.

use std::{cell::RefCell, path::PathBuf, process::ExitCode, rc::Rc};

use addict_core::APP_ID;
use addict_platform::{
    desktop,
    shortcut::{self, ShortcutEvent, ShortcutHandle},
};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use gtk::{gio, glib, prelude::*};

/// Preferred trigger in the XDG shortcuts syntax; the user may change it.
const TRIGGER: &str = "LOGO+q";

#[derive(Parser)]
#[command(name = "addict", version, about = "Addict translator")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Write the desktop entry that lets the portal recognise the application
    Install,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Some(Command::Install) => install(),
        None => run(),
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
    _shortcut: ShortcutHandle,
}

fn run() -> ExitCode {
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
        let label = gtk::Label::new(Some("Addict"));
        let window = gtk::ApplicationWindow::builder()
            .application(application)
            .title("Addict")
            .default_width(480)
            .default_height(240)
            .child(&label)
            .build();
        // Closing hides; the process and this window stay.
        window.connect_close_request(|window| {
            window.set_visible(false);
            glib::Propagation::Stop
        });

        // The worker thread pushes; the main context wakes only for an event.
        let (sender, mut receiver) = futures_channel::mpsc::unbounded();
        let handle = shortcut::start(APP_ID, TRIGGER, move |event| {
            let _ = sender.unbounded_send(event);
        });
        glib::spawn_future_local(async move {
            while let Some(event) = receiver.next().await {
                match event {
                    ShortcutEvent::Bound { trigger } => {
                        eprintln!("addict: shortcut bound to {trigger}");
                    }
                    ShortcutEvent::Activated {
                        activation_token, ..
                    } => {
                        // A fresh token per press is what lets the window
                        // take focus from the application in front.
                        if let Some(token) = activation_token.as_deref() {
                            window.set_startup_id(token);
                        }
                        window.present();
                    }
                    ShortcutEvent::Failed { message } => {
                        eprintln!("addict: shortcut unavailable: {message}");
                    }
                }
            }
        });
        *slot.borrow_mut() = Some(Resident {
            _hold: application.hold(),
            _shortcut: handle,
        });
    });
    // A launch, the first or a repeated one, shows nothing.
    application.connect_activate(|_| {});
    application.connect_shutdown(move |_| {
        // Stops the worker, which closes the portal session.
        resident.borrow_mut().take();
    });

    let status = application.run_with_args::<&str>(&[]);
    if status == glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
