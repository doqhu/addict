//! The resident application: one hidden window, the attempt to bind the
//! global shortcut and the loop that shows the window for each event.
//!
//! This module owns the GTK application, its uniqueness, the window with its
//! close and Esc handling, and the order of what happens on each event of the
//! shortcut worker, of Retry, of a repeated launch, of the field and of a
//! lookup, which runs on the work runner and comes back as an event. The
//! dictionary indexes are opened and read there too: where they stand is
//! asked once at the start and comes with the outcome of every lookup, and
//! the window says it whenever one is missing or unreadable.
//!
//! The module keeps the shown result with the sense chosen in it, and a
//! click on a sense only changes that choice.
//!
//! Only the explicit definition action sends the chosen definition or
//! English gloss without context, from the language it is in; a separate
//! job is cancelled on a changed sense or replacement article and guards
//! its outcome by request identity. A click on a word of a sentence also
//! asks online for the word in its sentence and, once for that sentence,
//! for the sentence itself; a typed text is one sentence,
//! while in a recognized one it is the sentence around the word, and a word
//! that is a sentence alone there asks nothing online; Enter in the field,
//! or a paste, asks for a lone word. Each request is a job of its own on the
//! runner, in one of two slots, the word's and the sentence's, and comes
//! back as an event with the number of its text: the article never waits
//! for it, a repeated click leaves it alone, another word gives up only the
//! word's, another sentence the sentence's too, and a new text gives up
//! both.
//!
//! Capture hides the window and starts one job on the runner, which takes
//! the screenshot, recognizes its text and deletes the snapshot; the window
//! comes back in the reading state once the snapshot is there, and the
//! recognized text, its lines joined by [`sentence::recognized`], then
//! replaces the field as a new text with the source OCR, which follows the
//! manual path from there. A capture without text leaves the field as it
//! was, starts no lookup and no online request, and says why in the source
//! line under the status; the line stays, also when the window is shown
//! again, until the field is edited, Capture or the shortcut is pressed or a
//! text is recognized. Capture cannot be pressed while the area is being chosen;
//! once the snapshot is there it can, and it is the retry: pressing it, or
//! editing the field, gives up the recognition that runs. Giving up is
//! dropping the job, which kills the Tesseract child and deletes the
//! snapshot, and shows nothing. Each capture job has a number, so what a
//! given-up job had already sent is ignored. Every capture end that is
//! shown, also one that could not start, is recorded with `--evidence` as it
//! is shown; a given-up one is not. The job is [`capture`](crate::capture)'s.
//!
//! The shortcut reads the text selected or copied, the same for every
//! application. The press gives up a running capture and the read of the
//! press before, and clears the field (the lookup and the online requests
//! of the old text are given up); the window is shown at once, in the
//! waiting state with the source "selection or clipboard", and nothing
//! waits for the read before the window is presented. Wayland gives a
//! client the clipboard only with its keyboard focus, so the read starts
//! once the window is active, at once when it is already. The primary
//! selection of the window's display, the text selected without a copy, is
//! read first, and GTK's clipboard only when the selection gives no line.
//! Waiting for the activity and both reads together end
//! [`CLIPBOARD_LIMIT`] after the press, which cancels the read. A text of
//! either as one line, of at most [`CLIPBOARD_MAX_CHARS`] characters,
//! replaces the field as a new text with the source clipboard, without a
//! word chosen unless it is one word, and nothing is asked online. A line
//! that is exactly a command the window offered to copy, such as the
//! install of Tesseract, is the user's Copy and counts as no line. When
//! neither gives a line (empty, blank, not text, too long, a command,
//! unreadable or late), why is written to stderr, the waiting ends, the
//! empty state stays with its focused field, and the source line says that
//! nothing is selected or copied, with the manual path; when one of them was
//! too long, the line says that instead, the selection's before the
//! clipboard's. The capture shares the line: they never run together. The
//! line goes, like the capture's, when the field is edited, Capture or the
//! shortcut is pressed.
//!
//! Each read has a number; a new press, an edit of the field and a capture
//! give up the read that runs, and whatever it reports later is ignored and
//! shows no line. Retry, a launch, Show of the tray and a capture read no
//! clipboard.
//!
//! The user reads in one language, chosen with the EN / ES / CA switch
//! beside the field; English at first. The online requests about the
//! field's text are sent from it, but for a word answered by another
//! language (below). English is looked up offline in its two
//! indexes. Spanish and Catalan are each looked up in their own indexes when
//! the last known state of the indexes says one of that language is there;
//! otherwise the result is the line that there is no offline dictionary for
//! the language, and no lookup runs. A lookup that runs tries the chosen
//! language first and, when it has no entry for the word, the other
//! languages in the order EN, ES, CA; the first that has the word gives the
//! article, and the window names it. The word's online requests, alone or
//! in its sentence, wait for that lookup and are sent from the language
//! that answered, or from the chosen one when none did; the sentence's
//! request is sent at once, from the chosen language. Neither the switch
//! nor the language file changes. With Spanish or Catalan chosen and its
//! indexes not known to be there, the indexes are checked again, so that an
//! import done while the application runs is found by the next pick. The
//! definition of an English sense is OEWN's and is sent from English; that
//! of a Spanish or Catalan sense from its language, and an English gloss
//! from English, each recorded with the source it was sent from. A capture
//! recognizes the text with the data of the language. A press of another
//! language forgets the online texts of the field's text and, for the word
//! shown, redoes its offline result and asks again what its click or its
//! submit asked; with no word nothing is sent. The choice is kept in the
//! language file, read once on the runner after the start and written on the runner at each change
//! ([`language`](crate::language)); a read that comes after the user chose
//! changes nothing.
//!
//! The session keeps the history of the field for as long as the process
//! lives, hiding the window included, and writes none of it: a submitted
//! text, by Enter, a paste or a synonym, and a text that replaces the
//! field, recognized or read from the selection or the clipboard, are recorded,
//! but not a typed one nor a click on a word of the sentence. Back and
//! Forward left of the field, and Alt+Left and Alt+Right wherever the focus
//! is, put the text before or after into the field as a paste does, so it
//! is looked up and submitted again; Back from a typed text records it
//! first. Ctrl+Left and Ctrl+Right stay the field's. The history itself is
//! [`history`](crate::history)'s.
//!
//! What is asked for and what is kept is [`online`](crate::online)'s. What
//! the window says is [`view`](crate::view)'s; its widgets and the paste
//! into its field are [`widgets`](crate::widgets)'s; what an edit or a click
//! does to the manual input is [`manual`](crate::manual)'s. How a request
//! starts and ends is [`translate`](crate::translate)'s; what is recorded
//! with `--evidence` is [`recording`](crate::recording)'s.

use std::{
    cell::{Cell, RefCell},
    process::ExitCode,
    rc::{Rc, Weak},
    sync::Arc,
    time::Duration,
};

use addict_core::{
    APP_ID,
    input::{Source, window_state},
    sentence,
    translation::{Request, Source as Language, Translator},
};
use addict_deepl::{DeepL, key};
use addict_platform::{
    ocr::OcrFailure,
    shortcut::{self, ShortcutEvent, ShortcutHandle},
    tray::{self, TrayEvent, TrayHandle},
    work::{Job, Outcome, Work},
};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_util::StreamExt;
use gtk::{gdk, gio, glib, prelude::*};

use crate::{
    about::About,
    cache::{self, Cached},
    capture::{self, Capture, Captured, Failure, Timing},
    dictionaries::{Articles, Dictionaries, Indexes, Looked},
    evidence,
    history::History,
    language,
    manual::{self, CLIPBOARD_MAX_CHARS, Edit, Manual, NoLine},
    online::{self, Kept, Subject},
    recording::Recording,
    settings::{self, CacheState, KeyState, Settings},
    style,
    translate::{self, Translated},
    view::{
        Binding, Choice, Held, Lookup, NOTHING_SELECTED_TEXT, Online, Shown, command_in,
        failure_text, too_long_text,
    },
    widgets::View,
};

/// Preferred trigger in the XDG shortcuts syntax; the user may change it.
const TRIGGER: &str = "LOGO+q";

/// How long one attempt to bind the shortcut may take.
const BIND_LIMIT: Duration = Duration::from_secs(60);

/// How long the reset of the stored binding may take.
const FORGET_LIMIT: Duration = Duration::from_secs(2);

/// How long the read of the selection and the clipboard may take from the
/// press, the wait for the window to be active included.
const CLIPBOARD_LIMIT: Duration = Duration::from_secs(1);

