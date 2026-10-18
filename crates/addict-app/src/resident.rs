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
//! Only the explicit definition action sends its English definition without
//! context; a separate job is cancelled on a changed sense or replacement
//! article and guards its outcome by request identity. A click on a word of
//! a sentence also asks online for the word in its sentence and, once for
//! that sentence, for the sentence itself; a typed text is one sentence,
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
//! The shortcut also starts the read of the focused editor, when that is
//! Android Studio. The press first takes the source: the application the
//! listener of [`addict_platform::foreground`] saw activated last, or, when
//! the product's own window is active at the press, the source kept from the
//! press that opened it. When the window is not active, the press also asks
//! the listener, at once and before anything of the window changes, to probe
//! whether a frame of Android Studio is active: Android Studio sends no
//! activation, but its frame's state follows the focus. The job waits for
//! that answer, and an active frame makes the source Android Studio; any
//! other answer leaves the observed source. The job tells the source it
//! settled on, and the session keeps it for the press of the running read,
//! so a later press in the window keeps Android Studio.
//!
//! The press also takes whether accessibility is on, as the listener last
//! read it; that is a property of the session and is not kept. The window is
//! shown at once, in the waiting state with the source AT-SPI and the field
//! cleared (the lookup and the online requests of the old text are given
//! up), and one job on the runner asks the accessibility bus for the
//! selected word and its sentence; nothing waits for it, nor for the probe,
//! before the window is presented. With accessibility off, or with another
//! source or none, the job ends without reading text.
//!
//! The sentence replaces the field as a new text with that word chosen, and
//! the lookup of the word starts; as the click of a word it is the click
//! path, but it asks nothing online: the online texts are asked when a word
//! is clicked. A read that gives nothing is written to stderr and is
//! followed by the read of the clipboard (below); when that gives nothing
//! too, the waiting ends, the empty state stays with its focused field, and
//! the source line says why the read of the bus gave nothing; the capture
//! shares the line: they never run together. The line names the cause and
//! the manual path, and goes, like the capture's, when the field is edited,
//! Capture or the shortcut is pressed.
//!
//! Each read has a number; a new press, an edit of the field and a capture
//! give up the read that runs, and whatever it reports later is ignored and
//! shows no line. A press also gives up a running capture. Every end of the
//! read of the bus, also a given-up one and one that could not start, is
//! recorded with `--evidence` as one `context` record, timed on the worker;
//! the read of the clipboard after it is not. The job is
//! [`context`](crate::context)'s.
//!
//! A read of the press that ends without context, for any cause, is written
//! to stderr at once, and the window then reads the primary selection or the
//! clipboard once, in place of showing the line: the waiting goes on with the
//! source clipboard, and the field stays editable. Wayland gives a client the
//! clipboard only with its keyboard focus, so the read starts once the window
//! is active, at once when it is already. The primary selection of the
//! window's display, the text selected without a copy, is read first, and
//! GTK's clipboard only when the selection gives no line. Waiting for the
//! activity and both reads together end after [`CLIPBOARD_LIMIT`], which
//! cancels the read. A text of either as one line, of at most
//! [`CLIPBOARD_MAX_CHARS`] characters, replaces the field as a new text with
//! the source clipboard, as a read context does, without a word chosen unless
//! it is one word, and nothing is asked online; the line is not shown. When
//! neither gives a line (empty, blank, not text, too long, unreadable or
//! late), why is written to stderr, and the empty state stays with the line
//! of the read; when one of them was too long, the line says that instead,
//! the selection's before the clipboard's. The read has a number and is given
//! up like the read it follows, by a press, an edit and a capture; what it
//! reports later is ignored. Retry, a launch and a capture read no clipboard.
//!
//! The user reads in one language, chosen with the EN / ES / CA switch
//! beside the field; English at first. Every online request about the
//! field's text is sent from it, and only English is looked up offline: for
//! Spanish and Catalan the result is the line that there is no offline
//! dictionary for the language, and no lookup runs. The definition of a
//! sense is OEWN's and is always sent from English. A capture recognizes the
//! text with the data of the language. A press of another language forgets
//! the online texts of the field's text and, for the word shown, redoes its
//! offline result and asks again what its click or its submit asked; with no
//! word nothing is sent. The choice is kept in the language file, read once
//! on the runner after the start and written on the runner at each change
//! ([`language`](crate::language)); a read that comes after the user chose
//! changes nothing.
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
use addict_deepl::DeepL;
use addict_platform::{
    atspi::ContextFailure,
    foreground::{self, Accessibility, Foreground, Probe, Source as Foregrounded},
    ocr::OcrFailure,
    shortcut::{self, ShortcutEvent, ShortcutHandle},
    tray::{self, TrayEvent, TrayHandle},
    work::{Job, Outcome, Work},
};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_util::StreamExt;
use gtk::{gdk, gio, glib, prelude::*};

