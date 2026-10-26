//! Addict application entry point.
//!
//! `addict install` writes the desktop entry twice: into the applications
//! directory, where the portal needs it, and into the autostart directory,
//! which starts the application at login. It also writes the app icon, four
//! sizes, into the user icon theme, where the entry's `Icon` finds it.
//! `addict uninstall` removes all of them.
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
//! The word is looked up in the reading language first. When that language
//! has indexes and none has the word, the other languages are asked in the
//! order English, Spanish, Catalan, and the first that has it gives the
//! article, under a line that names the chosen language and the one that
//! answered. The reading language stays the user's: the next word starts
//! from it.
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
//! forgets the choice. Each sense row starts with its part of speech as a
//! small dim tag when the dictionary gives one, for the sense or for all of
//! its entries alike. Under the headword stand the transcriptions of the
//! article's entries, each once, with their regional labels and, for an
//! entry of another headword such as the lemma of a verb form, that
//! headword; the first three are shown and the rest behind Show more. Each
//! transcription that has a recording, and each recording without a
//! transcription, has a play button, which streams it with GTK's media playback: one recording plays
//! at a time, and a new result stops it. A playback that fails says so in
//! the button's tooltip.
//!
//! A click on a word of a sentence also asks DeepL, online, for two Russian
//! texts: the word translated in that sentence, for which the word and the
//! sentence as its context are sent, and, once for the text of the field,
//! the sentence itself, sent without context. A selected nonblank dictionary
//! definition, or English gloss of a Spanish or Catalan word, is sent without
//! context only by Translate definition into Russian, from the language it
//! is in: English for an English definition and for a gloss, Spanish or
//! Catalan for a definition of that language. Its labelled Russian answer
//! appears below it, before synonyms; changing the choice or replacing the
//! article cancels and clears it. Each
//! stands below the result under a label that says what it is, apart from
//! the article, which is shown as before and does not wait for them; the
//! word's request waits for its lookup and is sent from the language that
//! answered, the sentence's from the reading language. What
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
//! when its mode has any bit outside `0600` it is refused unread. The key
//! can be entered in the Settings window, which writes that file with mode
//! `0600` and never shows the key it holds. A request
//! has a deadline. Any change of the text, an edit or a paste, drops every
//! online text and gives up every request, and a click on another word gives
//! up the request of the word before. When a request gives no translation,
//! its line distinguishes key, connection, quota, language and timeout failures.
//! Key failures name the configured key file; unsafe permissions name 0600.
//! The next eligible click or Enter retries failed requests; successful
//! translations stay available. No request is automatically retried. Every
//! answer, and never a refusal, is also kept on disk in
//! `$XDG_CACHE_HOME/addict/translations.sqlite3`, or
//! `$HOME/.cache/addict/translations.sqlite3`, mode `0600` in a `0700`
//! directory with a `CACHEDIR.TAG`, and the same request is answered from
//! there without DeepL, even after a restart, for up to 90 days since its
//! last use and within 10 MB; the Settings window shows its size and clears
//! it, and a cache file that cannot be used is deleted and does not stop a
//! translation.
//!
//! A paste into the field with Ctrl+V, Shift+Insert or the menu replaces the
//! whole field with the clipboard text as one line, and with it the input and
//! the result; the pasted text is then submitted, as by Enter. The clipboard
//! is read once per paste, without blocking the window, and otherwise only at
//! a press of the shortcut (below). A clipboard that holds no text changes
//! nothing.
//!
//! Capture, beside the field, hides the window and asks the desktop for an
//! interactive screenshot: the user chooses an area, or cancels. Once the
//! snapshot is taken the window comes back, saying that the capture is being
//! read, and Tesseract recognizes its English text off the UI thread, with a
//! limit; the snapshot and Tesseract's temporary files are deleted whatever
//! the outcome. The Tesseract is found by one rule in every channel: the
//! program named by the file `tesseract` of the configuration directory, else
//! `tesseract` beside the executable, else on `PATH`; the data directory named
//! by the file `tessdata` there, else `../share/tessdata` from the
//! executable's directory, else Tesseract's own. The text, as one line,
//! replaces the field, and the source is "screen capture (OCR)": it is a new
//! text, and from there everything is as for a typed one, with the whole text
//! as the sentence. It is not submitted:
//! a lone word is looked up and not sent online. An edit keeps the source;
//! a field without a word is manual input again. A capture that gives no text,
//! for any reason, leaves the field as it was and says "No text was
//! recognized." until the next state is shown. Capture cannot be pressed
//! while a capture runs.
//!
//! The shortcut reads the text selected or copied, the same for every
//! application: the window shows at once, empty and saying that the source,
//! "selection or clipboard", is being read; once the window is active, the
//! primary selection is read, then the clipboard when the selection gave no
//! line, both within a limit from the press. The line replaces the field, a
//! lone word is looked up, and nothing is sent online until a word is
//! clicked. A read that gives nothing leaves the field empty and says that
//! nothing is selected or copied. A new press, an edit of the field or a
//! capture gives up a read that runs.
//!
//! The application has an icon in the top bar, a StatusNotifierItem, with a
//! menu of Show, Settings, About and Exit, which a click on the icon opens.
//! Show shows the window as a repeated launch does, without a read. Settings
//! shows a window with the trigger of the shortcut, the file of the DeepL key
//! and the state it is in, never the key, and a button that asks the portal
//! for its dialog to change the shortcut; the line there says when it cannot.
//! About shows a window with the icon, the name, the version and the license
//! of the application, and a Credits page that lists every dictionary and OCR
//! source with its credit line. Exit ends the application. A session with no
//! StatusNotifierWatcher says so once on stderr and goes on without the icon;
//! the icon appears when a watcher comes, such as a shell extension that
//! starts after the application.
//!
//! The application is unique: a second launch exits at once and makes the
//! resident instance show its window as it is, without a read.
//!
//! An attempt to bind the shortcut that ends without one, for any reason or
//! for none within `resident::BIND_LIMIT`, shows the window by itself with a
//! line saying so and Retry. Retry starts a new attempt; a bound shortcut
//! removes the line.
//!
//! Before GTK starts, the resident lets the Vulkan loader list only the
//! driver of the GPU that drives the display, so that the first window does
//! not wake a runtime-suspended discrete GPU: when sysfs shows two cards or
//! more and exactly one is marked `boot_vga`, an Intel or AMD one, it sets
//! `VK_LOADER_DRIVERS_SELECT` to that vendor's drivers, and in any other
//! case it sets nothing. A variable already in the environment is kept. One
//! line in `vulkan-drivers` of the configuration directory, next to the
//! DeepL key, overrides the choice: `all` sets nothing, and any other line,
//! such as `nvidia_icd*`, is set as it is. The other commands do not touch
//! the variable.
//!
//! With the hidden `--evidence <FILE> --origin <APPLICATION>` the application
//! appends measurement evidence to the file: the start of the session and, for
//! each press, the activation, the presentation of the frame that follows it
//! and the focus; for each lookup that ended, its answer; and for each online
//! request that ended, of a word, of the sentence or of a definition, its
//! kind, its outcome, its delay and the memory of the process, and never its
//! text; for each capture that ended as shown, its outcome, the time to the
//! snapshot, the time and peak memory of the recognition, the memory of the
//! process and how many child processes it has, and never the text, the
//! snapshot or a failure's detail. The read of the selection and the
//! clipboard at a press is not recorded. Without the flag none of that
//! exists.
//!
//! This module owns the command line and what each form of it starts; the entry
//! commands are [`entries`]'s, the import is `addict_dictionary`'s, the indexes
//! as a source of articles are [`dictionaries`]'s, the online translator is
//! `addict_deepl`'s, what is asked of it and kept is [`online`]'s, where every
//! request to it starts is [`translate`]'s, the capture job is [`capture`]'s,
//! the Vulkan drivers the resident lets the loader list are [`gpu`]'s,
//! and its portal and Tesseract adapters are `addict_platform`'s, the
//! resident application is [`resident`]'s, the history of its field is
//! [`history`]'s, the Settings window is [`settings`]'s, the About window and
//! its credits are [`about`]'s, the texts of its window are [`view`]'s and the
//! widgets of the window are [`widgets`]'s.

mod about;
mod cache;
mod capture;
mod dictionaries;
mod entries;
mod evidence;
mod gpu;
mod history;
mod language;
mod manual;
mod online;
mod presentation;
mod recording;
mod resident;
mod settings;
mod style;
mod translate;
mod view;
mod widgets;
mod wrap;

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
    /// Write the desktop entries and the app icons: the portal recognises the
    /// application, the session starts it at login and the shell shows its icon
    Install,
    /// Remove the desktop entries and the icons that install wrote
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
        None => {
            gpu::select_vulkan_drivers();
            match open_evidence(cli.evidence.zip(cli.origin)) {
                Ok(writer) => resident::run(writer),
                Err(message) => fail(&message),
            }
        }
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