/// What lives as long as the resident application.
struct Resident {
    _hold: gio::ApplicationHoldGuard,
    /// The worker of the current attempt; Retry replaces it.
    _shortcut: Rc<RefCell<Option<ShortcutHandle>>>,
    /// Where a repeated launch reports itself.
    events: UnboundedSender<Event>,
    /// The runner of the lookups; `None` when it could not be started.
    _work: Option<Rc<Work>>,
    /// What the lookups ask.
    _articles: Arc<dyn Articles>,
    /// The icon in the top bar and its menu.
    _tray: TrayHandle,
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
    /// The text of the field changed; this is the new text.
    Sentence(String),
    /// The word with this index in the shown sentence was clicked.
    Word(usize),
    /// The sense with this index in the shown list was clicked.
    Sense(usize),
    /// The English gloss with this index in the shown article was clicked.
    Gloss(usize),
    /// The text of the field was submitted, by Enter or by a paste.
    Submitted,
    /// Back was pressed, or Alt+Left.
    Back,
    /// Forward was pressed, or Alt+Right.
    Forward,
    /// The user explicitly requested the selected dictionary definition.
    DefinitionAction,
    /// Add DeepL key… was pressed in the card: the Settings window is shown.
    AddKey,
    /// The English word of a gloss was clicked: it is looked up in English.
    LookUpEnglish(String),
    /// The separate definition job ended with its individual request number;
    /// it was sent from `source`. `elapsed` is the time the request took, as
    /// the worker saw it; taken only when evidence is recorded.
    DefinitionTranslated {
        number: u64,
        source: Language,
        outcome: Outcome<Translated>,
        elapsed: Option<evidence::Elapsed>,
    },
    /// The lookup started as number `run` ended, answered by `language`:
    /// the chosen one, or the one that had the word when the chosen one had
    /// no entry for it.
    Lookup {
        run: u64,
        language: Language,
        outcome: Outcome<Looked>,
    },
    /// The online request about `subject`, sent from `source` for the text
    /// with the number `text`, ended. `elapsed` is as for a definition.
    Translated {
        text: u64,
        subject: Subject,
        source: Language,
        number: u64,
        outcome: Outcome<Translated>,
        elapsed: Option<evidence::Elapsed>,
    },
    /// The language file, read on the runner after the start, holds this
    /// reading language.
    LanguageRead(Language),
    /// The user pressed the button of this language in the switch.
    LanguageChosen(Language),
    /// The check at the start says where the dictionary indexes stand.
    Indexes(Indexes),
    /// Capture was pressed.
    Capture,
    /// The window hidden for a capture is off the screen.
    Hidden,
    /// The capture job with this number has its snapshot and recognizes it
    /// now.
    Captured(u64),
    /// The capture job with this number ended.
    Recognized(u64, Outcome<Capture>),
    /// The window became active.
    Active,
    /// A choice of the tray's menu, or its end.
    Tray(TrayEvent),
    /// Change shortcut was pressed in the Settings window.
    ChangeShortcut,
    /// The reset of the stored binding, which a portal without a dialog of
    /// its own for a change needs, ended.
    BindingForgotten(Outcome<Result<(), String>>),
    /// The state of the key file, as the runner read it for the Settings
    /// window.
    KeyFile(KeyState),
    /// Save key, or Enter in the key field, was pressed in the Settings
    /// window with this text, trimmed and not empty. `Event` has no `Debug`,
    /// so the key is never printed.
    SaveKey(String),
    /// The write of the key file ended: why it failed, in one line that
    /// does not hold the key.
    KeySaved(Outcome<Result<(), String>>),
    /// The size of the cache, as the runner read it for the Settings window.
    CacheSize(CacheState),
    /// Clear cache was pressed in the Settings window.
    ClearCache,
    /// The clear of the cache on the runner ended: why it failed, in one
    /// line.
    CacheCleared(Outcome<Result<(), String>>),
    /// The read of the clipboard with this number ended: the line of its
    /// text, or why there is none.
    Clipboard(u64, Result<String, Missed>),
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

/// How long a hidden window takes to leave the screen: the shell's hide
/// animation and a frame without the window.
const HIDE_SETTLE: Duration = Duration::from_millis(300);

/// The width the window starts with, in pixels: a sentence of a dozen
/// words fits in the field beside the switch and Capture.
const WINDOW_WIDTH: i32 = 800;

/// The height the window starts with, in pixels: an article of a few
/// senses with one of them chosen fits without scrolling its list.
const WINDOW_HEIGHT: i32 = 680;

/// The header bar: `name` beside the accent mark of the application, and
/// the window's buttons.
pub(crate) fn header_bar(name: &str) -> gtk::HeaderBar {
    let mark = gtk::Box::builder()
        .valign(gtk::Align::Center)
        .css_classes(["brand-mark"])
        .build();
    let name = gtk::Label::builder()
        .label(name)
        .css_classes(["brand-name"])
        .build();
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    title.append(&mark);
    title.append(&name);
    gtk::HeaderBar::builder().title_widget(&title).build()
}

/// The hidden window around the `view`: closing it hides it, and so does Esc.
fn build_window(application: &gtk::Application, view: &View) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Addict")
        .default_width(WINDOW_WIDTH)
        .default_height(WINDOW_HEIGHT)
        .child(view.root())
        .css_classes(["addict"])
        .titlebar(&header_bar("Addict"))
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
    /// The text of the field, a word or a sentence, and what was chosen in
    /// it.
    manual: Manual,
    /// The texts the field held, for Back and Forward; kept for the life of
    /// the process, hidden window included.
    history: History,
    /// Where the lookup of the chosen word stands.
    looking_up: LookingUp,
    /// The result the window shows and the sense chosen in it.
    shown: Shown,
    /// Weak: the runner must go when the resident state is dropped. Never
    /// upgrades when the runner could not be started.
    work: Weak<Work>,
    articles: Arc<dyn Articles>,
    /// What a word or a sentence is translated by online.
    translator: Arc<dyn Translator>,
    key_file: Option<std::path::PathBuf>,
    /// The on-disk cache of the translator, for its size and its clear.
    cache: Arc<cache::Store>,
    /// The read of the size of the cache; dropping it gives it up.
    cache_job: Option<Job>,
    /// The clear of the cache; a newer clear replaces it.
    clear_job: Option<Job>,
    /// The Settings window, which the tray shows.
    settings: Settings,
    /// The About window, which the tray shows.
    about: About,
    /// The trigger the portal bound, for the Settings window.
    trigger: Option<String>,
    /// The read of the key file's state; dropping it gives it up.
    key_job: Option<Job>,
    /// The write of the key file; a newer save replaces it.
    save_job: Option<Job>,
    /// The reset of the stored binding; dropping it gives it up.
    forget_job: Option<Job>,
    /// The shortcut is being changed through the system's bind dialog: the
    /// line of Settings is the one for that until the bind ends.
    rebinding: bool,
    /// The online request of a word, in its sentence or alone, whose outcome
    /// is awaited; dropping it cancels the request.
    translating_word: Option<Job>,
    /// The same for the request of the sentence.
    translating_sentence: Option<Job>,
    /// Dropping this separate job cancels the selected definition request.
    translating_definition: Option<Job>,
    /// The online texts of the text the field holds.
    online: Kept,
    /// The check of the indexes at the start, or the last one a Spanish or
    /// Catalan word without its indexes asked for, which replaces any still running;
    /// dropping it cancels the check.
    check: Option<Job>,
    /// Where the indexes stood when last asked, by a check or a lookup;
    /// `None` until the first answer.
    indexes: Option<Indexes>,
    /// Where the capture stands.
    capturing: Capturing,
    /// Where the read of the selection and the clipboard of a press stands.
    copying: Copying,
    /// The language the user reads: the sentence requests are sent from
    /// it, and its indexes are looked up first.
    language: Language,
    /// Where the reading language is kept across restarts; `None` when the
    /// environment names no configuration directory.
    language_file: Option<std::path::PathBuf>,
    /// Whether the user chose the language in this session: the read of the
    /// file, should it come later, changes nothing then.
    language_chosen: bool,
    /// The read or the write of the language file on the runner.
    language_job: Option<Job>,
}

/// Where the capture of a [`Session`] stands; at first no capture runs.
#[derive(Default)]
struct Capturing {
    /// The capture job, from Capture to its end; dropping it cancels the
    /// job, which kills its Tesseract child and deletes its snapshot.
    job: Option<Job>,
    /// The number of the last started capture job. An event that carries
    /// another number, or comes when no job runs, is from a job that was
    /// given up.
    number: u64,
    /// Between Capture and the start of its job, while the window leaves
    /// the screen.
    hiding: bool,
    /// The language the last started capture job recognizes, which its
    /// line of missing data names.
    language: Language,
}

/// Where the lookup of the word chosen in a [`Session`] stands; at first
/// none runs.
#[derive(Default)]
struct LookingUp {
    /// The lookup whose outcome is awaited; dropping it cancels the lookup.
    job: Option<Job>,
    /// The language that gave the result of the last started lookup, once
    /// it ended with one; `None` before. The word's online requests are
    /// sent from it.
    answered: Option<Language>,
    /// The online requests of the word, alone or in its sentence, asked
    /// while its lookup runs: they are sent once it ends, from the language
    /// that answered.
    deferred: Vec<Subject>,
}

/// Where the read of the clipboard of a [`Session`] stands; at first none
/// runs.
#[derive(Default)]
struct Copying {
    /// The read, from the press to its end.
    read: Option<ClipboardRead>,
    /// The number of the last started read. An event that carries another
    /// number, or comes when none runs, is from a read that was given up.
    number: u64,
}

/// One read of the primary selection, then of the clipboard.
struct ClipboardRead {
    /// Cancels the read of the clipboard; `None` while the window is not yet
    /// active and nothing is read.
    cancellable: Option<gio::Cancellable>,
    /// Set when the selection was too long, with its length, so that a read
    /// of the clipboard that runs out of time still says so.
    selection: Rc<Cell<Option<(Held, usize)>>>,
}

impl Copying {
    /// A new read starts, and the one before ends; the number of the new
    /// one is given back.
    fn begin(&mut self) -> u64 {
        self.end();
        self.number += 1;
        self.read = Some(ClipboardRead {
            cancellable: None,
            selection: Rc::default(),
        });
        self.number
    }

    /// Whether the read with `number` is the one that runs.
    fn is_running(&self, number: u64) -> bool {
        self.read.is_some() && number == self.number
    }

