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
//! The window has a field for a sentence. A pasted sentence is shown with its
//! words as links; a click on a word makes it the input, with the sentence
//! and the source "manual input", and looks it up in the built-in sample
//! dictionary of a few words. The lookup runs off the UI thread with a
//! deadline, and the window shows its outcome as plain text: the article, a
//! line saying the dictionary has no entry, or a line saying the lookup did
//! not finish. Editing the field drops the input and the result.
//!
//! The application is unique: a second launch exits at once and makes the
//! resident instance show its window, as a press of the shortcut does.
//!
//! An attempt to bind the shortcut that ends without one, for any reason or
//! for none within `resident::BIND_LIMIT`, shows the window by itself with a
//! line saying so and Retry. Retry starts a new attempt; a bound shortcut
//! removes the line.
//!
//! With the hidden `--evidence <FILE> --origin <APPLICATION>` the application
//! appends measurement evidence to the file: the start of the session and, for
//! each press, the activation, the presentation of the frame that follows it
//! and the focus. Without the flag none of that exists.
//!
//! This module owns the command line and what each form of it starts; the
//! entry commands are [`entries`]'s and the resident application is
//! [`resident`]'s.

mod entries;
mod evidence;
mod manual;
mod presentation;
mod recording;
mod resident;
mod view;

use std::{cell::RefCell, path::PathBuf, process::ExitCode, rc::Rc};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "addict", version, about = "Addict translator")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
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
        Some(Command::Install) => report(entries::install_entries()),
        Some(Command::Uninstall) => report(entries::uninstall_entries()),
        None => match open_evidence(cli.evidence.zip(cli.origin)) {
            Ok(writer) => resident::run(writer),
            Err(message) => fail(&message),
        },
    }
}

/// Says why on stderr, in one line, and fails the launch.
fn fail(message: &str) -> ExitCode {
    eprintln!("addict: {message}");
    ExitCode::FAILURE
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
        Err(message) => fail(&message),
    }
}
