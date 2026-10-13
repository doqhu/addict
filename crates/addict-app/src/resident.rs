//! The resident application: one hidden window, the attempt to bind the
//! global shortcut and the loop that shows the window for each event.
//!
//! This module owns the GTK application, its uniqueness, the window with its
//! close and Esc handling, and the order of what happens on each event of the
//! shortcut worker, of Retry, of a repeated launch, of the field and of a
//! lookup, which runs on the work runner and comes back as an event. The
//! dictionary indexes are opened and read there too: where they stand is
//! asked once at the start and comes with the outcome of every lookup, and
//! the window says it whenever one is missing or unreadable. It keeps
//! the shown result with the sense chosen in it, and a click on a sense only
//! changes that choice. A click on a word of a sentence also asks online for
//! the word in the sentence and, once for the text, for the sentence itself;
//! Enter in the field, or a paste, asks for a lone word. Each request is a
//! job of its own on the runner, in one of two slots, the word's and the
//! sentence's, and comes back as an event with the number of its text: the
//! article never waits for it, a repeated click leaves it alone, another
//! word gives up only the word's, and a new text gives up both. What is
//! asked for and what is kept is [`online`](crate::online)'s. What the
//! window says is [`view`](crate::view)'s; its widgets and the paste into
//! its field are [`widgets`](crate::widgets)'s; what an edit or a click does
//! to the manual input is [`manual`](crate::manual)'s; what is recorded with
//! `--evidence` is [`recording`](crate::recording)'s.

use std::{
    cell::{Cell, RefCell},
    process::ExitCode,
    rc::{Rc, Weak},
    sync::Arc,
    time::Duration,
};

use addict_core::{APP_ID, input::window_state, translation::Translator};
use addict_deepl::DeepL;
use addict_platform::{
    shortcut::{self, ShortcutEvent, ShortcutHandle},
    work::{Job, Outcome, Work},
};
use futures_channel::mpsc::UnboundedSender;
use futures_util::StreamExt;
use gtk::{gdk, gio, glib, prelude::*};

use crate::{
    dictionaries::{Articles, Dictionaries, Indexes, Looked},
    evidence,
    manual::{self, Edit, Manual, Translated},
    online::{Kept, Subject},
    recording::Recording,
    view::{Binding, Lookup, Shown},
    widgets::View,
};

/// Preferred trigger in the XDG shortcuts syntax; the user may change it.
const TRIGGER: &str = "LOGO+q";

/// How long one attempt to bind the shortcut may take.
const BIND_LIMIT: Duration = Duration::from_secs(60);

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
    /// The lookup started as number `run` ended.
    Lookup { run: u64, outcome: Outcome<Looked> },
    /// The online request about `subject`, sent for the text with the number
    /// `text`, ended.
    Translated {
        text: u64,
        subject: Subject,
        number: u64,
        outcome: Outcome<Translated>,
    },
    /// The check at the start says where the dictionary indexes stand.
    Indexes(Indexes),
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

/// The height the window starts with, in pixels: an article of a few
/// senses with one of them chosen fits without scrolling its list.
const WINDOW_HEIGHT: i32 = 480;