    /// The read ends: one that reads is cancelled. The read that ran, if one
    /// did.
    fn end(&mut self) -> Option<ClipboardRead> {
        let read = self.read.take()?;
        if let Some(cancellable) = &read.cancellable {
            cancellable.cancel();
        }
        Some(read)
    }
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
            Event::Shortcut(ShortcutEvent::ConfigureEnded { message }, _) => {
                self.settings
                    .say(&settings::change_text(message.as_deref()));
            }
            Event::Tray(event) => self.tray(event),
            Event::Shortcut(ShortcutEvent::ConfigureUnsupported, _) => self.rebind(),
            Event::BindingForgotten(outcome) => self.binding_forgotten(outcome),
            Event::ChangeShortcut => self.change_shortcut(),
            Event::KeyFile(state) => self.settings.render_key(Some(state)),
            Event::SaveKey(text) => self.save_key(text),
            Event::KeySaved(outcome) => self.key_saved(outcome),
            Event::CacheSize(state) => self.settings.render_cache(state),
            Event::ClearCache => self.clear_cache(),
            Event::CacheCleared(outcome) => self.cache_cleared(outcome),
            Event::Launched => self.launched(),
            Event::Retry => self.retry(),
            Event::Sentence(text) => self.sentence_changed(text),
            Event::Word(index) => self.word_chosen(index),
            Event::Sense(index) => self.sense_chosen(index),
            Event::Gloss(index) => self.gloss_chosen(index),
            Event::Submitted => self.submitted(),
            Event::Back => self.back(),
            Event::Forward => self.forward(),
            Event::DefinitionAction => self.definition_action(),
            Event::AddKey => self.show_settings(),
            Event::LookUpEnglish(word) => self.look_up_english(&word),
            Event::DefinitionTranslated {
                number,
                source,
                outcome,
                elapsed,
            } => self.definition_translated(number, source, outcome, elapsed),
            Event::Lookup {
                run,
                language,
                outcome,
            } => self.looked_up(run, language, outcome),
            Event::Translated {
                text,
                subject,
                source,
                number,
                outcome,
                elapsed,
            } => self.translated(text, subject, source, number, outcome, elapsed),
            Event::LanguageRead(language) => {
                self.language_job = None;
                if !self.language_chosen {
                    self.apply_language(language);
                }
            }
            Event::LanguageChosen(language) => self.language_chosen(language),
            Event::Indexes(indexes) => {
                self.check = None;
                self.indexes = Some(indexes);
                self.view.render_indexes(indexes);
            }
            Event::Capture => self.capture(),
            Event::Hidden => self.start_capture(),
            Event::Captured(number) => self.snapshot_taken(number),
            Event::Recognized(number, outcome) => self.recognized(number, outcome),
            Event::Active => self.window_active(),
            Event::Clipboard(number, line) => self.copied(number, line),
        }
    }

    /// Capture was pressed: the window hides, so it is not in the capture,
    /// and Capture cannot be pressed again until the snapshot is there. The
    /// job starts once the hide has reached the screen. A press while a
    /// snapshot is being recognized gives that recognition up and starts a
    /// new capture; a press queued behind one whose area is being chosen
    /// starts nothing. The source line goes.
    fn capture(&mut self) {
        if self.capturing.hiding {
            return;
        }
        if self.capturing.job.is_some() {
            if self.manual.reading() != Some(Source::Ocr) {
                return;
            }
            self.give_up_capture();
        }
        // The capture's text is the one to come: a read that runs is given up.
        self.give_up_read();
        self.view.clear_source_failure();
        self.capturing.hiding = true;
        // Hidden, so on Wayland the compositor places the window anew when it
        // comes back. The opacity of a GTK 4 toplevel is ignored, and making
        // its children transparent left the window in GNOME's capture.
        self.window.set_visible(false);
        self.view.set_capture_sensitive(false);
        let hidden = self.sender.clone();
        // GNOME's capture interface freezes the screen as it opens, and the
        // shell animates a window that goes away: started at once, the
        // capture shows the window still there.
        glib::timeout_add_local_once(HIDE_SETTLE, move || {
            let _ = hidden.unbounded_send(Event::Hidden);
        });
    }

    /// The window is off the screen: the capture job starts.
    fn start_capture(&mut self) {
        if !self.capturing.hiding {
            return;
        }
        self.capturing.hiding = false;
        self.capturing.number += 1;
        self.capturing.language = self.language;
        let number = self.capturing.number;
        let captured = self.sender.clone();
        let ended = self.sender.clone();
        let started = capture::start(
            self.work.upgrade().as_deref(),
            self.capturing.language,
            move || {
                // On the worker: the UI thread wakes for the event.
                let _ = captured.unbounded_send(Event::Captured(number));
            },
            move |outcome| {
                let _ = ended.unbounded_send(Event::Recognized(number, outcome));
            },
        );
        match started {
            Ok(job) => self.capturing.job = Some(job),
            Err(capture) => self.capture_ended(capture.captured, &capture.timing),
        }
    }

    /// Whether the capture job with `number` is the one that runs.
    fn is_capturing(&self, number: u64) -> bool {
        self.capturing.job.is_some() && number == self.capturing.number
    }

    /// Gives up the capture job: dropping it kills its Tesseract child and
    /// deletes its snapshot; it shows nothing.
    fn give_up_capture(&mut self) {
        self.capturing.job = None;
        self.manual.set_reading(None);
    }

    /// The snapshot of the job with `number` is there and is being
    /// recognized: the window comes back and says so while there is no
    /// input, and Capture can be pressed again, as the retry.
    fn snapshot_taken(&mut self, number: u64) {
        if !self.is_capturing(number) {
            return;
        }
        self.manual.set_reading(Some(Source::Ocr));
        self.view.set_capture_sensitive(true);
        self.reveal();
    }

    /// The capture job with `number` ended; one that was given up shows
    /// nothing.
    fn recognized(&mut self, number: u64, outcome: Outcome<Capture>) {
        if !self.is_capturing(number) {
            return;
        }
        // A cancelled job was dropped here on purpose and shows nothing.
        let Some((captured, timing)) = capture::captured_result(outcome) else {
            return;
        };
        self.capture_ended(captured, &timing);
    }

    /// The capture is over with `captured`: Capture can be pressed again. A
    /// text replaces the field as a new text from the capture and the
    /// source line goes; anything else, a text without a word among it,
    /// leaves the field as it was, starts nothing and says why in the
    /// source line. The window is shown either way, the field focused. With
    /// `--evidence` the end is recorded as shown, with `timing`.
    fn capture_ended(&mut self, captured: Captured, timing: &Timing) {
        self.capturing.job = None;
        self.view.set_capture_sensitive(true);
        self.manual.set_reading(None);
        let line = match captured {
            Captured::Text(text) => {
                sentence::recognized(&text).ok_or(Failure::Ocr(OcrFailure::Empty))
            }
            Captured::Failed(failure) => Err(failure),
        };
        if let Some(recording) = &self.recording {
            let read = match &line {
                Ok(_) => evidence::Read::Recognized,
                Err(failure) => evidence::Read::failed(failure),
            };
            recording.ocr_ended(&evidence::ocr_record(read, timing));
        }
        let line = match line {
            Ok(line) => line,
            Err(failure) => {
                eprintln!("addict: no text from the capture: {failure:?}");
                self.reveal();
                self.view
                    .render_source_failure(&failure_text(&failure, self.capturing.language));
                return;
            }
        };
        self.view.clear_source_failure();
        let edit = self.manual.recognized(line);
        self.replace_field(edit);
        self.reveal();
    }

    /// Gives up the read of the selection and the clipboard: it is
    /// cancelled, and whatever it reports later is ignored. Whether a read
    /// ran.
    fn give_up_read(&mut self) -> bool {
        let copied = self.copying.end().is_some();
        if self.manual.reading() == Some(Source::Clipboard) {
            self.manual.set_reading(None);
        }
        copied
    }

    /// Clears the field for the read that starts now: a new empty text, so
    /// the lookup and the online requests of the old one are given up, and
    /// the source line goes.
    fn clear_for_read(&mut self) {
        self.view.clear_source_failure();
        let edit = self.manual.edit(String::new());
        self.view.set_field_text("");
        self.render_history();
        self.text_replaced(edit);
    }

    /// Starts the read of the primary selection and the clipboard as the
    /// next number: the window waits with the source clipboard, the read
    /// starts once the window is active, and after [`CLIPBOARD_LIMIT`] it
    /// ends without text.
    fn start_copy(&mut self) {
        let number = self.copying.begin();
        self.manual.set_reading(Some(Source::Clipboard));
        self.view.render(self.manual.state(), self.manual.source());
        let late = self.sender.clone();
        glib::timeout_add_local_once(CLIPBOARD_LIMIT, move || {
            let _ = late.unbounded_send(Event::Clipboard(
                number,
                Err(Missed::because(format!(
                    "no answer within {CLIPBOARD_LIMIT:?}"
                ))),
            ));
        });
        if self.window.is_active() {
            self.read_clipboard();
        }
    }

    /// The window became active: the read of the clipboard that waits for
    /// it starts.
    fn window_active(&mut self) {
        if self.window.is_active() {
            self.read_clipboard();
        }
    }

    /// Reads the primary selection of the window's display for the read that
    /// runs and still waits, and the clipboard when the selection gives no
    /// line; nothing else does. Both reads share its cancellable.
    fn read_clipboard(&mut self) {
        let number = self.copying.number;
        let Some(read) = self.copying.read.as_mut() else {
            return;
        };
        if read.cancellable.is_some() {
            return;
        }
        let cancellable = gio::Cancellable::new();
        read.cancellable = Some(cancellable.clone());
        let too_long = read.selection.clone();
        let sender = self.sender.clone();
        let display = WidgetExt::display(&self.window);
        let clipboard = display.clipboard();
        read_line(
            &display.primary_clipboard(),
            Held::Selection,
            &cancellable.clone(),
            move |primary| {
                let Err(selection) = primary else {
                    let _ = sender.unbounded_send(Event::Clipboard(number, primary));
                    return;
                };
                too_long.set(selection.too_long);
                read_line(&clipboard, Held::Clipboard, &cancellable, move |copied| {
                    let line = copied.map_err(|copied| Missed {
                        reason: format!(
                            "selection: {}; clipboard: {}",
                            selection.reason, copied.reason
                        ),
                        too_long: copied.too_long,
                    });
                    let _ = sender.unbounded_send(Event::Clipboard(number, line));
                });
            },
        );
    }

    /// The read of the primary selection and the clipboard with `number`
    /// ended; one that was given up shows nothing. A `line` replaces the
    /// field as a new text with the source clipboard; nothing is asked
    /// online. Without one, the reason is logged, the waiting ends and the
    /// source line says that a text was too long, when one was, or else that
    /// nothing is selected or copied.
    fn copied(&mut self, number: u64, line: Result<String, Missed>) {
        if !self.copying.is_running(number) {
            return;
        }
        let Some(read) = self.copying.end() else {
            return;
        };
        match line {
            Ok(line) => {
                let edit = self.manual.copied(&line);
                self.replace_field(edit);
            }
            Err(missed) => {
                eprintln!(
                    "addict: no text from the selection or the clipboard: {}",
                    missed.reason
                );
                if self.manual.reading() == Some(Source::Clipboard) {
                    self.manual.set_reading(None);
                }
                self.view.render(self.manual.state(), self.manual.source());
                let line = missed_line(read.selection.get(), missed.too_long);
                self.view.render_source_failure(&line);
            }
        }
    }

    /// Shows the window with the state as it is now, the field focused,
    /// without an activation token.
    fn reveal(&self) {
        self.view.render(self.manual.state(), self.manual.source());
        self.focus();
        self.window.present();
    }

    /// The field takes the keyboard focus, unless Retry is offered: then the
    /// focus is Retry's.
    fn focus(&self) {
        if !matches!(self.binding, Binding::NotBound { .. }) {
            self.view.focus_field();
        }
    }

    /// The field holds another text: what was chosen in the old one is gone,
    /// and so is every online text of it. A text of one word is that word
    /// without context, looked up at once; nothing of it is sent online.
    ///
    /// The source line goes. A snapshot being recognized and a read of the
    /// selection and the clipboard are given up: the user's text wins, and the state no
    /// longer says that a source is read.
    fn sentence_changed(&mut self, text: String) {
        self.view.clear_source_failure();
        let gave_up_capture =
            self.capturing.job.is_some() && self.manual.reading() == Some(Source::Ocr);
        if gave_up_capture {
            self.give_up_capture();
        }
        let gave_up_read = self.give_up_read();
        let edit = self.manual.edit(text);
        self.text_replaced(edit);
        // A typed text is not recorded, but Back can go from it.
        self.render_history();
        if gave_up_capture || gave_up_read {
            self.view.render(self.manual.state(), self.manual.source());
        }
    }

    /// A text that did not come from the field, recognized, selected or
    /// copied, replaced the manual input as `edit`: the
    /// field shows it, and what follows a new text follows.
    fn replace_field(&mut self, edit: Edit) {
        self.view.set_field_text(self.manual.sentence());
        // `Cleared` is any new text with no word chosen; an empty one is
        // not recorded.
        if edit != Edit::Same {
            self.history.record(self.manual.sentence());
        }
        self.render_history();
        self.text_replaced(edit);
    }

    /// Back or Alt+Left: the text before in the history is put into the
    /// field as a paste puts it, so it is looked up and submitted again. A
    /// typed text that is not in the history is recorded first, so Forward
    /// comes back to it.
    fn back(&mut self) {
        let current = self.view.field_text();
        let text = self.history.back(&current).map(str::to_owned);
        self.walked(text);
    }

    /// Forward or Alt+Right: the text after in the history is put into the
    /// field as Back puts one.
    fn forward(&mut self) {
        let text = self.history.forward().map(str::to_owned);
        self.walked(text);
    }

    /// Puts `text`, where Back or Forward led, into the field. Nothing is
    /// recorded: its submit records the text at the position, which changes
    /// nothing.
    fn walked(&mut self, text: Option<String>) {
        if let Some(text) = text {
            self.view.put_into_field(&text);
        }
        self.render_history();
    }

    /// Whether Back and Forward can be pressed, for the field's text now.
    fn render_history(&self) {
        self.view.set_history(
            self.history.can_back(&self.view.field_text()),
            self.history.can_forward(),
        );
    }

    /// What follows a new text in the field, typed, pasted or recognized:
    /// the online texts of the old one are given up, and a lone word is
    /// looked up at once.
    fn text_replaced(&mut self, edit: Edit) {
        if edit == Edit::Same {
            return;
        }
        // Cancels the online requests; whatever they still report has the
        // number of an old text.
        self.translating_word = None;
        self.looking_up.deferred.clear();
        self.translating_sentence = None;
        self.online = Kept::of(self.manual.text_number(), self.manual.kind());
        self.render_online();
        match edit {
            Edit::Same => {}
            Edit::Cleared => {
                // Cancels the lookup; whatever it still reports has an old
                // number.
                self.looking_up.job = None;
                self.render_manual();
                self.show_result(None);
            }
            Edit::Lone(run) => self.start_lookup(run),
        }
    }

    /// A word was clicked: it becomes the input, with its sentence when it
    /// has one, and is looked up. In a sentence, what is not yet known
    /// online about the word and about its sentence is asked for.
    fn word_chosen(&mut self, index: usize) {
        // A click that came before the field's newer text was handled here
        // names a word of a sentence that is no longer shown.
        if self.field_is_newer() {
            return;
        }
        let Some(run) = self.manual.choose(index) else {
            return;
        };
        self.start_lookup(run);
        // After the lookup: the runner takes its jobs in this order, so the
        // article is read before the key file and the network are touched.
        // A word that is a sentence alone in a recognized text has no
        // sentence: its article only, nothing sent online.
        let Some((number, span)) = self.manual.sentence_of(index) else {
            self.render_online();
            return;
        };
        let long = online::is_long(&self.manual.sentence()[span]);
        let click = self.online.click(index, number, long);
        if click.gave_up {
            self.translating_word = None;
            self.looking_up.deferred.clear();
        }
        if click.gave_up_sentence {
            self.translating_sentence = None;
        }
        for subject in click.ask {
            self.start_translation(subject);
        }
        self.render_online();
    }

    /// The text of the field was submitted, by Enter or by a paste: a lone
    /// word is asked for online, unless it was already. Nothing else
    /// changes, and an empty field or a sentence sends nothing.
    fn submitted(&mut self) {
        // As for a click: the field may hold a newer text already.
        if self.field_is_newer() {
            return;
        }
        self.history.record(self.manual.sentence());
        self.render_history();
        if self.online.submit() {
            self.start_translation(Subject::Lone);
            self.render_online();
        }
    }

    /// Whether the field holds a newer text than the one handled here: an
    /// event that came before that text was handled is about what is no
    /// longer shown.
    fn field_is_newer(&self) -> bool {
        self.view.field_text() != self.manual.sentence()
    }

    /// Shows the input that was just chosen and starts its lookup, which has
    /// the number `run`; for a reading language without an offline
    /// dictionary no lookup starts, and the line that says so is shown. The
    /// online texts are not touched.
    fn start_lookup(&mut self, run: u64) {
        // The lookup of another word, if it runs, is replaced.
        self.looking_up.job = None;
        self.looking_up.answered = None;
        self.render_manual();
        let Some(word) = self.manual.input().map(|input| input.word().to_owned()) else {
            self.show_result(None);
            return;
        };
        let sender = self.sender.clone();
        let language = self.language;
        let started = manual::offline(
            self.work.upgrade().as_deref(),
            self.articles.clone(),
            word,
            language,
            self.indexes,
            move |language, outcome| {
                // On the worker: the UI thread wakes for the event.
                let _ = sender.unbounded_send(Event::Lookup {
                    run,
                    language,
                    outcome,
                });
            },
        );
        match started {
            Ok(job) => {
                self.looking_up.job = Some(job);
                self.show_result(Some(Lookup::Pending));
            }
            Err(lookup) => {
                let checks = manual::checks_again(&lookup, language);
                self.show_result(Some(*lookup));
                // No lookup event comes: what waited for one goes now.
                self.send_deferred();
                if checks {
                    self.check = start_check(
                        self.work.upgrade().as_deref(),
                        self.articles.clone(),
                        &self.sender,
                    );
                }
            }
        }
    }

    /// A lookup ended, answered by `language`. Only the last started one is
    /// shown, with the line that names the language when it is not the
    /// chosen one, and the word's online requests asked while it ran are
    /// sent then, from that language; where it found the indexes is shown
    /// for every one that read them, and every one that was not given up is
    /// recorded.
    fn looked_up(&mut self, run: u64, language: Language, outcome: Outcome<Looked>) {
        if let Some(recording) = &self.recording
            && let Some(answered) = evidence::answered(&outcome)
        {
            recording.looked_up(answered);
        }
        if let Outcome::Finished(looked) = &outcome {
            self.indexes = Some(looked.indexes);
            self.view.render_indexes(looked.indexes);
        }
        if !self.manual.awaits(run) {
            return;
        }
        self.looking_up.job = None;
        let Some(input) = self.manual.input() else {
            return;
        };
        let finished = matches!(outcome, Outcome::Finished(_));
        if let Some(lookup) = manual::result_in(outcome, input.word(), language) {
            self.show_answered(Some(lookup), language);
        }
        // A lookup that did not finish names no language: the chosen one.
        self.looking_up.answered = finished.then_some(language);
        self.send_deferred();
    }

    /// Sends the word's online requests held while its lookup ran, now that
    /// no lookup runs, from the language that answered or the chosen one.
    fn send_deferred(&mut self) {
        let deferred = std::mem::take(&mut self.looking_up.deferred);
        if deferred.is_empty() {
            return;
        }
        for subject in deferred {
            self.start_translation(subject);
        }
        self.render_online();
    }

    /// Sends the online request about `subject` that [`Kept`] has just
    /// asked for, as a job in the slot of its subject. A request that could
    /// not start is a refusal at once. The sentence is sent from the chosen
    /// language at once. The word, alone or in its sentence, waits while its
    /// lookup runs, and is then sent from the language that answered, or
    /// from the chosen one when none did. Nothing is sent for a Russian word,
    /// neither the word nor its sentence ([`translate::is_sent`]).
    fn start_translation(&mut self, subject: Subject) {
        let text = self.manual.text_number();
        let Some(number) = self.online.request_number(subject) else {
            return;
        };
        // A Russian word is read offline only: nothing is sent for it or its
        // sentence, nothing is recorded, and its request is not awaited.
        if let Some(input) = self.manual.input()
            && !translate::is_sent(input, subject)
        {
            self.online.withdraw(subject);
            return;
        }
        let word = matches!(subject, Subject::Word(_) | Subject::Lone);
        if word && self.looking_up.job.is_some() {
            if !self.looking_up.deferred.contains(&subject) {
                self.looking_up.deferred.push(subject);
            }
            return;
        }
        let source = translate::source_of(subject, self.language, self.looking_up.answered);
        let request = self
            .manual
            .input()
            .and_then(|input| translate::request(input, subject, source));
        let started = self.send_online(request, subject.into(), move |outcome, elapsed| {
            Event::Translated {
                text,
                subject,
                source,
                number,
                outcome,
                elapsed,
            }
        });
        match started {
            // Replaces, and so cancels, the request the slot held.
            Ok(job) => *self.slot(subject) = Some(job),
            Err(online) => {
                self.online.answer(text, subject, number, online);
            }
        }
    }

    /// Starts the online `request` for `requested` as a job; every online
    /// request of the session starts here. When it ends, `event` makes, on
    /// the worker, what the UI thread is told from its outcome and the time
    /// it took. A request that could not start is recorded at once and its
    /// refusal is given back.
    fn send_online(
        &self,
        request: Option<Request>,
        requested: evidence::Requested,
        event: impl FnOnce(Outcome<Translated>, Option<evidence::Elapsed>) -> Event + Send + 'static,
    ) -> Result<Job, Online> {
        let sender = self.sender.clone();
        let probe = self.probe();
        // A request that is not there is recorded with the language it would
        // have been sent from.
        let source = request
            .as_ref()
            .map_or(self.language, |request| request.source);
        let started = translate::translate(
            self.work.upgrade().as_deref(),
            self.translator.clone(),
            request,
            move |outcome| {
                // On the worker: the UI thread wakes for the event, and the
                // delay is the request's, not the redraw's.
                let elapsed = probe.ended();
                let _ = sender.unbounded_send(event(outcome, elapsed));
            },
        );
        if let Err(online) = &started {
            self.record(translate::translation_unstarted(
                online, requested, source, probe,
            ));
        }
        started
    }

    /// What an ended online request for `requested`, sent from `source`,
    /// shows, after its record is written when there is one; `None` for a
    /// request that was given up: it shows nothing and is not recorded.
    fn online_ended(
        &self,
        outcome: Outcome<Translated>,
        requested: evidence::Requested,
        source: Language,
        elapsed: Option<evidence::Elapsed>,
    ) -> Option<Online> {
        let (online, record) = translate::translation_ended(outcome, requested, source, elapsed)?;
        self.record(record);
        Some(online)
    }

    /// The measuring of an online request that starts now; it measures
    /// nothing unless evidence is recorded.
    fn probe(&self) -> evidence::Probe {
        evidence::Probe::start(self.recording.is_some())
    }

    /// Writes the `record` of an online request, when there is one. Whether
    /// there is one is [`translate`]'s to say.
    fn record(&self, record: Option<evidence::Record>) {
        if let Some((recording, record)) = self.recording.as_ref().zip(record) {
            recording.translated_online(&record);
        }
    }

    /// The slot of the job that asks about `subject`. No wildcard arm: a new
    /// subject does not compile until it is given its slot.
    fn slot(&mut self, subject: Subject) -> &mut Option<Job> {
        match subject {
            Subject::Word(_) | Subject::Lone => &mut self.translating_word,
            Subject::Sentence(_) => &mut self.translating_sentence,
        }
    }

    /// An online request ended. Its outcome is kept and shown only when it
    /// belongs to the text the field holds and is still awaited; a cancelled
    /// one was given up and shows nothing. Every one that was not given up
    /// is recorded, shown or not.
    fn translated(
        &mut self,
        text: u64,
        subject: Subject,
        source: Language,
        number: u64,
        outcome: Outcome<Translated>,
        elapsed: Option<evidence::Elapsed>,
    ) {
        let Some(online) = self.online_ended(outcome, subject.into(), source, elapsed) else {
            return;
        };
        if self.online.answer(text, subject, number, online) {
            *self.slot(subject) = None;
            self.render_online();
        }
    }

    /// Shows the online texts as they are now, with those of the chosen
    /// word.
    fn render_online(&self) {
        self.view.render_online(
            &self.online.shown(self.manual.chosen()),
            self.key_file.as_deref(),
        );
    }

    /// The one place the shown result changes: the sense chosen in the one
    /// before is forgotten.
    fn show_result(&mut self, lookup: Option<Lookup>) {
        self.show_answered(lookup, self.language);
    }

    /// As [`Session::show_result`], for a result that the language `found`
    /// gave: when it is not the chosen one, an article says so.
    fn show_answered(&mut self, lookup: Option<Lookup>, found: Language) {
        self.translating_definition = None;
        self.shown.show_elsewhere(lookup, self.language, found);
        self.render_shown();
    }

    /// Shows the result and the sense chosen in it as they are now.
    fn render_shown(&self) {
        self.view
            .render_result(&self.shown, self.key_file.as_deref());
    }

    /// A sense was clicked: it is the chosen one when the shown result is an
    /// article that has it. Nothing is looked up or read. A click on the
    /// chosen sense renders too: GTK keeps a clicked link in the visited
    /// colour until the markup is set again.
    fn sense_chosen(&mut self, index: usize) {
        let previous = self.shown.choice();
        if self.shown.choose(index) {
            self.chosen(previous);
        }
    }

    /// An English gloss was clicked: it is the chosen one, instead of any
    /// sense, when the shown result is an article of a language beside
    /// English that has it. Nothing is looked up or read.
    fn gloss_chosen(&mut self, index: usize) {
        let previous = self.shown.choice();
        if self.shown.choose_gloss(index) {
            self.chosen(previous);
            // A gloss is translated as it is chosen, with no button: once,
            // and again only after a refusal.
            self.definition_action();
        }
    }

    /// The English word of a gloss was clicked: it goes into the field as a
    /// paste puts it, and English becomes the reading language. The field
    /// is newer than the shown text then, so the language change asks
    /// nothing for the old one; the field's own events look the word up.
    fn look_up_english(&mut self, word: &str) {
        self.view.put_into_field(word);
        self.language_chosen(Language::En);
    }

    /// Something was chosen where `previous` was: the job translating the
    /// definition of another choice is cancelled, and the result is shown
    /// anew.
    fn chosen(&mut self, previous: Option<Choice>) {
        if previous != self.shown.choice() {
            self.translating_definition = None;
        }
        self.render_shown();
    }

    /// "Translate definition into Russian" was pressed: the definition of
    /// the chosen sense or gloss is sent alone, from its own language, as a
    /// job of its own, unless the field
    /// holds a newer text or the shown result offers no definition to send.
    fn definition_action(&mut self) {
        // As for a click: the field may hold a newer text already.
        if self.field_is_newer() {
            return;
        }
        let Some((number, request)) = self.shown.request_definition() else {
            return;
        };
        let source = request.source;
        let started = self.send_online(
            Some(request),
            evidence::Requested::Definition,
            move |outcome, elapsed| Event::DefinitionTranslated {
                number,
                source,
                outcome,
                elapsed,
            },
        );
        match started {
            Ok(job) => self.translating_definition = Some(job),
            Err(online) => {
                self.shown.answer_definition(number, online);
            }
        }
        self.render_shown();
    }

    /// The definition request, sent from `source`, ended; it is recorded as
    /// a word's is, with that source.
    fn definition_translated(
        &mut self,
        number: u64,
        source: Language,
        outcome: Outcome<Translated>,
        elapsed: Option<evidence::Elapsed>,
    ) {
        let Some(online) =
            self.online_ended(outcome, evidence::Requested::Definition, source, elapsed)
        else {
            return;
        };
        if self.shown.answer_definition(number, online) {
            self.translating_definition = None;
            self.render_shown();
        }
    }

    /// The user pressed the button of `language` in the switch: it is the
    /// reading language from now on, kept in the language file by a write on
    /// the runner, and what the window shows of the current word is asked
    /// anew in it.
    fn language_chosen(&mut self, language: Language) {
        self.language_chosen = true;
        if language == self.language {
            return;
        }
        self.apply_language(language);
        self.save_language();
    }

    /// Makes `language` the reading language and the switch shows it. The
    /// word shown, if there is one, has its offline result redone and its
    /// online texts forgotten and asked again, as its click or its submit
    /// asked them: a word of a sentence with its sentence, a lone word only
    /// when it was submitted. Without a word nothing is sent.
    fn apply_language(&mut self, language: Language) {
        self.view.set_language(language);
        if language == self.language {
            return;
        }
        self.language = language;
        // The field holds a newer text, whose own event comes next and is
        // handled in the new language.
        if self.field_is_newer() {
            return;
        }
        // Cancels the online requests; whatever they still report has a
        // number that is no longer awaited.
        self.translating_word = None;
        self.looking_up.deferred.clear();
        self.translating_sentence = None;
        let submitted = self.online.forget();
        if let Some(run) = self.manual.again() {
            self.start_lookup(run);
            let chosen = self.manual.chosen().and_then(|index| {
                self.manual
                    .sentence_of(index)
                    .map(|sentence| (index, sentence))
            });
            if let Some((index, (number, span))) = chosen {
                let long = online::is_long(&self.manual.sentence()[span]);
                for subject in self.online.click(index, number, long).ask {
                    self.start_translation(subject);
                }
            } else if submitted && self.online.submit() {
                self.start_translation(Subject::Lone);
            }
        }
        self.render_online();
    }

    /// Writes the reading language to its file on the runner. A write that
    /// fails is written to stderr and nothing else: the choice holds until
    /// the application ends.
    fn save_language(&mut self) {
        let path = self.language_file.clone();
        let language = self.language;
        self.language_job = self.work.upgrade().map(|work| {
            work.run(
                language::FILE_LIMIT,
                async move { language::write(path.as_deref(), language) },
                |outcome| match outcome {
                    // Replaced by a newer choice, which is written instead.
                    Outcome::Finished(Ok(())) | Outcome::Cancelled => {}
                    Outcome::Finished(Err(e)) => {
                        eprintln!("addict: the reading language was not kept: {e}");
                    }
                    Outcome::TimedOut | Outcome::Panicked => {
                        eprintln!("addict: the reading language was not kept");
                    }
                },
            )
        });
        if self.language_job.is_none() {
            eprintln!("addict: the reading language was not kept: no runner");
        }
    }

    /// Shows the state, the sentence and the chosen word as they are now.
    fn render_manual(&self) {
        self.view.render(self.manual.state(), self.manual.source());
        self.view.render_sentence(
            self.manual.sentence(),
            self.manual.words(),
            self.manual.chosen(),
        );
    }

    fn bound(&mut self, trigger: &str) {
        eprintln!("addict: shortcut bound to {trigger}");
        self.binding = Binding::Bound;
        self.trigger = Some(trigger.to_owned());
        self.view.render_binding(&self.binding);
        self.settings
            .render_binding(&self.binding, self.trigger.as_deref());
        self.end_rebinding();
    }

    /// The bind ended, one way or the other: the line for the change through
    /// the system's dialog goes. A line about a failed reset stays.
    fn end_rebinding(&mut self) {
        if std::mem::take(&mut self.rebinding) {
            self.settings.say("");
        }
    }

    /// The shortcut was pressed: a running capture and read are given up,
    /// the field is cleared, the read of the selection and the clipboard
    /// starts and the window is shown at once in the waiting state with the
    /// source clipboard; the window does not wait for the read, which ends
    /// at most [`CLIPBOARD_LIMIT`] after the press.
    fn activated(
        &mut self,
        activation_token: Option<&str>,
        timestamp_ms: u64,
        receipt_ns: Option<u64>,
    ) {
        self.give_up_press();
        self.clear_for_read();
        // Started, not awaited: the window is presented below.
        self.start_copy();
        // Before `present()`: the first frame has the labels.
        self.view.render(self.manual.state(), self.manual.source());
        self.focus();
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

    /// Gives up what a press of the shortcut replaces: a running capture,
    /// also one that is still hiding the window, and the previous read.
    fn give_up_press(&mut self) {
        self.capturing.job = None;
        if self.capturing.hiding {
            // The hide of the window ends in an event that starts nothing.
            self.capturing.hiding = false;
        }
        self.view.set_capture_sensitive(true);
        self.give_up_read();
    }

    /// The attempt ended without a shortcut: the window shows itself to say so.
    fn failed(&mut self, message: String) {
        eprintln!("addict: shortcut unavailable: {message}");
        self.binding = Binding::NotBound { detail: message };
        self.trigger = None;
        self.view.render_binding(&self.binding);
        self.settings.render_binding(&self.binding, None);
        self.end_rebinding();
        // No press stands behind this, so there is no token.
        if let Some(recording) = &self.recording {
            recording.shown_without_press();
        }
        self.window.present();
    }

    fn launched(&mut self) {
        self.view.render(self.manual.state(), self.manual.source());
        self.focus();
        // No `set_startup_id`: GTK has already applied the
        // token of the launcher, when there was one.
        if let Some(recording) = &self.recording {
            recording.shown_without_press();
        }
        self.window.present();
    }

    /// A choice of the tray. Show is a repeated launch: the window as it is,
    /// no read. Settings reads the state of the key file on the runner and
    /// shows the window; About shows its window on its About page; Exit ends
    /// the application through its shutdown. A tray that cannot be had is
    /// said once and changes nothing.
    fn tray(&mut self, event: TrayEvent) {
        match event {
            TrayEvent::Show => self.launched(),
            TrayEvent::Settings => self.show_settings(),
            TrayEvent::About => self.about.present(),
            TrayEvent::Exit => {
                if let Some(application) = self.window.application() {
                    application.quit();
                }
            }
            TrayEvent::Unavailable { message } => {
                eprintln!("addict: no tray icon: {message}");
            }
        }
    }

    fn show_settings(&mut self) {
        self.settings
            .render_binding(&self.binding, self.trigger.as_deref());
        self.settings.say("");
        self.settings.clear_key_entry();
        self.settings.say_key("");
        self.settings.say_cache("");
        self.check_key();
        self.check_cache();
        self.settings.present();
    }

    /// Reads the size of the cache on the runner, for the Settings window; a
    /// read still running is given up.
    fn check_cache(&mut self) {
        self.settings.render_cache(CacheState::Checking);
        let sender = self.sender.clone();
        let cache = self.cache.clone();
        self.cache_job = self.work.upgrade().map(|work| {
            work.run(
                settings::CACHE_LIMIT,
                async move { cache.footprint() },
                move |outcome| {
                    let state = match outcome {
                        Outcome::Finished(footprint) => CacheState::Read(footprint),
                        Outcome::TimedOut | Outcome::Panicked => CacheState::Unknown,
                        // Replaced by the next press of Settings or clear.
                        Outcome::Cancelled => return,
                    };
                    let _ = sender.unbounded_send(Event::CacheSize(state));
                },
            )
        });
        if self.cache_job.is_none() {
            self.settings.render_cache(CacheState::Unknown);
        }
    }

    /// Clear cache was pressed: every row is deleted on the runner. A clear
    /// still running is replaced.
    fn clear_cache(&mut self) {
        let Some(work) = self.work.upgrade() else {
            self.settings
                .say_cache(&settings::clear_text(Err("the runner is not available")));
            return;
        };
        self.settings.say_cache("");
        self.settings.render_cache(CacheState::Checking);
        let sender = self.sender.clone();
        let cache = self.cache.clone();
        self.clear_job = Some(work.run(
            settings::CACHE_LIMIT,
            async move { cache.clear() },
            move |outcome| {
                let _ = sender.unbounded_send(Event::CacheCleared(outcome));
            },
        ));
    }

    /// The clear of the cache ended: the line says why when it failed, and
    /// the size is read again either way. The job slot is left alone, as for
    /// a save of the key.
    fn cache_cleared(&mut self, outcome: Outcome<Result<(), String>>) {
        let result = match outcome {
            Outcome::Finished(result) => result,
            Outcome::TimedOut => Err("the clear did not answer in time".to_owned()),
            Outcome::Panicked => Err("the clear did not run".to_owned()),
            // Replaced by a newer clear, whose end is awaited.
            Outcome::Cancelled => return,
        };
        self.settings.say_cache(&settings::clear_text(
            result.as_ref().map(|_| ()).map_err(String::as_str),
        ));
        self.check_cache();
    }

    /// Reads the state of the key file on the runner, for the Settings
    /// window; a read still running is given up.
    fn check_key(&mut self) {
        self.settings.render_key(None);
        let sender = self.sender.clone();
        let path = self.key_file.clone();
        self.key_job = self.work.upgrade().map(|work| {
            work.run(
                settings::KEY_CHECK_LIMIT,
                async move { KeyState::of(path.as_deref()) },
                move |outcome| {
                    let state = match outcome {
                        Outcome::Finished(state) => state,
                        Outcome::TimedOut | Outcome::Panicked => KeyState::Unknown,
                        // Replaced by the next press of Settings or save.
                        Outcome::Cancelled => return,
                    };
                    let _ = sender.unbounded_send(Event::KeyFile(state));
                },
            )
        });
        if self.key_job.is_none() {
            self.settings.render_key(Some(KeyState::Unknown));
        }
    }

    /// Save key was pressed: the key file is written on the runner. A save
    /// still running is replaced. DeepL is not asked whether the key works.
    fn save_key(&mut self, text: String) {
        let Some(path) = self.key_file.clone() else {
            self.settings
                .say_key(&settings::save_text(Err(settings::NO_KEY_PLACE)));
            return;
        };
        let Some(work) = self.work.upgrade() else {
            self.settings
                .say_key(&settings::save_text(Err("the runner is not available")));
            return;
        };
        self.settings.say_key("Saving the key…");
        let sender = self.sender.clone();
        self.save_job = Some(work.run(
            settings::KEY_CHECK_LIMIT,
            async move { key::write(&path, &text).map_err(|error| error.to_string()) },
            move |outcome| {
                let _ = sender.unbounded_send(Event::KeySaved(outcome));
            },
        ));
    }

    /// The write of the key file ended: the field is emptied when it
    /// worked, the line says why when it did not, and the state of the file
    /// is read again either way. The job slot is left alone: only a newer
    /// save replaces it, and clearing it would cancel that one.
    fn key_saved(&mut self, outcome: Outcome<Result<(), String>>) {
        let result = match outcome {
            Outcome::Finished(result) => result,
            // The write cannot be stopped, so it may have happened.
            Outcome::TimedOut => {
                self.settings.say_key(settings::SAVE_UNANSWERED);
                self.check_key();
                return;
            }
            Outcome::Panicked => Err("the write did not run".to_owned()),
            // Replaced by a newer save, whose end is awaited.
            Outcome::Cancelled => return,
        };
        if result.is_ok() {
            self.settings.clear_key_entry();
        }
        self.settings.say_key(&settings::save_text(
            result.as_ref().map(|_| ()).map_err(String::as_str),
        ));
        self.check_key();
    }

    /// Change shortcut was pressed: the portal is asked for its dialog
    /// through the worker that holds the session.
    fn change_shortcut(&mut self) {
        if !matches!(self.binding, Binding::Bound) {
            self.settings.say(settings::CHANGE_UNBOUND);
            return;
        }
        if let Some(current) = self.current.upgrade()
            && let Some(handle) = current.borrow().as_ref()
        {
            handle.configure();
        }
    }

    /// The portal has no dialog for a change: the session is closed, then
    /// the stored binding is reset, so that the bind that follows shows the
    /// system's dialog. The session is closed first, or the old binding is
    /// held.
    fn rebind(&mut self) {
        if !matches!(self.binding, Binding::Bound) {
            return;
        }
        let (Some(current), Some(work)) = (self.current.upgrade(), self.work.upgrade()) else {
            self.settings
                .say(&settings::change_text(Some("the runner is not available")));
            return;
        };
        current.borrow_mut().take();
        self.binding = Binding::Attempting;
        self.trigger = None;
        self.view.render_binding(&self.binding);
        self.settings.render_binding(&self.binding, None);
        self.settings.say(settings::REBINDING);
        self.rebinding = true;
        let sender = self.sender.clone();
        self.forget_job = Some(work.run(
            FORGET_LIMIT,
            async { shortcut::forget_binding(APP_ID) },
            move |outcome| {
                let _ = sender.unbounded_send(Event::BindingForgotten(outcome));
            },
        ));
    }

    /// The reset ended: the bind starts again. When the reset failed, the
    /// binding is still stored, so the bind gives it back without a dialog,
    /// and the line says why nothing changed.
    fn binding_forgotten(&mut self, outcome: Outcome<Result<(), String>>) {
        self.forget_job = None;
        if let Some(failure) = reset_failure(outcome) {
            self.rebinding = false;
            self.settings.say(&settings::change_text(Some(&failure)));
        }
        let Some(current) = self.current.upgrade() else {
            return;
        };
        current.borrow_mut().take();
        *current.borrow_mut() = Some(start_shortcut(&self.sender, self.timed));
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
        self.settings.render_binding(&self.binding, None);
        // `Failed` is the last act of the old worker, sent
        // after its session was closed: joining it does not
        // wait.
        current.borrow_mut().take();
        *current.borrow_mut() = Some(start_shortcut(&self.sender, self.timed));
    }
}

