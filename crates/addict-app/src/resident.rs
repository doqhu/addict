//! The resident application: a reusable window, two global shortcut actions
//! and the loop that shows the window or captures a region for each event.
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
//! The primary session loads the DeepL key on a separate worker at startup.
//! Only requests to DeepL and Settings saves wait for the initial result.
//! Settings saves update the active key. Direct edits take effect at restart.
//!
//! Capture hides the window and starts one job on the runner, which takes
//! the screenshot, recognizes its text and deletes the snapshot; the window
//! comes back in the reading state once the snapshot is there, and the
//! recognized text, its lines joined by [`addict_core::sentence::recognized`], then
//! replaces the field as a new text with the source OCR, which follows the
//! manual path from there. A capture without text leaves the field as it
//! was, starts no lookup and no online request, and says why in the source
//! line under the status; the line stays, also when the window is shown
//! again, until the field is edited, Capture or the shortcut is pressed or a
//! text is recognized. The window comes back after every such end, also after
//! the screenshot shortcut with the window closed: GNOME gives code 2 for Esc
//! and for a real failure alike, so the two cannot be told apart. The keyboard
//! focus is then on Capture, or on Retry when it is offered, and a press of
//! Capture is a new interactive request; the line of a capture that left no snapshot names
//! it. No permission is granted anywhere on this path: the portal asks for
//! none for an interactive screenshot. Capture cannot be pressed while the area is being chosen;
//! once the snapshot is there it can, and it is the retry: pressing it, or
//! editing the field, gives up the recognition that runs. The controller cancels the current request,
//! which stops the adapter and releases its snapshot, and ignores late results. Every capture end that is
//! shown, also one that could not start, is recorded with `--evidence` as it
//! is shown; a given-up one is not. The job is [`capture`]'s.
//!
//! The clipboard action reads the text selected or copied, the same for every
//! application. The press gives up a running capture and the read of the
//! press before, and clears the field (the lookup and the online requests
//! of the old text are given up); the window is shown at once, in the
//! waiting state with the source "selection or clipboard", and nothing
//! waits for the read before the window is presented. Wayland gives a
//! client the clipboard only with its keyboard focus, so the read starts
//! once the window is active, at once when it is already. The read is the
//! controller's `ReadSelection` effect, issued after its `ShowPanel` (Linux
//! shows first, `read_before_show` false; the window is already up, so
//! `ShowPanel` changes nothing here). The shell reads the primary selection
//! of the window's display, the text selected without a copy, and GTK's
//! clipboard, raw, and sends both back in one result; a failed GTK read is
//! written to stderr, never the text. The controller applies 0.5.0's rule
//! and owns the deadline: [`addict_controller::deadlines::CLIPBOARD_LIMIT`]
//! after the press it cancels the read. A text of either as one line, of
//! at most `CLIPBOARD_MAX_CHARS` characters, replaces the field as a new
//! text with the source clipboard, the selection's first, without a word
//! chosen unless it is one word, and nothing is asked online. A line that
//! is exactly a command the window offered to copy, such as the install of
//! Tesseract, is the user's Copy and counts as no line. When neither gives
//! a line, the waiting ends, the empty state stays with its focused field,
//! and the source line says that nothing is selected or copied, with the
//! manual path; when one of them was too long, the line says that instead,
//! the selection's before the clipboard's. The capture shares the line:
//! they never run together. The line goes, like the capture's, when the
//! field is edited, Capture or the shortcut is pressed.
//!
//! Each read has the controller's request id; a new press, an edit of the
//! field, Back, Forward and a capture give up the read that runs (the
//! controller's `Cancel` stops the GTK reads), and whatever it reports
//! later is ignored and shows no line. Retry, a launch, Show of the tray and
//! a capture read no clipboard.
//!
//! The user reads in one language, chosen with the switch beside the
//! field, whose three buttons are the reading languages of my language
//! (EN / ES / CA with Russian mine) and change with it; English at first,
//! or Spanish when English is mine. The online requests about the
//! field's text are sent from it into my language, but for a word answered
//! by another language and for a Russian word (below). English is looked up
//! offline in its two
//! indexes. Spanish and Catalan are each looked up in their own indexes when
//! the last known state of the indexes says one of that language is there;
//! otherwise the result is the line that there is no offline dictionary for
//! the language, and no lookup runs. A lookup that runs tries the chosen
//! language first and, when it has no entry for the word, the other
//! languages in the order EN, ES, CA; the first that has the word gives the
//! article, and the window names it. The word's online requests, alone or
//! in its sentence, wait for that lookup and are sent from the language
//! that answered, or from the chosen one when none did; the sentence's
//! request is sent at once, from the chosen language. A Russian word, one
//! with a Cyrillic letter, and its sentence are sent from Russian whatever
//! is chosen; with Russian as my language nothing is sent for it, neither
//! the word nor its sentence. Neither the switch
//! nor the language file changes. With Spanish or Catalan chosen and its
//! indexes not known to be there, the indexes are checked again, so that an
//! import done while the application runs is found by the next pick. The
//! definition of an English sense is OEWN's and is sent from English; that
//! of a Spanish or Catalan sense from its language, and an English gloss
//! from English, each into my language and recorded with the source it was
//! sent from; a definition already in my language is not offered. A capture
//! recognizes the text with the data of the language. A press of another
//! language forgets the online texts of the field's text and, for the word
//! shown, redoes its offline result and asks again what its click or its
//! submit asked; with no word nothing is sent. The choice is kept in the
//! language file, read once on the runner after the start and written on the runner at each change
//! ([`language`](crate::language)); a read that comes after the user chose
//! changes nothing. The language kept is apart from the one read in: while
//! it is my language, the first reading language of my language is pressed
//! instead, and nothing is written of that fallback.
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
    collections::{HashMap, VecDeque},
    fs::File,
    future::Future,
    io::{self, Write},
    os::fd::FromRawFd,
    path::{Path, PathBuf},
    process::ExitCode,
    rc::{Rc, Weak},
    sync::{Arc, Mutex, Weak as WorkRef},
    time::Duration,
};

use addict_controller::{
    self as controller, ArticleId, Bindings as ControllerBindings, Controller, Dispatcher, Effect,
    Platform, RequestId, ShortcutState, StartFacts, ViewState,
    deadlines::{
        AUDIO_LIMIT, BIND_ANSWER_LIMIT, BIND_START_LIMIT, CREDIT_LIMIT, DOWNLOAD_LIMIT,
        FIRST_RUN_FILE_LIMIT, KEY_SAVE_LIMIT, LOOKUP_LIMIT, MANIFEST_LIMIT, STARTUP_LIMIT,
        STARTUP_READ_LIMIT,
    },
};
use addict_core::{
    APP_ID,
    dirs::Dirs,
    failure::{
        Ended,
        capture::CaptureFailure,
        login_item::{LoginItemShown, StartupEnd},
        recognize::OcrFailure,
        selection::SelectionFailure,
    },
    input::{Source, window_state},
    translation::{Source as Language, reading_languages},
};
use addict_deepl::key::{self, KeyError, Secret, WriteError};
use addict_dictionary::release::{Manifest, Published};
use addict_engine::{
    recording::{AudioFailure, fetch_audio},
    work::{Job, Outcome, Work},
};
use addict_platform::{
    shortcut::{self, Action, KeyOwner, ShortcutEvent, ShortcutHandle},
    startup,
    tray::{self, TrayEvent, TrayHandle},
};
use futures_channel::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
};
use futures_util::StreamExt;
use gtk::{gdk, gio, glib, prelude::*};

use crate::{
    about::About,
    capture,
    dictionaries::{Articles, Dictionaries, Indexes},
    download::{self, Downloader},
    entries, evidence,
    first_run::{self, Offer},
    manual,
    recording::Recording,
    settings::{self, Settings},
    style,
    view::{Binding, Choice, FailedEndFocus, Held, failed_end_focus},
    widgets::{FieldEdit, KEY_LOCATION_HINT, PlaybackMedia, View},
};

/// Preferred trigger in the XDG shortcuts syntax; the user may change it.
const TRIGGER: &str = "LOGO+q";

/// How long one attempt to bind the shortcuts may take: a minute until the
/// bind is asked for, and ten minutes for its answer, which on GNOME is a
/// person's in a dialog that the window may cover on a first launch.
const BIND_LIMITS: shortcut::Limits = shortcut::Limits {
    start: BIND_START_LIMIT,
    answer: BIND_ANSWER_LIMIT,
};

/// What lives as long as the resident application.
struct Resident {
    _hold: gio::ApplicationHoldGuard,
    /// The worker of the current attempt; Retry replaces it.
    _shortcut: Rc<RefCell<Option<ShortcutHandle>>>,
    /// Where a repeated launch reports itself.
    events: UnboundedSender<Event>,
    /// Owns the lookup loop and the shared runner.
    _controller: Option<Controller>,
    /// The icon in the top bar and its menu.
    _tray: TrayHandle,
}

/// Identity of the text and article under the rendered GTK links.
#[derive(Clone, Copy, Default)]
struct ViewActions {
    text: u64,
    article: ArticleId,
    choice: Option<Choice>,
}

/// Everything the worker threads tell the UI thread, over one channel.
enum Event {
    /// Presentation and input actions shared by the main window and launchers.
    Window(WindowEvent),
    /// The text of the field changed; this is the new text.
    Sentence(String, FieldEdit),
    /// The word with this index in the shown sentence was clicked.
    Word(u64, usize),
    /// A synonym word was clicked: it goes into the field.
    Synonym(ArticleId, String),
    /// The sense with this index in the shown list was clicked.
    Sense(ArticleId, usize),
    /// The English gloss with this index in the shown article was clicked.
    Gloss(ArticleId, usize),
    /// The text of the field was submitted, by Enter or by a paste.
    Submitted,
    /// Back was pressed, or Alt+Left.
    Back,
    /// Forward was pressed, or Alt+Right.
    Forward,
    /// The user explicitly requested the selected dictionary definition.
    DefinitionAction(ArticleId, Choice),
    /// The English word of a gloss was clicked: it is looked up in English.
    LookUpEnglish(ArticleId, usize, String),
    /// The shared controller's complete snapshot and effects.
    Controller(Box<ViewState>, Vec<Effect>),
    /// A setting was changed in the window, or an OS effect answered: the
    /// controller decides everything that follows.
    Setting(controller::Event),
    /// The check at the start says where the dictionary indexes stand.
    Indexes {
        generation: u64,
        indexes: Indexes,
    },
    /// The post-import check finished off-thread. Acknowledge only after
    /// the resident cache and view have received these states.
    ImportRefreshing(oneshot::Sender<u64>),
    Imported(ImportCompletion),
    SettingsDownload,
    RecoveryPrepared(u64, Outcome<Result<Manifest, String>>),
    /// The first-run offer to download a dictionary is due; this is the
    /// manifest of the published indexes.
    FirstRunOffer(u64, Manifest),
    /// Download was pressed in the first-run dialog.
    FirstRunDownload(u64),
    /// Not now was pressed in the first-run dialog.
    FirstRunNotNow(u64),
    /// This many bytes of the asset of the named index came.
    FirstRunProgress(u64, String, u64),
    /// Cancel was pressed on the row of the named index, or, with `None`,
    /// the dialog's own.
    FirstRunCancel(u64, Option<String>),
    /// The download and the install of the named index ended: its path, or
    /// why not, in one line.
    FirstRunInstalled(u64, String, Outcome<Result<std::path::PathBuf, String>>),
    /// Capture was pressed.
    Capture,
    /// The window hidden for a capture is off the screen.
    Hidden(RequestId),
    /// A shell capture effect finished and can release its cancellation handle.
    CaptureFinished(RequestId, controller::Event),
    /// The window became active.
    Active,
    /// A choice of the tray's menu, or its end.
    Tray(TrayEvent),
    /// The selection read with this id ended: the raw texts of the primary
    /// selection and the clipboard. The controller applies its rule.
    SelectionRead(RequestId, controller::SelectionText),
}

/// A queued controller render batch.
type ControllerQueue = Arc<Mutex<VecDeque<(ViewState, Vec<Effect>)>>>;

