//! Addict application entry point.
//!
//! `addict install` writes the desktop entry twice: into the applications
//! directory, where the portal needs it, and into the autostart directory,
//! which starts the application at login. `addict uninstall` removes both.
//!
//! `addict import <FILE>` turns a dictionary file the user has downloaded
//! into an index in the data directory, `$XDG_DATA_HOME/addict` or
//! `$HOME/.local/share/addict`. Only the two chosen files are accepted, by
//! size and SHA-256: `english-wordnet-2025.xml.gz`, the English definitions
//! and synonyms, gives `oewn.index`, and
//! `freedict-eng-rus-2025.11.23.src.tar.xz`, the Russian translations, gives
//! `freedict.index`. The index replaces the previous one of the same source
//! and the index of the other source is not touched. The command prints the
//! path of the index and the counts of its headwords and exception forms.
//! The resident application does not read the indexes yet.
//!
//! With no arguments the process is the resident application: one hidden
//! window, shown each time the global shortcut is pressed. On each press the
//! window shows the source of the input and the state derived from the input
//! contract; Esc hides it, as the title-bar close does.
//!
//! The window has a field for a word or a sentence; the source of what is
//! entered there is "manual input". A text of exactly one word is that word
//! without context: it is the input as soon as the field holds it, with no
//! click, and each further keystroke replaces it. A longer text is shown with
//! its words as links, and a click on a word makes it the input together with
//! the sentence. A word is a run of letters; an apostrophe between two
//! letters and a combining mark after a letter belong to it, a hyphen
//! separates two words. The field is empty when it holds no word: then there
//! is no input and the window waits for one.
//!
//! The input word is looked up in the built-in sample dictionary of a few
//! words. The lookup runs off the UI thread with a deadline, and the window
//! shows its outcome as plain text: the article, a line saying the dictionary
//! has no entry, or a line saying the lookup did not finish. Another word, a
//! second click on the same one or an edit of the field replaces the lookup,
//! and the outcome of a replaced lookup is never shown.
//!
//! A paste into the field with Ctrl+V, Shift+Insert or the menu replaces the
//! whole field with the clipboard text as one line, and with it the input and
//! the result. This is the only time the clipboard is read: once per paste,
//! without blocking the window. A clipboard that holds no text changes
//! nothing.
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
//! entry commands are [`entries`]'s, the import is `addict_dictionary`'s and
//! the resident application is [`resident`]'s.

mod entries;
mod evidence;
mod manual;
mod presentation;
mod recording;
mod resident;
mod view;

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    process::ExitCode,
    rc::Rc,
};

use addict_dictionary::import;

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
    /// Build the dictionary index from a downloaded dictionary file
    Import {
        /// The dictionary file, as downloaded
        file: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Install) => report(entries::install_entries()),
        Some(Command::Uninstall) => report(entries::uninstall_entries()),
        Some(Command::Import { file }) => import_file(&file),
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

/// Imports the dictionary file and prints the path of its index and what the
/// index holds.
fn import_file(file: &Path) -> ExitCode {
    let Some(dir) = import::data_dir() else {
        return fail(
            "cannot find the data directory: XDG_DATA_HOME is not set to an absolute path and HOME is unset or empty",
        );
    };
    match import::import(file, &dir) {
        Ok(imported) => {
            println!("{}", imported.path.display());
            println!("headwords: {}", imported.headwords);
            println!("exception forms: {}", imported.exception_forms);
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error.to_string()),
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
        Err(message) => fail(&message),
    }
}