/// Why the reset of the stored binding did not happen, in one line; `None`
/// when it did.
fn reset_failure(outcome: Outcome<Result<(), String>>) -> Option<String> {
    match outcome {
        Outcome::Finished(Ok(())) => None,
        Outcome::Finished(Err(message)) => Some(message),
        Outcome::TimedOut => Some(format!("no reset within {FORGET_LIMIT:?}")),
        Outcome::Cancelled | Outcome::Panicked => Some("the reset did not run".to_owned()),
    }
}

/// Why the selection and the clipboard gave no line: the reason for stderr,
/// and where a text too long to take was held, with its length.
#[derive(Debug)]
struct Missed {
    reason: String,
    too_long: Option<(Held, usize)>,
}

impl Missed {
    /// Missed for `reason`, with no text too long.
    fn because(reason: String) -> Self {
        Self {
            reason,
            too_long: None,
        }
    }
}

/// Whether `line` is exactly a command the window offers to copy: such a
/// line is the user's Copy, not a text to look up.
fn is_offered_command(line: &str) -> bool {
    command_in(line).as_deref() == Some(line)
}

/// The source line when neither the selection nor the clipboard gave a
/// line: that the selection was too long, when it was, else that the
/// clipboard was, else that nothing is selected or copied.
fn missed_line(selection: Option<(Held, usize)>, clipboard: Option<(Held, usize)>) -> String {
    selection
        .or(clipboard)
        .map_or(NOTHING_SELECTED_TEXT.to_owned(), |(held, chars)| {
            too_long_text(held, chars, CLIPBOARD_MAX_CHARS)
        })
}