use crate::{
    capture::{self, Capture, Captured, Failure, Timing},
    context::{self, Read, Reading},
    dictionaries::{Articles, Dictionaries, Indexes, Looked},
    evidence, language,
    manual::{self, CLIPBOARD_MAX_CHARS, Edit, Manual, NoLine},
    online::{self, Kept, Subject},
    recording::Recording,
    settings::{self, KeyState, Settings},
    style,
    translate::{self, Translated},
    view::{Binding, Held, Lookup, Online, Shown, failure_text, read_failure_text, too_long_text},
    widgets::View,
};

/// Preferred trigger in the XDG shortcuts syntax; the user may change it.
const TRIGGER: &str = "LOGO+q";

/// How long one attempt to bind the shortcut may take.
const BIND_LIMIT: Duration = Duration::from_secs(60);

/// How long the read of the clipboard after a press without context may
/// take, the wait for the window to be active included.
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
    /// The listener of the application in front; the session asks it at
    /// each press.
    _foreground: Rc<Foreground>,
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
    /// The text of the field was submitted, by Enter or by a paste.
    Submitted,
    /// The user explicitly requested the selected dictionary definition.
    DefinitionAction,
    /// The separate definition job ended with its individual request number.
    /// `elapsed` is the time the request took, as the worker saw it; taken
    /// only when evidence is recorded.
    DefinitionTranslated {
        number: u64,
        outcome: Outcome<Translated>,
        elapsed: Option<evidence::Elapsed>,
    },
    /// The lookup started as number `run` ended.
    Lookup { run: u64, outcome: Outcome<Looked> },
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
    /// The read of the accessibility bus with this number settled on this
    /// application in front, before it reads.
    ContextSource(u64, Foregrounded),
    /// The read of the accessibility bus with this number ended. The time is
    /// the read's, as the worker saw it; taken only when evidence is
    /// recorded.
    Context(u64, Outcome<Read>, Option<evidence::Elapsed>),
    /// The window became active.
    Active,
    /// A choice of the tray's menu, or its end.
    Tray(TrayEvent),
    /// Change shortcut was pressed in the Settings window.
    ChangeShortcut,
    /// The state of the key file, as the runner read it for the Settings
    /// window.
    KeyFile(KeyState),
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

/// The height the window starts with, in pixels: an article of a few
/// senses with one of them chosen fits without scrolling its list.
const WINDOW_HEIGHT: i32 = 480;

