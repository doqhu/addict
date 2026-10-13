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
//! The input word is looked up in the imported indexes, as written and by
//! its dictionary forms. The indexes are opened and read off the UI thread,
//! and a lookup has a deadline. The window shows its outcome: the article, a
//! line saying the dictionary has no entry, a line saying that nothing is
//! imported or that an index cannot be read, each with the import command,
//! or a line saying the lookup did not finish. One index is enough: the
//! article has what that index gives. Whenever an index is missing or cannot
//! be read, the window says so in a line of its own from the moment it is
//! shown, with the file to import. An index imported while the application
//! runs is found by the next lookup; one imported again over an open index
//! is read after a restart. Another word, a second click
//! on the same one or an edit of the field replaces the lookup, and the
//! outcome of a replaced lookup is never shown.
//!
//! An article is shown as the list of its senses, numbered, each definition
//! a link; the list scrolls when it is taller than its area. No sense is
//! chosen when the article arrives, and a line below the list asks to choose
//! one. A click on a sense shows its definition there as a short explanation,
//! under a mark saying that it is the dictionary definition of the chosen
//! sense and is not checked against the sentence, and with it the synonyms of
//! that sense. Below that stand the Russian translations, which belong to the
//! word and not to a sense. What the article lacks has its own line: no
//! English definitions, no synonyms for the chosen sense, no Russian
//! translation. Choosing a sense looks nothing up, and the next result
//! forgets the choice.
//!
//! A click on a word of a sentence also asks DeepL, online, for two Russian
//! texts: the word translated in that sentence, for which the word and the
//! sentence as its context are sent, and, once for the text of the field,
//! the sentence itself, sent without context. A selected nonblank dictionary
//! definition is sent without context only by Translate definition into Russian.
//! Its labelled Russian answer appears below English, before synonyms; changing
//! sense or replacing the article cancels and clears it. Each
//! stands below the result under a label that says what it is, apart from
//! the article, which is shown as before and does not wait for them. What
//! was answered or is still awaited is not asked for again: a second click
//! on the word sends nothing, another word sends only itself and keeps the
//! translation of the sentence, and going back to a word shows what was
//! kept. A text of more than 500 characters is not sent as a sentence: a
//! line under the label of the sentence says it is too long, and the clicked
//! word is still sent with the whole text as its context. A lone word is
//! sent, without context, only when Enter is pressed in the field or the
//! word was pasted there, and stands under a label of its own; typing it or
//! clicking it sends nothing, and neither does Enter in an empty field or in
//! a sentence, or the paste of a sentence. The key is read at each
//! request from `$XDG_CONFIG_HOME/addict/deepl.key`, or
//! `$HOME/.config/addict/deepl.key`, a file only its owner may read or write:
//! when its mode has any bit outside `0600` it is refused unread. A request
//! has a deadline. Any change of the text, an edit or a paste, drops every
//! online text and gives up every request, and a click on another word gives
//! up the request of the word before. When a request gives no translation,
//! its line distinguishes key, connection, quota, language and timeout failures.
//! Key failures name the configured key file; unsafe permissions name 0600.
//! The next eligible click or Enter retries failed requests; successful
//! translations stay available. No request is automatically retried.
//!
//! A paste into the field with Ctrl+V, Shift+Insert or the menu replaces the
//! whole field with the clipboard text as one line, and with it the input and
//! the result; the pasted text is then submitted, as by Enter. This is the
//! only time the clipboard is read: once per paste, without blocking the
//! window. A clipboard that holds no text changes nothing.
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
//! entry commands are [`entries`]'s, the import is `addict_dictionary`'s, the
//! indexes as a source of articles are [`dictionaries`]'s, the online
//! translator is `addict_deepl`'s, what is asked of it and kept is
//! [`online`]'s, the resident
//! application is [`resident`]'s, the texts of its window are [`view`]'s and
//! the widgets of the window are [`widgets`]'s.

mod dictionaries;
mod entries;
mod evidence;
mod manual;
mod online;
mod presentation;
mod recording;
mod resident;
mod view;
mod widgets;

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