/// Reads the text of `clipboard`, which holds what is `held`, once and
/// gives `done` its line, of at most [`CLIPBOARD_MAX_CHARS`] characters, or
/// why there is none.
fn read_line(
    clipboard: &gdk::Clipboard,
    held: Held,
    cancellable: &gio::Cancellable,
    done: impl FnOnce(Result<String, Missed>) + 'static,
) {
    clipboard.read_text_async(Some(cancellable), move |read| {
        done(match read {
            Ok(Some(text)) => manual::clipboard_line(&text)
                .map_err(|no_line| match no_line {
                    NoLine::Blank => Missed::because("blank".to_owned()),
                    NoLine::TooLong(chars) => Missed {
                        reason: format!("{chars} characters, over {CLIPBOARD_MAX_CHARS}"),
                        too_long: Some((held, chars)),
                    },
                })
                .and_then(|line| {
                    if is_offered_command(&line) {
                        Err(Missed::because(
                            "a command the window offered to copy".to_owned(),
                        ))
                    } else {
                        Ok(line)
                    }
                }),
            Ok(None) => Err(Missed::because("no text".to_owned())),
            Err(e) => Err(Missed::because(e.to_string())),
        });
    });
}

/// Sends what the user does in the `view` to `sender`: Retry, an edit of the
/// field, a submit of its text, Back and Forward, Capture, a click on a word
/// and a click on a sense.
fn connect_view(view: &View, sender: &UnboundedSender<Event>) {
    let retry = sender.clone();
    view.connect_retry(move || {
        let _ = retry.unbounded_send(Event::Retry);
    });
    let changed = sender.clone();
    view.connect_sentence_changed(move |text| {
        let _ = changed.unbounded_send(Event::Sentence(text.to_owned()));
    });
    let submitted = sender.clone();
    view.connect_submitted(move || {
        let _ = submitted.unbounded_send(Event::Submitted);
    });
    let back = sender.clone();
    view.connect_back(move || {
        let _ = back.unbounded_send(Event::Back);
    });
    let forward = sender.clone();
    view.connect_forward(move || {
        let _ = forward.unbounded_send(Event::Forward);
    });
    let chosen = sender.clone();
    view.connect_word_chosen(move |index| {
        let _ = chosen.unbounded_send(Event::Word(index));
    });
    let capture = sender.clone();
    view.connect_capture(move || {
        let _ = capture.unbounded_send(Event::Capture);
    });
    let definition = sender.clone();
    view.connect_definition_action(move || {
        let _ = definition.unbounded_send(Event::DefinitionAction);
    });
    let english = sender.clone();
    view.connect_look_up_english(move |word| {
        let _ = english.unbounded_send(Event::LookUpEnglish(word.to_owned()));
    });
    let add_key = sender.clone();
    view.connect_add_key(move || {
        let _ = add_key.unbounded_send(Event::AddKey);
    });
    let sense = sender.clone();
    view.connect_sense_chosen(move |index| {
        let _ = sense.unbounded_send(Event::Sense(index));
    });
    let gloss = sender.clone();
    view.connect_gloss_chosen(move |index| {
        let _ = gloss.unbounded_send(Event::Gloss(index));
    });
    let language = sender.clone();
    view.connect_language_chosen(move |chosen| {
        let _ = language.unbounded_send(Event::LanguageChosen(chosen));
    });
}