/// The header bar: the name of the application beside its accent mark, and
/// the window's buttons.
pub(crate) fn header_bar() -> gtk::HeaderBar {
    let mark = gtk::Box::builder()
        .valign(gtk::Align::Center)
        .css_classes(["brand-mark"])
        .build();
    let name = gtk::Label::builder()
        .label("Addict")
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
        .default_width(480)
        .default_height(WINDOW_HEIGHT)
        .child(view.root())
        .css_classes(["addict"])
        .titlebar(&header_bar())
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
    /// The lookup whose outcome is awaited; dropping it cancels the lookup.
    pending: Option<Job>,
    /// The result the window shows and the sense chosen in it.
    shown: Shown,
    /// Weak: the runner must go when the resident state is dropped. Never
    /// upgrades when the runner could not be started.
    work: Weak<Work>,
    articles: Arc<dyn Articles>,
    /// What a word or a sentence is translated by online.
    translator: Arc<dyn Translator>,
    key_file: Option<std::path::PathBuf>,
    /// The Settings window, which the tray shows.
    settings: Settings,
    /// The trigger the portal bound, for the Settings window.
    trigger: Option<String>,
    /// The read of the key file's state; dropping it gives it up.
    key_job: Option<Job>,
    /// The online request of a word, in its sentence or alone, whose outcome
    /// is awaited; dropping it cancels the request.
    translating_word: Option<Job>,
    /// The same for the request of the sentence.
    translating_sentence: Option<Job>,
    /// Dropping this separate job cancels the selected definition request.
    translating_definition: Option<Job>,
    /// The online texts of the text the field holds.
    online: Kept,
    /// The check of the indexes at the start; dropping it cancels the check.
    check: Option<Job>,
    /// Where the capture stands.
    capturing: Capturing,
    /// Where the read of the accessibility bus stands.
    reading: Reading,
    /// Where the read of the clipboard after it stands.
    copying: Copying,
    /// The language the user reads: the requests are sent from it, and only
    /// English is looked up offline.
    language: Language,
    /// Where the reading language is kept across restarts; `None` when the
    /// environment names no configuration directory.
    language_file: Option<std::path::PathBuf>,
    /// Whether the user chose the language in this session: the read of the
    /// file, should it come later, changes nothing then.
    language_chosen: bool,
    /// The read or the write of the language file on the runner.
    language_job: Option<Job>,
    /// The listener of the application in front; gone with the resident.
    foreground: Weak<Foreground>,
    /// What the press that last started a read took as the application in
    /// front; none at first.
    kept: Foregrounded,
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

/// Where the read of the clipboard of a [`Session`] stands; at first none
/// runs.
#[derive(Default)]
struct Copying {
    /// The read, from the failed read of the bus to its end.
    read: Option<ClipboardRead>,
    /// The number of the last started read. An event that carries another
    /// number, or comes when none runs, is from a read that was given up.
    number: u64,
}

/// One read of the primary selection, then of the clipboard.
struct ClipboardRead {
    /// The source line of the read of the bus that failed, shown when the
    /// clipboard gives nothing.
    line: String,
    /// Cancels the read of the clipboard; `None` while the window is not yet
    /// active and nothing is read.
    cancellable: Option<gio::Cancellable>,
    /// Set when the selection was too long, with its length, so that a read
    /// of the clipboard that runs out of time still says so.
    selection: Rc<Cell<Option<(Held, usize)>>>,
}

impl Copying {
    /// A new read starts after the failure that `line` says; the number of
    /// the new one is given back.
    fn begin(&mut self, line: String) -> u64 {
        self.end();
        self.number += 1;
        self.read = Some(ClipboardRead {
            line,
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
            Event::ChangeShortcut => self.change_shortcut(),
            Event::KeyFile(state) => self.settings.render_key(Some(state)),
            Event::Launched => self.launched(),
            Event::Retry => self.retry(),
            Event::Sentence(text) => self.sentence_changed(text),
            Event::Word(index) => self.word_chosen(index),
            Event::Sense(index) => self.sense_chosen(index),
            Event::Submitted => self.submitted(),
            Event::DefinitionAction => self.definition_action(),
            Event::DefinitionTranslated {
                number,
                outcome,
                elapsed,
            } => self.definition_translated(number, outcome, elapsed),
            Event::Lookup { run, outcome } => self.looked_up(run, outcome),
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
                self.view.render_indexes(indexes);
            }
            Event::Capture => self.capture(),
            Event::Hidden => self.start_capture(),
            Event::Captured(number) => self.snapshot_taken(number),
            Event::Recognized(number, outcome) => self.recognized(number, outcome),
            Event::ContextSource(number, source) => self.context_source(number, source),
            Event::Context(number, outcome, elapsed) => self.context_read(number, outcome, elapsed),
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

    /// Gives up the read of the accessibility bus and the read of the
    /// clipboard after it: dropping the job cancels the one, the other is
    /// cancelled, and whatever they report later is ignored. Whether a read
    /// ran.
    fn give_up_read(&mut self) -> bool {
        let running = self.reading.running();
        let ran = self.reading.end();
        // A read given up is an outcome too: it ran for as long as this.
        if let Some(started) = running {
            self.record_context(
                evidence::ContextEnd::Cancelled,
                started,
                started.probe.ended(),
            );
        }
        let copied = self.copying.end().is_some();
        if matches!(
            self.manual.reading(),
            Some(Source::AtSpi | Source::Clipboard)
        ) {
            self.manual.set_reading(None);
        }
        ran || copied
    }

    /// Clears the field for the read that starts now: a new empty text, so
    /// the lookup and the online requests of the old one are given up, and
    /// the source line goes. The window shows the empty field and, with the
    /// state of the read, the waiting state.
    fn clear_for_read(&mut self) {
        self.view.clear_source_failure();
        let edit = self.manual.edit(String::new());
        self.view.set_field_text("");
        self.manual.set_reading(Some(Source::AtSpi));
        self.text_replaced(edit);
    }

    /// What the press takes as the application in front, and keeps for the
    /// next press that finds the window active; with the probe of Android
    /// Studio's frames when the window is not active, asked first.
    fn take_source(&mut self) -> (Foregrounded, Option<Probe>) {
        let listener = self.foreground.upgrade();
        let window_active = self.window.is_active();
        let probe = listener
            .as_ref()
            .filter(|_| !window_active)
            .map(|listener| listener.probe());
        let observed = listener.map_or(Foregrounded::Unknown, |listener| listener.source());
        let source = context::choose_source(window_active, self.kept, observed);
        self.kept = source;
        (source, probe)
    }

    /// Whether accessibility is on, as the listener last read it. Taken at
    /// each press and not kept: it is the session's, not the press's.
    fn accessibility(&self) -> Accessibility {
        self.foreground
            .upgrade()
            .map_or(Accessibility::Unknown, |listener| listener.accessibility())
    }

    /// Starts the read of the accessibility bus as the next number, with
    /// `accessibility`, `source` in front and the `probe` of the press.
    fn start_read(
        &mut self,
        accessibility: Accessibility,
        source: Foregrounded,
        probe: Option<Probe>,
    ) {
        let begun = context::Started {
            probe: evidence::Probe::start(self.recording.is_some()),
            accessibility,
            source,
        };
        let number = self.reading.begin(begun);
        let settled = self.sender.clone();
        let sender = self.sender.clone();
        let started = context::start(
            self.work.upgrade().as_deref(),
            accessibility,
            source,
            probe,
            move |source| {
                // On the worker: the UI thread wakes for the event.
                let _ = settled.unbounded_send(Event::ContextSource(number, source));
            },
            move |outcome| {
                // On the worker: the UI thread wakes for the event, and the
                // time is the read's, not the redraw's.
                let elapsed = begun.probe.ended();
                let _ = sender.unbounded_send(Event::Context(number, outcome, elapsed));
            },
        );
        match started {
            Ok(job) => self.reading.run(job),
            Err(failure) => {
                // No time passed: the read could not start.
                let end = evidence::ContextEnd::of(&Err(failure.clone()));
                self.record_context(end, begun, begun.probe.not_started());
                self.read_failed(&failure);
            }
        }
    }

    /// Writes the record of a read of the accessibility bus that ended as
    /// `end`, when evidence is recorded and there is a time for it.
    fn record_context(
        &self,
        end: evidence::ContextEnd,
        started: context::Started,
        elapsed: Option<evidence::Elapsed>,
    ) {
        let record = evidence::context_record(end, started.source, started.accessibility, elapsed);
        if let Some((recording, record)) = self.recording.as_ref().zip(record) {
            recording.context_ended(&record);
        }
    }

    /// The read with `number` settled on `source`: when it is the one that
    /// runs, that is the source its press keeps, for a later press in the
    /// window. One that was given up changes nothing.
    fn context_source(&mut self, number: u64, source: Foregrounded) {
        if self.reading.is_running(number) {
            self.kept = source;
            self.reading.settle(number, source);
        }
    }

    /// The read with `number` ended; one that was given up shows nothing.
    /// The context replaces the field as a new text with the selected word
    /// chosen and its lookup starts, nothing is asked online; a failure
    /// leaves the field as it is and starts the read of the clipboard.
    fn context_read(
        &mut self,
        number: u64,
        outcome: Outcome<Read>,
        elapsed: Option<evidence::Elapsed>,
    ) {
        if !self.reading.is_running(number) {
            return;
        }
        let started = self.reading.running();
        self.reading.end();
        // A cancelled job was dropped here on purpose and shows nothing.
        let Some(read) = context::read_result(outcome) else {
            return;
        };
        if let Some(started) = started {
            self.record_context(evidence::ContextEnd::of(&read), started, elapsed);
        }
        match read {
            Ok(context) => {
                let edit = self.manual.arrived(context);
                self.replace_field(edit);
            }
            Err(failure) => self.read_failed(&failure),
        }
    }

    /// The read gave nothing: the failure is logged at once, and the
    /// clipboard is read in its place; the line that says why, with the
    /// manual path, waits for it.
    fn read_failed(&mut self, failure: &ContextFailure) {
        eprintln!("addict: no context from the accessibility bus: {failure:?}");
        self.give_up_read();
        self.start_copy(read_failure_text(failure).into_owned());
    }

    /// Starts the read of the primary selection and the clipboard as the
    /// next number, after the failure that `line` says: the window waits
    /// with the source clipboard, the read starts once the window is active,
    /// and after [`CLIPBOARD_LIMIT`] it ends without text.
    fn start_copy(&mut self, line: String) {
        let number = self.copying.begin(line);
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
    /// source line says that a text was too long, when one was, or else why
    /// the read of the bus gave nothing.
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
                let line = missed_line(read.line, read.selection.get(), missed.too_long);
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
    /// accessibility bus are given up: the user's text wins, and the state no
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
        if gave_up_capture || gave_up_read {
            self.view.render(self.manual.state(), self.manual.source());
        }
    }

    /// A text that did not come from the field, recognized, read from the
    /// accessibility bus or copied, replaced the manual input as `edit`: the
    /// field shows it, and what follows a new text follows.
    fn replace_field(&mut self, edit: Edit) {
        self.view.set_field_text(self.manual.sentence());
        self.text_replaced(edit);
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
        self.translating_sentence = None;
        self.online = Kept::of(self.manual.text_number(), self.manual.kind());
        self.render_online();
        match edit {
            Edit::Same => {}
            Edit::Cleared => {
                // Cancels the lookup; whatever it still reports has an old
                // number.
                self.pending = None;
                self.render_manual();
                self.show_result(None);
            }
            Edit::Lone(run) | Edit::Chosen(run) => self.start_lookup(run),
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
        self.pending = None;
        self.render_manual();
        let Some(word) = self.manual.input().map(|input| input.word().to_owned()) else {
            self.show_result(None);
            return;
        };
        let sender = self.sender.clone();
        let started = manual::offline(
            self.work.upgrade().as_deref(),
            self.articles.clone(),
            word,
            self.language,
            move |outcome| {
                // On the worker: the UI thread wakes for the event.
                let _ = sender.unbounded_send(Event::Lookup { run, outcome });
            },
        );
        match started {
            Ok(job) => {
                self.pending = Some(job);
                self.show_result(Some(Lookup::Pending));
            }
            Err(lookup) => self.show_result(Some(lookup)),
        }
    }

    /// A lookup ended. Only the last started one is shown; where it found
    /// the indexes is shown for every one that read them, and every one
    /// that was not given up is recorded.
    fn looked_up(&mut self, run: u64, outcome: Outcome<Looked>) {
        if let Some(recording) = &self.recording
            && let Some(answered) = evidence::answered(&outcome)
        {
            recording.looked_up(answered);
        }
        if let Outcome::Finished(looked) = &outcome {
            self.view.render_indexes(looked.indexes);
        }
        if !self.manual.awaits(run) {
            return;
        }
        self.pending = None;
        let Some(input) = self.manual.input() else {
            return;
        };
        if let Some(lookup) = manual::lookup_result(outcome, input.word()) {
            self.show_result(Some(lookup));
        }
    }

    /// Sends the online request about `subject` that [`Kept`] has just
    /// asked for, as a job in the slot of its subject. A request that could
    /// not start is a refusal at once.
    fn start_translation(&mut self, subject: Subject) {
        let text = self.manual.text_number();
        let Some(number) = self.online.request_number(subject) else {
            return;
        };
        let source = self.language;
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
        self.translating_definition = None;
        self.shown.show(lookup);
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
        let previous = self.shown.sense();
        if self.shown.choose(index) {
            if previous != self.shown.sense() {
                self.translating_definition = None;
            }
            self.render_shown();
        }
    }

    /// "Translate definition into Russian" was pressed: the definition of
    /// the chosen sense is sent alone, as a job of its own, unless the field
    /// holds a newer text or the shown result offers no definition to send.
    fn definition_action(&mut self) {
        // As for a click: the field may hold a newer text already.
        if self.field_is_newer() {
            return;
        }
        let Some((number, request)) = self.shown.request_definition() else {
            return;
        };
        let started = self.send_online(
            Some(request),
            evidence::Requested::Definition,
            move |outcome, elapsed| Event::DefinitionTranslated {
                number,
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

    /// The definition request ended; it is recorded as a word's is.
    fn definition_translated(
        &mut self,
        number: u64,
        outcome: Outcome<Translated>,
        elapsed: Option<evidence::Elapsed>,
    ) {
        // A definition is OEWN's, so always sent from English.
        let Some(online) = self.online_ended(
            outcome,
            evidence::Requested::Definition,
            Language::En,
            elapsed,
        ) else {
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
    }

    /// The shortcut was pressed: the application in front is taken first,
    /// then a running capture and read are given up, the field is cleared and
    /// the window is shown at once in the waiting state with the source
    /// AT-SPI, and the read of the accessibility bus starts, which ends at
    /// once when that application was not Android Studio; the window does
    /// not wait for it. A read that ends without context is followed by the
    /// read of the clipboard: the window waits on with the source clipboard,
    /// for at most [`CLIPBOARD_LIMIT`].
    fn activated(
        &mut self,
        activation_token: Option<&str>,
        timestamp_ms: u64,
        receipt_ns: Option<u64>,
    ) {
        // Before anything of the window changes: the window's own activity
        // and the listener are what they were at the press, and the probe
        // starts before the window can take the focus.
        let (source, probe) = self.take_source();
        let accessibility = self.accessibility();
        self.give_up_press();
        self.clear_for_read();
        // Started, not awaited: the window is presented below.
        self.start_read(accessibility, source, probe);
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
        // No press stands behind this, so there is no token.
        self.forget_source();
        if let Some(recording) = &self.recording {
            recording.shown_without_press();
        }
        self.window.present();
    }

    /// The window is about to be shown without a press. Unless it is in
    /// front already, no press opened it, so the source of an earlier press
    /// is not its source.
    fn forget_source(&mut self) {
        if !self.window.is_active() {
            self.kept = Foregrounded::Unknown;
        }
    }

    fn launched(&mut self) {
        self.forget_source();
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
    /// shows the window; Exit ends the application through its shutdown. A
    /// tray that cannot be had is said once and changes nothing.
    fn tray(&mut self, event: TrayEvent) {
        match event {
            TrayEvent::Show => self.launched(),
            TrayEvent::Settings => self.show_settings(),
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
                        // Replaced by the next press of Settings.
                        Outcome::Cancelled => return,
                    };
                    let _ = sender.unbounded_send(Event::KeyFile(state));
                },
            )
        });
        if self.key_job.is_none() {
            self.settings.render_key(Some(KeyState::Unknown));
        }
        self.settings.present();
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

/// The source line when neither the selection nor the clipboard gave a
/// line: that the selection was too long, when it was, else that the
/// clipboard was, else `read`, why the read of the bus gave nothing.
fn missed_line(
    read: String,
    selection: Option<(Held, usize)>,
    clipboard: Option<(Held, usize)>,
) -> String {
    selection.or(clipboard).map_or(read, |(held, chars)| {
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
            Ok(Some(text)) => manual::clipboard_line(&text).map_err(|no_line| match no_line {
                NoLine::Blank => Missed::because("blank".to_owned()),
                NoLine::TooLong(chars) => Missed {
                    reason: format!("{chars} characters, over {CLIPBOARD_MAX_CHARS}"),
                    too_long: Some((held, chars)),
                },
            }),
            Ok(None) => Err(Missed::because("no text".to_owned())),
            Err(e) => Err(Missed::because(e.to_string())),
        });
    });
}

/// Sends what the user does in the `view` to `sender`: Retry, an edit of the
/// field, a submit of its text, Capture, a click on a word and a click on a
/// sense.
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
    let sense = sender.clone();
    view.connect_sense_chosen(move |index| {
        let _ = sense.unbounded_send(Event::Sense(index));
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
    let active = sender.clone();
    window.connect_is_active_notify(move |window| {
        if window.is_active() {
            let _ = active.unbounded_send(Event::Active);
        }
    });
    (view, window)
}

/// The Settings window above `window`, hidden, naming `key_file`; Change
/// shortcut goes to `sender` as an event.
fn start_settings(
    window: &gtk::ApplicationWindow,
    key_file: Option<&std::path::Path>,
    sender: &UnboundedSender<Event>,
) -> Settings {
    let change = sender.clone();
    Settings::new(window, key_file, move || {
        let _ = change.unbounded_send(Event::ChangeShortcut);
    })
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

/// The online translator and the key file it reads, for the window to name.
/// Nothing is read or connected here: the key file and the network are the
/// runner's, at the first click on a word of a sentence or the first submit
/// of a lone word.
fn online_translator() -> (Arc<dyn Translator>, Option<std::path::PathBuf>) {
    let translator = DeepL::new();
    let key_file = translator.key_file().map(std::path::Path::to_owned);
    (Arc::new(translator), key_file)
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
    // One connection to the accessibility bus and a few strings; it asks
    // nothing of the applications.
    let listener = Rc::new(foreground::watch(|_| {}));
    let check = start_check(work.as_deref(), articles.clone(), &sender);
    // After the check: the runner takes its jobs in this order. Only the
    // path is made here; the file is the runner's.
    let language_file = language::path();
    let language_job = start_language(work.as_deref(), language_file.clone(), &sender);
    let (translator, key_file) = online_translator();
    let settings = start_settings(&window, key_file.as_deref(), &sender);
    let session = Session {
        binding,
        view,
        window,
        recording,
        timed,
        sender,
        current: Rc::downgrade(&handle),
        manual: Manual::default(),
        pending: None,
        shown: Shown::default(),
        work: work.as_ref().map(Rc::downgrade).unwrap_or_default(),
        articles: articles.clone(),
        translator,
        key_file,
        settings,
        trigger: None,
        key_job: None,
        translating_word: None,
        translating_sentence: None,
        translating_definition: None,
        online: Kept::default(),
        check,
        capturing: Capturing::default(),
        reading: Reading::default(),
        copying: Copying::default(),
        language: Language::default(),
        language_file,
        language_chosen: false,
        language_job,
        foreground: Rc::downgrade(&listener),
        kept: Foregrounded::Unknown,
    };
    serve(session, receiver);
    let tray = start_tray(&events);
    Resident {
        _hold: application.hold(),
        _shortcut: handle,
        events,
        _work: work,
        _articles: articles,
        _foreground: listener,
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

    use super::{CLIPBOARD_LIMIT, CLIPBOARD_MAX_CHARS, Copying, missed_line};
    use crate::view::{Held, too_long_text};

    #[test]
    fn only_the_last_clipboard_read_is_the_one_that_runs() {
        let mut copying = Copying::default();
        assert!(!copying.is_running(0));
        assert!(copying.end().is_none());

        let first = copying.begin("first line".to_owned());
        assert!(copying.is_running(first));
        let cancellable = gio::Cancellable::new();
        copying.read.as_mut().expect("read").cancellable = Some(cancellable.clone());

        // A new read, after a new press, cancels the one that reads.
        let second = copying.begin("second line".to_owned());
        assert!(cancellable.is_cancelled());
        assert_ne!(first, second);
        assert!(!copying.is_running(first));
        assert!(copying.is_running(second));

        // Ended by an edit or a capture: what it reports later is ignored.
        let read = copying.end().expect("read");
        assert_eq!(read.line, "second line");
        assert!(!copying.is_running(second));
        assert!(copying.end().is_none());
    }

    #[test]
    fn a_read_that_waits_for_the_window_is_ended_without_a_cancellable() {
        let mut copying = Copying::default();
        let number = copying.begin("line".to_owned());
        // The window is not active yet: nothing is read, nothing to cancel.
        let read = copying.end().expect("read");
        assert!(read.cancellable.is_none());
        assert!(!copying.is_running(number));
    }

    #[test]
    fn a_too_long_selection_is_named_before_the_clipboard() {
        let read = || "Android Studio line".to_owned();
        let selection = Some((Held::Selection, 1200));
        let clipboard = Some((Held::Clipboard, 54828));
        let selected = too_long_text(Held::Selection, 1200, CLIPBOARD_MAX_CHARS);
        let copied = too_long_text(Held::Clipboard, 54828, CLIPBOARD_MAX_CHARS);
        assert_eq!(missed_line(read(), selection, clipboard), selected);
        // The clipboard ran out of time: the selection still says so.
        assert_eq!(missed_line(read(), selection, None), selected);
        // The selection was blank.
        assert_eq!(missed_line(read(), None, clipboard), copied);
        // Neither was too long.
        assert_eq!(missed_line(read(), None, None), read());
    }

    #[test]
    fn the_clipboard_limit_is_finite_and_short() {
        assert!(!CLIPBOARD_LIMIT.is_zero());
        assert!(CLIPBOARD_LIMIT.as_secs() <= 1);
    }
}