/// Starts one attempt to bind the shortcuts; its events go to `sender`. With
/// `timed`, an activation carries the time the worker thread received it.
fn start_shortcut(
    sender: &UnboundedSender<Event>,
    timed: bool,
    config: Option<&Path>,
) -> ShortcutHandle {
    let sender = sender.clone();
    let file = entries::shortcut_file(config);
    shortcut::start(
        APP_ID,
        KeyOwner::detect(),
        TRIGGER,
        BIND_LIMITS,
        file,
        move |event| {
            // Here, on the worker: the UI thread may be busy when it arrives.
            let receipt_ns = (timed && matches!(event, ShortcutEvent::Activated { .. }))
                .then(evidence::monotonic_ns);
            let _ = sender.unbounded_send(Event::Window(WindowEvent::Shortcut(event, receipt_ns)));
        },
    )
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

/// The button of the main menu: Settings and About, and Exit apart. These are
/// the tray's choices, here too as a desktop may have no tray.
fn main_menu() -> gtk::MenuButton {
    let window_items = gio::Menu::new();
    window_items.append(Some("Settings"), Some("app.settings"));
    window_items.append(Some("About Addict"), Some("app.about"));
    let exit_items = gio::Menu::new();
    exit_items.append(Some("Exit"), Some("app.exit"));
    let menu = gio::Menu::new();
    menu.append_section(None, &window_items);
    menu.append_section(None, &exit_items);
    let popover = gtk::PopoverMenu::builder()
        .menu_model(&menu)
        .has_arrow(false)
        .css_classes(["main-menu"])
        .build();
    gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main menu")
        .valign(gtk::Align::Center)
        .primary(true)
        .popover(&popover)
        .css_classes(["main-menu"])
        .build()
}

/// The hidden window around the `view`: closing it hides it, and so does Esc.
fn build_window(application: &gtk::Application, view: &View) -> gtk::ApplicationWindow {
    let header = header_bar("Addict");
    header.pack_end(&main_menu());
    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Addict")
        .default_width(WINDOW_WIDTH)
        .default_height(WINDOW_HEIGHT)
        .child(view.root())
        .css_classes(["addict"])
        .titlebar(&header)
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
    dirs: Dirs,
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
    /// The controller's snapshot rendered last: the field's text and words,
    /// what was chosen in it, the shown result and the history buttons.
    view_state: ViewState,
    view_actions: Rc<Cell<ViewActions>>,
    /// The last applied `field.revision`; controller text applies on a rise.
    applied_field_revision: u64,
    /// The text of the field the controller was last told of, or set.
    sent_text: String,
    /// How many `FieldEdited` events were sent to the controller.
    edits_sent: u64,
    /// `edits_sent` when an event that lets the controller set the field
    /// (Back, Forward, a synonym, clearing) was sent last. A controller text
    /// applies only while no edit followed it: a newer one is on its way.
    edits_at_set: u64,
    /// Advances when the shell handles Read, so older snapshots and capture
    /// presentation cannot overwrite the locally cleared field or its focus.
    read_sequence: u64,
    /// Weak: the runner must go when the resident state is dropped. Never
    /// upgrades when the runner could not be started.
    work: WorkRef<Work>,
    controller: Option<Dispatcher>,
    articles: Arc<dyn Articles>,
    /// Where the Linux key file is, for the `StoreKey` effect.
    key_file: Option<PathBuf>,
    /// The jobs of the settings effects; each is kept until its one result
    /// arrives, since dropping a `Job` cancels it.
    store_job: Option<Job>,
    startup_job: Option<Job>,
    startup_read_job: Option<Job>,
    /// A user choice of my language made before the files were read; its
    /// side effects run when they are, and only for that choice.
    my_choice_pending: Option<(Language, u64)>,
    /// The Settings window, reached from the main window or tray.
    settings: Settings,
    /// The About window, which the tray shows.
    about: About,
    /// The check of the indexes at the start, or the last one a Spanish or
    /// Catalan word without its indexes asked for, which replaces any still running;
    /// dropping it cancels the check.
    check: Option<Job>,
    /// Where the indexes stood when last asked, by a check or a lookup;
    /// `None` until the first answer.
    indexes: IndexCache,
    /// Where the capture stands.
    capturing: Capturing,
    /// Where the read of the selection and the clipboard of a press stands.
    copying: Copying,
    /// Where the first-run offer to download a dictionary stands.
    first_run: FirstRun,
    download_run: u64,
    /// Where the dictionaries are downloaded from: [`download::BASE_URL`],
    /// a stub in the tests.
    release: String,
    /// Independent of recovery, which must never cancel a remembered choice.
    first_run_choice: Option<Job>,
}

/// Reject checks and lookups that started before an in-app import's
/// refresh. A late old answer must not put Missing back in the cache.
#[derive(Default)]
struct IndexCache {
    generation: u64,
    known: Option<Indexes>,
    known_generation: u64,
}

impl IndexCache {
    fn update(&mut self, generation: u64, indexes: Indexes) -> bool {
        if generation != self.generation {
            return false;
        }
        self.known = Some(indexes);
        self.known_generation = generation;
        true
    }

    fn current(&self) -> Option<Indexes> {
        self.known
            .filter(|_| self.known_generation == self.generation)
    }

    fn begin_import(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }
}

struct ImportCompletion {
    generation: u64,
    indexes: Indexes,
    ready: oneshot::Sender<()>,
}

impl ImportCompletion {
    fn apply(self, apply: impl FnOnce(u64, Indexes)) {
        apply(self.generation, self.indexes);
        let _ = self.ready.send(());
    }
}

/// Where the first-run offer of a [`Session`] stands. When the index check
/// and my-language read have resolved, the runner reads the choice file and
/// fetches the manifest if all required indexes are missing. The dialog
/// waits for a visible window on a `--background` launch.
enum FirstRun {
    /// Waiting for the startup results.
    Unasked,
    /// The choice file is read and the manifest fetched; dropping the job
    /// gives them up.
    Asking {
        _job: Job,
    },
    Preparing {
        _job: Job,
    },
    /// The offer is due and waits for the window to be shown.
    Pending(Manifest),
    /// The dialog is shown and waits for a press.
    Shown {
        offer: Offer,
        manifest: Manifest,
        recovery: bool,
    },
    /// Download was pressed: the indexes of the queue are installed one
    /// after another into `dir`. `running` is the index whose job runs;
    /// dropping the job gives that one up.
    Installing {
        offer: Offer,
        queue: first_run::Queue,
        dir: std::path::PathBuf,
        running: Option<(String, Job)>,
        recovery: bool,
    },
    /// The install ended, and the dialog says how.
    Ended {
        _offer: Offer,
    },
    /// Nothing is offered in this run.
    Done,
}

/// GTK presentation state and cancellation handles for controller effects.
#[derive(Default)]
struct Capturing {
    pending: HashMap<RequestId, CaptureEffect>,
    activation_token: Option<String>,
    edits_at_recognition: u64,
}

enum CaptureEffect {
    Settling {
        read_sequence: u64,
        hidden: bool,
    },
    Picker {
        cancel: Option<tokio::sync::oneshot::Sender<()>>,
        _job: Job,
    },
    Recognition(Job),
}

/// Where the read of the selection and the clipboard of a [`Session`] stands;
/// at first none runs.
#[derive(Default)]
struct Copying {
    /// The read, from its effect to its one result.
    read: Option<ClipboardRead>,
}

/// One read of the primary selection and the clipboard.
struct ClipboardRead {
    /// The controller's request; its result event carries it back.
    id: RequestId,
    /// Cancels both reads; `None` while the window is not yet active and
    /// nothing is read.
    cancellable: Option<gio::Cancellable>,
}

impl Session {
    fn handle(&mut self, event: Event) {
        self.dispatch(event);
        self.first_indexes();
        self.offer_when_shown();
    }

    fn dispatch(&mut self, event: Event) {
        match event {
            Event::Window(event) => dispatch_window(self, event),
            Event::Tray(event) => self.tray(event),
            Event::Setting(event) => self.dispatch_controller(event),
            Event::Sentence(text, origin) => self.sentence_changed(text, origin),
            Event::Word(text, index) => self.word_chosen(text, index),
            Event::Synonym(article, word) => self.synonym_chosen(article, word),
            Event::Sense(article, index) => {
                self.article_action(
                    article,
                    addict_controller::Event::SenseChosen { article, index },
                );
            }
            Event::Gloss(article, index) => {
                self.article_action(
                    article,
                    addict_controller::Event::GlossChosen { article, index },
                );
            }
            Event::Submitted => self.submitted(),
            Event::Back => self.back(),
            Event::Forward => self.forward(),
            Event::DefinitionAction(article, choice) => self.article_action(
                article,
                addict_controller::Event::DefinitionAction { article, choice },
            ),
            Event::LookUpEnglish(article, index, word) => {
                self.look_up_english(article, index, word)
            }
            Event::Controller(state, effects) => self.render_controller(*state, effects),
            Event::Indexes {
                generation,
                indexes,
            } => {
                if self.indexes.update(generation, indexes) {
                    self.check = None;
                    self.view
                        .render_indexes(indexes, self.view_state.settings.my_language);
                    self.relist_first_run();
                    self.report_indexes();
                }
            }
            Event::ImportRefreshing(ready) => {
                self.check = None;
                let _ = ready.send(self.indexes.begin_import());
                self.report_indexes();
            }
            Event::Imported(completed) => completed.apply(|generation, indexes| {
                if self.indexes.update(generation, indexes) {
                    self.check = None;
                    self.view
                        .render_indexes(indexes, self.view_state.settings.my_language);
                    self.relist_first_run();
                    self.report_indexes();
                }
            }),
            Event::SettingsDownload => self.settings_download(),
            Event::RecoveryPrepared(run, outcome) => self.recovery_prepared(run, outcome),
            Event::FirstRunOffer(run, manifest) if run == self.download_run => {
                self.first_run_offer(manifest)
            }
            Event::FirstRunDownload(run) if run == self.download_run => self.first_run_download(),
            Event::FirstRunNotNow(run) if run == self.download_run => self.first_run_not_now(),
            Event::FirstRunProgress(run, name, received) if run == self.download_run => {
                self.first_run_progress(&name, received)
            }
            Event::FirstRunCancel(run, name) if run == self.download_run => {
                self.first_run_cancel(name)
            }
            Event::FirstRunInstalled(run, name, outcome) if run == self.download_run => {
                self.first_run_installed(&name, outcome)
            }
            Event::FirstRunOffer(..)
            | Event::FirstRunDownload(..)
            | Event::FirstRunNotNow(..)
            | Event::FirstRunProgress(..)
            | Event::FirstRunCancel(..)
            | Event::FirstRunInstalled(..) => {}
            Event::Capture => self.capture(None),
            Event::Hidden(id) => self.capture_hidden(id),
            Event::CaptureFinished(id, result) => {
                let finished = self.capturing.pending.remove(&id);
                self.dispatch_controller(result);
                if matches!(finished, Some(CaptureEffect::Picker { .. })) {
                    let ready = self.capturing.pending.iter().find_map(|(&id, effect)| {
                        matches!(effect, CaptureEffect::Settling { hidden: true, .. }).then_some(id)
                    });
                    if let Some(id) = ready {
                        self.start_capture(id);
                    }
                }
            }
            Event::Active => self.window_active(),
            Event::SelectionRead(id, text) => self.selection_read(id, text),
        }
    }

    /// Asks for the offer once startup has the language and index results.
    /// An offer still waiting for the manifest or window uses the latest
    /// accepted index snapshot.
    fn first_indexes(&mut self) {
        if !matches!(
            self.first_run,
            FirstRun::Unasked | FirstRun::Asking { .. } | FirstRun::Pending(_)
        ) || !self.view_state.settings.languages_read
        {
            return;
        }
        let Some(indexes) = self.indexes.current() else {
            return;
        };
        let my = self.view_state.settings.my_language;
        let due = first_run::should_offer(&indexes, my, false);
        match self.first_run {
            FirstRun::Unasked if self.check.is_some() => {}
            FirstRun::Unasked if due => {
                self.download_run += 1;
                self.first_run =
                    self.work
                        .upgrade()
                        .map_or(FirstRun::Done, |work| FirstRun::Asking {
                            _job: start_first_run(
                                &work,
                                Downloader::with_base_url(self.release.clone()),
                                first_run::path(self.dirs.config.as_deref()),
                                indexes,
                                my,
                                &self.sender,
                                self.download_run,
                            ),
                        });
            }
            FirstRun::Unasked => self.first_run = FirstRun::Done,
            FirstRun::Pending(_) | FirstRun::Asking { .. } if !due => {
                self.first_run = FirstRun::Done;
            }
            _ => {}
        }
    }

    /// The offer is due: it waits for the window, with every index of
    /// `manifest`. A manifest without an index offers nothing.
    fn first_run_offer(&mut self, manifest: Manifest) {
        if !matches!(self.first_run, FirstRun::Asking { .. }) {
            return;
        }
        self.first_run = if manifest.indexes.is_empty() {
            eprintln!("addict: no dictionary download is offered: the manifest lists no index");
            FirstRun::Done
        } else {
            FirstRun::Pending(manifest)
        };
    }

    /// Shows the dialog of a pending offer once the window is shown.
    fn offer_when_shown(&mut self) {
        if !matches!(self.first_run, FirstRun::Pending(_))
            || !self.window.is_visible()
            || self.indexes.current().is_none()
        {
            return;
        }
        let FirstRun::Pending(manifest) = std::mem::replace(&mut self.first_run, FirstRun::Done)
        else {
            return;
        };
        self.show_download_offer(manifest, false);
    }

    fn show_download_offer(&mut self, manifest: Manifest, recovery: bool) {
        let download = self.sender.clone();
        let not_now = self.sender.clone();
        let cancel = self.sender.clone();
        let chosen = self.sender.clone();
        let run = self.download_run;
        let (listed, my) = if recovery {
            (self.recovery_indexes(&manifest), None)
        } else {
            (
                self.first_run_indexes(&manifest),
                Some(self.view_state.settings.my_language),
            )
        };
        let parent: &gtk::Window = if recovery {
            self.settings.window()
        } else {
            self.window.upcast_ref()
        };
        let offer = Offer::new(
            parent,
            &listed,
            recovery,
            my,
            move |language| {
                let _ = chosen.unbounded_send(Event::Setting(
                    addict_controller::Event::MyLanguageChosen(language),
                ));
            },
            move || {
                let _ = download.unbounded_send(Event::FirstRunDownload(run));
            },
            move || {
                let _ = not_now.unbounded_send(Event::FirstRunNotNow(run));
            },
            move |name| {
                let _ = cancel.unbounded_send(Event::FirstRunCancel(run, name));
            },
        );
        offer.present();
        if recovery {
            self.settings
                .render_download(false, "The dictionary download dialog is open.");
        }
        self.first_run = FirstRun::Shown {
            offer,
            manifest,
            recovery,
        };
    }

    /// The indexes of `manifest` that the first-run offer lists and
    /// installs: those my language needs that the latest accepted snapshot
    /// does not show present. The rows and the queue both come from here,
    /// so they agree.
    fn first_run_indexes(&self, manifest: &Manifest) -> Vec<Published> {
        let indexes = self.indexes.known;
        first_run::offered(
            &manifest.indexes,
            self.view_state.settings.my_language,
            |name| indexes.is_some_and(|indexes| first_run::is_present(&indexes, name)),
        )
    }

    /// The indexes of a recovery manifest that the Settings offer lists
    /// and installs: those of my language's set. The manifest holds only
    /// what is missing or unreadable, so no presence is asked; indexes of
    /// other languages stay out, and none is removed.
    fn recovery_indexes(&self, manifest: &Manifest) -> Vec<Published> {
        first_run::offered(
            &manifest.indexes,
            self.view_state.settings.my_language,
            |_| false,
        )
    }

    /// An offer that waits for a press lists anew what my language needs,
    /// after my language or the snapshot changed. A recovery offer left
    /// with nothing to list closes, and Settings says why.
    fn relist_first_run(&mut self) {
        match &self.first_run {
            FirstRun::Shown {
                offer,
                manifest,
                recovery: false,
            } => {
                offer.render_my_language(self.view_state.settings.my_language);
                offer.list(&self.first_run_indexes(manifest));
            }
            FirstRun::Shown {
                offer,
                manifest,
                recovery: true,
            } => {
                let listed = self.recovery_indexes(manifest);
                if listed.is_empty() {
                    offer.close();
                    self.first_run = FirstRun::Done;
                    self.settings.render_download(
                        false,
                        &first_run::nothing_text(self.view_state.settings.my_language),
                    );
                } else {
                    offer.list(&listed);
                }
            }
            _ => {}
        }
    }

    /// A change of my language in Settings offers what the new language's
    /// set lacks, through the Settings offer for missing indexes. Only when
    /// no offer, preparation or install is under way: the first-run offer
    /// decides for itself, and a waiting offer follows by
    /// [`Self::relist_first_run`].
    fn offer_missing_for_my_language(&mut self) {
        // A waiting recovery offer, re-listed already, may have been closed
        // (it only hides): it is shown again.
        if let FirstRun::Shown {
            offer,
            recovery: true,
            ..
        } = &self.first_run
        {
            offer.present();
            return;
        }
        if !matches!(self.first_run, FirstRun::Done | FirstRun::Ended { .. }) {
            return;
        }
        let lacks = self.indexes.known.is_some_and(|indexes| {
            !first_run::missing(&indexes, self.view_state.settings.my_language).is_empty()
        });
        if lacks {
            self.settings_download();
        }
    }

    /// Explicit recovery supersedes an unstarted automatic offer, while an
    /// active preparation or installation keeps its job and progress.
    fn settings_download(&mut self) {
        match &self.first_run {
            FirstRun::Preparing { .. } => return,
            FirstRun::Installing { offer, .. }
            | FirstRun::Shown {
                offer,
                recovery: true,
                ..
            } => {
                offer.present();
                return;
            }
            _ => {}
        }
        let (Some(work), Some(dir)) = (self.work.upgrade(), self.dirs.data.clone()) else {
            self.settings
                .render_download(false, &first_run::recovery_failed_text(first_run::NO_PLACE));
            return;
        };
        self.download_run += 1;
        let run = self.download_run;
        let prepared = self.sender.clone();
        let downloader = Downloader::with_base_url(self.release.clone());
        // The manifest has its own deadline inside; the health scan of
        // every installed index needs the longer one.
        let job = work.run(
            DOWNLOAD_LIMIT,
            async move {
                download::recovery_manifest(&downloader, dir, MANIFEST_LIMIT)
                    .await
                    .map_err(|error| error.to_string())
            },
            move |outcome| {
                let _ = prepared.unbounded_send(Event::RecoveryPrepared(run, outcome));
            },
        );
        self.first_run = FirstRun::Preparing { _job: job };
        self.settings.render_download(true, first_run::FETCHING);
    }

    fn recovery_prepared(&mut self, run: u64, outcome: Outcome<Result<Manifest, String>>) {
        if run != self.download_run || !matches!(self.first_run, FirstRun::Preparing { .. }) {
            return;
        }
        self.first_run = FirstRun::Done;
        match outcome {
            Outcome::Finished(Ok(manifest)) if self.recovery_indexes(&manifest).is_empty() => {
                self.settings.render_download(
                    false,
                    &first_run::nothing_text(self.view_state.settings.my_language),
                );
            }
            Outcome::Finished(Ok(manifest)) => self.show_download_offer(manifest, true),
            outcome => {
                let why = match outcome {
                    Outcome::Finished(Err(error)) => error,
                    Outcome::TimedOut => "the dictionary check did not finish in time".to_owned(),
                    _ => first_run::STOPPED.to_owned(),
                };
                self.settings
                    .render_download(false, &first_run::recovery_failed_text(&why));
            }
        }
    }

    /// Download was pressed: the indexes of the manifest are downloaded and
    /// installed on the runner, one after another, each within
    /// [`DOWNLOAD_LIMIT`].
    fn first_run_download(&mut self) {
        if !matches!(self.first_run, FirstRun::Shown { .. }) {
            return;
        }
        let FirstRun::Shown {
            offer,
            manifest,
            recovery,
        } = std::mem::replace(&mut self.first_run, FirstRun::Done)
        else {
            return;
        };
        offer.installing();
        let indexes = if recovery {
            self.recovery_indexes(&manifest)
        } else {
            self.first_run_indexes(&manifest)
        };
        let mut queue = first_run::Queue::new(indexes);
        let (Some(_), Some(dir)) = (self.work.upgrade(), self.dirs.data.clone()) else {
            let why = first_run::NO_PLACE;
            eprintln!("addict: the dictionary download failed: {why}");
            queue.cancel_all();
            offer.show(&queue);
            let text = if recovery {
                first_run::recovery_failed_text(why)
            } else {
                first_run::failed_text(why)
            };
            offer.ended(first_run::FAILED_HEADING, &text, false);
            self.settings.render_download(false, &text);
            self.first_run = FirstRun::Ended { _offer: offer };
            return;
        };
        offer.show(&queue);
        self.first_run = FirstRun::Installing {
            offer,
            queue,
            dir,
            running: None,
            recovery,
        };
        self.first_run_next();
    }

    /// Starts the next queued index when none runs; once every index has
    /// ended, the dialog says how it went and the indexes are checked
    /// again, so the line of the window follows.
    fn first_run_next(&mut self) {
        let FirstRun::Installing {
            offer,
            queue,
            dir,
            running,
            recovery,
        } = &mut self.first_run
        else {
            return;
        };
        if running.is_some() {
            return;
        }
        while let Some(published) = queue.next() {
            let Some(work) = self.work.upgrade() else {
                queue.finished(&published.name, Err(first_run::NO_PLACE.to_owned()));
                continue;
            };
            let name = published.name.clone();
            let dir = dir.clone();
            let progress = self.sender.clone();
            let installed = self.sender.clone();
            let (of, ended) = (name.clone(), name.clone());
            let run = self.download_run;
            let recovery = *recovery;
            let articles = self.articles.clone();
            let refresh = self.sender.clone();
            let downloader = Downloader::with_base_url(self.release.clone());
            let job = work.run(
                DOWNLOAD_LIMIT,
                async move {
                    install_download(
                        downloader,
                        published,
                        dir,
                        recovery,
                        articles,
                        refresh,
                        move |received| {
                            let _ = progress.unbounded_send(Event::FirstRunProgress(
                                run,
                                of.clone(),
                                received,
                            ));
                        },
                    )
                    .await
                },
                move |outcome| {
                    // On the worker: the UI thread wakes for the event.
                    let _ = installed.unbounded_send(Event::FirstRunInstalled(run, ended, outcome));
                },
            );
            *running = Some((name, job));
            offer.show(queue);
            return;
        }
        offer.show(queue);
        if !queue.is_done() {
            return;
        }
        let FirstRun::Installing {
            offer,
            queue,
            recovery,
            ..
        } = std::mem::replace(&mut self.first_run, FirstRun::Done)
        else {
            return;
        };
        let text = if recovery {
            first_run::recovery_summary_text(&queue)
        } else {
            first_run::summary_text(&queue)
        };
        offer.ended(
            first_run::ended_heading(&queue),
            &text,
            first_run::is_complete(&queue),
        );
        self.settings.render_download(false, &text);
        self.first_run = FirstRun::Ended { _offer: offer };
    }

    /// Bytes of the running index came: its row says how many.
    fn first_run_progress(&mut self, name: &str, received: u64) {
        let FirstRun::Installing {
            offer,
            queue,
            running: Some((running, _)),
            ..
        } = &mut self.first_run
        else {
            return;
        };
        if running != name {
            return;
        }
        queue.progress(name, received);
        offer.show(queue);
    }

    /// Cancel was pressed on a row, or on the dialog: that index, or every
    /// index that has not ended, ends as cancelled. The job of the running
    /// one is dropped, so nothing of it is put in place, and the next index
    /// starts.
    fn first_run_cancel(&mut self, name: Option<String>) {
        let FirstRun::Installing {
            offer,
            queue,
            running,
            ..
        } = &mut self.first_run
        else {
            return;
        };
        let was_running = match &name {
            Some(name) => queue.cancel(name),
            None => queue.cancel_all(),
        };
        if was_running {
            // Its outcome, Cancelled, comes later and is ignored.
            *running = None;
        }
        // A queued row says Cancelled at once, while another one runs.
        offer.show(queue);
        self.first_run_next();
    }

    /// The running index ended: its row says how, and the next one starts.
    /// An end of an index that no longer runs, as one cancelled, changes
    /// nothing.
    fn first_run_installed(
        &mut self,
        name: &str,
        outcome: Outcome<Result<std::path::PathBuf, String>>,
    ) {
        let FirstRun::Installing { queue, running, .. } = &mut self.first_run else {
            return;
        };
        if running.as_ref().is_none_or(|(running, _)| running != name) {
            return;
        }
        *running = None;
        let result = match outcome {
            Outcome::Finished(Ok(_)) => Ok(()),
            Outcome::Finished(Err(error)) => Err(error),
            Outcome::TimedOut => Err(first_run::late_text(DOWNLOAD_LIMIT)),
            Outcome::Cancelled | Outcome::Panicked => Err(first_run::STOPPED.to_owned()),
        };
        if let Err(error) = &result {
            eprintln!(
                "addict: the dictionary download failed: {}",
                first_run::failure_text(name, error)
            );
        }
        queue.finished(name, result);
        self.first_run_next();
    }

    /// Not now was pressed: the choice is written on the runner, and the
    /// offer is not made again. A write that fails is written to stderr.
    fn first_run_not_now(&mut self) {
        let FirstRun::Shown { recovery, .. } = &self.first_run else {
            return;
        };
        if *recovery {
            self.first_run = FirstRun::Done;
            self.settings.render_download(false, "");
            return;
        }
        let path = first_run::path(self.dirs.config.as_deref());
        let job = self.work.upgrade().map(|work| {
            work.run(
                FIRST_RUN_FILE_LIMIT,
                async move { first_run::write(path.as_deref()) },
                |outcome| match outcome {
                    Outcome::Finished(Ok(())) | Outcome::Cancelled => {}
                    Outcome::Finished(Err(e)) => {
                        eprintln!(
                            "addict: the answer to the dictionary download was not kept: {e}"
                        );
                    }
                    Outcome::TimedOut | Outcome::Panicked => {
                        eprintln!("addict: the answer to the dictionary download was not kept");
                    }
                },
            )
        });
        if job.is_none() {
            eprintln!("addict: the answer to the dictionary download was not kept: no runner");
        }
        self.first_run_choice = job;
        self.first_run = FirstRun::Done;
    }

    fn capture(&mut self, activation_token: Option<&str>) {
        // The window stops waiting for a read; the controller cancels the
        // read itself, before any result still queued in the Read adapter.
        self.give_up_read();
        if self.controller.is_none() {
            let failure = Ended::Unfinished;
            self.capturing.activation_token = activation_token.map(str::to_owned);
            self.view
                .render_source_failure(&crate::view::capture_failure_text(&failure));
            self.reveal_capture(self.read_sequence, true);
            self.record_capture(evidence::Read::capture_failed(&failure), Default::default());
            return;
        }
        self.dispatch_controller(addict_controller::Event::CapturePressed {
            activation_token: activation_token.map(str::to_owned),
        });
    }

    fn hide_for_capture(
        &mut self,
        id: RequestId,
        read_sequence: u64,
        activation_token: Option<String>,
    ) {
        if read_sequence != self.read_sequence {
            self.picker_cancelled(id);
            return;
        }
        self.view.clear_source_failure();
        self.capturing.activation_token = activation_token;
        let settle = if self.window.is_visible() {
            HIDE_SETTLE
        } else {
            Duration::ZERO
        };
        // GNOME's picker freezes the screen when it opens. Give the
        // compositor time to finish animating the hidden window first.
        self.window.set_visible(false);
        self.capturing.pending.insert(
            id,
            CaptureEffect::Settling {
                read_sequence,
                hidden: false,
            },
        );
        let hidden = self.sender.clone();
        glib::timeout_add_local_once(settle, move || {
            let _ = hidden.unbounded_send(Event::Hidden(id));
        });
    }

    fn capture_hidden(&mut self, id: RequestId) {
        if let Some(CaptureEffect::Settling { hidden, .. }) = self.capturing.pending.get_mut(&id) {
            *hidden = true;
            self.start_capture(id);
        }
    }

    fn start_capture(&mut self, id: RequestId) {
        let Some(&CaptureEffect::Settling {
            read_sequence,
            hidden: true,
        }) = self.capturing.pending.get(&id)
        else {
            return;
        };
        if read_sequence != self.read_sequence {
            self.capturing.pending.remove(&id);
            self.picker_cancelled(id);
            return;
        }
        if self
            .capturing
            .pending
            .values()
            .any(|effect| matches!(effect, CaptureEffect::Picker { .. }))
        {
            return;
        }
        self.capturing.pending.remove(&id);
        self.dispatch_controller(addict_controller::Event::PickerStarted { id });
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        let sender = self.sender.clone();
        let work = self.work.upgrade();
        if let Some(job) = capture::start_picker(
            work.as_deref(),
            async move {
                let _ = cancelled.await;
            },
            move |result, capture_ms| {
                let _ = sender.unbounded_send(Event::CaptureFinished(
                    id,
                    addict_controller::Event::AreaCaptured {
                        id,
                        result,
                        capture_ms,
                    },
                ));
            },
        ) {
            self.capturing.pending.insert(
                id,
                CaptureEffect::Picker {
                    cancel: Some(cancel),
                    _job: job,
                },
            );
        }
    }

    fn picker_cancelled(&self, id: RequestId) {
        self.dispatch_controller(addict_controller::Event::AreaCaptured {
            id,
            result: Err(Ended::Shell(CaptureFailure::Cancelled)),
            capture_ms: None,
        });
    }

    fn recognize_capture(
        &mut self,
        id: RequestId,
        image: controller::ImageRef,
        language: Language,
        read_sequence: u64,
    ) {
        if read_sequence != self.read_sequence {
            drop(image);
            self.dispatch_controller(addict_controller::Event::TextRecognized {
                id,
                result: Err(Ended::Shell(OcrFailure::Cancelled)),
                timing: Default::default(),
            });
            return;
        }
        self.capturing.edits_at_recognition = self.edits_sent;
        let sender = self.sender.clone();
        let work = self.work.upgrade();
        if let Some(job) = capture::start_recognition(
            work.as_deref(),
            self.dirs.config.clone(),
            image,
            language,
            move |result, timing| {
                let _ = sender.unbounded_send(Event::CaptureFinished(
                    id,
                    addict_controller::Event::TextRecognized { id, result, timing },
                ));
            },
        ) {
            self.capturing
                .pending
                .insert(id, CaptureEffect::Recognition(job));
        }
    }

    fn cancel_capture_effect(&mut self, id: RequestId) {
        match self.capturing.pending.get_mut(&id) {
            Some(CaptureEffect::Settling { .. }) => {
                self.capturing.pending.remove(&id);
                self.picker_cancelled(id);
            }
            Some(CaptureEffect::Picker { cancel, .. }) => {
                if let Some(cancel) = cancel.take() {
                    let _ = cancel.send(());
                }
            }
            Some(CaptureEffect::Recognition(job)) => job.cancel(),
            None => {}
        }
    }

    fn reveal_capture(&mut self, read_sequence: u64, retry: bool) {
        if read_sequence != self.read_sequence {
            return;
        }
        if let Some(token) = self.capturing.activation_token.take() {
            self.window.set_startup_id(&token);
        }
        self.reveal();
        if retry {
            match failed_end_focus(&self.view_state.settings.binding) {
                FailedEndFocus::Capture => self.view.focus_capture(),
                FailedEndFocus::Retry => self.view.focus_retry(),
            }
        }
    }

    fn record_capture(&self, read: evidence::Read, timing: addict_engine::capture::Timing) {
        if let Some(recording) = &self.recording {
            recording.ocr_ended(&evidence::ocr_record(read, &timing));
        }
    }

    /// The window stops waiting for the read of the selection and the
    /// clipboard. The controller gives the read up on the actions that
    /// replace it, and its `Cancel` effect stops the GTK reads; this only
    /// ends what the window shows now. Whether a read ran.
    fn give_up_read(&mut self) -> bool {
        let read = self.copying.read.is_some();
        if self.view_state.reading_source == Some(Source::Clipboard) {
            self.set_reading_source(None);
        }
        read
    }

    /// Clears the field for the read that starts now: a new empty text, so
    /// the lookup and the online requests of the old one are given up, and
    /// the source line goes. The window waits with the source clipboard
    /// until the controller's snapshot comes.
    fn clear_for_read(&mut self) {
        self.view.clear_source_failure();
        // The controller clears the field (`field.revision` rises); the
        // window shows it cleared at once, before its snapshot comes, and
        // the snapshots queued before it are not rendered.
        self.read_sequence += 1;
        self.dispatch_setting_field(addict_controller::Event::ReadPressed {
            sequence: self.read_sequence,
        });
        self.view.set_field_text("");
        self.sent_text.clear();
        let view = &mut self.view_state;
        view.source_failure = None;
        view.reading_source = Some(Source::Clipboard);
        view.sentence.clear();
        view.words.clear();
        view.chosen = None;
        view.input = None;
        view.context = None;
        view.looking_up = false;
        view.shown.show(None);
        view.online.clear();
        self.view_actions.set(ViewActions::default());
        view.window_state = window_state(view.reading_source, None);
        self.render_manual();
        self.render_history();
        self.render_shown();
        self.render_online();
    }

    /// The read of the primary selection and the clipboard starts for `id`:
    /// the window waits with the source clipboard, and the GTK reads start
    /// once the window is active, at once when it is. A press that a newer
    /// one has replaced is given up.
    fn read_selection(&mut self, id: RequestId, read_sequence: u64) {
        if read_sequence != self.read_sequence {
            self.send_selection(id, Err(Ended::Shell(SelectionFailure::Cancelled)));
            return;
        }
        self.copying.read = Some(ClipboardRead {
            id,
            cancellable: None,
        });
        self.render_window();
        if self.window.is_active() {
            self.read_clipboard();
        }
    }

    /// The window became active: the read of the selection and the clipboard
    /// that waits for it starts.
    fn window_active(&mut self) {
        if self.window.is_active() {
            self.read_clipboard();
        }
    }

    /// Reads both the primary selection of the window's display and the
    /// clipboard for the read that runs and still waits, raw, and sends them
    /// in one result. A source with no text or a failed read gives `None`;
    /// the error goes to stderr. Both reads share its cancellable.
    fn read_clipboard(&mut self) {
        let Some(read) = self.copying.read.as_mut() else {
            return;
        };
        if read.cancellable.is_some() {
            return;
        }
        let cancellable = gio::Cancellable::new();
        read.cancellable = Some(cancellable.clone());
        let id = read.id;
        let sender = self.sender.clone();
        let display = WidgetExt::display(&self.window);
        let selection_clipboard = display.primary_clipboard();
        let clipboard = display.clipboard();
        read_text(
            &selection_clipboard,
            Held::Selection,
            &cancellable.clone(),
            move |selection| {
                read_text(
                    &clipboard,
                    Held::Clipboard,
                    &cancellable,
                    move |clipboard| {
                        let _ = sender.unbounded_send(Event::SelectionRead(
                            id,
                            controller::SelectionText {
                                selection,
                                clipboard,
                            },
                        ));
                    },
                );
            },
        );
    }

    /// The raw texts of the read with `id` came back; one given up since is
    /// ignored. The controller applies the line rule, the length limit and
    /// the source line.
    fn selection_read(&mut self, id: RequestId, text: controller::SelectionText) {
        if !self.copying.read.as_ref().is_some_and(|read| read.id == id) {
            return;
        }
        self.copying.read = None;
        self.send_selection(id, Ok(text));
    }

    /// Gives up a read whose `Cancel` came: both GTK reads are cancelled and
    /// the one cancelled result is sent. Whether `id` was the read.
    fn cancel_read_effect(&mut self, id: RequestId) -> bool {
        let Some(read) = self.copying.read.take() else {
            return false;
        };
        if read.id != id {
            self.copying.read = Some(read);
            return false;
        }
        if let Some(cancellable) = &read.cancellable {
            cancellable.cancel();
        }
        self.send_selection(id, Err(Ended::Shell(SelectionFailure::Cancelled)));
        true
    }

    /// A controller effect is given up: the read when it is that request,
    /// else the capture.
    fn cancel_effect(&mut self, id: RequestId) {
        if !self.cancel_read_effect(id) {
            self.cancel_capture_effect(id);
        }
    }

    /// Shows the window for the press when it is not shown for it yet: Linux
    /// presents at the press, so this does nothing there, and the order of
    /// a read-before-show platform is the controller's to decide.
    fn show_panel(&mut self, read_sequence: u64) {
        if read_sequence != self.read_sequence || self.window.is_visible() {
            return;
        }
        self.reveal();
    }

    /// Sends the one result of the read with `id` to the controller.
    fn send_selection(
        &mut self,
        id: RequestId,
        result: Result<controller::SelectionText, Ended<SelectionFailure>>,
    ) {
        self.dispatch_controller(addict_controller::Event::SelectionRead { id, result });
    }

    /// Shows the window with the state as it is now, the field focused, with
    /// no press of the clipboard shortcut behind it. No `set_startup_id` here:
    /// the caller gave a token if it has one, and for a launch GTK has already
    /// applied the launcher's.
    fn reveal(&self) {
        self.render_window();
        self.focus();
        if let Some(recording) = &self.recording {
            recording.shown_without_press();
        }
        self.window.present();
    }

    /// The field takes the keyboard focus, unless Retry is offered: then the
    /// focus is Retry's.
    fn focus(&self) {
        if !matches!(self.view_state.settings.binding, Binding::NotBound { .. }) {
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
    fn sentence_changed(&mut self, text: String, origin: FieldEdit) {
        self.give_up_for_new_text();
        // The edit ends the reading of a source in the controller too; its
        // snapshot renders the window.
        self.send_edit(text, matches!(origin, FieldEdit::Replaced), None);
    }

    /// What goes before another text from the user, typed, walked to or
    /// clicked: the source line, a snapshot being recognized and a read of
    /// the selection and the clipboard.
    fn give_up_for_new_text(&mut self) {
        self.view.clear_source_failure();
        self.give_up_read();
    }

    /// Tells the controller the field holds `text`, counted for the field
    /// guard ([`Session::apply_snapshot`]).
    fn send_edit(&mut self, text: String, replaced: bool, source: Option<Source>) {
        self.sent_text.clone_from(&text);
        self.edits_sent += 1;
        self.dispatch_controller(addict_controller::Event::FieldEdited {
            text,
            replaced,
            source,
        });
    }

    /// Sends an event that lets the controller set the field's text: its
    /// text applies only while no edit follows it.
    fn dispatch_setting_field(&mut self, event: controller::Event) {
        self.edits_at_set = self.edits_sent;
        self.dispatch_controller(event);
    }

    /// Back or Alt+Left: the text before in the history is put into the
    /// field by the controller, looked up and submitted again.
    fn back(&mut self) {
        self.give_up_for_new_text();
        self.dispatch_setting_field(addict_controller::Event::Back);
    }

    /// Forward or Alt+Right: the text after in the history is put into the
    /// field as Back puts one.
    fn forward(&mut self) {
        self.give_up_for_new_text();
        self.dispatch_setting_field(addict_controller::Event::Forward);
    }

    /// Whether Back and Forward can be pressed, for the field's text now.
    fn render_history(&self) {
        self.view
            .set_history(self.view_state.can_back, self.view_state.can_forward);
    }

    /// A word link belongs to the text it was drawn for.
    fn word_chosen(&mut self, text: u64, index: usize) {
        if self.field_is_newer() || self.snapshot_is_old() || text != self.view_state.text_number {
            return;
        }
        self.dispatch_controller(addict_controller::Event::WordChosen { text, index });
    }

    /// Records and submits the current text.
    fn submitted(&mut self) {
        if !self.field_is_newer() {
            self.dispatch_controller(addict_controller::Event::FieldSubmitted);
        }
    }

    /// Whether the field holds a newer text than the one last given to the
    /// controller: an event that came before that text was handled is
    /// about what is no longer shown.
    fn field_is_newer(&self) -> bool {
        self.view.field_text() != self.sent_text
    }

    /// Whether the field holds another text than the snapshot just
    /// rendered: what the snapshot asks is about what is no longer shown.
    fn snapshot_is_old(&self) -> bool {
        self.view.field_text() != self.view_state.sentence
    }

    /// Applies the newest snapshot and handles every effect, even when the
    /// visual state of that snapshot has already been superseded.
    fn render_controller(&mut self, state: ViewState, effects: Vec<Effect>) {
        if state.read_sequence == self.read_sequence && state.revision > self.view_state.revision {
            let previous = std::mem::replace(&mut self.view_state, state);
            self.apply_snapshot(&previous);
        }
        for effect in effects {
            self.run_effect(effect);
        }
    }

    fn apply_snapshot(&mut self, previous: &ViewState) {
        let current = &self.view_state;
        let field = &current.field;
        // Apply a controller-set field only while no newer edit is on its way.
        if field.revision > self.applied_field_revision {
            self.applied_field_revision = field.revision;
            let edits_at_set = if current.ocr_field_revision == Some(field.revision) {
                self.capturing.edits_at_recognition
            } else {
                self.edits_at_set
            };
            if self.edits_sent == edits_at_set && !self.field_is_newer() {
                self.view.set_field_text(&field.text);
                self.sent_text.clone_from(&field.text);
            }
        }
        if previous.capture_sensitive != current.capture_sensitive {
            self.view.set_capture_sensitive(current.capture_sensitive);
        }
        if previous.source_failure != current.source_failure {
            match &current.source_failure {
                Some(message) => self.view.render_source_failure(message),
                None => self.view.clear_source_failure(),
            }
        }
        self.view_actions.set(ViewActions {
            text: current.text_number,
            article: current.article_id(),
            choice: current.shown.choice(),
        });
        if previous.text_number != current.text_number
            || previous.chosen != current.chosen
            || previous.source != current.source
            || previous.window_state != current.window_state
        {
            self.render_manual();
        }
        if previous.can_back != current.can_back || previous.can_forward != current.can_forward {
            self.render_history();
        }
        if previous.shown.generation() != current.shown.generation()
            || previous.shown.translating() != current.shown.translating()
            || previous.choice_revision != current.choice_revision
        {
            self.render_shown();
        } else if previous.shown.definition() != current.shown.definition() {
            self.view
                .render_definition(&current.shown, self.key_file.as_deref());
        }
        if previous.online != current.online {
            self.render_online();
        }
        let my = current.settings.my_language;
        let reading = current.settings.reading;
        let languages_read = current.settings.languages_read;
        if previous.settings.my_language != my || previous.settings.reading != reading {
            self.view
                .set_reading_languages(reading_languages(my), reading);
            self.view.set_language(reading);
            // The definition button names my language, so it follows a
            // choice made in Settings too. The indexes line does as well.
            self.render_shown();
            if let Some(indexes) = self.indexes.known {
                self.view.render_indexes(indexes, my);
            }
        }
        self.settings.render(&current.settings);
        // The main window's binding row follows both the binding state and
        // its assignments: a `Changed` report can keep `Bound` and still
        // change what the row says.
        if previous.settings.binding != current.settings.binding
            || previous.settings.bindings != current.settings.bindings
        {
            self.view.render_binding(&current.settings.binding);
        }
        // My-language side effects belong to a user choice, not to the read
        // of the file at start. The controller's counter tells the two apart.
        let chosen = (my, current.settings.my_chosen);
        if previous.settings.my_chosen != current.settings.my_chosen
            && previous.settings.my_language != my
        {
            self.my_choice_pending = Some(chosen);
        }
        if let Some(pending) = self.my_choice_pending {
            if languages_read {
                self.my_choice_pending = None;
                if let Some(indexes) = self.indexes.known {
                    self.view.render_indexes(indexes, pending.0);
                }
                self.relist_first_run();
                // The first-run decision for the new language first: an
                // offer it ends leaves the set to the Settings offer.
                self.first_indexes();
                self.offer_missing_for_my_language();
            }
        } else if !previous.settings.languages_read && languages_read {
            // The start read ended: only the indexes line and the first-run
            // decision follow. No Settings offer opens on its own here.
            if let Some(indexes) = self.indexes.known {
                self.view.render_indexes(indexes, my);
            }
            self.first_indexes();
        }
    }

    /// Runs a controller effect.
    fn run_effect(&mut self, effect: Effect) {
        match effect {
            Effect::CaptureArea {
                id,
                read_sequence,
                activation_token,
            } => {
                self.hide_for_capture(id, read_sequence, activation_token);
            }
            Effect::Recognize {
                id,
                image,
                language,
                read_sequence,
            } => {
                self.recognize_capture(id, image, language, read_sequence);
            }
            Effect::Cancel(id) => self.cancel_effect(id),
            Effect::ReadSelection { id, read_sequence } => {
                self.read_selection(id, read_sequence);
            }
            Effect::ShowPanel { read_sequence } => self.show_panel(read_sequence),
            Effect::PresentCapture {
                read_sequence,
                retry,
            } => self.reveal_capture(read_sequence, retry),
            Effect::RecordCapture { read, timing } => self.record_capture(read, timing),
            Effect::CheckIndexes { index_generation } => {
                self.check_lookup_indexes(index_generation);
            }
            Effect::ObserveIndexes {
                generation,
                indexes,
            } => {
                self.observe_lookup_indexes(generation, indexes);
            }
            Effect::RecordLookup(answered) => {
                if let Some(recording) = &self.recording {
                    recording.looked_up(answered);
                }
            }
            Effect::RecordOnline(record) => {
                if let Some(recording) = &self.recording {
                    recording.translated_online(&record);
                }
            }
            Effect::StoreKey { id, key } => self.store_key(id, key),
            Effect::SetLoginItem { id, on } => self.set_login_item(id, on),
            Effect::ReadLoginItem => self.read_login_item(),
            Effect::BindShortcuts => self.bind_shortcuts(),
            Effect::ConfigureShortcuts => self.configure_shortcuts(),
        }
    }

    /// The lookup of the shown word found a dictionary missing: the indexes
    /// are checked again, unless a newer refresh, an import's, already owns
    /// that.
    fn check_lookup_indexes(&mut self, index_generation: u64) {
        if index_generation != self.indexes.generation {
            return;
        }
        self.check = start_check(
            self.work.upgrade().as_deref(),
            self.articles.clone(),
            &self.sender,
            self.indexes.generation,
        );
    }

    fn observe_lookup_indexes(&mut self, generation: u64, indexes: Indexes) {
        if self.indexes.update(generation, indexes) {
            self.view
                .render_indexes(indexes, self.view_state.settings.my_language);
            self.report_indexes();
        }
    }

    /// Tells the controller where the `IndexCache` stands now: its lookups
    /// start from what it knows, and carry its generation.
    fn report_indexes(&self) {
        self.dispatch_controller(addict_controller::Event::KnownIndexes {
            generation: self.indexes.generation,
            indexes: self.indexes.known,
        });
    }

    /// Updates the word and sentence rows without rebuilding the article.
    fn render_online(&self) {
        self.view.render_online_rows(&self.view_state.online);
    }

    /// Shows the result and the sense chosen in it as they are now.
    fn render_shown(&self) {
        self.view
            .render_result(&self.view_state.shown, self.key_file.as_deref());
    }

    fn accepts_article(&self, article: ArticleId) -> bool {
        !self.field_is_newer() && !self.snapshot_is_old() && article == self.view_state.article_id()
    }

    fn article_action(&self, article: ArticleId, event: controller::Event) {
        if self.accepts_article(article) {
            self.dispatch_controller(event);
        }
    }

    /// A synonym submits a replacement field in the current reading language.
    fn synonym_chosen(&mut self, article: ArticleId, word: String) {
        if self.accepts_article(article) {
            self.give_up_for_new_text();
            self.dispatch_setting_field(addict_controller::Event::SynonymChosen { article, word });
        }
    }

    /// The English link submits the gloss's word; the controller owns the
    /// reading language it chooses on the way.
    fn look_up_english(&mut self, article: ArticleId, index: usize, word: String) {
        if !self.accepts_article(article) {
            return;
        }
        self.give_up_for_new_text();
        self.dispatch_setting_field(addict_controller::Event::LookUpEnglish {
            article,
            index,
            word,
        });
    }

    /// Shows the state, the sentence and the chosen word as they are now.
    fn render_manual(&self) {
        self.render_window();
        self.view.render_sentence(
            &self.view_state.sentence,
            &self.view_state.words,
            self.view_state.chosen,
        );
    }

    /// The window state and source line from the controller snapshot.
    fn render_window(&self) {
        self.view
            .render(self.view_state.window_state, self.view_state.source);
    }

    /// A source is being read now, or none is: the controller is told, and
    /// the window shows its state at once, before the snapshot comes.
    fn set_reading_source(&mut self, source: Option<Source>) {
        self.dispatch_controller(addict_controller::Event::SetReadingSource(source));
        let view = &mut self.view_state;
        view.reading_source = source;
        view.window_state = window_state(source, view.input.as_ref());
    }

    /// Sends a fact to the shared controller.
    fn dispatch_controller(&self, event: controller::Event) {
        if let Some(controller) = &self.controller {
            controller.dispatch(event);
        }
    }

    /// The shortcut was pressed: a running capture and read are given up,
    /// the field is cleared, the read of the selection and the clipboard
    /// starts and the window is shown at once in the waiting state with the
    /// source clipboard; the window does not wait for the read, which ends
    /// at most `CLIPBOARD_LIMIT` after the press. The controller issues
    /// the read and its own `ShowPanel`, which changes nothing once the
    /// window is up.
    fn activated(
        &mut self,
        activation_token: Option<&str>,
        timestamp_ms: u64,
        receipt_ns: Option<u64>,
    ) {
        self.give_up_press();
        self.clear_for_read();
        if self.controller.is_none() {
            // No controller issues the read or its deadline: the waiting
            // ends at once with the line of an unfinished read.
            self.set_reading_source(None);
            self.view
                .render_source_failure(&crate::view::selection_failure_text(&Ended::Unfinished));
        }
        // Before `present()`: the first frame has the labels.
        self.render_window();
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
        self.capturing.activation_token = None;
        self.view_state.capture_sensitive = true;
        self.view.set_capture_sensitive(true);
        self.give_up_read();
    }

    /// A choice of the tray. Show is a repeated launch: the window as it is,
    /// no read. Settings shows the current key state from the shared source.
    /// About shows its window on its About page. Exit ends
    /// the application through its shutdown. A tray that cannot be had is
    /// said once and changes nothing.
    fn tray(&mut self, event: TrayEvent) {
        match event {
            TrayEvent::Show => self.reveal(),
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

    /// Settings opened: the controller clears the lines and asks for what it
    /// wants to read; this shell only presents the window.
    fn show_settings(&mut self) {
        if self.controller.is_some() {
            self.dispatch_controller(addict_controller::Event::SettingsOpened);
        } else {
            // Without a controller no read or change can run (`check_startup`
            // and `toggle_startup` in 0.5.0).
            self.settings
                .say_startup(addict_engine::view::STARTUP_NO_RUNNER);
            self.settings.set_startup_sensitive(false);
        }
        self.settings.present();
    }

    /// Writes the key with the shell's file format, then tells the controller
    /// both the new active key and the end of the store, in that order when
    /// the write commits. The key never goes into a snapshot.
    fn store_key(&mut self, id: RequestId, key: Secret) {
        use addict_core::failure::key_store::KeyStoreFailure as Failure;

        let Some(path) = self.key_file.clone() else {
            self.dispatch_controller(addict_controller::Event::KeyStored {
                id,
                result: Err(Ended::Shell(Failure::NoPlace)),
            });
            return;
        };
        let Some(dispatcher) = self.controller.clone() else {
            self.dispatch_controller(addict_controller::Event::KeyStored {
                id,
                result: Err(Ended::Shell(Failure::Unavailable)),
            });
            return;
        };
        let Some(work) = self.work.upgrade() else {
            dispatcher.dispatch(addict_controller::Event::KeyStored {
                id,
                result: Err(Ended::Shell(Failure::Unavailable)),
            });
            return;
        };
        let answered = dispatcher.clone();
        self.store_job = Some(work.run(
            KEY_SAVE_LIMIT,
            persist_key(path, key, dispatcher),
            move |outcome| {
                let result = match outcome {
                    Outcome::Finished(Ok(())) => Ok(()),
                    Outcome::Finished(Err(WriteError::Invalid)) => {
                        Err(Ended::Shell(Failure::Invalid))
                    }
                    Outcome::Finished(Err(error)) => {
                        Err(Ended::Shell(Failure::Failed(error.to_string())))
                    }
                    Outcome::Cancelled => Err(Ended::Shell(Failure::Cancelled)),
                    Outcome::TimedOut => Err(Ended::DeadlinePassed),
                    Outcome::Panicked => Err(Ended::Unfinished),
                };
                answered.dispatch(addict_controller::Event::KeyStored { id, result });
            },
        ));
    }

    /// Asks the Background portal through the shell's adapter.
    fn set_login_item(&mut self, id: RequestId, on: bool) {
        let Some(dispatcher) = self.controller.clone() else {
            self.dispatch_controller(addict_controller::Event::LoginItemSet {
                id,
                result: Err(Ended::Unfinished),
                runner_missing: true,
            });
            return;
        };
        let Some(work) = self.work.upgrade() else {
            dispatcher.dispatch(addict_controller::Event::LoginItemSet {
                id,
                result: Err(Ended::Unfinished),
                runner_missing: true,
            });
            return;
        };
        // A read queued before the toggle is given up; its answer would be
        // about the state before the change (`toggle_startup` in 0.5.0).
        self.startup_read_job = None;
        let answered = dispatcher.clone();
        self.startup_job = Some(work.run(
            STARTUP_LIMIT,
            request_startup_change(on, entries::startup_file(self.dirs.config.as_deref())),
            move |outcome| {
                let result = match outcome {
                    Outcome::Finished(end) => Ok(end),
                    Outcome::Cancelled => Err(Ended::Unfinished),
                    Outcome::TimedOut => Err(Ended::DeadlinePassed),
                    Outcome::Panicked => Err(Ended::Unfinished),
                };
                answered.dispatch(addict_controller::Event::LoginItemSet {
                    id,
                    result,
                    runner_missing: false,
                });
            },
        ));
    }

    /// Reads the live login item, without asking the portal to change it.
    fn read_login_item(&mut self) {
        let Some(dispatcher) = self.controller.clone() else {
            return;
        };
        let Some(work) = self.work.upgrade() else {
            dispatcher.dispatch(addict_controller::Event::LoginItemRead {
                shown: None,
                runner_missing: true,
            });
            return;
        };
        let state = entries::startup_file(self.dirs.config.as_deref());
        self.startup_read_job = Some(work.run(
            STARTUP_READ_LIMIT,
            async move {
                let saved = state.as_deref().and_then(startup::read_saved);
                // The sandbox cannot see the entry; a foreign one is no state.
                let entry = (!startup::is_sandboxed())
                    .then(entries::login_entry)
                    .and_then(Result::ok);
                login_item_shown(saved, entry)
            },
            move |outcome| {
                // A read replaced by a newer opening says nothing
                // (`startup_read` returned on `Cancelled` in 0.5.0).
                let shown = match outcome {
                    Outcome::Finished(shown) => Some(shown),
                    Outcome::Cancelled => return,
                    Outcome::TimedOut | Outcome::Panicked => None,
                };
                dispatcher.dispatch(addict_controller::Event::LoginItemRead {
                    shown,
                    runner_missing: false,
                });
            },
        ));
    }

    /// Configure shortcuts: the portal's dialog belongs to the session's
    /// worker. The controller only says when the action is available.
    fn configure_shortcuts(&self) {
        if let Some(current) = self.current.upgrade()
            && let Some(handle) = current.borrow().as_ref()
        {
            handle.configure();
        }
    }

    /// One attempt at a time: a press queued behind another starts nothing.
    /// The controller changes the binding state before this effect arrives.
    fn bind_shortcuts(&self) {
        let Some(current) = self.current.upgrade() else {
            return;
        };
        // `Failed` is the last act of the old worker, sent after its session
        // was closed: joining it does not wait.
        current.borrow_mut().take();
        *current.borrow_mut() = Some(start_shortcut(
            &self.sender,
            self.timed,
            self.dirs.config.as_deref(),
        ));
    }
}

/// Reads the raw text of `clipboard` once, for `held`, and gives `done` its
/// text when it has one: a blank text and a failed read give `None`, and
/// the error goes to stderr. The controller applies the line rule and the
/// length limit. Nothing of the text is logged.
fn read_text(
    clipboard: &gdk::Clipboard,
    held: Held,
    cancellable: &gio::Cancellable,
    done: impl FnOnce(Option<String>) + 'static,
) {
    clipboard.read_text_async(Some(cancellable), move |read| {
        done(match read {
            Ok(Some(text)) if !text.trim().is_empty() => Some(text.to_string()),
            Ok(Some(_) | None) => None,
            // A read given up says nothing.
            Err(e) if e.matches(gio::IOErrorEnum::Cancelled) => None,
            Err(e) => {
                let what = match held {
                    Held::Selection => "selection",
                    Held::Clipboard => "clipboard",
                };
                eprintln!("addict: no text from the {what}: {e}");
                None
            }
        });
    });
}

/// Sends what the user does in the `view` to `sender`: Retry, an edit of the
/// field, a submit of its text, Back and Forward, Capture, a click on a word
/// and a click on a sense.
fn connect_view(view: &View, sender: &UnboundedSender<Event>, actions: &Rc<Cell<ViewActions>>) {
    let retry = sender.clone();
    view.connect_retry(move || {
        let _ = retry.unbounded_send(Event::Setting(addict_controller::Event::RetryPressed));
    });
    let changed = sender.clone();
    view.connect_sentence_changed(move |text, origin| {
        let _ = changed.unbounded_send(Event::Sentence(text.to_owned(), origin));
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
    let word_actions = actions.clone();
    view.connect_word_chosen(move |index| {
        let _ = chosen.unbounded_send(Event::Word(word_actions.get().text, index));
    });
    let capture = sender.clone();
    view.connect_capture(move || {
        let _ = capture.unbounded_send(Event::Capture);
    });
    let definition = sender.clone();
    let definition_actions = actions.clone();
    view.connect_definition_action(move || {
        let action = definition_actions.get();
        if let Some(choice) = action.choice {
            let _ = definition.unbounded_send(Event::DefinitionAction(action.article, choice));
        }
    });
    let english = sender.clone();
    let english_actions = actions.clone();
    view.connect_look_up_english(move |word| {
        let action = english_actions.get();
        if let Some(Choice::Gloss(index)) = action.choice {
            let _ = english.unbounded_send(Event::LookUpEnglish(
                action.article,
                index,
                word.to_owned(),
            ));
        }
    });
    let add_key = sender.clone();
    view.connect_add_key(move || {
        let _ = add_key.unbounded_send(Event::Window(WindowEvent::Settings));
    });
    let synonym = sender.clone();
    let synonym_actions = actions.clone();
    view.connect_synonym_chosen(move |word| {
        let _ = synonym.unbounded_send(Event::Synonym(synonym_actions.get().article, word));
    });
    let sense = sender.clone();
    let sense_actions = actions.clone();
    view.connect_sense_chosen(move |index| {
        let _ = sense.unbounded_send(Event::Sense(sense_actions.get().article, index));
    });
    let gloss = sender.clone();
    let gloss_actions = actions.clone();
    view.connect_gloss_chosen(move |index| {
        let _ = gloss.unbounded_send(Event::Gloss(gloss_actions.get().article, index));
    });
    let language = sender.clone();
    view.connect_language_chosen(move |chosen| {
        let _ = language.unbounded_send(Event::Setting(addict_controller::Event::ReadingChosen(
            chosen,
        )));
    });
}

/// The style, the view and the hidden window around it, for `binding`: what
/// the user does in the view goes to `sender`, and so does the window
/// becoming active.
fn build_ui(
    application: &gtk::Application,
    binding: &Binding,
    sender: &UnboundedSender<Event>,
) -> (View, gtk::ApplicationWindow, Rc<Cell<ViewActions>>) {
    style::install();
    let view = View::new(window_state(None, None), binding);
    let actions = Rc::new(Cell::new(ViewActions::default()));
    connect_view(&view, sender, &actions);
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
    (view, window, actions)
}

/// What the switch shows from the saved state and the login entry. The saved
/// state wins; with none, only a legacy entry makes it on. `None` for `entry`
/// means it could not be read.
fn login_item_shown(
    saved: Option<bool>,
    entry: Option<addict_platform::desktop::LoginEntry>,
) -> LoginItemShown {
    settings::startup_shown(saved, entry)
}

/// One change of the switch: the checks before the portal, then the flow of
/// [`settings::change_startup`] with the real portal and the state file.
async fn request_startup_change(enable: bool, state: Option<PathBuf>) -> StartupEnd {
    // Only an enable needs it: a disable must work with this binary's path gone.
    let command = if enable {
        startup::relaunch_command()
    } else {
        Ok(Vec::new())
    };
    let checked = {
        let state = state.is_some();
        let command = command.as_ref().err().cloned();
        move || {
            if !state {
                return Err(
                    "there is no place to save it: set an absolute XDG_CONFIG_HOME or HOME"
                        .to_owned(),
                );
            }
            if let Some(reason) = command {
                return Err(reason);
            }
            // A foreign file or a symlink stays refused; the sandbox cannot see one.
            if !startup::is_sandboxed() {
                entries::login_entry()?;
                entries::check_launcher_installed()?;
            }
            Ok(())
        }
    };
    let command = command.unwrap_or_default();
    let command = &command;
    settings::change_startup(
        enable,
        checked,
        |asked| async move {
            let answered = startup::request(APP_ID, asked, command).await?;
            startup::confirmed(asked, answered)
        },
        |saved| {
            let Some(path) = &state else {
                return Err("no place to save it".to_owned());
            };
            startup::save(path, saved).map_err(|e| e.to_string())
        },
    )
    .await
}

/// The Settings window above `window`, hidden, naming `key_file`; Change
/// shortcut, Save key and the Start at login switch go to `sender` as events.
fn start_settings(
    window: &gtk::ApplicationWindow,
    key_file: Option<&Path>,
    data_dir: Option<PathBuf>,
    owner: KeyOwner,
    articles: Arc<dyn Articles>,
    sender: &UnboundedSender<Event>,
) -> Settings {
    let change = sender.clone();
    let save = sender.clone();
    let clear = sender.clone();
    let startup = sender.clone();
    let imported = sender.clone();
    let my_language = sender.clone();
    let english = sender.clone();
    let at_once = sender.clone();
    let download = sender.clone();
    Settings::new(
        window,
        key_file,
        data_dir,
        owner,
        move || {
            let _ =
                change.unbounded_send(Event::Setting(addict_controller::Event::ConfigurePressed));
        },
        move |key| {
            let _ = save.unbounded_send(Event::Setting(addict_controller::Event::SaveKey(key)));
        },
        move || {
            let _ =
                clear.unbounded_send(Event::Setting(addict_controller::Event::ClearCachePressed));
        },
        move |on| {
            let _ = startup.unbounded_send(Event::Setting(
                addict_controller::Event::LoginItemToggled(on),
            ));
        },
        move |chosen| {
            let _ = my_language.unbounded_send(Event::Setting(
                addict_controller::Event::MyLanguageChosen(chosen),
            ));
        },
        move |chosen| {
            let _ = english.unbounded_send(Event::Setting(
                addict_controller::Event::EnglishChosen(chosen),
            ));
        },
        move |chosen| {
            let _ = at_once.unbounded_send(Event::Setting(addict_controller::Event::AtOnceChosen(
                chosen,
            )));
        },
        move || refresh_import(articles.clone(), imported.clone()),
        move || {
            let _ = download.unbounded_send(Event::SettingsDownload);
        },
    )
}

/// The check does disk work on a separate thread. Waiting for the event's
/// acknowledgement guarantees the next lookup sees the refreshed states
/// before Settings advertises that the dictionary is ready.
async fn refresh_import(
    articles: Arc<dyn Articles>,
    sender: UnboundedSender<Event>,
) -> Result<(), String> {
    let (started, generation) = oneshot::channel();
    sender
        .unbounded_send(Event::ImportRefreshing(started))
        .map_err(|_| "The resident is no longer available.".to_owned())?;
    let generation = generation
        .await
        .map_err(|_| "The resident is no longer available.".to_owned())?;
    let indexes = download::blocking(move || articles.indexes()).await?;
    let (ready, received) = oneshot::channel();
    sender
        .unbounded_send(Event::Imported(ImportCompletion {
            generation,
            indexes,
            ready,
        }))
        .map_err(|_| "The resident is no longer available.".to_owned())?;
    received
        .await
        .map_err(|_| "The resident did not refresh its dictionary states.".to_owned())
}

/// The writer starts finalization from its commit callback, outside the
/// cancellable queue future. Success waits for the resident acknowledgement.
async fn install_download(
    downloader: Downloader,
    published: addict_dictionary::release::Published,
    dir: std::path::PathBuf,
    recovery: bool,
    articles: Arc<dyn Articles>,
    sender: UnboundedSender<Event>,
    progress: impl FnMut(u64),
) -> Result<std::path::PathBuf, String> {
    let (ready, received) = oneshot::channel();
    let ready = std::sync::Mutex::new(Some(ready));
    let downloader = downloader.on_commit(move || {
        let Some(ready) = ready.lock().expect("commit callback lock").take() else {
            return;
        };
        let articles = articles.clone();
        let sender = sender.clone();
        // Dropping this join handle detaches, rather than aborts, refresh.
        tokio::spawn(async move {
            let _ = ready.send(refresh_import(articles, sender).await);
        });
    });
    let downloader = if recovery {
        downloader.recovering()
    } else {
        downloader
    };
    let path = downloader
        .install(&published, &dir, progress)
        .await
        .map_err(|error| error.to_string())?;
    received
        .await
        .map_err(|_| "The dictionary refresh stopped.".to_owned())??;
    Ok(path)
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

/// Starts the shared lookup loop and hands its renders to the resident in
/// FIFO order on the GTK main context.
fn start_controller(
    dirs: &Dirs,
    key: Option<Result<Secret, KeyError>>,
    recording: bool,
    sender: &UnboundedSender<Event>,
) -> io::Result<Controller> {
    controller::start(
        dirs.clone(),
        controller_platform(),
        StartFacts { key, recording },
        controller_renderer(sender),
    )
}

fn controller_platform() -> Platform {
    Platform {
        name: "Linux".to_owned(),
        key_location_hint: KEY_LOCATION_HINT,
        // Wayland gives the clipboard only to the focused window: show first.
        read_before_show: false,
    }
}

fn controller_renderer(sender: &UnboundedSender<Event>) -> impl controller::Renderer {
    let queue: ControllerQueue = Arc::new(Mutex::new(VecDeque::new()));
    let context = glib::MainContext::default();
    let sender = sender.clone();
    move |state, effects| {
        let first = {
            let mut pending = queue.lock().expect("controller render queue");
            let first = pending.is_empty();
            pending.push_back((state, effects));
            first
        };
        if first {
            let queue = queue.clone();
            let sender = sender.clone();
            // GLib may invoke immediately when the context is unowned.
            // Only the channel is touched here. `serve` owns the widgets.
            context.invoke(move || {
                // The context forwards this batch before another invoke
                // can deliver the next one. Enqueueing can continue now.
                let pending = std::mem::take(&mut *queue.lock().expect("controller render queue"));
                for (state, effects) in pending {
                    let _ = sender.unbounded_send(Event::Controller(Box::new(state), effects));
                }
            });
        }
    }
}

/// GTK 4.20's GStreamer backend needs a file URI even for buffered audio.
/// An anonymous file gives it a seekable source and disappears when closed.
fn buffer_recording(audio: &[u8]) -> io::Result<File> {
    let create = |flags| {
        // SAFETY: The name is NUL-terminated. A nonnegative result is a new fd.
        let fd = unsafe { libc::memfd_create(c"addict-recording".as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: This takes sole ownership of the newly created fd.
        Ok(unsafe { File::from_raw_fd(fd) })
    };
    let mut file = create(libc::MFD_CLOEXEC | libc::MFD_NOEXEC_SEAL).or_else(|error| {
        if error.raw_os_error() == Some(libc::EINVAL) {
            // Older kernels do not recognize MFD_NOEXEC_SEAL.
            create(libc::MFD_CLOEXEC)
        } else {
            Err(error)
        }
    })?;
    file.write_all(audio)?;
    Ok(file)
}

/// Moves the outcome of a recording request from the worker to GTK.
fn start_recording_job<T: Send + 'static>(
    work: &Work,
    limit: Duration,
    run: impl Future<Output = T> + Send + 'static,
    ready: impl FnOnce(Outcome<T>) + 'static,
) -> Job {
    let (send, receive) = oneshot::channel();
    let job = work.run(limit, run, move |outcome| {
        let _ = send.send(outcome);
    });
    glib::spawn_future_local(async move {
        if let Ok(outcome) = receive.await {
            ready(outcome);
        }
    });
    job
}

/// Starts the check of the indexes in `articles` on `work`; where they stand
/// comes to `sender` as an event.
fn start_check(
    work: Option<&Work>,
    articles: Arc<dyn Articles>,
    sender: &UnboundedSender<Event>,
    generation: u64,
) -> Option<Job> {
    let checked = sender.clone();
    manual::check_indexes(work, LOOKUP_LIMIT, articles, move |indexes| {
        // On the worker: the UI thread wakes for the event.
        let _ = checked.unbounded_send(Event::Indexes {
            generation,
            indexes,
        });
    })
}

/// Starts the decision of the first-run offer on `work`, for the `indexes`
/// of the first check: the choice file is read and, when the offer is due,
/// the manifest is fetched by `downloader` and comes to `sender` as an
/// event. When the manifest cannot be had, why is written to stderr and
/// nothing is kept, so the next start asks again.
fn start_first_run(
    work: &Work,
    downloader: Downloader,
    path: Option<PathBuf>,
    indexes: Indexes,
    my: Language,
    sender: &UnboundedSender<Event>,
    run: u64,
) -> Job {
    let offered = sender.clone();
    work.run(
        MANIFEST_LIMIT,
        async move {
            let chosen = first_run::read(path.as_deref());
            first_run::offered_manifest(indexes, my, chosen, &downloader).await
        },
        move |outcome| match outcome {
            Outcome::Finished(Ok(Some(manifest))) => {
                // On the worker: the UI thread wakes for the event.
                let _ = offered.unbounded_send(Event::FirstRunOffer(run, manifest));
            }
            Outcome::Finished(Ok(None)) | Outcome::Cancelled => {}
            Outcome::Finished(Err(e)) => {
                eprintln!("addict: no dictionary download is offered: {e}");
            }
            Outcome::TimedOut => {
                eprintln!(
                    "addict: no dictionary download is offered: the manifest did not come in time"
                );
            }
            Outcome::Panicked => eprintln!("addict: no dictionary download is offered"),
        },
    )
}

/// Loads the configured key once without blocking GTK or the translation
/// runner. The result reaches the controller as its first `KeyChanged`.
fn start_key_load(path: Option<PathBuf>, sender: &UnboundedSender<Event>) {
    let loaded = sender.clone();
    let read = move || {
        let result = path
            .as_deref()
            .map_or(Err(KeyError::NoDirectory), key::read);
        let _ = loaded.unbounded_send(Event::Setting(addict_controller::Event::KeyChanged(result)));
    };
    if std::thread::Builder::new()
        .name("addict-key-load".to_owned())
        .spawn(read)
        .is_err()
    {
        eprintln!("addict: could not start the key reader");
        let _ = sender.unbounded_send(Event::Setting(addict_controller::Event::KeyChanged(Err(
            KeyError::Unreadable,
        ))));
    }
}

/// Writes the key file and publishes the written key to the controller
/// before the job yields again: a committed write is active even if the
/// runner then reports a timeout or a cancel for its job, as 0.5.0's
/// `persist_key` made it.
async fn persist_key(path: PathBuf, key: Secret, controller: Dispatcher) -> Result<(), WriteError> {
    let persisted = key::write(&path, key)?;
    controller.dispatch(addict_controller::Event::KeyChanged(Ok(persisted)));
    Ok(())
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

/// Builds the hidden window, starts the first attempt to bind the shortcuts
/// and the event loop. Runs once, in the primary instance.
fn start(
    application: &gtk::Application,
    evidence: Option<Rc<RefCell<evidence::Writer>>>,
    dirs: &Dirs,
) -> Resident {
    // The worker threads push; the main context wakes only for an event.
    let (sender, receiver) = futures_channel::mpsc::unbounded();
    let key_file = key::key_path(dirs.config.as_deref());

    // The first attempt starts below, with the window still hidden.
    let binding = Binding::Attempting;
    connect_settings_action(application, &sender);
    connect_menu_actions(application, &sender);
    application.set_accels_for_action("app.settings", &["<Control>comma"]);
    let (view, window, view_actions) = build_ui(application, &binding, &sender);

    // Only the primary instance gets here, so a second launch records no
    // session.
    let recording = evidence.map(|writer| Recording::start(writer, &window));
    let timed = recording.is_some();
    let config = dirs.config.as_deref();
    let handle = Rc::new(RefCell::new(Some(start_shortcut(&sender, timed, config))));
    let events = sender.clone();
    let controller = match start_controller(dirs, None, timed, &sender) {
        Ok(controller) => Some(controller),
        Err(e) => {
            eprintln!("addict: lookups cannot run: {e}");
            None
        }
    };
    // The key is read off GTK's thread; the controller publishes its result
    // as the first `KeyChanged` once this event is dispatched.
    start_key_load(key_file.clone(), &sender);
    let work = controller
        .as_ref()
        .map_or_else(WorkRef::new, Controller::work);
    if controller.is_some() {
        view.connect_recording_audio({
            let work = work.clone();
            move |url, ready| {
                let Some(work) = work.upgrade() else {
                    ready(Err(AudioFailure::Failed));
                    return None;
                };
                Some(start_recording_job(
                    &work,
                    AUDIO_LIMIT,
                    async move {
                        let audio = fetch_audio(url).await?;
                        buffer_recording(&audio).map_err(|error| {
                            eprintln!("addict: could not buffer recording: {error}");
                            AudioFailure::Failed
                        })
                    },
                    move |outcome| {
                        let result = match outcome {
                            Outcome::Finished(audio) => audio,
                            Outcome::Cancelled => return,
                            Outcome::TimedOut => Err(AudioFailure::Network),
                            Outcome::Panicked => Err(AudioFailure::Failed),
                        };
                        ready(result.map(PlaybackMedia::from_recording));
                    },
                ))
            }
        });
        let work = work.clone();
        view.connect_recording_credit(move |file, ready| {
            let work = work.upgrade()?;
            Some(start_recording_job(
                &work,
                CREDIT_LIMIT,
                async move { file.fetch_credit().await },
                move |outcome| {
                    if let Outcome::Finished(credit) = outcome {
                        ready(credit);
                    }
                },
            ))
        });
    }
    // Nothing is opened here: the indexes are files, and files are the
    // runner's.
    let articles: Arc<dyn Articles> = controller.as_ref().map_or_else(
        || Arc::new(Dictionaries::new(dirs.data.clone())),
        Controller::dictionaries,
    );
    let runner = work.upgrade();
    let check = start_check(runner.as_deref(), articles.clone(), &sender, 0);
    let settings = start_settings(
        &window,
        key_file.as_deref(),
        dirs.data.clone(),
        KeyOwner::detect(),
        articles.clone(),
        &sender,
    );
    let about = About::new(&window);
    let session = Session {
        dirs: dirs.clone(),
        view,
        window,
        recording,
        timed,
        sender,
        current: Rc::downgrade(&handle),
        view_state: ViewState::default(),
        view_actions,
        applied_field_revision: 0,
        sent_text: String::new(),
        edits_sent: 0,
        edits_at_set: 0,
        read_sequence: 0,
        work,
        controller: controller.as_ref().map(Controller::dispatcher),
        articles,
        key_file,
        store_job: None,
        startup_job: None,
        startup_read_job: None,
        my_choice_pending: None,
        settings,
        about,
        check,
        indexes: IndexCache::default(),
        capturing: Capturing::default(),
        copying: Copying::default(),
        first_run: FirstRun::Unasked,
        download_run: 0,
        release: download::BASE_URL.to_owned(),
        first_run_choice: None,
    };
    serve(session, receiver);
    let tray = start_tray(&events);
    Resident {
        _hold: application.hold(),
        _shortcut: handle,
        events,
        _controller: controller,
        _tray: tray,
    }
}

/// The mode travels with each invocation, including launches forwarded over D-Bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    Normal,
    Background,
}

impl LaunchMode {
    pub fn from_background(background: bool) -> Self {
        if background {
            Self::Background
        } else {
            Self::Normal
        }
    }
}

/// These events own presentation policy; binding failures only update state.
/// Keeping their dispatcher independent of GTK lets tests observe the same
/// callbacks used by the resident, including silent initialization failures.
enum WindowEvent {
    Shortcut(ShortcutEvent, Option<u64>),
    Launched(LaunchMode),
    Settings,
}

trait WindowActions {
    fn shortcut(&mut self, state: ShortcutState);
    fn present_main(&mut self);
    fn present_settings(&mut self);
    fn clipboard(&mut self, token: Option<&str>, timestamp: u64, receipt: Option<u64>);
    fn capture(&mut self, token: Option<&str>);
}

/// The shell's binding report becomes one controller event; activation stays
/// local because it starts a Read or a Capture, not a settings change.
fn shortcut_state(event: ShortcutEvent) -> Option<ShortcutState> {
    let convert = |bindings: addict_platform::shortcut::Bindings| ControllerBindings {
        clipboard: bindings.clipboard,
        screenshot: bindings.screenshot,
    };
    match event {
        ShortcutEvent::Bound { bindings } => Some(ShortcutState::Bound(convert(bindings))),
        ShortcutEvent::Changed { bindings } => Some(ShortcutState::Changed(convert(bindings))),
        ShortcutEvent::Failed { message } => Some(ShortcutState::Failed(message)),
        ShortcutEvent::ConfigureEnded { message } => {
            Some(ShortcutState::ConfigureAnswered(message))
        }
        ShortcutEvent::Rebinding { bindings } => Some(ShortcutState::Rebinding(convert(bindings))),
        ShortcutEvent::RebindEnded { message } => Some(ShortcutState::RebindEnded(message)),
        ShortcutEvent::Activated { .. } => None,
    }
}

fn dispatch_window(ui: &mut impl WindowActions, event: WindowEvent) {
    match event {
        WindowEvent::Launched(LaunchMode::Normal) => ui.present_main(),
        WindowEvent::Launched(LaunchMode::Background) => {}
        WindowEvent::Settings => ui.present_settings(),
        WindowEvent::Shortcut(event, receipt) => match event {
            ShortcutEvent::Activated {
                action,
                activation_token,
                timestamp_ms,
            } => match action {
                Action::Clipboard => {
                    ui.clipboard(activation_token.as_deref(), timestamp_ms, receipt)
                }
                Action::Screenshot => ui.capture(activation_token.as_deref()),
            },
            ShortcutEvent::Failed { message } => {
                // 0.5.0 still printed why the portal is unavailable; the
                // controller's line says the same in the window.
                eprintln!("addict: shortcut unavailable: {message}");
                ui.shortcut(ShortcutState::Failed(message));
            }
            event => {
                if let Some(state) = shortcut_state(event) {
                    ui.shortcut(state);
                }
            }
        },
    }
}

impl WindowActions for Session {
    fn shortcut(&mut self, state: ShortcutState) {
        self.dispatch_controller(addict_controller::Event::ShortcutChanged(state));
    }

    fn present_main(&mut self) {
        self.reveal();
    }
    fn present_settings(&mut self) {
        self.show_settings();
    }
    fn clipboard(&mut self, token: Option<&str>, timestamp: u64, receipt: Option<u64>) {
        self.activated(token, timestamp, receipt);
    }
    fn capture(&mut self, token: Option<&str>) {
        Session::capture(self, token);
    }
}

/// The main menu's Settings activates this same named application action.
fn connect_settings_action(
    application: &impl IsA<gio::ActionMap>,
    sender: &UnboundedSender<Event>,
) {
    let settings = gio::SimpleAction::new("settings", None);
    let sender = sender.clone();
    settings.connect_activate(move |_, _| {
        let _ = sender.unbounded_send(Event::Window(WindowEvent::Settings));
    });
    application.add_action(&settings);
}

/// The main menu's About and Exit are the tray's own events.
fn connect_menu_actions(application: &impl IsA<gio::ActionMap>, sender: &UnboundedSender<Event>) {
    for (name, event) in [("about", TrayEvent::About), ("exit", TrayEvent::Exit)] {
        let action = gio::SimpleAction::new(name, None);
        let sender = sender.clone();
        action.connect_activate(move |_, _| {
            let _ = sender.unbounded_send(Event::Tray(event.clone()));
        });
        application.add_action(&action);
    }
}

fn connect_launches(
    application: &impl IsA<gio::Application>,
    on_launch: impl Fn(LaunchMode) + 'static,
) {
    let on_launch = Rc::new(on_launch);
    let activation = on_launch.clone();
    // Direct D-Bus activation and desktop launchers without a command line are normal launches.
    application.connect_activate(move |_| activation(LaunchMode::Normal));
    application.connect_command_line(move |_, command| {
        let background = command
            .options_dict()
            .lookup::<bool>("background")
            .ok()
            .flatten()
            .unwrap_or(false);
        on_launch(LaunchMode::from_background(background));
        glib::ExitCode::SUCCESS
    });
    application.add_main_option(
        "background",
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Start silently for login",
        None,
    );
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
pub fn run(
    mode: LaunchMode,
    evidence: Option<Rc<RefCell<evidence::Writer>>>,
    dirs: Dirs,
) -> ExitCode {
    // Without a display there is nothing to be resident for: leave before the
    // application or the portal worker exist.
    if let Err(e) = gtk::init() {
        eprintln!("addict: {e}");
        return ExitCode::FAILURE;
    }
    let mut application = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    let resident: Rc<RefCell<Option<Resident>>> = Rc::default();
    let recording = evidence.is_some();

    let slot = resident.clone();
    application.connect_startup(move |application| {
        let resident = start(application, evidence.clone(), &dirs);
        *slot.borrow_mut() = Some(resident);
    });
    let launches = resident.clone();
    connect_launches(&application, move |mode| {
        if let Some(resident) = launches.borrow().as_ref() {
            let _ = resident
                .events
                .unbounded_send(Event::Window(WindowEvent::Launched(mode)));
        }
    });
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
    let args = match mode {
        LaunchMode::Normal => vec!["addict"],
        LaunchMode::Background => vec!["addict", "--background"],
    };
    let status = application.run_with_args(&args);
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
        Event, LOOKUP_LIMIT, LaunchMode, WindowActions, WindowEvent, connect_launches,
        connect_menu_actions, connect_settings_action, dispatch_window,
    };
    use addict_controller::ShortcutState;
    use addict_core::{dirs::Dirs, translation::Source as Language};

    #[test]
    fn the_lookup_limit_is_finite_and_short() {
        assert!(!LOOKUP_LIMIT.is_zero());
        assert!(LOOKUP_LIMIT.as_secs() <= 10);
    }

    #[test]
    fn the_translate_limit_is_finite_and_its_own() {
        use addict_controller::deadlines::TRANSLATE_LIMIT;
        assert!(!TRANSLATE_LIMIT.is_zero());
        assert_eq!(TRANSLATE_LIMIT.as_secs(), 10);
        assert!(TRANSLATE_LIMIT > LOOKUP_LIMIT);
    }

    #[test]
    fn buffered_recording_can_be_reopened_from_the_beginning() {
        use std::os::fd::AsRawFd;

        let audio = b"buffered recording";
        let file = super::buffer_recording(audio).expect("recording descriptor");
        let path = format!("/proc/self/fd/{}", file.as_raw_fd());
        assert_eq!(std::fs::read(path).unwrap(), audio);
    }

    #[tokio::test]
    async fn import_completion_refreshes_missing_languages_before_the_next_lookup() {
        use crate::{
            dictionaries::{Answer, Articles, Dictionaries, State},
            manual,
        };
        use addict_dictionary::{
            format::{self, Contents, Headword},
            import::{self, Source},
        };
        use addict_engine::work::{Outcome, Work};
        use futures_util::{StreamExt, pin_mut, poll};
        use std::{path::PathBuf, sync::Arc};

        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let temp = Temp(
            std::env::temp_dir().join(format!("addict-import-resident-{}", std::process::id())),
        );
        std::fs::create_dir(&temp.0).unwrap();
        let articles: Arc<dyn Articles> = Arc::new(Dictionaries::new(Some(temp.0.clone())));
        let missing = articles.indexes();
        assert_eq!(missing.spanish, State::Missing);
        assert_eq!(missing.catalan, State::Missing);
        let mut cached = super::IndexCache {
            known: Some(missing),
            ..Default::default()
        };
        let old_generation = cached.generation;
        let work = Work::start().expect("lookup runner");

        for (language, source, word) in [
            (Language::Es, Source::WikdictEsRu, "banco"),
            (Language::Ca, Source::WikdictCaRu, "banc"),
        ] {
            assert!(
                manual::offline(
                    Some(&work),
                    LOOKUP_LIMIT,
                    articles.clone(),
                    word.to_owned(),
                    language,
                    Language::Ru,
                    cached.known,
                    |_, _| {}
                )
                .is_err(),
                "initially gated by Missing"
            );
            // The index writer is a small fixture for the resident cache
            // regression; the real chooser/importer is tested separately.
            let bytes = format::encode(&Contents {
                source: "manual-import-regression".to_owned(),
                headwords: vec![Headword {
                    word: word.to_owned(),
                    translations: vec!["банк".to_owned()],
                    ..Default::default()
                }],
                ..Default::default()
            })
            .unwrap();
            std::fs::write(import::index_path(&temp.0, source), bytes).unwrap();
        }

        let (sender, mut events) = futures_channel::mpsc::unbounded();
        let refreshed = super::refresh_import(articles.clone(), sender);
        pin_mut!(refreshed);
        assert!(poll!(refreshed.as_mut()).is_pending());
        let Event::ImportRefreshing(started) = events.next().await.unwrap() else {
            panic!("refresh start");
        };
        started.send(cached.begin_import()).unwrap();
        let event = tokio::select! {
            result = &mut refreshed => panic!("completed before resident delivery: {result:?}"),
            event = events.next() => event.expect("completion event"),
        };
        let Event::Imported(completed) = event else {
            panic!("expected import completion");
        };
        assert!(
            poll!(refreshed.as_mut()).is_pending(),
            "not ready before resident delivery"
        );
        assert_eq!(cached.known, Some(missing));
        // The same ordered completion operation used by Session::handle.
        completed.apply(|generation, indexes| assert!(cached.update(generation, indexes)));
        refreshed.await.expect("resident acknowledgement");
        assert!(
            !cached.update(old_generation, missing),
            "late pre-import events are ignored"
        );

        for (language, word) in [(Language::Es, "banco"), (Language::Ca, "banc")] {
            let (send, receive) = futures_channel::oneshot::channel();
            let _job = manual::offline(
                Some(&work),
                LOOKUP_LIMIT,
                articles.clone(),
                word.to_owned(),
                language,
                Language::Ru,
                cached.known,
                move |language, outcome| {
                    let _ = send.send((language, outcome));
                },
            )
            .expect("the first lookup after completion must be allowed");
            let (answered, Outcome::Finished(looked)) = receive.await.unwrap() else {
                panic!("lookup did not finish");
            };
            assert_eq!(answered, language);
            let Answer::Grouped(grouped) = looked.answer else {
                panic!("expected the imported language entry");
            };
            assert!(
                grouped
                    .translations_into(addict_core::translation::Source::Ru)
                    .unwrap_or_default()
                    .iter()
                    .any(|text| text == "банк")
            );
        }
    }

    #[tokio::test]
    async fn downloaded_dictionaries_refresh_even_when_cancelled_after_commit_before_refresh() {
        use crate::{
            dictionaries::{Answer, Articles, Dictionaries, State},
            download::Downloader,
            manual,
            stub::{Route, Stub},
        };
        use addict_dictionary::{
            format::{self, Contents, Headword},
            import::{self, Source},
            release::Published,
        };
        use addict_engine::work::{Outcome, Work};
        use futures_util::StreamExt;
        use std::{collections::HashMap, path::PathBuf, sync::Arc};
        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let temp = Temp(std::env::temp_dir().join(format!(
            "addict-download-cancel-resident-{}",
            std::process::id()
        )));
        std::fs::create_dir(&temp.0).unwrap();
        let articles: Arc<dyn Articles> = Arc::new(Dictionaries::new(Some(temp.0.clone())));
        let mut cached = super::IndexCache {
            known: Some(articles.indexes()),
            ..Default::default()
        };
        assert_eq!(cached.known.unwrap().spanish, State::Missing);
        assert_eq!(cached.known.unwrap().catalan, State::Missing);
        let work = Work::start().unwrap();
        let (sender, mut events) = futures_channel::mpsc::unbounded();
        for (language, source, name, word) in [
            (Language::Es, Source::WikdictEsRu, "wikdict-es-ru", "banco"),
            (Language::Ca, Source::WikdictCaRu, "wikdict-ca-ru", "banc"),
        ] {
            let stale = cached.known.unwrap();
            let stale_generation = cached.generation;
            let bytes = format::encode(&Contents {
                source: "download-cancel-regression".to_owned(),
                headwords: vec![Headword {
                    word: word.to_owned(),
                    translations: vec!["банк".to_owned()],
                    ..Default::default()
                }],
                ..Default::default()
            })
            .unwrap();
            let mut xz = Vec::new();
            lzma_rs::xz_compress(&mut &bytes[..], &mut xz).unwrap();
            let published = Published::of(name, &xz);
            let stub = Stub::start(HashMap::from([(
                format!(
                    "/{}/{}",
                    addict_dictionary::release::tag(),
                    published.asset()
                ),
                Route::File(xz),
            )]))
            .unwrap();
            let dir = temp.0.clone();
            let refreshed = sender.clone();
            let dictionaries = articles.clone();
            let (finished, ended) = futures_channel::oneshot::channel();
            let job = work.run(
                addict_controller::deadlines::DOWNLOAD_LIMIT,
                super::install_download(
                    Downloader::with_base_url(stub.base_url()),
                    published,
                    dir,
                    true,
                    dictionaries,
                    refreshed,
                    |_| {},
                ),
                move |outcome| {
                    let _ = finished.send(outcome);
                },
            );
            // The commit callback has fired, but its independent finalizer
            // is paused before the availability disk check can begin.
            let Event::ImportRefreshing(started) = events.next().await.unwrap() else {
                panic!("refresh start");
            };
            assert!(
                import::index_path(&temp.0, source).is_file(),
                "committed before cancellation"
            );
            drop(job);
            assert!(matches!(ended.await.unwrap(), Outcome::Cancelled));
            let generation = cached.begin_import();
            assert!(
                !cached.update(stale_generation, stale),
                "advance before accepting old checks"
            );
            started.send(generation).unwrap();
            let Event::Imported(completed) = events.next().await.unwrap() else {
                panic!("independent completion");
            };
            completed.apply(|generation, indexes| assert!(cached.update(generation, indexes)));
            assert!(!cached.update(stale_generation, stale));
            let (send, receive) = futures_channel::oneshot::channel();
            let _lookup = manual::offline(
                Some(&work),
                LOOKUP_LIMIT,
                articles.clone(),
                word.to_owned(),
                language,
                Language::Ru,
                cached.known,
                move |language, outcome| {
                    let _ = send.send((language, outcome));
                },
            )
            .expect("next lookup must be allowed");
            let (answered, Outcome::Finished(looked)) = receive.await.unwrap() else {
                panic!("lookup failed");
            };
            assert_eq!(answered, language);
            let Answer::Grouped(grouped) = looked.answer else {
                panic!("restored article");
            };
            assert!(
                grouped
                    .translations_into(addict_core::translation::Source::Ru)
                    .unwrap_or_default()
                    .iter()
                    .any(|translation| translation == "банк")
            );
        }
    }

    /// A full session of the window for a test, its application registered
    /// under `id`, with the supplied directories. Returns its events and
    /// runner. Needs a display.
    fn test_session(
        id: &str,
        dirs: Dirs,
    ) -> (
        super::Session,
        futures_channel::mpsc::UnboundedReceiver<Event>,
        std::sync::Arc<addict_engine::work::Work>,
    ) {
        test_session_with_dirs(id, dirs)
    }

    fn test_session_with_dirs(
        id: &str,
        dirs: Dirs,
    ) -> (
        super::Session,
        futures_channel::mpsc::UnboundedReceiver<Event>,
        std::sync::Arc<addict_engine::work::Work>,
    ) {
        use super::*;
        let app = gtk::Application::builder()
            .application_id(id)
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(gio::Cancellable::NONE).unwrap();
        let (sender, events) = futures_channel::mpsc::unbounded();
        let binding = Binding::Attempting;
        let (view, window, view_actions) = build_ui(&app, &binding, &sender);
        let work = Arc::new(Work::start().unwrap());
        let articles: Arc<dyn Articles> = Arc::new(Dictionaries::new(dirs.data.clone()));
        let key_file = key::key_path(dirs.config.as_deref());
        // Not the desktop the tests run on: the portal's dialog, as on GNOME.
        let settings = start_settings(
            &window,
            key_file.as_deref(),
            dirs.data.clone(),
            KeyOwner::Portal,
            articles.clone(),
            &sender,
        );
        let about = About::new(&window);
        let session = Session {
            dirs,
            view,
            window,
            recording: None,
            timed: false,
            sender,
            current: Weak::new(),
            view_state: ViewState::default(),
            view_actions,
            applied_field_revision: 0,
            sent_text: String::new(),
            edits_sent: 0,
            edits_at_set: 0,
            read_sequence: 0,
            work: Arc::downgrade(&work),
            controller: None,
            articles: articles.clone(),
            key_file,
            store_job: None,
            startup_job: None,
            startup_read_job: None,
            my_choice_pending: None,
            settings,
            about,
            check: None,
            indexes: IndexCache::default(),
            capturing: Capturing::default(),
            copying: Copying::default(),
            first_run: FirstRun::Done,
            download_run: 0,
            release: download::BASE_URL.to_owned(),
            first_run_choice: None,
        };
        (session, events, work)
    }

    struct KeyTestDir(std::path::PathBuf);

    impl KeyTestDir {
        fn new(name: &str) -> Self {
            use std::os::unix::fs::DirBuilderExt;

            let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .expect("create a private key test directory");
            Self(path)
        }
    }

    impl Drop for KeyTestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn find<T: IsA<gtk::Widget>>(root: &gtk::Widget, accept: &impl Fn(&T) -> bool) -> Option<T> {
        if let Some(widget) = root.downcast_ref::<T>().filter(|widget| accept(widget)) {
            return Some(widget.clone());
        }
        let mut child = root.first_child();
        while let Some(widget) = child {
            if let Some(found) = find(&widget, accept) {
                return Some(found);
            }
            child = widget.next_sibling();
        }
        None
    }

    /// The key the real start would hand to the controller, read from the
    /// session's configuration directory.
    fn controller_key_source(session: &super::Session) -> addict_deepl::key::KeySource {
        use addict_deepl::key::{self, KeyError, KeySource};

        let path = key::key_path(session.dirs.config.as_deref());
        let loaded = path
            .as_deref()
            .map_or(Err(KeyError::NoDirectory), key::read);
        KeySource::new(loaded)
    }

    /// Gives `session` a running controller, as `start` does; the session
    /// lives as long as the returned owner.
    fn attach_controller(session: &mut super::Session) -> super::Controller {
        let key = controller_key_source(session).snapshot();
        let controller =
            super::start_controller(&session.dirs, key, false, &session.sender).unwrap();
        connect_controller(session, &controller);
        controller
    }

    fn attach_translator(
        session: &mut super::Session,
        translator: impl addict_core::translation::Translator + 'static,
    ) -> super::Controller {
        let source = controller_key_source(session);
        let controller = addict_controller::start_with_translator(
            session.dirs.clone(),
            super::controller_platform(),
            translator,
            false,
            source,
            super::controller_renderer(&session.sender),
        )
        .unwrap();
        connect_controller(session, &controller);
        controller
    }

    /// Attaches a controller whose translation provider and key source are
    /// supplied by the test, as the stub-server key tests need.
    fn attach_translator_with_source(
        session: &mut super::Session,
        translator: impl addict_core::translation::Translator + 'static,
        source: addict_deepl::key::KeySource,
    ) -> super::Controller {
        let controller = addict_controller::start_with_translator(
            session.dirs.clone(),
            super::controller_platform(),
            translator,
            false,
            source,
            super::controller_renderer(&session.sender),
        )
        .unwrap();
        connect_controller(session, &controller);
        controller
    }

    fn connect_controller(session: &mut super::Session, controller: &super::Controller) {
        session.work = controller.work();
        session.controller = Some(controller.dispatcher());
        session.articles = controller.dictionaries();
    }

    #[derive(Clone)]
    struct SharedCanned(std::sync::Arc<addict_core::translation::Canned>);

    impl SharedCanned {
        fn new(answer: &str) -> Self {
            Self(std::sync::Arc::new(addict_core::translation::Canned::new(
                Ok(answer),
            )))
        }

        fn asked(&self) -> Vec<addict_core::translation::Request> {
            self.0.asked()
        }
    }

    impl addict_core::translation::Translator for SharedCanned {
        fn translate(
            &self,
            request: addict_core::translation::Request,
        ) -> addict_core::translation::Translating<'_> {
            self.0.translate(request)
        }
    }

    /// Handles the session's events until `done` holds, for at most 5 s.
    fn pump_until(
        session: &mut super::Session,
        events: &mut futures_channel::mpsc::UnboundedReceiver<Event>,
        done: impl Fn(&super::Session) -> bool,
    ) {
        use futures_util::{
            StreamExt,
            future::{Either, select},
        };
        let context = gtk::glib::MainContext::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !done(session) {
            assert!(
                std::time::Instant::now() < deadline,
                "the session did not settle"
            );
            let next = context.block_on(async {
                match select(
                    events.next(),
                    gtk::glib::timeout_future(std::time::Duration::from_millis(100)),
                )
                .await
                {
                    Either::Left((event, _)) => event,
                    Either::Right(_) => None,
                }
            });
            if let Some(event) = next {
                session.handle(event);
            }
        }
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_gtk_edit_renders_the_controller_article_and_enter_waits_for_its_language() {
        use super::*;
        use addict_dictionary::{
            format::{Contents, Headword, SenseRecord, encode},
            import::{self, Source as IndexSource},
        };

        const DEFINITION: &str = "the ground alongside a river";
        let data = KeyTestDir::new("addict-typed-lookup");
        let index = encode(&Contents {
            source: "oewn".to_owned(),
            senses: vec![SenseRecord {
                definition: DEFINITION.to_owned(),
                members: vec!["bank".to_owned()],
                ..Default::default()
            }],
            headwords: vec![Headword {
                word: "bank".to_owned(),
                senses: vec![0],
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        std::fs::write(import::index_path(&data.0, IndexSource::Oewn), index).unwrap();
        // A Spanish index with no entry for bank lets the lookup fall back to English.
        let spanish = encode(&Contents {
            source: "kaikki-es".to_owned(),
            headwords: vec![Headword {
                word: "otro".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        std::fs::write(import::index_path(&data.0, IndexSource::KaikkiEs), spanish).unwrap();

        gtk::init().expect("a display");
        let (mut session, mut events, work) = test_session(
            "io.github.doqhu.Addict.TypedLookupTest",
            Dirs {
                data: Some(data.0.clone()),
                ..Dirs::default()
            },
        );
        drop(work);
        let canned = SharedCanned::new("берег");
        let _controller = attach_translator(&mut session, canned.clone());
        // An earlier text in the history, submitted while the window was
        // empty: nothing of it is asked online.
        session.dispatch_controller(addict_controller::Event::FieldEdited {
            text: "earlier text".to_owned(),
            replaced: false,
            source: None,
        });
        session.dispatch_controller(addict_controller::Event::FieldSubmitted);
        session.dispatch_controller(addict_controller::Event::FieldEdited {
            text: String::new(),
            replaced: false,
            source: None,
        });
        pump_until(&mut session, &mut events, |session| {
            // Two new texts: the earlier one, then none.
            session.view_state.text_number == 2
        });
        session.window.present();
        let entry =
            find::<gtk::Entry>(session.view.root().upcast_ref(), &|_| true).expect("lookup entry");

        entry.insert_text("bank", &mut 0);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.answered == Some(Language::En)
        });
        assert!(
            find::<gtk::Label>(session.view.root().upcast_ref(), &|label| {
                label.text() == DEFINITION && label.is_visible()
            })
            .is_some()
        );
        assert!(canned.asked().is_empty(), "typing sends no online request");
        // Typing keeps no history: back at the earlier text, Back has
        // nowhere to go.
        entry.set_text("earlier text");
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == "earlier text" && !session.view_state.looking_up
        });
        assert!(!session.view_state.can_back, "typing keeps no history");

        entry.delete_text(0, -1);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence.is_empty()
        });
        session.handle(Event::Setting(addict_controller::Event::ReadingChosen(
            Language::Es,
        )));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.reading == Language::Es
        });
        entry.insert_text("bank", &mut 0);
        entry.emit_activate();
        pump_until(&mut session, &mut events, |session| {
            !session.view_state.looking_up
                && session
                    .view_state
                    .online
                    .iter()
                    .any(|row| row.line == "берег")
        });
        // Enter records the word: from the earlier text Back goes to it.
        entry.set_text("earlier text");
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == "earlier text" && !session.view_state.looking_up
        });
        assert!(session.view_state.can_back, "Enter records the word");
        let requests = canned.asked();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].text, "bank");
        assert_eq!(requests[0].source, Language::En);
        assert_eq!(requests[0].target, Language::Ru);
        session.window.destroy();
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_delayed_typed_snapshot_refreshes_a_refusal_without_overwriting_newer_indexes() {
        use super::*;
        use crate::dictionaries::State;
        use futures_util::future::{Either, select};

        gtk::init().expect("a display");
        let (mut session, mut events, work) =
            test_session("io.github.doqhu.Addict.TypedSnapshotTest", Dirs::default());
        drop(work);
        let _controller = attach_controller(&mut session);
        let context = glib::MainContext::default();
        // The snapshot of the settled lookup of `word`, read in `language`,
        // with its effects, not yet handled.
        let mut completed = |session: &mut Session, word: &str, language: Language| {
            if session.view_state.settings.reading != language {
                session.dispatch_controller(addict_controller::Event::ReadingChosen(language));
            }
            session.view.set_field_text(word);
            session.sentence_changed(word.to_owned(), FieldEdit::Typed);
            context.block_on(async {
                let wait = async {
                    while let Some(event) = events.next().await {
                        if let Event::Controller(snapshot, effects) = event
                            && snapshot.sentence == word
                            && !snapshot.looking_up
                        {
                            return (snapshot, effects);
                        }
                    }
                    panic!("resident event channel closed");
                };
                match select(Box::pin(wait), glib::timeout_future(Duration::from_secs(5))).await {
                    Either::Left((snapshot, _)) => snapshot,
                    Either::Right(_) => panic!("no completed typed lookup"),
                }
            })
        };

        // A missing Spanish dictionary refreshes the index check even when
        // its snapshot comes after a newer text.
        let (refused, effects) = completed(&mut session, "banco", Language::Es);
        assert_eq!(
            refused.shown.lookup(),
            Some(&crate::view::Lookup::NoDictionary(Language::Es))
        );
        session.view.set_field_text("new text");
        session.sentence_changed("new text".to_owned(), FieldEdit::Replaced);
        session.render_controller(*refused, effects);
        assert!(
            session.check.take().is_some(),
            "a stale refusal still refreshes"
        );

        // An import already owning the refresh wins.
        let (refused, effects) = completed(&mut session, "otro", Language::Es);
        session.indexes.begin_import();
        session.report_indexes();
        session.render_controller(*refused, effects);
        assert!(
            session.check.is_none(),
            "the import already owns the refresh"
        );

        // A lookup started in the new generation reports the indexes; a
        // delayed one of the old generation does not overwrite them.
        let generation = session.indexes.generation;
        let (observed, effects) = completed(&mut session, "bank", Language::En);
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::ObserveIndexes { generation: g, .. } if *g == generation
        )));
        session.render_controller(*observed, effects);
        let current = session.indexes.current().expect("observed indexes");
        assert_eq!(current.definitions, State::Missing);
        session.render_controller(
            session.view_state.clone(),
            vec![Effect::ObserveIndexes {
                generation: generation - 1,
                indexes: Indexes::OPEN,
            }],
        );
        assert_eq!(session.indexes.current(), Some(current));
        session.window.destroy();
    }

    /// With Translate sentence at once on, a sentence read at a press is
    /// sent at once, alone, and shown with no word chosen; a click on a word
    /// then sends the word only. With it off nothing is sent until a click,
    /// and a later read of the file does not undo the user's choice. Run
    /// alone: GTK starts once per process, on one thread.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_copied_sentence_is_translated_at_once_only_while_the_switch_is_on() {
        use crate::view::{OnlineKind, OnlineState};
        gtk::init().expect("a display");
        let (mut session, mut events, _work) =
            test_session("io.github.doqhu.Addict.AtOnceTest", Dirs::default());
        let canned = SharedCanned::new("Я сидел на берегу реки.");
        let _controller = attach_translator(&mut session, canned.clone());
        let line = "I sat on the bank of the river.";

        let id = start_read(&mut session, &mut events);
        session.handle(selected(id, line));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == line
                && session
                    .view_state
                    .online
                    .iter()
                    .any(|row| row.state == OnlineState::Translated)
        });
        assert_eq!(session.view_state.chosen, None);
        let asked = canned.asked();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].text, line);
        assert_eq!(asked[0].context, None);
        assert_eq!(
            (asked[0].source, asked[0].target),
            (Language::En, Language::Ru)
        );
        assert_eq!(session.view_state.online[0].line, "Я сидел на берегу реки.");
        // A click on bank adds the word and uses the existing sentence.
        session.word_chosen(session.view_state.text_number, 4);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.chosen == Some(4)
                && session
                    .view_state
                    .online
                    .iter()
                    .any(|row| row.kind == OnlineKind::Word && row.state == OnlineState::Translated)
        });
        let asked = canned.asked();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[1].text, "bank");
        assert_eq!(asked[1].context.as_deref(), Some(line));

        // Off affects the next read; a late startup read cannot undo it,
        // because the controller ignores a read after a choice.
        session.handle(Event::Setting(addict_controller::Event::AtOnceChosen(
            false,
        )));
        // The controller applies the choice in event order; the next read
        // carries the new snapshot.
        let id = start_read(&mut session, &mut events);
        session.handle(selected(id, "The bank was closed."));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == "The bank was closed."
        });
        assert!(session.view_state.online.is_empty());
        assert_eq!(canned.asked().len(), 2);
    }

    /// Presses Read as the shortcut does, up to the controller's read effect
    /// in the shell; gives the read's id. The window is not active, so
    /// nothing is read from GTK.
    fn start_read(
        session: &mut super::Session,
        events: &mut futures_channel::mpsc::UnboundedReceiver<Event>,
    ) -> addict_controller::RequestId {
        session.clear_for_read();
        pump_until(session, events, |session| session.copying.read.is_some());
        session.copying.read.as_ref().expect("a read").id
    }

    /// The raw texts of a read whose selection is `line`.
    fn selected(id: addict_controller::RequestId, line: &str) -> Event {
        Event::SelectionRead(
            id,
            addict_controller::SelectionText {
                selection: Some(line.to_owned()),
                clipboard: None,
            },
        )
    }

    /// Puts `texts` into the field one after another as the user would
    /// submit them, with a controller attached, and waits for them: Back
    /// has somewhere to go.
    fn submit_texts(
        session: &mut super::Session,
        events: &mut futures_channel::mpsc::UnboundedReceiver<Event>,
        texts: &[&str],
    ) {
        use super::FieldEdit;
        for text in texts {
            session.view.set_field_text(text);
            session.sentence_changed((*text).to_owned(), FieldEdit::Typed);
            session.submitted();
        }
        let last = texts.last().copied().unwrap_or_default().to_owned();
        pump_until(session, events, move |session| {
            session.view_state.sentence == last
                && session.view_state.can_back
                && !session.view_state.looking_up
        });
    }

    /// A line read after the field was cleared for the read reaches the
    /// field through the controller's snapshot, and its words can be
    /// clicked: the snapshot of the clear never overwrites it (AD-21). The
    /// read starts with that snapshot's effects, so the line cannot come
    /// before it. Run alone: GTK starts once per process, on one thread.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_read_line_handled_before_the_cleared_snapshot_keeps_the_line() {
        gtk::init().expect("a display");
        let (mut session, mut events, _work) =
            test_session("io.github.doqhu.Addict.ClearedLineTest", Dirs::default());
        let _controller = attach_controller(&mut session);
        session.dispatch_controller(addict_controller::Event::AtOnceChosen(false));
        let line = "I sat on the bank of the river.";

        let id = start_read(&mut session, &mut events);
        session.handle(selected(id, line));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == line && !session.view_state.looking_up
        });
        assert_eq!(session.view.field_text(), line);
        assert_eq!(session.sent_text, line);
        assert_eq!(session.view_state.field.text, line);

        session.word_chosen(session.view_state.text_number, 4);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.chosen == Some(4) && !session.view_state.looking_up
        });
        assert_eq!(session.view.field_text(), line);
    }

    /// A key typed right after Back, before Back's snapshot comes, is kept:
    /// the controller's text of Back does not overwrite it, and the shell
    /// and the controller agree on the typed text after. Run alone.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_keystroke_after_back_before_its_snapshot_is_kept() {
        use super::FieldEdit;
        gtk::init().expect("a display");
        let (mut session, mut events, _work) =
            test_session("io.github.doqhu.Addict.BackKeystrokeTest", Dirs::default());
        let _controller = attach_controller(&mut session);
        submit_texts(&mut session, &mut events, &["river", "bank"]);
        let revision = session.applied_field_revision;

        session.back();
        session.view.set_field_text("banks");
        session.sentence_changed("banks".to_owned(), FieldEdit::Typed);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == "banks" && !session.view_state.looking_up
        });
        assert_eq!(session.view.field_text(), "banks");
        assert_eq!(session.sent_text, "banks");
        assert_eq!(session.view_state.field.text, "banks");
        assert!(
            session.applied_field_revision > revision,
            "Back's revision is applied, its text is not"
        );
        // The shell is in step: a word click of the typed text is taken.
        assert!(!session.field_is_newer());
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_delayed_ocr_snapshot_respects_newer_edits_and_read_presentation() {
        use super::{Effect, FieldEdit, ViewState};

        gtk::init().expect("a display");
        let (mut session, _, _work) = test_session(
            "io.github.doqhu.Addict.CaptureSnapshotTest",
            Dirs::default(),
        );
        let mut recognized = ViewState {
            revision: 1,
            ocr_field_revision: Some(1),
            ..ViewState::default()
        };
        recognized.field.text = "recognized words".to_owned();
        recognized.field.revision = 1;
        session.capturing.edits_at_recognition = session.edits_sent;
        session.view.set_field_text("new typing");
        session.sentence_changed("new typing".to_owned(), FieldEdit::Typed);
        session.render_controller(recognized.clone(), Vec::new());
        assert_eq!(session.applied_field_revision, 1);
        assert_eq!(session.view.field_text(), "new typing");

        session.give_up_press();
        session.clear_for_read();
        recognized.revision += 1;
        recognized.field.revision += 1;
        recognized.ocr_field_revision = Some(recognized.field.revision);
        session.render_controller(
            recognized,
            vec![Effect::PresentCapture {
                read_sequence: 0,
                retry: true,
            }],
        );
        assert_eq!(session.applied_field_revision, 1);
        assert!(session.view.field_text().is_empty());
        assert!(!session.window.is_visible());
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_hidden_picker_waits_for_close_and_rechecks_read_before_starting() {
        use super::*;

        gtk::init().expect("a display");
        let (send, renders) = std::sync::mpsc::channel();
        let controller = controller::start(
            Dirs::default(),
            controller_platform(),
            StartFacts {
                key: Some(Err(KeyError::NoDirectory)),
                recording: false,
            },
            move |_, effects| send.send(effects).unwrap(),
        )
        .unwrap();
        let press = || {
            controller.dispatch(addict_controller::Event::CapturePressed {
                activation_token: None,
            })
        };
        assert!(press());
        assert!(controller.dispatch(addict_controller::Event::ReadPressed { sequence: 1 }));
        assert!(press());
        let ids: Vec<_> = (0..4)
            .flat_map(|_| renders.recv_timeout(Duration::from_secs(5)).unwrap())
            .filter_map(|effect| match effect {
                Effect::CaptureArea { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        let &[closing, next] = ids.as_slice() else {
            panic!("two picker IDs: {ids:?}");
        };

        let (mut session, mut events, work) =
            test_session("io.github.doqhu.Addict.CaptureCloseTest", Dirs::default());
        // Any actual start uses the missing-runner path instead of a real picker.
        session.work = std::sync::Weak::new();
        for read_wins in [false, true] {
            let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
            let job = work.run(Duration::from_secs(5), std::future::pending::<()>(), |_| {});
            session.capturing.pending.insert(
                closing,
                CaptureEffect::Picker {
                    cancel: Some(cancel),
                    _job: job,
                },
            );
            session.cancel_capture_effect(closing);
            assert_eq!(cancelled.try_recv(), Ok(()));
            session.capturing.pending.insert(
                next,
                CaptureEffect::Settling {
                    read_sequence: session.read_sequence,
                    hidden: false,
                },
            );
            session.handle(Event::Hidden(next));
            assert!(matches!(
                session.capturing.pending.get(&next),
                Some(CaptureEffect::Settling { hidden: true, .. })
            ));
            if read_wins {
                session.clear_for_read();
            }
            session.handle(Event::CaptureFinished(
                closing,
                addict_controller::Event::AreaCaptured {
                    id: closing,
                    result: Err(Ended::Shell(CaptureFailure::Cancelled)),
                    capture_ms: None,
                },
            ));
            assert!(!session.capturing.pending.contains_key(&next));
            let mut started = false;
            while let Ok(event) = events.try_recv() {
                if matches!(event, Event::CaptureFinished(id, addict_controller::Event::AreaCaptured {
                    result: Err(Ended::Unfinished), ..
                }) if id == next)
                {
                    started = true;
                }
            }
            assert_eq!(started, !read_wins);
        }
    }

    /// Without a controller, a Read press does not wait forever: the window
    /// shows at once with the field empty and says nothing is selected or
    /// copied. Run alone.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn read_without_a_controller_ends_the_wait_and_says_nothing_is_selected() {
        gtk::init().expect("a display");
        let (mut session, _, _work) =
            test_session("io.github.doqhu.Addict.ReadFallbackTest", Dirs::default());
        assert!(session.controller.is_none());
        session.activated(None, 0, None);
        assert!(session.window.is_visible());
        assert_eq!(session.view_state.reading_source, None);
        assert_eq!(
            session.view.source_failure_text().as_deref(),
            Some(crate::view::NOTHING_SELECTED_TEXT)
        );
        session.window.destroy();
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn capture_without_a_controller_shows_retry_and_records_unfinished() {
        use super::*;

        gtk::init().expect("a display");
        let data = std::env::temp_dir().join(format!(
            "addict-capture-fallback-{}.jsonl",
            std::process::id()
        ));
        let (mut session, _, _work) = test_session(
            "io.github.doqhu.Addict.CaptureFallbackTest",
            Dirs::default(),
        );
        let writer = evidence::Writer::open(&data, evidence::Origin::Local).unwrap();
        let writer = Rc::new(RefCell::new(writer));
        session.recording = Some(Recording::start(writer, &session.window));

        session.capture(Some("test-startup-token"));
        assert!(session.window.is_visible());
        assert!(session.capturing.activation_token.is_none());
        let focus = gtk::prelude::GtkWindowExt::focus(&session.window)
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap();
        assert_eq!(focus.label().as_deref(), Some(crate::view::CAPTURE_LABEL));
        let recorded = std::fs::read_to_string(&data).unwrap();
        let ocr = recorded
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|row| row["event"] == "ocr")
            .expect("OCR evidence");
        assert_eq!(ocr["details"]["outcome"], "unfinished");
        session.window.destroy();
        std::fs::remove_file(data).unwrap();
    }

    /// Back while the selection and the clipboard are being read gives the
    /// read up, as a typed text does: the line it reports later does not
    /// replace Back's text. Run alone.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn back_during_a_clipboard_read_gives_the_read_up() {
        gtk::init().expect("a display");
        let (mut session, mut events, _work) =
            test_session("io.github.doqhu.Addict.BackReadTest", Dirs::default());
        let _controller = attach_controller(&mut session);
        submit_texts(&mut session, &mut events, &["river", "bank"]);

        let id = start_read(&mut session, &mut events);
        session.back();
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == "river"
                && !session.view_state.looking_up
                && session.copying.read.is_none()
        });
        // The controller's `Cancel` ended the read: what it reports later
        // is ignored.
        session.handle(selected(id, "The bank was closed."));
        assert_eq!(session.view.field_text(), "river");
        assert_eq!(session.sent_text, "river");
    }

    /// On a fresh install the first-run offer, fed `manifest-3.json` by the
    /// download stub, queues Russian's set when no choice is made, and
    /// exactly English's when English is chosen in the dialog, which makes
    /// it my language and shows it in Settings. Nothing is installed: the
    /// stub has no asset, and the installs are cancelled.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn first_run_queues_the_set_of_the_language_chosen_in_the_offer() {
        use std::collections::HashMap;

        use super::*;
        use crate::stub::{Route, Stub};
        use addict_dictionary::{pairs, release, sources};

        struct Temp(std::path::PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let temp = Temp(
            std::env::temp_dir().join(format!("addict-first-run-language-{}", std::process::id())),
        );
        std::fs::create_dir(&temp.0).unwrap();
        let dirs = Dirs {
            config: Some(temp.0.join("config/addict")),
            data: Some(temp.0.join("data/addict")),
            cache: None,
        };

        let manifest = Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: sources::ROWS
                .iter()
                .filter(|row| addict_dictionary::release::MANIFEST_3_PUBLISHED.contains(&row.index))
                .map(|row| Published::of(row.index, row.index.as_bytes()))
                .collect(),
        };
        assert_eq!(manifest.indexes.len(), 20);
        let stub = Stub::start(HashMap::from([(
            format!("/{}/{}", release::tag(), release::MANIFEST_3),
            Route::File(manifest.to_json().into_bytes()),
        )]))
        .unwrap();

        gtk::init().expect("a display");
        let (mut session, mut events, _work) =
            test_session("io.github.doqhu.Addict.FirstRunLanguageTest", dirs);
        // My language lives in the controller: its choices need one.
        let _controller = attach_controller(&mut session);
        session.release = stub.base_url();
        let queued = |session: &Session| -> Vec<String> {
            let FirstRun::Installing { queue, .. } = &session.first_run else {
                panic!("installing");
            };
            queue
                .rows()
                .iter()
                .map(|(published, _)| published.name.clone())
                .collect()
        };
        let set = |my: Language| -> Vec<String> {
            pairs::required_index_names(my).map(str::to_owned).collect()
        };
        // The rows are grouped by language, so they are compared as a set.
        let sorted = |mut names: Vec<String>| -> Vec<String> {
            names.sort();
            names
        };

        for chosen in [None, Some(Language::En)] {
            // A fresh start: nothing chosen yet.
            session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
                Language::Ru,
            )));
            pump_until(&mut session, &mut events, |session| {
                session.view_state.settings.languages_read
                    && session.view_state.settings.my_language == Language::Ru
            });
            session.first_run = FirstRun::Unasked;
            session.indexes = IndexCache::default();
            session.window.set_visible(true);
            session.handle(Event::Indexes {
                generation: 0,
                indexes: Indexes::MISSING,
            });
            assert!(matches!(session.first_run, FirstRun::Asking { .. }));
            pump_until(&mut session, &mut events, |session| {
                matches!(session.first_run, FirstRun::Shown { .. })
            });
            let FirstRun::Shown { offer, .. } = &session.first_run else {
                unreachable!();
            };
            assert_eq!(offer.my_language().selected(), 3, "Russian at first");
            assert_eq!(sorted(offer.listed()), sorted(set(Language::Ru)));
            if chosen.is_none() {
                // A choice in Settings while the offer waits: it shows it
                // and lists its set, and back to Russian.
                session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
                    Language::Es,
                )));
                pump_until(&mut session, &mut events, |session| {
                    session.view_state.settings.my_language == Language::Es
                });
                let FirstRun::Shown { offer, .. } = &session.first_run else {
                    panic!("the offer stays");
                };
                assert_eq!(offer.my_language().selected(), 1);
                assert_eq!(sorted(offer.listed()), sorted(set(Language::Es)));
                session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
                    Language::Ru,
                )));
                pump_until(&mut session, &mut events, |session| {
                    session.view_state.settings.my_language == Language::Ru
                });
                let FirstRun::Shown { offer, .. } = &session.first_run else {
                    panic!("the offer stays");
                };
                assert_eq!(sorted(offer.listed()), sorted(set(Language::Ru)));
            }
            let FirstRun::Shown { offer, .. } = &session.first_run else {
                unreachable!();
            };
            if let Some(language) = chosen {
                let index = Language::ALL
                    .iter()
                    .position(|one| *one == language)
                    .unwrap();
                offer.my_language().set_selected(index as u32);
                pump_until(&mut session, &mut events, |session| {
                    session.view_state.settings.my_language == language
                });
                assert_eq!(session.settings.shown_my_language(), Some(language));
                let FirstRun::Shown { offer, .. } = &session.first_run else {
                    panic!("the offer stays");
                };
                assert_eq!(offer.my_language().selected(), index as u32);
                assert_eq!(sorted(offer.listed()), sorted(set(language)));
            }
            let run = session.download_run;
            session.handle(Event::FirstRunDownload(run));
            assert_eq!(
                queued(&session),
                set(chosen.unwrap_or(Language::Ru)),
                "{chosen:?}"
            );
            // Once Download is pressed the choice is gone, and one in
            // Settings leaves the queue as it is.
            let FirstRun::Installing { offer, .. } = &session.first_run else {
                panic!("installing");
            };
            assert!(!offer.shows_my_language());
            let before = queued(&session);
            session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
                Language::Ca,
            )));
            pump_until(&mut session, &mut events, |session| {
                session.view_state.settings.my_language == Language::Ca
            });
            assert_eq!(queued(&session), before);
            session.handle(Event::FirstRunCancel(run, None));
            // At most the first of the set was asked for before the cancel.
            let allowed = set(chosen.unwrap_or(Language::Ru));
            for path in stub.seen() {
                if let Some(asset) = path.rsplit('/').next()
                    && let Some(name) = asset.strip_suffix(".index.xz")
                {
                    assert!(allowed.iter().any(|one| one == name), "{path}");
                }
            }
        }
        assert_eq!(set(Language::Ru), release::LEGACY_PUBLISHED);
        assert_ne!(set(Language::En), set(Language::Ru));
        session.settings.window().destroy();
        session.window.destroy();
    }

    /// The reading switch and the gloss's English link end with the same
    /// reading language whichever file was read first. Run alone: GTK starts
    /// once per process, on one thread.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn the_reading_language_holds_in_either_read_order_and_on_a_gloss_click() {
        use addict_dictionary::{
            format::{Contents, Headword, SenseRecord, encode},
            import::{self, Source as IndexSource},
        };
        use addict_engine::dictionaries::{Indexes, State};
        gtk::init().expect("a display");
        let data = KeyTestDir::new("addict-gloss-reading");
        for (index, source, definition) in [
            (
                IndexSource::KaikkiEs,
                "kaikki-es",
                "Empresa que ofrece servicios monetarios.",
            ),
            (IndexSource::KaikkiEnSpanish, "es-en", "bank"),
        ] {
            let index_bytes = encode(&Contents {
                source: source.to_owned(),
                senses: vec![SenseRecord {
                    definition: definition.to_owned(),
                    ..Default::default()
                }],
                headwords: vec![Headword {
                    word: "banco".to_owned(),
                    senses: vec![0],
                    ..Default::default()
                }],
                ..Default::default()
            })
            .unwrap();
            std::fs::write(import::index_path(&data.0, index), index_bytes).unwrap();
        }
        let config = data.0.join("config");
        let (mut session, mut events, _work) = test_session(
            "io.github.doqhu.Addict.LanguageTest",
            Dirs {
                data: Some(data.0.clone()),
                config: Some(config.clone()),
                ..Dirs::default()
            },
        );
        let canned = SharedCanned::new("translated");
        let controller = attach_translator(&mut session, canned.clone());
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.languages_read
        });
        let read = |item: addict_controller::SettingsRead| {
            assert!(controller.dispatch_settings_read(item));
        };
        let reading = |session: &super::Session| session.view_state.settings.reading;
        let saved_file = || std::fs::read_to_string(config.join("language")).unwrap_or_default();
        let wait_saved = |expected: &str| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::fs::read_to_string(config.join("language"))
                .unwrap_or_default()
                .trim()
                != expected
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the write did not land"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };

        read(addict_controller::SettingsRead::Reading(Language::Ru));
        pump_until(&mut session, &mut events, |session| {
            reading(session) == Language::En
        });
        read(addict_controller::SettingsRead::My(Language::En));
        pump_until(&mut session, &mut events, |session| {
            reading(session) == Language::Ru
        });
        assert_eq!(reading(&session), Language::Ru);

        // Back to the start: nothing read, Russian mine.
        read(addict_controller::SettingsRead::My(Language::Ru));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.my_language == Language::Ru
        });
        read(addict_controller::SettingsRead::Reading(Language::default()));
        pump_until(&mut session, &mut events, |session| {
            reading(session) == Language::En
        });
        read(addict_controller::SettingsRead::My(Language::En));
        pump_until(&mut session, &mut events, |session| {
            reading(session) == Language::Es
        });
        read(addict_controller::SettingsRead::Reading(Language::Ru));
        pump_until(&mut session, &mut events, |session| {
            reading(session) == Language::Ru
        });

        session.handle(Event::Indexes {
            generation: 0,
            indexes: Indexes {
                spanish: State::Open,
                ..Indexes::MISSING
            },
        });
        session.view.set_field_text("banco");
        session.sentence_changed("banco".to_owned(), super::FieldEdit::Typed);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.answered == Some(Language::Es)
        });
        let article = session.view_state.article_id();
        session.handle(Event::Gloss(article, 0));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.shown.choice() == Some(super::Choice::Gloss(0))
        });
        session.handle(Event::LookUpEnglish(article, 0, "bank".to_owned()));
        session.handle(Event::LookUpEnglish(article, 0, "bank".to_owned()));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == "bank" && reading(session) == Language::Es
        });
        assert_eq!(reading(&session), Language::Es);
        // A read handled after the choice, while its write may still run,
        // neither undoes it nor drops (and so cancels) its write.
        let before = session.view_state.revision;
        read(addict_controller::SettingsRead::Reading(Language::Ru));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.revision > before
        });
        assert_eq!(reading(&session), Language::Es);
        wait_saved("es");
        assert_eq!(saved_file().trim(), "es");

        // A switch press before the gloss snapshot is delivered wins, even
        // when it names the language the shell already showed.
        session.handle(Event::Setting(addict_controller::Event::ReadingChosen(
            Language::Ru,
        )));
        session.view.set_field_text("banco");
        session.sentence_changed("banco".to_owned(), super::FieldEdit::Typed);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == "banco"
                && session.view_state.answered == Some(Language::Es)
        });
        let article = session.view_state.article_id();
        session.handle(Event::Gloss(article, 0));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.shown.choice() == Some(super::Choice::Gloss(0))
        });
        session.handle(Event::LookUpEnglish(article, 0, "bank".to_owned()));
        session.handle(Event::Setting(addict_controller::Event::ReadingChosen(
            Language::Ru,
        )));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == "bank"
                && session.view_state.shown.translating().pressed == Language::Ru
                && session
                    .view_state
                    .online
                    .iter()
                    .any(|row| row.line == "translated")
        });
        wait_saved("ru");
        assert_eq!(canned.asked().last().unwrap().source, Language::Ru);

        // The controller sees the gloss choice before the target change,
        // even while GTK still holds the previous saved reading language.
        session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
            Language::Ru,
        )));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.my_language == Language::Ru
        });
        session.handle(Event::Setting(addict_controller::Event::ReadingChosen(
            Language::Es,
        )));
        session.view.set_field_text("banco");
        session.sentence_changed("banco".to_owned(), super::FieldEdit::Typed);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == "banco"
                && session.view_state.answered == Some(Language::Es)
        });
        let article = session.view_state.article_id();
        session.handle(Event::Gloss(article, 0));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.shown.choice() == Some(super::Choice::Gloss(0))
        });
        let first = canned.asked().len();
        session.handle(Event::LookUpEnglish(article, 0, "bank".to_owned()));
        session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
            Language::Ca,
        )));
        pump_until(&mut session, &mut events, |session| {
            reading(session) == Language::En
                && session.view_state.shown.translating().my == Language::Ca
                && session.view_state.sentence == "bank"
                && session
                    .view_state
                    .online
                    .iter()
                    .any(|row| row.line == "translated")
        });
        let requests = canned.asked();
        let mut for_catalan = requests[first..]
            .iter()
            .filter(|request| request.target == Language::Ca);
        let request = for_catalan.next().expect("translation into Catalan");
        assert_eq!(
            (request.text.as_str(), request.source),
            ("bank", Language::En)
        );
        assert!(for_catalan.next().is_none());
        session.settings.window().destroy();
        session.window.destroy();
    }

    /// British English reaches the request as `EN-GB` with English mine,
    /// whichever setting was chosen first, and the row is shown only then.
    /// Run alone: GTK starts once per process, on one thread.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn the_english_spelling_holds_in_either_read_order_and_shows_only_for_english() {
        use super::*;
        use addict_core::translation::English;
        use addict_deepl::client::sent;
        gtk::init().expect("a display");
        let temp = KeyTestDir::new("addict-english-reading");
        let config = temp.0.join("config");
        let (mut session, mut events, _work) = test_session(
            "io.github.doqhu.Addict.EnglishTest",
            Dirs {
                config: Some(config.clone()),
                ..Dirs::default()
            },
        );
        let canned = SharedCanned::new("translated");
        let controller = attach_translator(&mut session, canned.clone());
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.languages_read
        });
        let read = |item: addict_controller::SettingsRead| {
            assert!(controller.dispatch_settings_read(item));
        };
        let wait_saved = |expected: &str| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::fs::read_to_string(config.join("english"))
                .unwrap_or_default()
                .trim()
                != expected
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the write did not land"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        let target_lang = |session: &mut super::Session, events: &mut _| {
            let previous = canned.asked().len();
            let text = format!("word{}", char::from(b'a' + previous as u8));
            session.view.set_field_text(&text);
            session.sentence_changed(text, super::FieldEdit::Typed);
            session.submitted();
            pump_until(session, events, |session| {
                canned.asked().len() > previous
                    && session
                        .view_state
                        .online
                        .iter()
                        .any(|row| row.line == "translated")
            });
            sent(canned.asked().last().unwrap()).target_lang
        };

        assert_eq!(session.settings.english_row(), (false, English::Us));
        assert_eq!(target_lang(&mut session, &mut events), "RU");

        // The spelling read first, then English mine.
        read(addict_controller::SettingsRead::English(English::Gb));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.english == English::Gb
        });
        assert_eq!(session.settings.english_row(), (false, English::Gb));
        assert_eq!(target_lang(&mut session, &mut events), "RU");
        session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
            Language::En,
        )));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.my_language == Language::En
        });
        assert_eq!(session.settings.english_row(), (true, English::Gb));
        assert_eq!(target_lang(&mut session, &mut events), "EN-GB");

        // Back to the start: nothing read, Russian mine, US.
        session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
            Language::Ru,
        )));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.my_language == Language::Ru
        });
        read(addict_controller::SettingsRead::English(English::Us));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.english == English::Us
        });
        // English mine read first, then the spelling.
        session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
            Language::En,
        )));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.my_language == Language::En
        });
        assert_eq!(session.settings.english_row(), (true, English::Us));
        assert_eq!(target_lang(&mut session, &mut events), "EN-US");
        read(addict_controller::SettingsRead::English(English::Gb));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.english == English::Gb
        });
        assert_eq!(session.settings.english_row(), (true, English::Gb));
        assert_eq!(target_lang(&mut session, &mut events), "EN-GB");

        // A choice holds against a later read, and its write is not dropped.
        session.handle(Event::Setting(addict_controller::Event::EnglishChosen(
            English::Us,
        )));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.english == English::Us
        });
        wait_saved("us");
        read(addict_controller::SettingsRead::English(English::Gb));
        assert_eq!(session.view_state.settings.english, English::Us);
        assert_eq!(target_lang(&mut session, &mut events), "EN-US");

        // Spanish mine: the row is hidden and the spelling changes nothing.
        session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
            Language::Es,
        )));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.my_language == Language::Es
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::fs::read_to_string(config.join("my-language"))
            .unwrap_or_default()
            .trim()
            != "es"
        {
            assert!(
                std::time::Instant::now() < deadline,
                "my language was not kept"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        read(addict_controller::SettingsRead::My(Language::Ru));
        assert_eq!(session.view_state.settings.my_language, Language::Es);
        assert_eq!(
            std::fs::read_to_string(config.join("my-language")).unwrap_or_default(),
            "es\n"
        );
        assert!(!session.settings.english_row().0);
        assert_eq!(target_lang(&mut session, &mut events), "ES");
        session.handle(Event::Setting(addict_controller::Event::EnglishChosen(
            English::Gb,
        )));
        wait_saved("gb");
        assert_eq!(target_lang(&mut session, &mut events), "ES");
        // British, chosen while Spanish was mine, holds once English is.
        session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
            Language::En,
        )));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.my_language == Language::En
        });
        assert!(session.settings.english_row().0);
        assert_eq!(target_lang(&mut session, &mut events), "EN-GB");
        session.settings.window().destroy();
        session.window.destroy();
    }

    /// The indexes line and the definition button follow my language from
    /// the controller's snapshot. Run alone: GTK starts once per process.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn the_indexes_line_and_the_definition_button_follow_my_language() {
        use addict_engine::dictionaries::{Indexes, State};
        gtk::init().expect("a display");
        let (mut session, mut events, _work) =
            test_session("io.github.doqhu.Addict.MyTextsTest", Dirs::default());
        let controller = attach_controller(&mut session);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.languages_read
        });
        // The definition button names my language, which the controller's
        // snapshot carries.
        let mine = |session: &mut super::Session,
                    events: &mut futures_channel::mpsc::UnboundedReceiver<Event>,
                    language: Language| {
            pump_until(session, events, |session| {
                session.view_state.shown.translating().my == language
            });
        };
        let indexes = Indexes {
            definitions: State::Open,
            spanish: State::Open,
            catalan: State::Open,
            ..Indexes::MISSING
        };
        session.handle(Event::Indexes {
            generation: 0,
            indexes,
        });
        let (visible, text) = session.view.indexes_line();
        assert!(visible && text.contains("Russian translations"), "{text}");
        session.render_shown();
        assert_eq!(
            session.view.definition_action_text(),
            "Translate definition into Russian"
        );

        // My language read after the indexes, as at startup.
        assert!(
            controller.dispatch_settings_read(addict_controller::SettingsRead::My(Language::Es))
        );
        mine(&mut session, &mut events, Language::Es);
        assert_eq!(session.view.indexes_line(), (false, String::new()));
        assert_eq!(
            session.view.definition_action_text(),
            "Translate definition into Spanish"
        );

        // Back to Russian, then English chosen in Settings.
        session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
            Language::Ru,
        )));
        mine(&mut session, &mut events, Language::Ru);
        assert!(session.view.indexes_line().0);
        assert_eq!(
            session.view.definition_action_text(),
            "Translate definition into Russian"
        );
        session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
            Language::En,
        )));
        mine(&mut session, &mut events, Language::En);
        assert_eq!(session.view.indexes_line(), (false, String::new()));
        assert_eq!(
            session.view.definition_action_text(),
            "Translate definition into English"
        );
        session.settings.window().destroy();
        session.window.destroy();
    }

    /// The first-run offer waits for both the saved language and the index
    /// snapshot, in either order. Run alone: GTK starts once per process.
    /// A my-language choice made before the start read ended gets its
    /// Settings offer once the read ends: the first-run decision is made
    /// first, as `my_language_chosen` made it, and the offer follows.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_choice_of_my_language_before_the_read_offers_its_missing_set_after_it() {
        use super::*;
        use addict_dictionary::pairs;

        gtk::init().expect("a display");
        let (mut session, _events, _work) =
            test_session("io.github.doqhu.Addict.DeferredOfferTest", Dirs::default());
        session.work = WorkRef::new();
        // A launch: the first run is not decided yet.
        session.first_run = FirstRun::Unasked;
        let required: Vec<_> = pairs::required_sources(Language::Es).collect();
        assert!(required.len() > 1, "a set with something left to lack");
        // One of Spanish's set is there: no first-run offer, but a Settings one.
        session.handle(Event::Indexes {
            generation: 0,
            indexes: Indexes {
                presence: Indexes::MISSING.presence.with(required[0]),
                ..Indexes::MISSING
            },
        });
        let mut revision = session.view_state.revision;
        let mut snapshot = |languages_read: bool, my: Language, my_chosen: u64| {
            revision += 1;
            let mut state = addict_controller::ViewState {
                revision,
                ..addict_controller::ViewState::default()
            };
            state.settings.languages_read = languages_read;
            state.settings.my_language = my;
            state.settings.my_chosen = my_chosen;
            Event::Controller(Box::new(state), Vec::new())
        };
        session.handle(snapshot(false, Language::Ru, 0));
        // Spanish chosen while the files are still being read: nothing yet.
        session.handle(snapshot(false, Language::Es, 1));
        assert!(matches!(session.first_run, FirstRun::Unasked));
        assert_eq!(session.settings.download_text(), "");
        // The read ends: the decision, then the offer of what Spanish lacks.
        session.handle(snapshot(true, Language::Es, 1));
        assert!(matches!(session.first_run, FirstRun::Done));
        assert_eq!(
            session.settings.download_text(),
            first_run::recovery_failed_text(first_run::NO_PLACE)
        );
        session.settings.window().destroy();
        session.window.destroy();
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn first_run_uses_the_saved_language_in_either_order() {
        use super::*;
        use addict_dictionary::{import::Source, release::Published};

        gtk::init().expect("a display");
        let (mut session, _events, work) =
            test_session("io.github.doqhu.Addict.FirstRunTest", Dirs::default());
        // Observe request IDs without contacting the release server.
        session.work = WorkRef::new();
        let pending = || {
            work.run(
                addict_controller::deadlines::LANGUAGE_FILE_LIMIT,
                std::future::pending::<()>(),
                |_| {},
            )
        };
        let indexes = Indexes {
            presence: Indexes::MISSING.presence.with(Source::WikdictRuEs),
            ..Indexes::MISSING
        };
        // The controller's snapshots reach the shell as this event; the read
        // of my language is the first one whose start read has ended.
        let mut revision = session.view_state.revision;
        let mut snapshot = |languages_read: bool, my: Language| {
            revision += 1;
            let mut state = addict_controller::ViewState {
                revision,
                ..addict_controller::ViewState::default()
            };
            state.settings.languages_read = languages_read;
            state.settings.my_language = my;
            Event::Controller(Box::new(state), Vec::new())
        };
        for my in [Language::En, Language::Ru] {
            for indexes_first in [false, true] {
                session.first_run = FirstRun::Unasked;
                session.download_run = 0;
                session.handle(snapshot(false, Language::Ru));
                session.indexes = IndexCache::default();
                session.check = Some(pending());
                let indexes = Event::Indexes {
                    generation: 0,
                    indexes,
                };
                let language = snapshot(true, my);
                let [first, second] = if indexes_first {
                    [indexes, language]
                } else {
                    [language, indexes]
                };
                session.handle(first);
                assert!(matches!(session.first_run, FirstRun::Unasked));
                assert_eq!(session.download_run, 0);
                session.handle(second);
                assert!(matches!(session.first_run, FirstRun::Done));
                assert_eq!(session.download_run, u64::from(my == Language::En));
            }
        }

        // A stale snapshot cannot dismiss a pending offer, and an unrelated
        // file in the latest accepted snapshot keeps the offer due.
        session.handle(snapshot(true, Language::En));
        let manifest = Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: vec![Published::of("oewn", b"xz")],
        };
        session.first_run = FirstRun::Pending(manifest.clone());
        let current = session.indexes.begin_import();
        session.handle(Event::Indexes {
            generation: current - 1,
            indexes: Indexes::OPEN,
        });
        assert!(matches!(session.first_run, FirstRun::Pending(_)));
        let (ready, _) = oneshot::channel();
        session.handle(Event::Imported(ImportCompletion {
            generation: current,
            indexes,
            ready,
        }));
        assert!(matches!(session.first_run, FirstRun::Pending(_)));

        // A manifest arriving while the main window is visible must wait
        // for the current generation before the offer can open.
        session.first_run = FirstRun::Asking { _job: pending() };
        session.window.set_visible(true);
        assert!(session.window.is_visible());
        let (ready, mut generation) = oneshot::channel();
        session.handle(Event::ImportRefreshing(ready));
        let current = generation.try_recv().unwrap().unwrap();
        assert_eq!(session.indexes.known, Some(indexes));
        session.handle(Event::FirstRunOffer(session.download_run, manifest));
        assert!(matches!(session.first_run, FirstRun::Pending(_)));
        let (ready, _) = oneshot::channel();
        session.handle(Event::Imported(ImportCompletion {
            generation: current,
            indexes: Indexes::OPEN,
            ready,
        }));
        assert!(matches!(session.first_run, FirstRun::Done));

        // The initial decision also waits when there is a stale snapshot.
        session.first_run = FirstRun::Unasked;
        assert!(session.indexes.update(current, indexes));
        let run = session.download_run;
        let (ready, mut generation) = oneshot::channel();
        session.handle(Event::ImportRefreshing(ready));
        let current = generation.try_recv().unwrap().unwrap();
        assert!(matches!(session.first_run, FirstRun::Unasked));
        assert_eq!(session.download_run, run);
        let (ready, _) = oneshot::channel();
        session.handle(Event::Imported(ImportCompletion {
            generation: current,
            indexes: Indexes::OPEN,
            ready,
        }));
        assert!(matches!(session.first_run, FirstRun::Done));
        assert_eq!(session.download_run, run);
        session.settings.window().destroy();
        session.window.destroy();
    }

    /// A change of my language in Settings offers just what the new set
    /// lacks, and a waiting offer follows later changes. Run alone.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_change_of_my_language_in_settings_offers_what_the_new_set_lacks() {
        use std::collections::HashMap;

        use super::*;
        use crate::stub::{Route, Stub};
        use addict_dictionary::{import, pairs, release, sources};

        struct Temp(std::path::PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let temp = Temp(
            std::env::temp_dir().join(format!("addict-language-change-{}", std::process::id())),
        );
        std::fs::create_dir(&temp.0).unwrap();
        let dirs = Dirs {
            config: Some(temp.0.join("config/addict")),
            data: Some(temp.0.join("data/addict")),
            cache: None,
        };

        // Russian's set, readable on disk and present in the snapshot.
        let dir = dirs.data.clone().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let index = addict_dictionary::format::encode(&addict_dictionary::format::Contents {
            source: "installed".to_owned(),
            senses: vec![addict_dictionary::format::SenseRecord {
                definition: "a bank".to_owned(),
                members: vec!["bank".to_owned()],
                ..Default::default()
            }],
            headwords: vec![addict_dictionary::format::Headword {
                word: "bank".to_owned(),
                senses: vec![0],
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        let russian: Vec<_> = pairs::required_sources(Language::Ru).collect();
        for source in &russian {
            std::fs::write(import::index_path(&dir, *source), &index).unwrap();
        }
        let indexes = Indexes {
            presence: russian
                .iter()
                .fold(Indexes::MISSING.presence, |presence, source| {
                    presence.with(*source)
                }),
            ..Indexes::MISSING
        };

        let manifest = Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: sources::ROWS
                .iter()
                .filter(|row| addict_dictionary::release::MANIFEST_3_PUBLISHED.contains(&row.index))
                .map(|row| Published::of(row.index, row.index.as_bytes()))
                .collect(),
        };
        assert_eq!(manifest.indexes.len(), 20);
        let stub = Stub::start(HashMap::from([(
            format!("/{}/{}", release::tag(), release::MANIFEST_3),
            Route::File(manifest.to_json().into_bytes()),
        )]))
        .unwrap();

        gtk::init().expect("a display");
        let (mut session, mut events, _work) =
            test_session("io.github.doqhu.Addict.LanguageChangeTest", dirs);
        session.release = stub.base_url();
        let controller = attach_controller(&mut session);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.languages_read
        });
        assert!(
            controller.dispatch_settings_read(addict_controller::SettingsRead::My(Language::Ru))
        );
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.my_language == Language::Ru
        });
        session.first_run = FirstRun::Done;
        assert!(session.indexes.update(0, indexes));
        let choose = |session: &mut super::Session, events: &mut _, language| {
            let before = session.view_state.settings.my_chosen;
            session.handle(Event::Setting(addict_controller::Event::MyLanguageChosen(
                language,
            )));
            pump_until(session, events, |session| {
                session.view_state.settings.my_chosen > before
            });
        };
        let lacks = |my: Language| -> Vec<String> {
            first_run::missing(&indexes, my)
                .into_iter()
                .map(str::to_owned)
                .collect()
        };
        let sorted = |mut names: Vec<String>| -> Vec<String> {
            names.sort();
            names
        };
        let shown = |session: &Session| -> Vec<String> {
            let FirstRun::Shown {
                offer,
                recovery: true,
                ..
            } = &session.first_run
            else {
                panic!("the Settings offer");
            };
            offer.listed()
        };

        // The same language again, and Russian's set present: nothing.
        choose(&mut session, &mut events, Language::Ru);
        assert!(matches!(session.first_run, FirstRun::Done));
        // On a fresh install the first-run offer owns the decision while it
        // is due (a `--background` start, the window not yet shown).
        assert!(session.indexes.update(0, Indexes::MISSING));
        session.first_run = FirstRun::Pending(manifest.clone());
        choose(&mut session, &mut events, Language::Es);
        assert!(matches!(session.first_run, FirstRun::Pending(_)));
        assert!(session.indexes.update(0, indexes));
        session.first_run = FirstRun::Done;
        choose(&mut session, &mut events, Language::Ru);
        assert!(matches!(session.first_run, FirstRun::Done));
        assert!(stub.seen().is_empty(), "no request: {:?}", stub.seen());

        // Download dictionaries with Russian's set readable: only other
        // languages' indexes are missing, so no dialog opens.
        session.handle(Event::SettingsDownload);
        pump_until(&mut session, &mut events, |session| {
            !matches!(session.first_run, FirstRun::Preparing { .. })
        });
        assert!(matches!(session.first_run, FirstRun::Done));
        assert_eq!(
            session.settings.download_text(),
            first_run::nothing_text(Language::Ru)
        );

        // Russian → English: exactly the four pairs English lacks, and the
        // Russian article.
        assert_eq!(
            lacks(Language::En),
            [
                "kaikki-ru-Russkiy",
                "wikdict-en-es",
                "wikdict-en-ca",
                "wikdict-es-en",
                "wikdict-ca-en"
            ]
        );
        choose(&mut session, &mut events, Language::En);
        assert!(matches!(session.first_run, FirstRun::Preparing { .. }));
        // A change while preparing keeps the preparation, which lists for
        // the language in use when it ends.
        let run = session.download_run;
        choose(&mut session, &mut events, Language::Es);
        choose(&mut session, &mut events, Language::En);
        assert!(matches!(session.first_run, FirstRun::Preparing { .. }));
        assert_eq!(session.download_run, run);
        pump_until(&mut session, &mut events, |session| {
            matches!(session.first_run, FirstRun::Shown { .. })
        });
        assert_eq!(sorted(shown(&session)), sorted(lacks(Language::En)));
        // The waiting offer follows a change to Catalan, and closes on
        // Russian, whose set is whole.
        // Closed (it hides), it is shown again by a change.
        let FirstRun::Shown { offer, .. } = &session.first_run else {
            unreachable!();
        };
        let window = offer.window().clone();
        window.close();
        assert!(!window.is_visible());
        choose(&mut session, &mut events, Language::Ca);
        assert_eq!(sorted(shown(&session)), sorted(lacks(Language::Ca)));
        assert!(window.is_visible(), "the waiting offer is shown again");
        choose(&mut session, &mut events, Language::Ru);
        assert!(matches!(session.first_run, FirstRun::Done));
        assert!(!window.is_visible(), "the empty offer closes");

        // English again, accepted: the queue is what English lacks, in
        // manifest order.
        choose(&mut session, &mut events, Language::En);
        pump_until(&mut session, &mut events, |session| {
            matches!(session.first_run, FirstRun::Shown { .. })
        });
        let run = session.download_run;
        session.handle(Event::FirstRunDownload(run));
        let FirstRun::Installing {
            queue, recovery, ..
        } = &session.first_run
        else {
            panic!("installing");
        };
        assert!(*recovery);
        let queued: Vec<String> = queue
            .rows()
            .iter()
            .map(|(published, _)| published.name.clone())
            .collect();
        assert_eq!(queued, lacks(Language::En));
        // A change while installing leaves the install as it is.
        choose(&mut session, &mut events, Language::Ca);
        assert!(matches!(session.first_run, FirstRun::Installing { .. }));
        session.handle(Event::FirstRunCancel(run, None));
        // English's set present too, back to Russian: nothing is offered.
        let english = pairs::required_sources(Language::En)
            .fold(indexes.presence, |presence, source| presence.with(source));
        assert!(session.indexes.update(
            0,
            Indexes {
                presence: english,
                ..indexes
            }
        ));
        let run = session.download_run;
        choose(&mut session, &mut events, Language::Ru);
        assert!(matches!(session.first_run, FirstRun::Ended { .. }));
        assert_eq!(session.download_run, run);

        // A first-run offer waiting under `--background` for Russian, with
        // one index of English's set on disk: English ends it (entry 4),
        // and the Settings offer takes over for the rest.
        assert!(
            session.indexes.update(
                0,
                Indexes {
                    presence: Indexes::MISSING
                        .presence
                        .with(addict_dictionary::import::Source::WikdictEnEs),
                    ..Indexes::MISSING
                }
            )
        );
        session.first_run = FirstRun::Pending(manifest.clone());
        choose(&mut session, &mut events, Language::En);
        // The pending first-run offer ends and the Settings offer takes over;
        // the controller's wait may already have carried preparation through.
        assert!(!matches!(session.first_run, FirstRun::Pending(_)));
        for source in &russian {
            assert!(
                import::index_path(&dir, *source).exists(),
                "nothing removed: {source:?}"
            );
        }
        session.settings.window().destroy();
        session.window.destroy();
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn settings_download_uses_the_shared_dialog_and_ignores_old_runs() {
        use super::*;
        use addict_dictionary::release::Published;
        gtk::init().expect("a display");
        let (mut session, mut events, work) =
            test_session("io.github.doqhu.Addict.RecoveryTest", Dirs::default());

        fn button(widget: &gtk::Widget, text: &str) -> Option<gtk::Button> {
            if let Some(button) = widget.downcast_ref::<gtk::Button>()
                && button.label().as_deref() == Some(text)
            {
                return Some(button.clone());
            }
            let mut child = widget.first_child();
            while let Some(current) = child {
                if let Some(found) = button(&current, text) {
                    return Some(found);
                }
                child = current.next_sibling();
            }
            None
        }
        fn label(widget: &gtk::Widget, text: &str) -> bool {
            if widget
                .downcast_ref::<gtk::Label>()
                .is_some_and(|label| label.label() == text)
            {
                return true;
            }
            let mut child = widget.first_child();
            while let Some(current) = child {
                if label(&current, text) {
                    return true;
                }
                child = current.next_sibling();
            }
            false
        }
        let pending = || work.run(MANIFEST_LIMIT, std::future::pending::<()>(), |_| {});
        session.download_run = 1;
        session.first_run = FirstRun::Preparing { _job: pending() };
        let action = button(
            session.settings.window().upcast_ref(),
            "Download dictionaries",
        )
        .unwrap();
        action.emit_clicked();
        assert!(
            matches!(events.try_recv().unwrap(), Event::SettingsDownload),
            "actual Settings callback"
        );
        session.handle(Event::SettingsDownload);
        assert_eq!(session.download_run, 1, "repeat preserves preparation");
        assert!(matches!(session.first_run, FirstRun::Preparing { .. }));
        let manifest = Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: vec![Published::of("oewn", b"asset")],
        };
        session.handle(Event::RecoveryPrepared(
            0,
            Outcome::Finished(Ok(manifest.clone())),
        ));
        assert!(
            matches!(session.first_run, FirstRun::Preparing { .. }),
            "stale preparation ignored"
        );
        session.handle(Event::RecoveryPrepared(
            1,
            Outcome::Finished(Ok(manifest.clone())),
        ));
        let FirstRun::Shown {
            offer, recovery, ..
        } = &session.first_run
        else {
            panic!("shared recovery offer");
        };
        assert!(*recovery);
        assert_eq!(
            offer.window().transient_for().as_ref(),
            Some(session.settings.window())
        );
        assert!(label(
            offer.window().upcast_ref(),
            first_run::RECOVERY_HEADING
        ));
        assert!(button(offer.window().upcast_ref(), first_run::NOT_NOW).is_none());
        button(offer.window().upcast_ref(), first_run::CLOSE)
            .unwrap()
            .emit_clicked();
        assert!(matches!(
            events.try_recv().unwrap(),
            Event::FirstRunNotNow(1)
        ));
        session.handle(Event::FirstRunNotNow(1));
        assert!(matches!(session.first_run, FirstRun::Done));
        assert!(
            session.first_run_choice.is_none(),
            "recovery Close keeps the first-run choice"
        );
        session.download_run = 2;
        session.show_download_offer(manifest, true);
        let FirstRun::Shown {
            offer,
            manifest,
            recovery,
        } = std::mem::replace(&mut session.first_run, FirstRun::Done)
        else {
            unreachable!()
        };
        let mut queue = first_run::Queue::new(manifest.indexes);
        queue.next();
        offer.installing();
        session.first_run = FirstRun::Installing {
            offer,
            queue,
            dir: std::env::temp_dir(),
            running: Some(("oewn".to_owned(), pending())),
            recovery,
        };
        session.handle(Event::FirstRunProgress(1, "oewn".to_owned(), 100));
        session.handle(Event::FirstRunInstalled(
            1,
            "oewn".to_owned(),
            Outcome::Finished(Ok(std::path::PathBuf::new())),
        ));
        session.handle(Event::SettingsDownload);
        session.handle(Event::FirstRunDownload(2));
        let FirstRun::Installing {
            queue,
            running,
            offer,
            ..
        } = &session.first_run
        else {
            panic!("repeat must preserve installation");
        };
        assert!(running.is_some());
        assert!(
            offer.window().is_visible(),
            "active progress stays reachable"
        );
        assert_eq!(
            queue.state("oewn"),
            Some(&first_run::RowState::Running { received: 0 })
        );
        session.handle(Event::FirstRunProgress(2, "oewn".to_owned(), 3));
        let FirstRun::Installing { queue, .. } = &session.first_run else {
            unreachable!()
        };
        assert_eq!(
            queue.state("oewn"),
            Some(&first_run::RowState::Running { received: 3 })
        );
        session.first_run = FirstRun::Done;
        session.settings.window().destroy();
        session.window.destroy();
    }

    /// A spy that records the reports the shell sends to the controller. It
    /// does not copy the controller's transitions: those are tested with a
    /// real controller and a real window.
    #[derive(Default)]
    struct ObservedWindow {
        reports: Vec<ShortcutState>,
        presentations: usize,
        settings: usize,
        clipboard: Vec<(Option<String>, u64, Option<u64>)>,
        captures: Vec<Option<String>>,
    }

    impl ObservedWindow {
        fn last(&self) -> Option<&ShortcutState> {
            self.reports.last()
        }
    }

    impl WindowActions for ObservedWindow {
        fn shortcut(&mut self, state: ShortcutState) {
            self.reports.push(state);
        }
        fn present_main(&mut self) {
            self.presentations += 1;
        }
        fn present_settings(&mut self) {
            self.settings += 1;
        }
        fn clipboard(&mut self, token: Option<&str>, timestamp: u64, receipt: Option<u64>) {
            self.clipboard
                .push((token.map(str::to_owned), timestamp, receipt));
        }
        fn capture(&mut self, token: Option<&str>) {
            self.captures.push(token.map(str::to_owned));
        }
    }

    /// The shell's shortcut events, mapped by [`super::shortcut_state`] and
    /// sent to a real controller, move Settings as 0.5.0 did. No display.
    #[test]
    fn a_rebind_shows_the_old_keys_and_holds_the_button_until_it_ends() {
        use std::{sync::mpsc, time::Duration};

        use addict_controller::{Bindings as Keys, SettingsView, StartFacts};
        use addict_deepl::key::KeyError;
        use addict_platform::shortcut::{Bindings, ShortcutEvent};

        use crate::view::Binding;

        let (views, rendered) = mpsc::channel::<SettingsView>();
        let controller = addict_controller::start(
            Dirs::default(),
            super::controller_platform(),
            StartFacts {
                key: Some(Err(KeyError::Missing)),
                recording: false,
            },
            move |state: addict_controller::ViewState, _effects| {
                let _ = views.send(state.settings);
            },
        )
        .unwrap();
        let dispatcher = controller.dispatcher();
        // Sends `event` as the shell does and returns the settled snapshot.
        let send = |event: ShortcutEvent, done: &dyn Fn(&SettingsView) -> bool| {
            let state = super::shortcut_state(event).expect("a shortcut state");
            assert!(dispatcher.dispatch(addict_controller::Event::ShortcutChanged(state)));
            loop {
                let view = rendered
                    .recv_timeout(Duration::from_secs(5))
                    .expect("a snapshot");
                if done(&view) {
                    return view;
                }
            }
        };
        let keys = |bindings: &Bindings| Keys {
            clipboard: bindings.clipboard.clone(),
            screenshot: bindings.screenshot.clone(),
        };
        let old = Bindings {
            clipboard: Some("F8".to_owned()),
            screenshot: None,
        };
        let new = Bindings {
            clipboard: Some("F7".to_owned()),
            screenshot: Some("F6".to_owned()),
        };

        let window = send(
            ShortcutEvent::Rebinding {
                bindings: old.clone(),
            },
            &|view| view.rebinding,
        );
        assert!(window.rebinding);
        assert_eq!(window.bindings, keys(&old));
        assert!(matches!(window.binding, Binding::Bound));
        assert_eq!(window.status_line, crate::settings::REBINDING);

        // Settings updates first; the button stays held until the end.
        let window = send(
            ShortcutEvent::Bound {
                bindings: new.clone(),
            },
            &|view| view.bindings.clipboard.as_deref() == Some("F7"),
        );
        assert_eq!(window.bindings, keys(&new));
        assert!(window.rebinding);
        let window = send(ShortcutEvent::RebindEnded { message: None }, &|view| {
            !view.rebinding
        });
        assert!(!window.rebinding);
        assert_eq!(window.status_line, crate::settings::rebind_text(None));

        // Each way a rebind ends without new keys says so.
        send(
            ShortcutEvent::Rebinding {
                bindings: old.clone(),
            },
            &|view| view.rebinding,
        );
        let window = send(
            ShortcutEvent::RebindEnded {
                message: Some("The keys are unchanged.".to_owned()),
            },
            &|view| !view.rebinding,
        );
        assert!(!window.rebinding);
        assert_eq!(window.status_line, "The keys are unchanged.");

        // A second failure ends it too, and leaves Retry.
        send(ShortcutEvent::Rebinding { bindings: new }, &|view| {
            view.rebinding
        });
        let window = send(
            ShortcutEvent::Failed {
                message: "broke".to_owned(),
            },
            &|view| matches!(view.binding, Binding::NotBound { .. }),
        );
        assert!(!window.rebinding);
        assert!(window.status_line.is_empty());
        assert!(matches!(window.binding, Binding::NotBound { .. }));
    }

    #[test]
    fn a_failed_capture_focuses_capture_or_retry_when_offered() {
        use crate::view::{Binding, FailedEndFocus, failed_end_focus};
        assert_eq!(
            failed_end_focus(&Binding::Attempting),
            FailedEndFocus::Capture
        );
        assert_eq!(failed_end_focus(&Binding::Bound), FailedEndFocus::Capture);
        assert_eq!(
            failed_end_focus(&Binding::NotBound {
                detail: "portal unavailable".to_owned(),
            }),
            FailedEndFocus::Retry
        );
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_failed_capture_focuses_retry_in_the_window_through_the_controller() {
        use super::*;
        gtk::init().expect("a display");
        let (mut session, mut events, work) =
            test_session("io.github.doqhu.Addict.FailedFocusTest", Dirs::default());
        drop(work);
        let _controller = attach_controller(&mut session);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.languages_read
        });
        // A portal that could not bind leaves Retry offered; that is where a
        // capture without text sends the focus.
        session.handle(Event::Setting(addict_controller::Event::ShortcutChanged(
            ShortcutState::Failed("portal unavailable".to_owned()),
        )));
        pump_until(&mut session, &mut events, |session| {
            matches!(
                session.view_state.settings.binding,
                Binding::NotBound { .. }
            )
        });
        assert_eq!(
            failed_end_focus(&session.view_state.settings.binding),
            FailedEndFocus::Retry
        );
        // Once it is bound again, the capture button takes the focus.
        session.handle(Event::Setting(addict_controller::Event::ShortcutChanged(
            ShortcutState::Bound(Default::default()),
        )));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.binding == Binding::Bound
        });
        assert_eq!(
            failed_end_focus(&session.view_state.settings.binding),
            FailedEndFocus::Capture
        );
        session.settings.window().destroy();
        session.window.destroy();
    }

    #[test]
    fn failed_initialization_stays_silent_until_a_normal_launch_and_retains_the_error() {
        use addict_platform::shortcut::ShortcutEvent;
        let mut window = ObservedWindow::default();
        dispatch_window(&mut window, WindowEvent::Launched(LaunchMode::Background));
        dispatch_window(
            &mut window,
            WindowEvent::Shortcut(
                ShortcutEvent::Failed {
                    message: "portal unavailable".to_owned(),
                },
                None,
            ),
        );
        assert!(
            matches!(window.last(), Some(ShortcutState::Failed(detail)) if detail == "portal unavailable")
        );
        assert_eq!(window.presentations, 0);
        assert_eq!(window.settings, 0);
        // A later login invocation must not accidentally reveal the retained failure either.
        dispatch_window(&mut window, WindowEvent::Launched(LaunchMode::Background));
        assert_eq!(window.presentations, 0);
        dispatch_window(&mut window, WindowEvent::Launched(LaunchMode::Normal));
        assert_eq!(window.presentations, 1);
        assert!(matches!(window.last(), Some(ShortcutState::Failed(_))));
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_rebind_holds_configure_and_the_binding_row_in_the_window() {
        use super::*;
        use addict_platform::shortcut::{Bindings, ShortcutEvent};

        /// The keycaps the Settings window shows on its two shortcut rows.
        fn keycaps(root: &gtk::Widget, into: &mut Vec<String>) {
            if let Some(label) = root.downcast_ref::<gtk::Label>()
                && label.has_css_class("keycap")
            {
                into.push(label.label().into());
            }
            let mut child = root.first_child();
            while let Some(widget) = child {
                keycaps(&widget, into);
                child = widget.next_sibling();
            }
        }

        let old = Bindings {
            clipboard: Some("F8".to_owned()),
            screenshot: None,
        };
        let new = Bindings {
            clipboard: Some("F7".to_owned()),
            screenshot: Some("F6".to_owned()),
        };
        gtk::init().expect("a display");
        let (mut session, mut events, work) =
            test_session("io.github.doqhu.Addict.RebindTest", Dirs::default());
        drop(work);
        let _controller = attach_controller(&mut session);
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.languages_read
        });
        let shortcut = |event| Event::Window(WindowEvent::Shortcut(event, None));
        let bindings = |session: &super::Session| session.view_state.settings.bindings.clone();
        let shown = |session: &super::Session| {
            let mut caps = Vec::new();
            keycaps(session.settings.window().upcast_ref(), &mut caps);
            caps
        };

        session.handle(shortcut(ShortcutEvent::Rebinding {
            bindings: old.clone(),
        }));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.rebinding
                && bindings(session).clipboard.as_deref() == Some("F8")
        });
        assert!(shown(&session).iter().any(|cap| cap == "F8"));
        assert_eq!(session.view_state.settings.status_line, settings::REBINDING);
        assert!(matches!(
            session.view_state.settings.binding,
            Binding::Bound
        ));
        assert!(
            !session.settings.change_is_sensitive(),
            "held while rebinding"
        );
        assert!(
            !session.view.binding_row().0,
            "a bound shortcut shows no row"
        );

        // Settings updates first; the button stays held until the end.
        session.handle(shortcut(ShortcutEvent::Bound {
            bindings: new.clone(),
        }));
        pump_until(&mut session, &mut events, |session| {
            bindings(session).clipboard.as_deref() == Some("F7")
                && session.view_state.settings.rebinding
        });
        assert!(shown(&session).iter().any(|cap| cap == "F7"));
        assert_eq!(session.view_state.settings.status_line, settings::REBINDING);
        assert!(!session.settings.change_is_sensitive(), "still held");

        session.handle(shortcut(ShortcutEvent::RebindEnded { message: None }));
        pump_until(&mut session, &mut events, |session| {
            !session.view_state.settings.rebinding
        });
        assert_eq!(
            session.view_state.settings.status_line,
            settings::rebind_text(None)
        );
        assert!(
            session.settings.change_is_sensitive(),
            "released at the end"
        );

        // Each way a rebind ends without new keys says so, and a failed bind
        // leaves Retry offered.
        session.handle(shortcut(ShortcutEvent::Rebinding { bindings: old }));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.rebinding
        });
        session.handle(shortcut(ShortcutEvent::RebindEnded {
            message: Some("The keys are unchanged.".to_owned()),
        }));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.status_line == "The keys are unchanged."
        });
        session.handle(shortcut(ShortcutEvent::Rebinding { bindings: new }));
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.rebinding
        });
        session.handle(shortcut(ShortcutEvent::Failed {
            message: "broke".to_owned(),
        }));
        pump_until(&mut session, &mut events, |session| {
            matches!(
                session.view_state.settings.binding,
                Binding::NotBound { .. }
            )
        });
        assert_eq!(session.view_state.settings.status_line, "");
        assert!(!session.view_state.settings.rebinding);
        // The main window shows the failure, with Retry offered.
        let (visible, _, retry) = session.view.binding_row();
        assert!(visible && retry, "the binding row offers Retry");
        assert!(
            !session.settings.change_is_sensitive(),
            "no session to configure"
        );
        session.settings.window().destroy();
        session.window.destroy();
    }

    #[test]
    fn main_window_settings_action_opens_settings_after_portal_failure() {
        use addict_platform::shortcut::ShortcutEvent;
        let actions = gio::SimpleActionGroup::new();
        let (sender, mut events) = futures_channel::mpsc::unbounded();
        connect_settings_action(&actions, &sender);
        let mut window = ObservedWindow::default();
        dispatch_window(&mut window, WindowEvent::Launched(LaunchMode::Normal));
        dispatch_window(
            &mut window,
            WindowEvent::Shortcut(
                ShortcutEvent::Failed {
                    message: "portal unavailable".to_owned(),
                },
                None,
            ),
        );
        actions.activate_action("settings", None);
        let Event::Window(event) = events.try_recv().expect("Settings action event") else {
            panic!("wrong Settings event")
        };
        dispatch_window(&mut window, event);
        assert_eq!(window.presentations, 1);
        assert_eq!(window.settings, 1);
    }

    #[test]
    fn the_main_menu_sends_the_events_of_the_tray() {
        use addict_platform::tray::TrayEvent;
        let actions = gio::SimpleActionGroup::new();
        let (sender, mut events) = futures_channel::mpsc::unbounded();
        connect_menu_actions(&actions, &sender);
        for (name, expected) in [("about", TrayEvent::About), ("exit", TrayEvent::Exit)] {
            actions.activate_action(name, None);
            let Event::Tray(event) = events.try_recv().expect("a menu event") else {
                panic!("wrong event for {name}")
            };
            assert_eq!(event, expected);
        }
    }

    #[test]
    fn shortcut_events_dispatch_clipboard_and_capture_independently_with_their_tokens() {
        use addict_platform::shortcut::{Action, ShortcutEvent};
        let mut window = ObservedWindow::default();
        for (action, token, time) in [
            (Action::Screenshot, "screen-token", 10),
            (Action::Clipboard, "clip-token", 20),
        ] {
            dispatch_window(
                &mut window,
                WindowEvent::Shortcut(
                    ShortcutEvent::Activated {
                        action,
                        activation_token: Some(token.to_owned()),
                        timestamp_ms: time,
                    },
                    Some(100),
                ),
            );
        }
        assert_eq!(window.captures, vec![Some("screen-token".to_owned())]);
        assert_eq!(
            window.clipboard,
            vec![(Some("clip-token".to_owned()), 20, Some(100))]
        );
    }

    #[test]
    fn launch_modes_are_per_invocation_and_direct_activation_is_visible() {
        use std::{cell::RefCell, rc::Rc};
        let launches = Rc::new(RefCell::new(Vec::new()));
        // GIO exercises the real option parser and command-line signal without GTK/display.
        for (args, expected) in [
            (vec!["addict", "--background"], LaunchMode::Background),
            (vec!["addict"], LaunchMode::Normal),
        ] {
            let application = gio::Application::new(
                None,
                gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::NON_UNIQUE,
            );
            let seen = launches.clone();
            connect_launches(&application, move |mode| seen.borrow_mut().push(mode));
            assert_eq!(
                application.run_with_args(&args),
                gtk::glib::ExitCode::SUCCESS
            );
            assert_eq!(launches.borrow().last(), Some(&expected));
            application.emit_by_name::<()>("activate", &[]);
            assert_eq!(launches.borrow().last(), Some(&LaunchMode::Normal));
        }
        assert_eq!(
            *launches.borrow(),
            vec![
                LaunchMode::Background,
                LaunchMode::Normal,
                LaunchMode::Normal,
                LaunchMode::Normal
            ]
        );
    }

    #[test]
    fn forwarded_launches_retain_each_invocations_mode() {
        // Its children would be counted by the capture's test of them.
        let _children = crate::capture::tests::children();
        use std::{
            cell::RefCell,
            process::Command,
            rc::Rc,
            time::{Duration, Instant},
        };
        const TEST: &str = "resident::tests::forwarded_launches_retain_each_invocations_mode";
        const ROLE: &str = "ADDICT_TEST_LAUNCH_ROLE";
        if let Ok(role) = std::env::var(ROLE) {
            let primary = role.starts_with("primary-");
            let background = role.ends_with("background");
            let application = gio::Application::new(
                Some("io.github.doqhu.AddictLaunchTest"),
                gio::ApplicationFlags::HANDLES_COMMAND_LINE,
            );
            let hold = Rc::new(RefCell::new(None));
            application
                .connect_startup(move |application| *hold.borrow_mut() = Some(application.hold()));
            let seen = Rc::new(RefCell::new(Vec::new()));
            let reported = seen.clone();
            let remotes = Rc::new(RefCell::new(None));
            let results = remotes.clone();
            let resident = application.clone();
            connect_launches(&application, move |mode| {
                reported.borrow_mut().push(mode);
                if primary && reported.borrow().len() == 1 {
                    *results.borrow_mut() = Some(std::thread::spawn(|| {
                        // Every remote uses the same public mode arguments, over our private bus.
                        for role in ["normal", "background", "normal"] {
                            let output = Command::new(std::env::current_exe().unwrap())
                                .args(["--exact", TEST, "--nocapture"])
                                .env(ROLE, role)
                                .output()
                                .expect("forwarded child");
                            assert!(output.status.success(), "{output:?}");
                        }
                    }));
                }
                if primary && reported.borrow().len() == 4 {
                    resident.quit();
                }
            });
            let quit = application.clone();
            let deadline =
                gtk::glib::timeout_add_local_once(Duration::from_secs(10), move || quit.quit());
            let args = if background {
                vec!["addict", "--background"]
            } else {
                vec!["addict"]
            };
            assert_eq!(
                application.run_with_args(&args),
                gtk::glib::ExitCode::SUCCESS
            );
            deadline.remove();
            if primary {
                remotes
                    .borrow_mut()
                    .take()
                    .expect("remote launcher")
                    .join()
                    .unwrap();
                assert_eq!(
                    *seen.borrow(),
                    vec![
                        LaunchMode::from_background(background),
                        LaunchMode::Normal,
                        LaunchMode::Background,
                        LaunchMode::Normal
                    ]
                );
            } else {
                assert!(
                    seen.borrow().is_empty(),
                    "a forwarded launch must reach the primary"
                );
            }
            return;
        }
        for role in ["primary-normal", "primary-background"] {
            let mut child = Command::new("dbus-run-session")
                .args([
                    "--",
                    std::env::current_exe().unwrap().to_str().unwrap(),
                    "--exact",
                    TEST,
                    "--nocapture",
                ])
                .env(ROLE, role)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("private session bus");
            let started = Instant::now();
            while child.try_wait().unwrap().is_none() {
                if started.elapsed() > Duration::from_secs(15) {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("forwarded launch test timed out");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{role}: {output:?}");
        }
    }

    /// The switch's whole job against a mock Background portal, which alone
    /// touches the autostart file.
    mod startup_portal {
        use std::{
            collections::{HashMap, HashSet},
            path::{Path, PathBuf},
            process::{Command, Stdio},
            sync::{Arc, Mutex},
            time::{Duration, Instant},
        };

        use addict_core::APP_ID;
        use addict_core::failure::login_item::StartupFailure;
        use addict_platform::desktop;
        use zbus::{
            Connection,
            message::Header,
            zvariant::{OwnedObjectPath, OwnedValue},
        };

        use super::super::request_startup_change;
        use addict_core::failure::login_item::StartupEnd;

        const TEST: &str =
            "resident::tests::startup_portal::the_switch_changes_startup_only_through_the_portal";
        const CHILD: &str = "ADDICT_TEST_RESIDENT_STARTUP_CHILD";
        const DESKTOP: &str = "/org/freedesktop/portal/desktop";
        const WRITTEN: &str = "[Desktop Entry]\nX-XDP-Autostart=io.github.doqhu.Addict\n";
        type Options = HashMap<String, OwnedValue>;

        /// What the mock saw and how it answers.
        struct Mock {
            entry: PathBuf,
            state_file: PathBuf,
            /// Code 0 doing what was asked, or else code 1.
            obeys: Mutex<bool>,
            registered: Mutex<HashSet<String>>,
            /// For each request: the state asked for and whether the state
            /// file existed when it came.
            asked: Mutex<Vec<(bool, bool)>>,
        }

        struct Registry(Arc<Mock>);

        #[zbus::interface(name = "org.freedesktop.host.portal.Registry")]
        impl Registry {
            fn register(
                &self,
                _app_id: String,
                _options: Options,
                #[zbus(header)] header: Header<'_>,
            ) {
                let sender = header.sender().unwrap().to_string();
                self.0.registered.lock().unwrap().insert(sender);
            }
        }

        struct Background(Arc<Mock>);

        #[zbus::interface(name = "org.freedesktop.portal.Background")]
        impl Background {
            #[zbus(property, name = "version")]
            fn version(&self) -> u32 {
                2
            }

            async fn request_background(
                &self,
                _parent: String,
                options: Options,
                #[zbus(header)] header: Header<'_>,
                #[zbus(connection)] connection: &Connection,
            ) -> zbus::fdo::Result<OwnedObjectPath> {
                let sender = header.sender().unwrap().to_string();
                if !self.0.registered.lock().unwrap().contains(&sender) {
                    return Err(zbus::fdo::Error::AccessDenied("not registered".to_owned()));
                }
                let autostart = bool::try_from(&options["autostart"]).unwrap();
                self.0
                    .asked
                    .lock()
                    .unwrap()
                    .push((autostart, self.0.state_file.exists()));
                let obeys = *self.0.obeys.lock().unwrap();
                if obeys && autostart {
                    std::fs::create_dir_all(self.0.entry.parent().unwrap()).unwrap();
                    std::fs::write(&self.0.entry, WRITTEN).unwrap();
                } else if obeys {
                    std::fs::remove_file(&self.0.entry).unwrap();
                }
                let token = <&str>::try_from(&options["handle_token"]).unwrap();
                let sender = sender.trim_start_matches(':').replace('.', "_");
                let request: OwnedObjectPath = format!("{DESKTOP}/request/{sender}/{token}")
                    .try_into()
                    .unwrap();
                let (code, results) = if obeys {
                    let results = HashMap::from([
                        ("background".to_owned(), OwnedValue::from(true)),
                        ("autostart".to_owned(), OwnedValue::from(autostart)),
                    ]);
                    (0u32, results)
                } else {
                    (1u32, Options::new())
                };
                connection
                    .emit_signal(
                        None::<&str>,
                        &request,
                        "org.freedesktop.portal.Request",
                        "Response",
                        &(code, results),
                    )
                    .await?;
                Ok(request)
            }
        }

        fn start_mock(mock: Arc<Mock>) {
            let (ready, started) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async move {
                    let _connection = zbus::connection::Builder::session()
                        .unwrap()
                        .name("org.freedesktop.portal.Desktop")
                        .unwrap()
                        .serve_at(DESKTOP, Registry(mock.clone()))
                        .unwrap()
                        .serve_at(DESKTOP, Background(mock))
                        .unwrap()
                        .build()
                        .await
                        .unwrap();
                    ready.send(()).unwrap();
                    std::future::pending::<()>().await;
                });
            });
            started.recv().expect("the mock portal started");
        }

        fn change(enable: bool, state_file: PathBuf) -> StartupEnd {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(request_startup_change(enable, Some(state_file)))
        }

        /// Runs this test again on a private bus with no service directory, so
        /// that nothing can activate the real portal, and with a private `HOME`
        /// and `XDG_CONFIG_HOME` and `XDG_DATA_HOME`, given at spawn.
        fn run_isolated(root: &Path) {
            let config = root.join("bus.conf");
            std::fs::write(
                &config,
                "<busconfig><type>session</type>\
                 <listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth>\
                 <policy context=\"default\"><allow send_destination=\"*\"/>\
                 <allow receive_sender=\"*\"/><allow own=\"*\"/></policy></busconfig>\n",
            )
            .unwrap();
            let mut child = Command::new("dbus-run-session")
                .arg("--config-file")
                .arg(&config)
                .arg("--")
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
                .env(CHILD, "1")
                .env("HOME", root)
                .env("XDG_CONFIG_HOME", root.join("config"))
                .env("XDG_DATA_HOME", root.join("data"))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn the test binary");
            let deadline = Instant::now() + Duration::from_secs(30);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("the child did not exit in time");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let output = child.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success() && stdout.contains("1 passed"),
                "stdout: {stdout}\nstderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        #[test]
        fn the_switch_changes_startup_only_through_the_portal() {
            // Its children would be counted by the capture's test of them.
            let _children = crate::capture::tests::children();
            if std::env::var_os(CHILD).is_none() {
                let root = std::env::temp_dir()
                    .join(format!("addict-resident-startup-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&root);
                std::fs::create_dir(&root).unwrap();
                run_isolated(&root);
                std::fs::remove_dir_all(&root).unwrap();
                return;
            }
            let home = PathBuf::from(std::env::var_os("HOME").unwrap());
            assert!(home.starts_with(std::env::temp_dir()), "not isolated");
            let config = home.join("config");
            let entry = desktop::entry_path(&config.join("autostart"), APP_ID);
            let state_file = config.join("addict/login-startup");
            let mock = Arc::new(Mock {
                entry: entry.clone(),
                state_file: state_file.clone(),
                obeys: Mutex::new(true),
                registered: Mutex::default(),
                asked: Mutex::default(),
            });
            start_mock(mock.clone());
            let asked = || mock.asked.lock().unwrap().clone();
            let change = |enable| change(enable, state_file.clone());

            // No launcher: the portal would not register a host app.
            let StartupEnd::NotAsked(reason) = change(true) else {
                panic!("asked without a launcher");
            };
            assert!(reason.contains("run `addict install` first"), "{reason}");
            assert!(asked().is_empty());
            assert!(!state_file.exists() && !entry.exists());

            desktop::install(
                &home.join("data/applications"),
                APP_ID,
                Path::new("/usr/bin/addict"),
            )
            .unwrap();

            // A refusal: nothing saved, nothing written.
            *mock.obeys.lock().unwrap() = false;
            assert_eq!(change(true), StartupEnd::Failed(StartupFailure::Refused));
            assert_eq!(asked(), [(true, false)]);
            assert!(!state_file.exists() && !entry.exists());

            // Confirmed: saved only after the answer, which found no state.
            *mock.obeys.lock().unwrap() = true;
            assert_eq!(change(true), StartupEnd::Done(true));
            assert_eq!(asked()[1], (true, false));
            assert_eq!(std::fs::read_to_string(&state_file).unwrap(), "enabled\n");
            assert_eq!(std::fs::read_to_string(&entry).unwrap(), WRITTEN);

            assert_eq!(change(false), StartupEnd::Done(false));
            assert_eq!(asked()[2], (false, true));
            assert_eq!(std::fs::read_to_string(&state_file).unwrap(), "disabled\n");
            assert!(!entry.exists());

            // A foreign entry never reaches the portal, either way.
            let foreign = "[Desktop Entry]\nName=Someone else\n";
            std::fs::write(&entry, foreign).unwrap();
            for enable in [true, false] {
                let StartupEnd::NotAsked(reason) = change(enable) else {
                    panic!("asked with a foreign entry");
                };
                assert!(reason.contains(&entry.display().to_string()), "{reason}");
            }
            assert_eq!(asked().len(), 3);
            assert_eq!(std::fs::read_to_string(&entry).unwrap(), foreign);
            assert_eq!(std::fs::read_to_string(&state_file).unwrap(), "disabled\n");
        }
    }

    /// A key the shell committed stays active even when the store's answer
    /// is a timeout: 0.5.0 published the active key before reporting the end.
    #[test]
    fn a_committed_key_save_stays_active_when_the_runner_reports_a_timeout() {
        use std::{future, sync::mpsc, time::Duration};

        use addict_controller::StartFacts;
        use addict_deepl::key::{self, KeyError, Secret};
        use addict_engine::view::KeyState;
        use addict_engine::work::{Outcome, Work};

        let temp = KeyTestDir::new("addict-key-timeout");
        let path = temp.0.join("deepl.key");
        // A real controller, rendering into a channel: no display is needed.
        let (views, rendered) = mpsc::channel();
        let controller = addict_controller::start(
            Dirs::default(),
            super::controller_platform(),
            StartFacts {
                key: Some(Err(KeyError::Missing)),
                recording: false,
            },
            move |state: addict_controller::ViewState, _effects| {
                let _ = views.send(state.settings.key);
            },
        )
        .unwrap();
        let key = Secret::new("synthetic-timed-out-deepl").unwrap();
        let save = super::persist_key(path.clone(), key.clone(), controller.dispatcher());
        assert_eq!(key::read(&path), Err(KeyError::Missing));

        let work = Work::start().unwrap();
        let (send, receive) = mpsc::channel();
        let _job = work.run(
            Duration::from_secs(1),
            async move {
                save.await?;
                // Hold completion until the runner has to report a timeout.
                future::pending().await
            },
            move |outcome: Outcome<Result<(), key::WriteError>>| send.send(outcome).unwrap(),
        );
        let outcome = receive.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(outcome, Outcome::TimedOut);
        assert_eq!(key::read(&path), Ok(key.clone()));
        // The written key went active although its job timed out.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match rendered.recv_timeout(left) {
                Ok(KeyState::Free | KeyState::Pro) => break,
                Ok(_) => {}
                Err(error) => panic!("the committed key never went active: {error}"),
            }
        }
    }

    /// Saving a key in Settings writes the file and makes the key active for
    /// the next request, with the stub server checking the authorization sent
    /// on every request, including a superseded save.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn settings_saves_update_the_active_deepl_key() {
        use super::*;
        use addict_deepl::{
            DeepL,
            key::{self, KeySource, Secret},
            stub::{Reply, Stub},
        };
        use std::fs;

        const INITIAL: &str = "synthetic-startup-deepl:fx";
        const SAVED: &str = "synthetic-settings-deepl";
        const SUPERSEDED: &str = "synthetic-superseded-deepl:fx";
        const FREE: &str = "A DeepL key of the Free plan is set.";
        const PRO: &str = "A DeepL key of the Pro plan is set.";

        gtk::init().expect("a display");
        let temp = KeyTestDir::new("addict-key-session");
        let path = temp.0.join("config/deepl.key");
        let initial = key::write(&path, Secret::new(INITIAL).unwrap()).expect("startup key");
        let (mut session, mut events, _work) = test_session(
            "io.github.doqhu.Addict.KeyTest",
            Dirs {
                config: Some(temp.0.join("config")),
                cache: Some(temp.0.join("cache")),
                ..Dirs::default()
            },
        );
        let stub = Stub::start(Reply::translation("translated")).expect("stub");
        let source = KeySource::pending();
        let deepl = DeepL::with_base_url(source.clone(), stub.base_url());
        let controller = attach_translator_with_source(&mut session, deepl, source.clone());
        pump_until(&mut session, &mut events, |session| {
            session.view_state.settings.languages_read
        });
        let request = |text: &str| addict_core::translation::Request {
            text: text.to_owned(),
            context: None,
            source: Language::En,
            target: Language::Ru,
            english: addict_core::translation::English::Us,
        };
        controller.cache().put(
            &addict_engine::cache::key_of(&request("cached")),
            "from cache",
        );
        let assert_authorization = |index: usize, expected: &str| {
            let seen = stub.seen();
            assert_eq!(seen.len(), index + 1);
            assert!(
                seen[index].header("authorization")
                    == Some(format!("DeepL-Auth-Key {expected}").as_str()),
                "unexpected authorization"
            );
        };
        let submit = |session: &mut super::Session, events: &mut _, text: &str| {
            session.view.set_field_text(text);
            session.sentence_changed(text.to_owned(), super::FieldEdit::Typed);
            pump_until(session, events, |session| {
                session.view_state.sentence == text
            });
            session.submitted();
        };
        let translate =
            |session: &mut super::Session, events: &mut _, text: &str, expected: &str| {
                let count = stub.seen().len();
                submit(session, events, text);
                pump_until(session, events, |session| {
                    stub.seen().len() > count
                        && session
                            .view_state
                            .online
                            .iter()
                            .any(|row| row.line == "translated")
                });
                assert_authorization(count, expected);
            };

        session.show_settings();
        let window = session.settings.window().clone();
        let root = window.upcast_ref::<gtk::Widget>();
        let entry = find::<gtk::PasswordEntry>(root, &|_| true).unwrap();
        let save = find::<gtk::Button>(root, &|button| {
            button.label().as_deref() == Some("Save key")
        })
        .unwrap();
        let state = find::<gtk::Label>(root, &|label| label.has_css_class("key-state")).unwrap();
        let error_shown = || {
            find::<gtk::Label>(root, &|label| {
                label.is_visible() && label.label().starts_with("The key was not saved:")
            })
            .is_some()
        };

        assert_eq!(state.label(), "Checking…");
        assert!(!entry.is_sensitive());
        // While the startup key is still loading, a cached answer is shown
        // and an uncached request waits: nothing goes to DeepL yet.
        submit(&mut session, &mut events, "cached");
        pump_until(&mut session, &mut events, |session| {
            session
                .view_state
                .online
                .iter()
                .any(|row| row.line == "from cache")
        });
        submit(&mut session, &mut events, "uncached");
        pump_until(&mut session, &mut events, |session| {
            session.view_state.sentence == "uncached"
                && session
                    .view_state
                    .online
                    .iter()
                    .any(|row| row.state == crate::view::OnlineState::Pending)
        });
        assert!(stub.seen().is_empty());

        // The shell's real key load publishes the startup key, and the
        // waiting request goes out with it.
        super::start_key_load(session.key_file.clone(), &session.sender);
        pump_until(&mut session, &mut events, |session| {
            source.snapshot() == Some(Ok(initial.clone()))
                && session.view_state.settings.key == crate::view::KeyState::Free
        });
        assert_eq!(state.label(), FREE);
        assert!(entry.is_sensitive());
        pump_until(&mut session, &mut events, |session| {
            session
                .view_state
                .online
                .iter()
                .any(|row| row.line == "translated")
        });
        assert_authorization(0, INITIAL);

        fs::remove_file(&path).expect("external removal");
        translate(&mut session, &mut events, "removed", INITIAL);
        key::write(&path, Secret::new("external-file-edit").unwrap()).expect("external edit");
        translate(&mut session, &mut events, "replaced", INITIAL);
        assert_eq!(source.snapshot(), Some(Ok(initial.clone())));

        let press = |session: &mut super::Session, events: &mut _, input: &str| {
            let before = session.view_state.settings.key_entry_clears;
            entry.set_text(input);
            assert!(save.is_sensitive());
            save.emit_clicked();
            pump_until(session, events, |session| {
                session.view_state.settings.key_entry_clears > before
                    || session
                        .view_state
                        .settings
                        .key_line
                        .starts_with("The key was not saved:")
            });
            session.view_state.settings.key_line.clone()
        };

        assert_eq!(state.label(), FREE);
        let line = press(&mut session, &mut events, &format!("  {SAVED}  "));
        assert!(line.is_empty(), "{line}");
        pump_until(&mut session, &mut events, |_| {
            source.snapshot() == Some(Ok(Secret::new(SAVED).unwrap()))
        });
        translate(&mut session, &mut events, "saved", SAVED);
        pump_until(&mut session, &mut events, |session| {
            session.settings.window().is_visible() && state.label() == PRO
        });
        assert!(entry.text().is_empty());
        assert!(!error_shown());

        let line = press(&mut session, &mut events, SUPERSEDED);
        assert!(line.is_empty(), "{line}");
        pump_until(&mut session, &mut events, |_| {
            source.snapshot() == Some(Ok(Secret::new(SUPERSEDED).unwrap()))
        });
        let line = press(&mut session, &mut events, "invalid input");
        assert!(line.starts_with("The key was not saved:"), "{line}");
        // A refused key stays in the field, and the active one is the
        // superseding save's.
        assert_eq!(entry.text(), "invalid input");
        assert!(error_shown());
        assert_eq!(state.label(), FREE);
        assert_eq!(
            source.snapshot(),
            Some(Ok(Secret::new(SUPERSEDED).unwrap()))
        );
        let persisted = fs::read_to_string(&path).expect("persisted file");
        assert_eq!(
            persisted,
            format!("{SUPERSEDED}\n"),
            "unexpected persisted key"
        );
        translate(&mut session, &mut events, "superseded", SUPERSEDED);

        let backup = path.with_extension("old");
        fs::rename(&path, &backup).expect("save old file");
        fs::create_dir(&path).expect("block atomic replacement");
        let line = press(&mut session, &mut events, "synthetic-unwritten-deepl");
        assert!(line.starts_with("The key was not saved:"), "{line}");
        assert!(error_shown());
        assert_eq!(state.label(), FREE);
        assert!(path.is_dir());
        assert_eq!(fs::read_to_string(backup).unwrap(), persisted);
        translate(&mut session, &mut events, "unwritten", SUPERSEDED);
        window.destroy();
        session.window.destroy();
    }

    /// Without a controller there is no runner to read or change the login
    /// item: Settings says so and leaves the switch alone.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn settings_without_a_controller_say_the_runner_is_missing() {
        use super::*;

        gtk::init().expect("a display");
        let (mut session, mut events, work) =
            test_session("io.github.doqhu.Addict.NoRunnerTest", Dirs::default());
        drop(work);
        session.show_settings();
        pump_until(&mut session, &mut events, |session| {
            session.settings.window().is_visible()
        });
        let root = session.settings.window().upcast_ref::<gtk::Widget>();
        let hint = find::<gtk::Label>(root, &|label| {
            label.has_css_class("hint")
                && label
                    .label()
                    .contains(addict_engine::view::STARTUP_NO_RUNNER)
        })
        .expect("the runner's message");
        assert!(hint.is_visible());
        // The Start at login switch itself, not the first switch of the window.
        assert!(!session.settings.startup_is_sensitive());
        session.settings.window().destroy();
        session.window.destroy();
    }
}