/// The style, the view and the hidden window around it, for `binding`: what
/// the user does in the view goes to `sender`, and so does the window
/// becoming active.
fn build_ui(
    application: &gtk::Application,
    binding: &Binding,
    sender: &UnboundedSender<Event>,
) -> (View, gtk::ApplicationWindow) {
    style::install();
    let view = View::new(window_state(None, None), binding);
    connect_view(&view, sender);
    let window = build_window(application, &view);
    // Capture phase: Alt+Left and Alt+Right are Back and Forward wherever
    // the focus is.
    window.add_controller(view.history_keys());
    let active = sender.clone();
    window.connect_is_active_notify(move |window| {
        if window.is_active() {
            let _ = active.unbounded_send(Event::Active);
        }
    });
    (view, window)
}

/// The Settings window above `window`, hidden, naming `key_file`; Change
/// shortcut and Save key go to `sender` as events.
fn start_settings(
    window: &gtk::ApplicationWindow,
    key_file: Option<&std::path::Path>,
    sender: &UnboundedSender<Event>,
) -> Settings {
    let change = sender.clone();
    let save = sender.clone();
    let clear = sender.clone();
    Settings::new(
        window,
        key_file,
        move || {
            let _ = change.unbounded_send(Event::ChangeShortcut);
        },
        move |text| {
            let _ = save.unbounded_send(Event::SaveKey(text));
        },
        move || {
            let _ = clear.unbounded_send(Event::ClearCache);
        },
    )
}