/// The hidden window around the `view`: closing it hides it, and so does Esc.
fn build_window(application: &gtk::Application, view: &View) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Addict")
        .default_width(480)
        .default_height(WINDOW_HEIGHT)
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
    /// The online request of a word, in its sentence or alone, whose outcome
    /// is awaited; dropping it cancels the request.
    translating_word: Option<Job>,
    /// The same for the request of the sentence.
    translating_sentence: Option<Job>,
    /// The online texts of the text the field holds.
    online: Kept,
    /// The check of the indexes at the start; dropping it cancels the check.
    check: Option<Job>,
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
            Event::Launched => self.launched(),
            Event::Retry => self.retry(),
            Event::Sentence(text) => self.sentence_changed(text),
            Event::Word(index) => self.word_chosen(index),
            Event::Sense(index) => self.sense_chosen(index),
            Event::Submitted => self.submitted(),
            Event::Lookup { run, outcome } => self.looked_up(run, outcome),
            Event::Translated {
                text,
                subject,
                number,
                outcome,
            } => self.translated(text, subject, number, outcome),
            Event::Indexes(indexes) => {
                self.check = None;
                self.view.render_indexes(indexes);
            }
        }
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
    fn sentence_changed(&mut self, text: String) {
        let edit = self.manual.edit(text);
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
            Edit::Lone(run) => self.start_lookup(run),
        }
    }

    /// A word was clicked: it becomes the input, with its sentence when it
    /// has one, and is looked up. In a sentence, what is not yet known
    /// online about the word and about the sentence is asked for.
    fn word_chosen(&mut self, index: usize) {
        // A click that came before the field's newer text was handled here
        // names a word of a sentence that is no longer shown.
        if self.view.field_text() != self.manual.sentence() {
            return;
        }
        let Some(run) = self.manual.choose(index) else {
            return;
        };
        self.start_lookup(run);
        // After the lookup: the runner takes its jobs in this order, so the
        // article is read before the key file and the network are touched.
        let click = self.online.click(index);
        if click.gave_up {
            self.translating_word = None;
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
        if self.view.field_text() != self.manual.sentence() {
            return;
        }
        if self.online.submit() {
            self.start_translation(Subject::Lone);
            self.render_online();
        }
    }

    /// Shows the input that was just chosen and starts its lookup, which has
    /// the number `run`. The online texts are not touched.
    fn start_lookup(&mut self, run: u64) {
        // The lookup of another word, if it runs, is replaced.
        self.pending = None;
        self.render_manual();
        let Some(word) = self.manual.input().map(|input| input.word().to_owned()) else {
            self.show_result(None);
            return;
        };
        let sender = self.sender.clone();
        let started = manual::look_up(
            self.work.upgrade().as_deref(),
            self.articles.clone(),
            word,
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
        let request = self
            .manual
            .input()
            .and_then(|input| manual::request(input, subject));
        let sender = self.sender.clone();
        let started = manual::translate(
            self.work.upgrade().as_deref(),
            self.translator.clone(),
            request,
            move |outcome| {
                // On the worker: the UI thread wakes for the event.
                let _ = sender.unbounded_send(Event::Translated {
                    text,
                    subject,
                    number,
                    outcome,
                });
            },
        );
        match started {
            // Replaces, and so cancels, the request the slot held.
            Ok(job) => *self.slot(subject) = Some(job),
            Err(online) => {
                self.online.answer(text, subject, number, online);
            }
        }
    }

    /// The slot of the job that asks about `subject`. No wildcard arm: a new
    /// subject does not compile until it is given its slot.
    fn slot(&mut self, subject: Subject) -> &mut Option<Job> {
        match subject {
            Subject::Word(_) | Subject::Lone => &mut self.translating_word,
            Subject::Sentence => &mut self.translating_sentence,
        }
    }

    /// An online request ended. Its outcome is kept and shown only when it
    /// belongs to the text the field holds and is still awaited; a cancelled
    /// one was given up and shows nothing.
    fn translated(
        &mut self,
        text: u64,
        subject: Subject,
        number: u64,
        outcome: Outcome<Translated>,
    ) {
        let Some(online) = manual::translation_result(outcome) else {
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
        self.shown.show(lookup);
        self.render_shown();
    }

    /// Shows the result and the sense chosen in it as they are now.
    fn render_shown(&self) {
        self.view
            .render_result(self.shown.lookup(), self.shown.sense());
    }

    /// A sense was clicked: it is the chosen one when the shown result is an
    /// article that has it. Nothing is looked up or read. A click on the
    /// chosen sense renders too: GTK keeps a clicked link in the visited
    /// colour until the markup is set again.
    fn sense_chosen(&mut self, index: usize) {
        if self.shown.choose(index) {
            self.render_shown();
        }
    }

    /// Shows the state, the sentence and the chosen word as they are now.
    fn render_manual(&self) {
        self.view.render(self.manual.state());
        self.view.render_sentence(
            self.manual.sentence(),
            self.manual.words(),
            self.manual.chosen(),
        );
    }

    fn bound(&mut self, trigger: &str) {
        eprintln!("addict: shortcut bound to {trigger}");
        self.binding = Binding::Bound;
        self.view.render_binding(&self.binding);
    }

    /// The shortcut was pressed: the window is shown and takes the focus.
    fn activated(
        &self,
        activation_token: Option<&str>,
        timestamp_ms: u64,
        receipt_ns: Option<u64>,
    ) {
        // Before `present()`: the first frame has the labels.
        self.view.render(self.manual.state());
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

    /// The attempt ended without a shortcut: the window shows itself to say so.
    fn failed(&mut self, message: String) {
        eprintln!("addict: shortcut unavailable: {message}");
        self.binding = Binding::NotBound { detail: message };
        self.view.render_binding(&self.binding);
        // No press stands behind this, so there is no token.
        if let Some(recording) = &self.recording {
            recording.shown_without_press();
        }
        self.window.present();
    }

    fn launched(&self) {
        self.view.render(self.manual.state());
        self.focus();
        // No `set_startup_id`: GTK has already applied the
        // token of the launcher, when there was one.
        if let Some(recording) = &self.recording {
            recording.shown_without_press();
        }
        self.window.present();
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
        // `Failed` is the last act of the old worker, sent
        // after its session was closed: joining it does not
        // wait.
        current.borrow_mut().take();
        *current.borrow_mut() = Some(start_shortcut(&self.sender, self.timed));
    }
}

/// Sends what the user does in the `view` to `sender`: Retry, an edit of the
/// field, a submit of its text, a click on a word and a click on a sense.
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
    let sense = sender.clone();
    view.connect_sense_chosen(move |index| {
        let _ = sense.unbounded_send(Event::Sense(index));
    });
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

/// Builds the hidden window, starts the first attempt to bind the shortcut
/// and the event loop. Runs once, in the primary instance.
fn start(
    application: &gtk::Application,
    evidence: Option<Rc<RefCell<evidence::Writer>>>,
) -> Resident {
    // The worker threads push; the main context wakes only for an event.
    let (sender, mut receiver) = futures_channel::mpsc::unbounded();

    // The first attempt starts below, with the window still hidden.
    let binding = Binding::Attempting;
    let view = View::new(window_state(None, None), &binding);
    connect_view(&view, &sender);
    let window = build_window(application, &view);

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
    let checked = sender.clone();
    let check = manual::check_indexes(work.as_deref(), articles.clone(), move |indexes| {
        // On the worker: the UI thread wakes for the event.
        let _ = checked.unbounded_send(Event::Indexes(indexes));
    });
    let translator = DeepL::new();
    let key_file = translator.key_file().map(std::path::Path::to_owned);
    let mut session = Session {
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
        // Nothing is read or connected here: the key file and the network
        // are the runner's, at the first click on a word of a sentence or
        // the first submit of a lone word.
        translator: Arc::new(translator),
        key_file,
        translating_word: None,
        translating_sentence: None,
        online: Kept::default(),
        check,
    };
    glib::spawn_future_local(async move {
        while let Some(event) = receiver.next().await {
            session.handle(event);
        }
    });
    Resident {
        _hold: application.hold(),
        _shortcut: handle,
        events,
        _work: work,
        _articles: articles,
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