/// Starts the icon in the top bar; the choices of its menu and the tray
/// being unavailable go to `sender` as events.
fn start_tray(sender: &UnboundedSender<Event>) -> TrayHandle {
    let told = sender.clone();
    tray::start(APP_ID, move |event| {
        // On the worker: the UI thread wakes for the event.
        let _ = told.unbounded_send(Event::Tray(event));
    })
}

/// Starts the runner of the lookups. Without it the window still works; a
/// click says that the lookup did not finish.
fn start_work() -> Option<Rc<Work>> {
    match Work::start() {
        Ok(work) => Some(Rc::new(work)),
        Err(e) => {
            eprintln!("addict: lookups cannot run: {e}");
            None
        }
    }
}

/// Starts the check of the indexes in `articles` on `work`; where they stand
/// comes to `sender` as an event.
fn start_check(
    work: Option<&Work>,
    articles: Arc<dyn Articles>,
    sender: &UnboundedSender<Event>,
) -> Option<Job> {
    let checked = sender.clone();
    manual::check_indexes(work, articles, move |indexes| {
        // On the worker: the UI thread wakes for the event.
        let _ = checked.unbounded_send(Event::Indexes(indexes));
    })
}

/// Starts the one read of the language file at `path` on `work`; the
/// language it holds, English for anything else, comes to `sender` as an
/// event. Without a runner nothing is read, and English stays.
fn start_language(
    work: Option<&Work>,
    path: Option<std::path::PathBuf>,
    sender: &UnboundedSender<Event>,
) -> Option<Job> {
    let read = sender.clone();
    let job = work?.run(
        language::FILE_LIMIT,
        async move { language::read(path.as_deref()) },
        move |outcome| {
            if let Outcome::Finished(language) = outcome {
                // On the worker: the UI thread wakes for the event.
                let _ = read.unbounded_send(Event::LanguageRead(language));
            }
        },
    );
    Some(job)
}

/// The online translator, DeepL behind the on-disk cache, the key file it
/// reads, for the window to name, and the cache, for its size and its clear.
/// Nothing is read, opened or connected here: the key file, the cache and the
/// network are the runner's, at the first click on a word of a sentence or
/// the first submit of a lone word.
fn online_translator() -> (
    Arc<dyn Translator>,
    Option<std::path::PathBuf>,
    Arc<cache::Store>,
) {
    let deepl = DeepL::new();
    let key_file = deepl.key_file().map(std::path::Path::to_owned);
    let translator = Cached::new(deepl, cache::cache_dir());
    let store = translator.store();
    (Arc::new(translator), key_file, store)
}

/// Hands each event of `receiver` to `session`, on the UI thread, for as
/// long as the channel is open.
fn serve(mut session: Session, mut receiver: UnboundedReceiver<Event>) {
    glib::spawn_future_local(async move {
        while let Some(event) = receiver.next().await {
            session.handle(event);
        }
    });
}

/// Builds the hidden window, starts the first attempt to bind the shortcut
/// and the event loop. Runs once, in the primary instance.
fn start(
    application: &gtk::Application,
    evidence: Option<Rc<RefCell<evidence::Writer>>>,
) -> Resident {
    // The worker threads push; the main context wakes only for an event.
    let (sender, receiver) = futures_channel::mpsc::unbounded();

    // The first attempt starts below, with the window still hidden.
    let binding = Binding::Attempting;
    let (view, window) = build_ui(application, &binding, &sender);

    // Only the primary instance gets here, so a second launch records no
    // session.
    let recording = evidence.map(|writer| Recording::start(writer, &window));
    let timed = recording.is_some();
    let handle = Rc::new(RefCell::new(Some(start_shortcut(&sender, timed))));
    let events = sender.clone();
    let work = start_work();
    // Nothing is opened here: the indexes are files, and files are the
    // runner's.
    let articles: Arc<dyn Articles> = Arc::new(Dictionaries::in_data_dir());
    let check = start_check(work.as_deref(), articles.clone(), &sender);
    // After the check: the runner takes its jobs in this order. Only the
    // path is made here; the file is the runner's.
    let language_file = language::path();
    let language_job = start_language(work.as_deref(), language_file.clone(), &sender);
    let (translator, key_file, cache) = online_translator();
    let settings = start_settings(&window, key_file.as_deref(), &sender);
    let about = About::new(&window);
    let session = Session {
        binding,
        view,
        window,
        recording,
        timed,
        sender,
        current: Rc::downgrade(&handle),
        manual: Manual::default(),
        history: History::default(),
        looking_up: LookingUp::default(),
        shown: Shown::default(),
        work: work.as_ref().map(Rc::downgrade).unwrap_or_default(),
        articles: articles.clone(),
        translator,
        key_file,
        cache,
        cache_job: None,
        clear_job: None,
        settings,
        about,
        trigger: None,
        key_job: None,
        save_job: None,
        forget_job: None,
        rebinding: false,
        translating_word: None,
        translating_sentence: None,
        translating_definition: None,
        online: Kept::default(),
        check,
        indexes: None,
        capturing: Capturing::default(),
        copying: Copying::default(),
        language: Language::default(),
        language_file,
        language_chosen: false,
        language_job,
    };
    serve(session, receiver);
    let tray = start_tray(&events);
    Resident {
        _hold: application.hold(),
        _shortcut: handle,
        events,
        _work: work,
        _articles: articles,
        _tray: tray,
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

#[cfg(test)]
mod tests {
    use gtk::{gio, prelude::*};

    use super::{
        CLIPBOARD_LIMIT, CLIPBOARD_MAX_CHARS, Copying, is_offered_command, missed_line,
        reset_failure,
    };
    use crate::view::{
        Held, NOTHING_SELECTED_TEXT, install_language, install_tesseract, too_long_text,
    };
    use addict_core::translation::Source as Language;

    #[test]
    fn a_reset_that_did_not_happen_says_so_in_a_line() {
        use addict_platform::work::Outcome;
        assert_eq!(reset_failure(Outcome::Finished(Ok(()))), None);
        assert_eq!(
            reset_failure(Outcome::Finished(Err("dconf reset: x".to_owned()))).as_deref(),
            Some("dconf reset: x")
        );
        for outcome in [Outcome::TimedOut, Outcome::Cancelled, Outcome::Panicked] {
            assert!(reset_failure(outcome).is_some());
        }
    }

    #[test]
    fn a_line_that_is_an_offered_command_is_not_a_text() {
        for language in Language::ALL {
            assert!(is_offered_command(&install_tesseract(language)));
            assert!(is_offered_command(&install_language(language)));
        }
        assert!(!is_offered_command("bank"));
        assert!(!is_offered_command(&format!(
            "run {}",
            install_tesseract(Language::En)
        )));
        assert!(!is_offered_command(
            "Catalan OCR data is not installed. Install it with: \
             sudo dnf install tesseract-langpack-cat"
        ));
    }

    #[test]
    fn only_the_last_clipboard_read_is_the_one_that_runs() {
        let mut copying = Copying::default();
        assert!(!copying.is_running(0));
        assert!(copying.end().is_none());

        let first = copying.begin();
        assert!(copying.is_running(first));
        let cancellable = gio::Cancellable::new();
        copying.read.as_mut().expect("read").cancellable = Some(cancellable.clone());

        // A new read, after a new press, cancels the one that reads.
        let second = copying.begin();
        assert!(cancellable.is_cancelled());
        assert_ne!(first, second);
        assert!(!copying.is_running(first));
        assert!(copying.is_running(second));

        // Ended by an edit or a capture: what it reports later is ignored.
        assert!(copying.end().is_some());
        assert!(!copying.is_running(second));
        assert!(copying.end().is_none());
    }

    #[test]
    fn a_read_that_waits_for_the_window_is_ended_without_a_cancellable() {
        let mut copying = Copying::default();
        let number = copying.begin();
        // The window is not active yet: nothing is read, nothing to cancel.
        let read = copying.end().expect("read");
        assert!(read.cancellable.is_none());
        assert!(!copying.is_running(number));
    }

    #[test]
    fn a_too_long_selection_is_named_before_the_clipboard() {
        let selection = Some((Held::Selection, 1200));
        let clipboard = Some((Held::Clipboard, 54828));
        let selected = too_long_text(Held::Selection, 1200, CLIPBOARD_MAX_CHARS);
        let copied = too_long_text(Held::Clipboard, 54828, CLIPBOARD_MAX_CHARS);
        assert_eq!(missed_line(selection, clipboard), selected);
        // The clipboard ran out of time: the selection still says so.
        assert_eq!(missed_line(selection, None), selected);
        // The selection was blank.
        assert_eq!(missed_line(None, clipboard), copied);
        // Neither was too long: nothing is selected or copied.
        assert_eq!(missed_line(None, None), NOTHING_SELECTED_TEXT);
    }

    #[test]
    fn the_clipboard_limit_is_finite_and_short() {
        assert!(!CLIPBOARD_LIMIT.is_zero());
        assert!(CLIPBOARD_LIMIT.as_secs() <= 1);
    }
}
