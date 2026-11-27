//! Shared product state and its event loop: the field, the manual input,
//! offline lookups, online translations, the shown result with its chosen
//! sense or gloss, and history. One thread, ordered full snapshots, rising
//! revision; GTK-free and platform-free.
//!
//! A shell renders [`ViewState`], dispatches [`Event`]s and runs
//! [`Effect`]s.

mod capture;
pub mod deadlines;

pub use capture::ImageRef;
mod online;
mod selection;
mod settings;

pub use settings::{Bindings, SettingsRead, SettingsView, ShortcutState};

use std::{
    io,
    ops::Range,
    sync::{Arc, Weak, mpsc},
    thread::{self, JoinHandle},
};

use addict_core::{
    dirs::Dirs,
    failure::{Ended, capture::CaptureFailure, recognize::OcrFailure, selection::SelectionFailure},
    input::{Input, Source, WindowState},
    translation::{English, Source as Language, Translator},
};
use addict_deepl::{
    DeepL,
    key::{self, KeySource, Secret},
};
use addict_engine::{
    cache,
    capture::Timing,
    dictionaries::{Dictionaries, Indexes, Looked},
    evidence::{self, Answered},
    history::History,
    language,
    manual::{self, Edit, Manual},
    view::{Choice, Lookup, OnlineRow, Shown},
    work::{Job, Outcome, Work},
};

/// Platform facts supplied by the shell.
pub struct Platform {
    pub name: String,
    /// Help for locating the DeepL key when there is no configuration directory.
    pub key_location_hint: &'static str,
    /// Whether a Read press must read the selection before the window is
    /// shown. The Mac shell needs its panel key only after the read; Linux
    /// shows the window first (AD-22).
    pub read_before_show: bool,
}

/// The key may still be loading when the controller starts: `None` means the
/// shell has not read it yet. The controller creates the key source from it.
pub struct StartFacts {
    pub key: Option<Result<Secret, addict_deepl::key::KeyError>>,
    pub recording: bool,
}

/// Identity of a controller request, issued in increasing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

/// The text and result that an article action belongs to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArticleId {
    text: u64,
    generation: u64,
}

/// The text field: what it holds and how often the controller set it.
///
/// The shell keeps its own text while typing; it replaces its text only
/// when `revision` rises. The controller raises `revision` only when it
/// sets the text itself: OCR, Back, Forward, a synonym, or clearing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Field {
    pub text: String,
    pub revision: u64,
}

/// What a read of the primary selection and the clipboard returned: the raw
/// text of each, or `None` when there is none or it could not be read. The
/// controller applies the line rule and the length limit to both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionText {
    pub selection: Option<String>,
    pub clipboard: Option<String>,
}

/// Facts sent to the controller by the shell or made by the user in Settings.
#[derive(Debug)]
pub enum Event {
    /// The field holds `text`, which the shell put there itself. `replaced`
    /// is true when a new text was put in rather than typed: a paste, a
    /// read selection or clipboard, a capture. `source` names where a
    /// replaced text came from when it was read rather than pasted:
    /// [`Source::Ocr`] for a capture, [`Source::Clipboard`] for the
    /// selection or the clipboard; `None` otherwise.
    FieldEdited {
        text: String,
        replaced: bool,
        source: Option<Source>,
    },
    /// The user pressed Capture, or invoked the capture shortcut.
    CapturePressed { activation_token: Option<String> },
    /// The hide has settled and the shell is opening the area picker.
    PickerStarted { id: RequestId },
    /// The current picker returned its opaque image, or a failure.
    AreaCaptured {
        id: RequestId,
        result: Result<ImageRef, Ended<CaptureFailure>>,
        capture_ms: Option<u64>,
    },
    /// The recognizer finished and released its image and resources.
    TextRecognized {
        id: RequestId,
        result: Result<String, Ended<OcrFailure>>,
        timing: Timing,
    },
    /// Read was pressed: the field is cleared and the selection and the
    /// clipboard are read. `sequence` numbers the press for presentation.
    ReadPressed { sequence: u64 },
    /// The read of the selection and the clipboard with this id ended, or
    /// was cancelled: the raw texts, or why there is none.
    SelectionRead {
        id: RequestId,
        result: Result<SelectionText, Ended<SelectionFailure>>,
    },
    /// The field is cleared for a read; the controller sets it empty.
    FieldCleared,
    /// The text of the field was submitted, by Enter or by a paste.
    FieldSubmitted,
    /// The word with this index in the shown sentence was chosen.
    WordChosen { text: u64, index: usize },
    /// The sense with this index in the shown list was chosen.
    SenseChosen { article: ArticleId, index: usize },
    /// The English gloss with this index in the shown article was chosen.
    GlossChosen { article: ArticleId, index: usize },
    /// A synonym was chosen: its word goes into the field and is submitted.
    SynonymChosen { article: ArticleId, word: String },
    /// The English link of the chosen gloss was clicked, submitting its word
    /// with the effective reading language.
    LookUpEnglish {
        article: ArticleId,
        index: usize,
        word: String,
    },
    /// Back was pressed.
    Back,
    /// Forward was pressed.
    Forward,
    /// The saved reading language was chosen, in the window or by a gloss.
    ReadingChosen(Language),
    /// My language, chosen in Settings or in a first-run offer.
    MyLanguageChosen(Language),
    /// The reading language pressed in the switch changed (rendering only).
    SetPressed(Language),
    /// A source is being read now (capture, clipboard), or no longer.
    SetReadingSource(Option<Source>),
    /// The spelling of English was chosen in Settings.
    EnglishChosen(English),
    /// Translate eligible sentences read from the clipboard from now on.
    AtOnceChosen(bool),
    /// Settings was opened: read the login item and the cache again.
    SettingsOpened,
    /// Clear cache was pressed.
    ClearCachePressed,
    /// The user entered a key, or the shell refused it as invalid.
    SaveKey(Result<Secret, addict_deepl::key::WriteError>),
    /// The shell published the key now in use, after a load or a store.
    KeyChanged(Result<Secret, addict_deepl::key::KeyError>),
    /// The store of the key ended for this request.
    KeyStored {
        id: RequestId,
        result: Result<(), Ended<addict_core::failure::key_store::KeyStoreFailure>>,
    },
    /// The user moved the Start at login switch.
    LoginItemToggled(bool),
    /// The shell's answer about the login item. `runner_missing` says the
    /// shell had no runner to do the work with (AD-22).
    LoginItemSet {
        id: RequestId,
        result: Result<
            addict_core::failure::login_item::StartupEnd,
            Ended<addict_core::failure::login_item::StartupFailure>,
        >,
        runner_missing: bool,
    },
    /// The shell read the login item when Settings opened, or says it could
    /// not because its runner was gone.
    LoginItemRead {
        shown: Option<addict_core::failure::login_item::LoginItemShown>,
        runner_missing: bool,
    },
    /// Retry was pressed under a shortcut that is not bound.
    RetryPressed,
    /// Configure shortcuts was pressed in Settings.
    ConfigurePressed,
    /// The shell reported a change of the bindings or of the dialog.
    ShortcutChanged(ShortcutState),
    /// The shell's `IndexCache` stands at `generation`, knowing `indexes`.
    KnownIndexes {
        generation: u64,
        indexes: Option<Indexes>,
    },
    /// Redo the offline result of the shown word and ask its online texts
    /// again.
    AskAgain,
    /// The user explicitly requested the selected dictionary definition.
    DefinitionAction { article: ArticleId, choice: Choice },
}

/// Commands for the shell. Lookup and translation use the shared runner.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Hide the window and start the area picker once it has left the screen.
    CaptureArea {
        id: RequestId,
        read_sequence: u64,
        activation_token: Option<String>,
    },
    /// Recognize the image in the language selected when its picker started.
    Recognize {
        id: RequestId,
        image: ImageRef,
        language: Language,
        read_sequence: u64,
    },
    /// Stop an effect if possible, and send its normal result event.
    Cancel(RequestId),
    /// Read the primary selection and the clipboard; one
    /// [`Event::SelectionRead`] follows.
    ReadSelection { id: RequestId, read_sequence: u64 },
    /// Show the window, before the read or after it. Does nothing when the
    /// press has already shown it.
    ShowPanel { read_sequence: u64 },
    /// Restore the capture window, with retry focus after a failure.
    PresentCapture { read_sequence: u64, retry: bool },
    /// Record an OCR outcome after the effect has cleaned up.
    RecordCapture {
        read: evidence::Read,
        timing: Timing,
    },
    /// The shell's `IndexCache` should check where the indexes stand and
    /// report back with [`Event::KnownIndexes`]. `index_generation` is the
    /// generation at emission; a shell refresh newer than this already owns
    /// it.
    CheckIndexes { index_generation: u64 },
    /// A lookup started at `generation` read where the dictionary indexes
    /// stand; the shell updates its `IndexCache`.
    ObserveIndexes { generation: u64, indexes: Indexes },
    /// A lookup ended; the shell records its evidence.
    RecordLookup(Answered),
    /// An online attempt ended and the shell records its evidence.
    RecordOnline(evidence::Record),
    /// Store this key in the shell's key store; one
    /// [`Event::KeyChanged`] and one [`Event::KeyStored`] follow, in that
    /// order when the store commits.
    StoreKey { id: RequestId, key: Secret },
    /// Ask the shell to turn the login item on or off; one
    /// [`Event::LoginItemSet`] follows.
    SetLoginItem { id: RequestId, on: bool },
    /// Read the login item's live state; one [`Event::LoginItemRead`] follows.
    ReadLoginItem,
    /// Make the first attempt to bind the shortcuts again.
    BindShortcuts,
    /// Open the desktop's dialog for the shortcuts.
    ConfigureShortcuts,
}

/// The entire controller-owned view state at one revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewState {
    pub revision: u64,
    pub field: Field,
    /// The last observed Linux Read interaction.
    pub read_sequence: u64,
    /// A field revision set by asynchronous recognition.
    pub ocr_field_revision: Option<u64>,
    pub capture_sensitive: bool,
    pub source_failure: Option<String>,
    pub shown: Shown,
    /// Changes on an explicit sense or gloss click, including a repeat click.
    pub choice_revision: u64,
    /// Word and sentence rows for the current text and choice.
    pub online: Vec<OnlineRow<'static>>,
    pub window_state: WindowState,
    pub source: Source,
    pub sentence: String,
    pub words: Vec<Range<usize>>,
    pub chosen: Option<usize>,
    pub can_back: bool,
    pub can_forward: bool,
    /// The number of the text; changes with every new text, not a click.
    pub text_number: u64,
    /// The source being read now, if any.
    pub reading_source: Option<Source>,
    /// The chosen word, with its sentence when it has one.
    pub input: Option<Input>,
    /// The sentence of the chosen word: its number in the text and its byte
    /// range, `None` when the word has none.
    pub context: Option<(usize, Range<usize>)>,
    /// Whether the offline lookup of the chosen word runs.
    pub looking_up: bool,
    /// The language that gave the result of the last lookup, once it
    /// finished with one; `None` before and for one that did not finish.
    pub answered: Option<Language>,
    /// Settings and the OS state beside them, as last known by the controller.
    pub settings: SettingsView,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            revision: 0,
            field: Field::default(),
            read_sequence: 0,
            ocr_field_revision: None,
            capture_sensitive: true,
            source_failure: None,
            shown: Shown::default(),
            choice_revision: 0,
            online: Vec::new(),
            window_state: WindowState::Empty,
            source: Source::Manual,
            sentence: String::new(),
            words: Vec::new(),
            chosen: None,
            can_back: false,
            can_forward: false,
            text_number: 0,
            reading_source: None,
            input: None,
            context: None,
            looking_up: false,
            answered: None,
            settings: SettingsView {
                my_language: Language::Ru,
                reading: Language::En,
                english: English::Us,
                english_shown: false,
                at_once: addict_engine::at_once::DEFAULT,
                languages_read: false,
                my_chosen: 0,
                key: addict_engine::view::KeyState::Checking,
                key_line: String::new(),
                key_entry_clears: 0,
                cache: addict_engine::view::CacheState::Checking,
                cache_line: String::new(),
                login_on: false,
                login_confirmed: false,
                login_sensitive: false,
                login_line: String::new(),
                runner_missing: false,
                binding: addict_engine::view::Binding::Attempting,
                bindings: Bindings::default(),
                rebinding: false,
                status_line: String::new(),
            },
        }
    }
}

impl ViewState {
    pub fn article_id(&self) -> ArticleId {
        ArticleId {
            text: self.text_number,
            generation: self.shown.generation(),
        }
    }
}

/// Receives snapshots on the controller thread. Renderers enqueue their work
/// for delivery on the shell's UI thread, in order.
pub trait Renderer: Send + 'static {
    fn render(&self, state: ViewState, effects: Vec<Effect>);
}

impl<F: Fn(ViewState, Vec<Effect>) + Send + 'static> Renderer for F {
    fn render(&self, state: ViewState, effects: Vec<Effect>) {
        self(state, effects);
    }
}

/// Enqueue-only access for an event producer. It does not own the loop.
#[derive(Clone)]
pub struct Dispatcher(mpsc::Sender<Message>);

impl Dispatcher {
    /// Returns whether the event was accepted by the queue.
    pub fn dispatch(&self, event: Event) -> bool {
        self.0.send(Message::Event(event)).is_ok()
    }
}

/// The owner of the controller loop and its one work runner.
pub struct Controller {
    dispatch: Dispatcher,
    thread: Option<JoinHandle<()>>,
    work: Weak<Work>,
    dictionaries: Arc<Dictionaries>,
    cache: Arc<cache::Store>,
}

impl Controller {
    /// Returns whether the event was accepted by the queue.
    pub fn dispatch(&self, event: Event) -> bool {
        self.dispatch.dispatch(event)
    }

    pub fn dispatcher(&self) -> Dispatcher {
        self.dispatch.clone()
    }

    /// Injects the outcome of a start read of a settings file. Shells report
    /// these through their own files; a test can use this seam to deliver a
    /// read in a chosen order, while a real read is still running.
    #[doc(hidden)]
    pub fn dispatch_settings_read(&self, read: SettingsRead) -> bool {
        self.dispatch.0.send(Message::SettingsRead(read)).is_ok()
    }

    /// The one runner instance; the shell's own jobs run on it too.
    pub fn work(&self) -> Weak<Work> {
        self.work.clone()
    }

    /// The one set of dictionaries; the shell's index checks read it too.
    pub fn dictionaries(&self) -> Arc<Dictionaries> {
        self.dictionaries.clone()
    }

    /// The translation cache used by requests, exposed for Settings.
    pub fn cache(&self) -> Arc<cache::Store> {
        self.cache.clone()
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let _ = self.dispatch.0.send(Message::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Opens no dictionary files. The event loop and the Rust jobs each get
/// their own thread, and both end when the controller is dropped.
pub fn start(
    dirs: Dirs,
    platform: Platform,
    start_facts: StartFacts,
    renderer: impl Renderer,
) -> io::Result<Controller> {
    let source = match start_facts.key {
        Some(result) => KeySource::new(result),
        None => KeySource::pending(),
    };
    let translator = DeepL::new(source.clone());
    start_with_translator(
        dirs,
        platform,
        translator,
        start_facts.recording,
        source,
        renderer,
    )
}

/// Starts the same controller with an alternate translation provider and the
/// key source the shell loaded at start.
pub fn start_with_translator(
    dirs: Dirs,
    platform: Platform,
    translator: impl Translator + 'static,
    recording: bool,
    source: KeySource,
    renderer: impl Renderer,
) -> io::Result<Controller> {
    start_with_capture(
        dirs,
        platform,
        translator,
        recording,
        source,
        crate::settings::Limits::default(),
        capture::Capture::default(),
        selection::Selection::default(),
        None,
        renderer,
    )
}

#[allow(clippy::too_many_arguments)]
fn start_with_capture(
    dirs: Dirs,
    platform: Platform,
    translator: impl Translator + 'static,
    recording: bool,
    source: KeySource,
    settings_limits: crate::settings::Limits,
    capture: capture::Capture,
    selection: selection::Selection,
    read_gate: Option<Arc<settings::ReadGate>>,
    renderer: impl Renderer,
) -> io::Result<Controller> {
    let work = Arc::new(Work::start()?);
    let worker = Arc::downgrade(&work);
    let mut settings = settings::Settings::with_limits(&dirs, source, settings_limits);
    settings.set_read_gate(read_gate);
    let dictionaries = Arc::new(Dictionaries::new(dirs.data.clone()));
    let translator = cache::Cached::new(translator, dirs.cache);
    let cache = translator.store();
    let (send, receive) = mpsc::channel();
    let mut model = Model {
        online: online::Translations::new(
            Arc::new(translator),
            key::key_path(dirs.config.as_deref()),
            platform.key_location_hint,
            recording,
        ),
        work,
        cache: cache.clone(),
        dictionaries: dictionaries.clone(),
        send: send.clone(),
        request: 0,
        revision: 0,
        field: Field::default(),
        capture,
        selection,
        read_sequence: 0,
        read_before_show: platform.read_before_show,
        ocr_field_revision: None,
        recording,
        manual: Manual::default(),
        history: History::default(),
        shown: Shown::default(),
        choice_revision: 0,
        settings,
        known: None,
        index_generation: 0,
        lookup: None,
        answered: None,
    };
    model.start_settings_read();
    let thread = thread::Builder::new()
        .name("addict-controller".to_owned())
        .spawn(move || model.serve(receive, renderer))?;
    Ok(Controller {
        dispatch: Dispatcher(send),
        thread: Some(thread),
        work: worker,
        dictionaries,
        cache,
    })
}

enum Message {
    Event(Event),
    LookedUp(LookedUp),
    Translated(online::Translated),
    CaptureDeadline(RequestId),
    SelectionDeadline(RequestId),
    SettingsRead(settings::SettingsRead),
    CacheFootprint(Option<cache::Footprint>),
    CacheCleared(Option<String>),
    KeySaveDeadline(RequestId),
    LoginDeadline(RequestId),
    Stop,
}

/// The end of the lookup `id`, started as `Manual`'s run `run` while the
/// shell's indexes stood at `generation`, answered by `language`.
struct LookedUp {
    id: RequestId,
    run: u64,
    generation: u64,
    language: Language,
    outcome: Box<Outcome<Looked>>,
}

/// The offline lookup of the chosen word; dropping `_job` cancels it.
struct Active {
    id: RequestId,
    _job: Job,
}

struct Model {
    online: online::Translations,
    work: Arc<Work>,
    /// The cache whose footprint and rows Settings reads and clears.
    cache: Arc<cache::Store>,
    dictionaries: Arc<Dictionaries>,
    send: mpsc::Sender<Message>,
    request: u64,
    revision: u64,
    field: Field,
    capture: capture::Capture,
    selection: selection::Selection,
    read_sequence: u64,
    read_before_show: bool,
    ocr_field_revision: Option<u64>,
    recording: bool,
    manual: Manual,
    /// The texts the field held, for Back and Forward; kept for the life of
    /// the process, hidden window included.
    history: History,
    shown: Shown,
    choice_revision: u64,
    settings: settings::Settings,
    /// What the shell's `IndexCache` knows of the indexes, and its
    /// generation.
    known: Option<Indexes>,
    index_generation: u64,
    /// The lookup whose outcome is awaited.
    lookup: Option<Active>,
    /// The language that gave the result of the last started lookup, once
    /// it finished.
    answered: Option<Language>,
}

impl Model {
    fn serve(mut self, receive: mpsc::Receiver<Message>, renderer: impl Renderer) {
        self.render(&renderer, Vec::new());
        while let Ok(message) = receive.recv() {
            let mut effects = Vec::new();
            let changed = match message {
                Message::Event(event) => self.apply(event, &mut effects),
                Message::LookedUp(looked) => self.looked_up(looked, &mut effects),
                Message::Translated(translated) => self.translated(translated, &mut effects),
                Message::CaptureDeadline(id) => self.capture_deadline(id, &mut effects),
                Message::SelectionDeadline(id) => self.selection_deadline(id, &mut effects),
                Message::SettingsRead(read) => self.settings_read(read, &mut effects),
                Message::CacheFootprint(found) => self.cache_read(found),
                Message::CacheCleared(reason) => self.cache_cleared(reason),
                Message::KeySaveDeadline(id) => self.key_save_deadline(id, &mut effects),
                Message::LoginDeadline(id) => self.login_deadline(id, &mut effects),
                Message::Stop => {
                    self.cancel_read(&mut effects);
                    self.cancel_capture(&mut effects);
                    if !effects.is_empty() {
                        self.render(&renderer, effects);
                    }
                    break;
                }
            };
            if changed || !effects.is_empty() {
                self.render(&renderer, effects);
            }
        }
    }

    fn render(&mut self, renderer: &impl Renderer, effects: Vec<Effect>) {
        self.revision += 1;
        renderer.render(self.view(), effects);
    }

    fn view(&self) -> ViewState {
        let manual = &self.manual;
        ViewState {
            revision: self.revision,
            field: self.field.clone(),
            read_sequence: self.read_sequence,
            ocr_field_revision: self.ocr_field_revision,
            capture_sensitive: !self.capture.picker_open(),
            source_failure: self
                .capture
                .source_failure
                .clone()
                .or_else(|| self.selection.source_failure.clone()),
            shown: self.shown.clone(),
            choice_revision: self.choice_revision,
            online: self.online.rows(manual.chosen(), self.settings.my_language),
            window_state: manual.state(),
            source: manual.source(),
            sentence: manual.sentence().to_owned(),
            words: manual.words().to_vec(),
            chosen: manual.chosen(),
            can_back: self.history.can_back(manual.sentence()),
            can_forward: self.history.can_forward(),
            text_number: manual.text_number(),
            reading_source: manual.reading(),
            input: manual.input().cloned(),
            context: manual.chosen().and_then(|index| manual.sentence_of(index)),
            looking_up: self.lookup.is_some(),
            answered: self.answered,
            settings: self.settings.view(),
        }
    }

    fn article_id(&self) -> ArticleId {
        ArticleId {
            text: self.manual.text_number(),
            generation: self.shown.generation(),
        }
    }

    fn next_request(&mut self) -> RequestId {
        self.request += 1;
        RequestId(self.request)
    }

    /// Applies `event`; says whether the view changed.
    fn apply(&mut self, event: Event, effects: &mut Vec<Effect>) -> bool {
        match event {
            Event::FieldEdited {
                text,
                replaced,
                source,
            } => self.field_edited(text, replaced, source, effects),
            Event::CapturePressed { activation_token } => {
                let cleared = self.clear_read_source(effects);
                self.capture_pressed(activation_token, effects) || cleared
            }
            Event::PickerStarted { id } => {
                self.picker_started(id);
                false
            }
            Event::AreaCaptured {
                id,
                result,
                capture_ms,
            } => self.area_captured(id, result, capture_ms, effects),
            Event::TextRecognized { id, result, timing } => {
                self.text_recognized(id, result, timing, effects)
            }
            Event::ReadPressed { sequence } => self.read_pressed(sequence, effects),
            Event::SelectionRead { id, result } => self.selection_read(id, result, effects),
            Event::FieldCleared => self.field_cleared(effects),
            Event::FieldSubmitted => self.submitted(effects),
            Event::WordChosen { text, index } => {
                text == self.manual.text_number() && self.word_chosen(index, effects)
            }
            Event::SenseChosen { article, index } => {
                article == self.article_id() && self.choose(index, false, effects)
            }
            Event::GlossChosen { article, index } => {
                article == self.article_id() && self.choose(index, true, effects)
            }
            Event::SynonymChosen { article, word } => {
                if article != self.article_id() {
                    return false;
                }
                let changed = self.clear_capture_source(effects);
                let changed = self.clear_read_source(effects) || changed;
                self.put(word, effects) || changed
            }
            Event::LookUpEnglish {
                article,
                index,
                word,
            } => {
                if article != self.article_id() || self.shown.choice() != Some(Choice::Gloss(index))
                {
                    return false;
                }
                // Apply the choice and put the replacement together, so
                // changing the language cannot send the previous input. The
                // choice is written by the controller, not by the shell.
                let reading = language::reading(Language::En, self.settings.my_language);
                self.settings.saved_reading = reading;
                self.settings.reading_chosen = true;
                self.update_reading();
                self.write_reading();
                self.clear_capture_source(effects);
                let changed = self.clear_read_source(effects);
                self.put(word, effects) || changed
            }
            Event::Back => {
                let changed = self.clear_capture_source(effects);
                let changed = self.clear_read_source(effects) || changed;
                self.back(effects) || changed
            }
            Event::Forward => {
                let changed = self.clear_capture_source(effects);
                let changed = self.clear_read_source(effects) || changed;
                self.forward(effects) || changed
            }
            Event::ReadingChosen(language) => self.choose_reading(language, effects),
            Event::MyLanguageChosen(language) => self.choose_my_language(language, effects),
            Event::SetPressed(language) => {
                self.shown.set_pressed(language);
                true
            }
            Event::SetReadingSource(source) => {
                if self.capture.recognizing() {
                    return false;
                }
                self.manual.set_reading(source);
                true
            }
            Event::EnglishChosen(english) => {
                self.shown.set_english(english);
                self.choose_english(english, effects)
            }
            Event::AtOnceChosen(at_once) => self.choose_at_once(at_once),
            Event::SettingsOpened => self.settings_opened(effects),
            Event::ClearCachePressed => self.clear_cache_pressed(),
            Event::SaveKey(key) => self.save_key(key, effects),
            Event::KeyChanged(result) => self.key_changed(result),
            Event::KeyStored { id, result } => self.key_stored(id, result),
            Event::LoginItemToggled(on) => self.login_toggled(on, effects),
            Event::LoginItemSet {
                id,
                result,
                runner_missing,
            } => self.login_set(id, result, runner_missing),
            Event::LoginItemRead {
                shown,
                runner_missing,
            } => self.login_read(shown, runner_missing),
            Event::RetryPressed => self.retry_pressed(effects),
            Event::ConfigurePressed => self.configure_pressed(effects),
            Event::ShortcutChanged(state) => self.shortcut_changed(state),
            Event::KnownIndexes {
                generation,
                indexes,
            } => {
                self.index_generation = generation;
                self.known = indexes;
                false
            }
            Event::AskAgain => self.ask_again(effects),
            Event::DefinitionAction { article, choice } => {
                article == self.article_id()
                    && self.shown.choice() == Some(choice)
                    && self.definition_action(effects)
            }
        }
    }

    /// The shell put `text` into the field: typed, pasted or read. What was
    /// chosen in the old text is gone, and what follows a new text follows.
    /// A replaced text is recorded; a typed one is not, but Back can go
    /// from it. The shell owns its text, so `field.revision` stays.
    fn field_edited(
        &mut self,
        text: String,
        replaced: bool,
        source: Option<Source>,
        effects: &mut Vec<Effect>,
    ) -> bool {
        self.clear_capture_source(effects);
        self.clear_read_source(effects);
        let edit = match source {
            Some(Source::Ocr) => self.manual.recognized(text),
            Some(Source::Clipboard) => self.manual.copied(&text),
            Some(Source::Manual) | None => self.manual.edit(text),
        };
        self.field.text = self.manual.sentence().to_owned();
        // `Cleared` is any new text with no word chosen; an empty one is
        // not recorded.
        if replaced && edit != Edit::Same {
            self.history.record(self.manual.sentence());
        }
        self.text_replaced(edit, effects);
        if replaced && source == Some(Source::Clipboard) && self.online.at_once {
            self.ask_whole(effects);
        }
        // An edit ends the reading of a source even when the text is the
        // same: the window state may have changed.
        true
    }

    /// The field is cleared for a read: the controller sets it empty.
    fn field_cleared(&mut self, effects: &mut Vec<Effect>) -> bool {
        self.cancel_read(effects);
        self.cancel_capture(effects);
        self.capture.source_failure = None;
        self.selection.source_failure = None;
        let edit = self.manual.edit(String::new());
        self.set_field(String::new());
        self.text_replaced(edit, effects);
        true
    }

    /// What follows a new text in the field: a lone word is looked up at
    /// once; a new text with no word chosen hides the result, and the
    /// lookup of the old one is given up.
    fn text_replaced(&mut self, edit: Edit, effects: &mut Vec<Effect>) {
        if edit != Edit::Same {
            self.reset_online();
        }
        match edit {
            Edit::Same => {}
            Edit::Cleared => {
                // Dropping the job cancels it; whatever it still reports
                // is not awaited.
                self.lookup = None;
                self.answered = None;
                self.shown.show(None);
            }
            Edit::Lone(run) => self.start_lookup(run, effects),
        }
    }

    /// The word with `index` is chosen, with its sentence, and looked up;
    /// its online texts are asked. An invalid index is ignored.
    fn word_chosen(&mut self, index: usize, effects: &mut Vec<Effect>) -> bool {
        let Some(run) = self.manual.choose(index) else {
            return false;
        };
        self.start_lookup(run, effects);
        self.ask_word(index, effects);
        true
    }

    /// A changed choice gives up the previous definition. A gloss asks
    /// immediately and retries after a refusal.
    fn choose(&mut self, index: usize, gloss: bool, effects: &mut Vec<Effect>) -> bool {
        let previous = self.shown.choice();
        let chosen = if gloss {
            self.shown.choose_gloss(index)
        } else {
            self.shown.choose(index)
        };
        if !chosen {
            return false;
        }
        if previous != self.shown.choice() {
            self.cancel_definition();
        }
        self.choice_revision += 1;
        if gloss {
            self.definition_action(effects);
        }
        true
    }

    /// Records the current text and asks its online texts.
    fn submitted(&mut self, effects: &mut Vec<Effect>) -> bool {
        self.history.record(self.manual.sentence());
        self.ask_lone(effects);
        true
    }

    /// Back: the text before in the history is put into the field as a
    /// paste puts it, so it is looked up and submitted again. A typed text
    /// that is not in the history is recorded first, so Forward comes back
    /// to it. At the start nothing happens.
    fn back(&mut self, effects: &mut Vec<Effect>) -> bool {
        let current = self.manual.sentence().to_owned();
        let Some(text) = self.history.back(&current).map(str::to_owned) else {
            return false;
        };
        self.put(text, effects)
    }

    /// Forward: the text after in the history is put into the field as Back
    /// puts one. At the end nothing happens.
    fn forward(&mut self, effects: &mut Vec<Effect>) -> bool {
        let Some(text) = self.history.forward().map(str::to_owned) else {
            return false;
        };
        self.put(text, effects)
    }

    /// Puts `text` into the field as a paste does: the controller sets it
    /// (`field.revision` rises), it is a new text even when it reads as the
    /// one held, and it is submitted. After Back or Forward the submit
    /// records the text at the position, which changes nothing.
    fn put(&mut self, text: String, effects: &mut Vec<Effect>) -> bool {
        // A paste replaces the text with none first, then with the new one.
        let cleared = self.manual.edit(String::new());
        self.text_replaced(cleared, effects);
        let edit = self.manual.edit(text.clone());
        self.set_field(text);
        self.text_replaced(edit, effects);
        self.submitted(effects)
    }

    /// The controller sets the field text itself: `field.revision` rises.
    fn set_field(&mut self, text: String) {
        self.field.text = text;
        self.field.revision += 1;
        self.ocr_field_revision = None;
    }

    /// Redoes the offline result of the shown word and its online texts.
    fn ask_again(&mut self, effects: &mut Vec<Effect>) -> bool {
        if let Some(run) = self.manual.again() {
            self.start_lookup(run, effects);
        }
        self.ask_online_again(effects);
        true
    }

    /// Shows the input that was just chosen and starts its lookup as run
    /// `run`, superseding any earlier one; for a reading language without
    /// an offline dictionary no lookup starts, and the line that says so is
    /// shown.
    fn start_lookup(&mut self, run: u64, effects: &mut Vec<Effect>) {
        // Dropping the old job cancels it. Any outcome already queued still
        // carries its old request id and run.
        self.lookup = None;
        self.answered = None;
        self.cancel_definition();
        let Some(word) = self.manual.input().map(|input| input.word().to_owned()) else {
            self.shown.show(None);
            return;
        };
        let id = self.next_request();
        let send = self.send.clone();
        let language = self.settings.reading();
        let generation = self.index_generation;
        let started = manual::offline(
            Some(&self.work),
            deadlines::LOOKUP_LIMIT,
            self.dictionaries.clone(),
            word,
            language,
            self.settings.my_language,
            self.known,
            move |language, outcome| {
                // On the worker: only the queue is touched.
                let _ = send.send(Message::LookedUp(LookedUp {
                    id,
                    run,
                    generation,
                    language,
                    outcome: Box::new(outcome),
                }));
            },
        );
        match started {
            Ok(job) => {
                self.lookup = Some(Active { id, _job: job });
                self.shown.show(Some(Lookup::Pending));
            }
            Err(lookup) => {
                let checks = manual::checks_again(&lookup, language);
                self.shown.show(Some(*lookup));
                if checks {
                    effects.push(Effect::CheckIndexes {
                        index_generation: generation,
                    });
                }
            }
        }
    }

    /// A lookup ended. Where it found the indexes is reported for every
    /// one that read them, and every one is recorded; only the last
    /// started one, whose word is still chosen, is shown, with the line
    /// that names the answering language when it is not the chosen one.
    fn looked_up(&mut self, looked: LookedUp, effects: &mut Vec<Effect>) -> bool {
        let LookedUp {
            id,
            run,
            generation,
            language,
            outcome,
        } = looked;
        if let Outcome::Finished(looked) = outcome.as_ref() {
            effects.push(Effect::ObserveIndexes {
                generation,
                indexes: looked.indexes,
            });
        }
        if let Some(answered) = evidence::answered(&outcome) {
            effects.push(Effect::RecordLookup(answered));
        }
        let current = self.lookup.as_ref().is_some_and(|active| active.id == id);
        if !current || !self.manual.awaits(run) {
            return false;
        }
        self.lookup = None;
        let Some(word) = self.manual.input().map(|input| input.word().to_owned()) else {
            return true;
        };
        let finished = matches!(outcome.as_ref(), Outcome::Finished(_));
        if let Some(lookup) = manual::result_in(*outcome, &word, language) {
            self.shown
                .show_elsewhere(Some(lookup), self.settings.reading(), language);
        }
        // A lookup that did not finish names no language: the chosen one.
        self.answered = finished.then_some(language);
        self.send_deferred(effects);
        true
    }
}

#[cfg(test)]
mod tests {
    mod online;
    mod selection;
    mod settings;

    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::Duration,
    };

    use addict_core::translation::{Refusal, Request, Source as Language, Translating};
    use addict_dictionary::{
        format::{Contents, Headword, SenseRecord, encode},
        import::{self, Source as IndexSource},
    };
    use addict_engine::view::{Online, OnlineKind, OnlineState};
    use futures_channel::oneshot;

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);
    const DEFINITION: &str = "the ground alongside a river";
    const RIVER: &str = "a large natural stream of water";
    /// What a check finds after the Spanish import.
    const SPANISH: Indexes = Indexes {
        spanish: addict_engine::dictionaries::State::Open,
        ..Indexes::OPEN
    };
    static TEST_DIR: AtomicU64 = AtomicU64::new(0);

    /// A data directory with an English index of `bank` and `river`, and on
    /// request a Spanish one of `banco` with its English gloss.
    struct TestData(PathBuf);

    impl TestData {
        fn with_bank() -> Self {
            let unique = TEST_DIR.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir()
                .join(format!("addict-controller-{}-{unique}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir(&dir).unwrap();
            let data = Self(dir);
            data.write(
                IndexSource::Oewn,
                Contents {
                    source: "oewn".to_owned(),
                    senses: vec![
                        SenseRecord {
                            definition: DEFINITION.to_owned(),
                            members: vec!["bank".to_owned(), "shore".to_owned()],
                            ..Default::default()
                        },
                        SenseRecord {
                            definition: RIVER.to_owned(),
                            members: vec!["river".to_owned()],
                            ..Default::default()
                        },
                        SenseRecord {
                            definition: "a financial institution".to_owned(),
                            members: vec!["bank".to_owned()],
                            ..Default::default()
                        },
                    ],
                    headwords: vec![
                        Headword {
                            word: "bank".to_owned(),
                            senses: vec![0, 2],
                            ..Default::default()
                        },
                        Headword {
                            word: "river".to_owned(),
                            senses: vec![1],
                            ..Default::default()
                        },
                        Headword {
                            word: "shore".to_owned(),
                            senses: vec![0],
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
            );
            data
        }

        fn with_banco() -> Self {
            let data = Self::with_bank();
            data.write(
                IndexSource::KaikkiEs,
                Contents {
                    source: "kaikki-es".to_owned(),
                    senses: vec![SenseRecord {
                        definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                        ..Default::default()
                    }],
                    headwords: vec![Headword {
                        word: "banco".to_owned(),
                        senses: vec![0],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            );
            data.write(
                IndexSource::WikdictEsRu,
                Contents {
                    source: "es-ru".to_owned(),
                    headwords: vec![Headword {
                        word: "banco".to_owned(),
                        translations: vec!["банк".to_owned()],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            );
            data.write(
                IndexSource::KaikkiEnSpanish,
                Contents {
                    source: "es-en".to_owned(),
                    senses: vec![SenseRecord {
                        definition: "bank".to_owned(),
                        ..Default::default()
                    }],
                    headwords: vec![Headword {
                        word: "banco".to_owned(),
                        senses: vec![0],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            );
            data
        }

        fn write(&self, source: IndexSource, contents: Contents) {
            fs::write(
                import::index_path(&self.0, source),
                encode(&contents).unwrap(),
            )
            .unwrap();
        }
    }

    impl Drop for TestData {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A scripted shell: events in, snapshots and effects out, in order.
    struct FakeShell {
        controller: Controller,
        renders: mpsc::Receiver<(ViewState, Vec<Effect>)>,
        revision: u64,
    }

    impl FakeShell {
        fn start(data: Option<PathBuf>) -> Self {
            Self::with_translator(data, Silent, false)
        }

        fn scripted(data: Option<PathBuf>, recording: bool) -> (Self, mpsc::Receiver<Asked>) {
            let (requests, asked) = mpsc::channel();
            (
                Self::with_translator(data, Scripted(requests), recording),
                asked,
            )
        }

        fn with_translator(
            data: Option<PathBuf>,
            translator: impl Translator + 'static,
            recording: bool,
        ) -> Self {
            Self::with_capture(data, translator, recording, capture::Capture::default())
        }

        fn with_capture(
            data: Option<PathBuf>,
            translator: impl Translator + 'static,
            recording: bool,
            capture: capture::Capture,
        ) -> Self {
            Self::with_parts(
                data,
                translator,
                recording,
                capture,
                crate::selection::Selection::default(),
                false,
            )
        }

        /// A shell on a platform that reads before it shows, or not, with
        /// `selection`'s deadline.
        fn with_selection(selection: crate::selection::Selection, read_before_show: bool) -> Self {
            Self::with_parts(
                None,
                Silent,
                false,
                capture::Capture::default(),
                selection,
                read_before_show,
            )
        }

        fn with_parts(
            data: Option<PathBuf>,
            translator: impl Translator + 'static,
            recording: bool,
            capture: capture::Capture,
            selection: crate::selection::Selection,
            read_before_show: bool,
        ) -> Self {
            Self::with_settings(
                Dirs {
                    data,
                    ..Dirs::default()
                },
                translator,
                recording,
                KeySource::pending(),
                crate::settings::Limits::default(),
                capture,
                selection,
                read_before_show,
                None,
            )
        }

        /// A scripted shell with its own directories, key and deadlines, for
        /// the settings tests.
        /// Builds a shell without waiting for the start reads. A test that
        /// uses a read gate calls this so it can act while files are pending.
        #[allow(clippy::too_many_arguments)]
        fn with_unread_settings(
            dirs: Dirs,
            translator: impl Translator + 'static,
            recording: bool,
            source: KeySource,
            limits: crate::settings::Limits,
            capture: capture::Capture,
            selection: crate::selection::Selection,
            read_before_show: bool,
            read_gate: Option<Arc<crate::settings::ReadGate>>,
        ) -> Self {
            let caller = thread::current().id();
            let (send, renders) = mpsc::channel();
            let controller = start_with_capture(
                dirs,
                Platform {
                    name: "test".to_owned(),
                    key_location_hint: "No key directory.",
                    read_before_show,
                },
                translator,
                recording,
                source,
                limits,
                capture,
                selection,
                read_gate,
                move |view, effects| {
                    assert_ne!(thread::current().id(), caller);
                    send.send((view, effects)).unwrap();
                },
            )
            .unwrap();
            Self {
                controller,
                renders,
                revision: 0,
            }
        }

        #[allow(clippy::too_many_arguments)]
        fn with_settings(
            dirs: Dirs,
            translator: impl Translator + 'static,
            recording: bool,
            source: KeySource,
            limits: crate::settings::Limits,
            capture: capture::Capture,
            selection: crate::selection::Selection,
            read_before_show: bool,
            read_gate: Option<Arc<crate::settings::ReadGate>>,
        ) -> Self {
            let mut shell = Self::with_unread_settings(
                dirs,
                translator,
                recording,
                source,
                limits,
                capture,
                selection,
                read_before_show,
                read_gate,
            );
            // The controller reads the four files at start. Let that
            // snapshot arrive before the script begins, so a test's first
            // `next` is the one caused by its own event.
            loop {
                let (view, effects) = shell.next();
                assert_eq!(view.field, Field::default());
                assert!(effects.is_empty());
                if view.settings.languages_read {
                    break;
                }
            }
            shell
        }

        /// The next render; its revision is newer than the last one.
        fn next(&mut self) -> (ViewState, Vec<Effect>) {
            self.next_within(WAIT)
        }

        fn next_within(&mut self, wait: Duration) -> (ViewState, Vec<Effect>) {
            let (view, effects) = self.renders.recv_timeout(wait).expect("controller render");
            assert!(view.revision > self.revision);
            self.revision = view.revision;
            (view, effects)
        }

        /// Renders until the lookup of the chosen word has ended.
        fn settled(&mut self) -> (ViewState, Vec<Effect>) {
            loop {
                let (view, effects) = self.next();
                if !view.looking_up {
                    return (view, effects);
                }
            }
        }

        fn dispatch(&self, event: Event) {
            assert!(self.controller.dispatch(event));
        }

        /// `event` changes nothing: the next render is the one of a marker
        /// sent after it.
        fn quiet(&mut self, event: Event) {
            self.dispatch(event);
            self.dispatch(Event::SetReadingSource(Some(Source::Clipboard)));
            let (view, effects) = self.next();
            assert_eq!(view.reading_source, Some(Source::Clipboard));
            assert!(effects.is_empty(), "{effects:?}");
            self.dispatch(Event::SetReadingSource(None));
            self.next();
        }

        fn edit(&self, text: &str) {
            self.dispatch(Event::FieldEdited {
                text: text.to_owned(),
                replaced: false,
                source: None,
            });
        }

        /// Types `text` and submits it, as Enter does; gives the settled
        /// view and the effects of the submit.
        fn submit(&mut self, text: &str) -> (ViewState, Vec<Effect>) {
            self.edit(text);
            self.settled();
            self.dispatch(Event::FieldSubmitted);
            self.next()
        }

        /// Holds the one worker until the returned sender sends or drops.
        fn hold_worker(&self) -> (mpsc::Sender<()>, Job) {
            let work = self.controller.work().upgrade().unwrap();
            let (started, waiting) = mpsc::channel();
            let (release, blocked) = mpsc::channel::<()>();
            let gate = work.run(
                WAIT,
                async move {
                    started.send(()).unwrap();
                    let _ = blocked.recv_timeout(WAIT);
                },
                |_| {},
            );
            waiting.recv_timeout(WAIT).unwrap();
            (release, gate)
        }
    }

    fn press_capture(shell: &mut FakeShell) -> RequestId {
        shell.dispatch(Event::CapturePressed {
            activation_token: Some("picker-token".into()),
        });
        let (view, effects) = shell.next();
        assert!(!view.capture_sensitive);
        assert!(view.source_failure.is_none());
        let [
            ..,
            Effect::CaptureArea {
                id,
                read_sequence,
                activation_token,
            },
        ] = effects.as_slice()
        else {
            panic!("picker effect: {effects:?}");
        };
        assert_eq!(*read_sequence, view.read_sequence);
        assert_eq!(activation_token.as_deref(), Some("picker-token"));
        *id
    }

    fn accept_image(shell: &mut FakeShell, picker: RequestId, language: Language) -> RequestId {
        shell.dispatch(Event::PickerStarted { id: picker });
        deliver_image(shell, picker, language)
    }

    fn deliver_image(shell: &mut FakeShell, picker: RequestId, language: Language) -> RequestId {
        shell.dispatch(Event::AreaCaptured {
            id: picker,
            result: Ok(ImageRef::new(())),
            capture_ms: Some(23),
        });
        let (view, effects) = shell.next();
        assert!(view.capture_sensitive);
        assert_eq!(view.reading_source, Some(Source::Ocr));
        let [
            Effect::PresentCapture { retry: false, .. },
            Effect::Recognize {
                id,
                language: selected,
                ..
            },
        ] = effects.as_slice()
        else {
            panic!("recognition effect: {effects:?}");
        };
        assert!(id.0 > picker.0);
        assert_eq!(*selected, language);
        *id
    }

    fn finish_recognition(shell: &FakeShell, id: RequestId, text: &str) {
        shell.dispatch(Event::TextRecognized {
            id,
            result: Ok(text.to_owned()),
            timing: Timing {
                recognition_ms: Some(8),
                child_peak_rss_bytes: Some(1024),
                ..Timing::default()
            },
        });
    }

    fn marker(shell: &mut FakeShell) -> ViewState {
        shell.dispatch(Event::SetPressed(Language::Ca));
        let (view, effects) = shell.next();
        assert!(effects.is_empty(), "{effects:?}");
        view
    }

    #[test]
    fn a_press_while_a_picker_waits_starts_nothing() {
        let mut shell = FakeShell::start(None);
        let picker = press_capture(&mut shell);
        shell.dispatch(Event::CapturePressed {
            activation_token: Some("ignored-token".into()),
        });
        assert!(!marker(&mut shell).capture_sensitive);
        accept_image(&mut shell, picker, Language::En);
    }

    #[test]
    fn a_retry_has_a_new_number_and_an_old_job_is_ignored() {
        let mut shell = FakeShell::start(None);
        let picker = press_capture(&mut shell);
        let recognition = accept_image(&mut shell, picker, Language::En);
        shell.dispatch(Event::CapturePressed {
            activation_token: None,
        });
        let (_, effects) = shell.next();
        let [Effect::Cancel(cancelled), Effect::CaptureArea { id, .. }] = effects.as_slice() else {
            panic!("retry effects: {effects:?}");
        };
        assert_eq!(*cancelled, recognition);
        assert!(id.0 > recognition.0);
        finish_recognition(&shell, recognition, "obsolete OCR");
        let view = marker(&mut shell);
        assert!(view.field.text.is_empty());
        assert!(!view.capture_sensitive);
        assert!(view.source_failure.is_none());
    }

    #[test]
    fn capture_restores_the_window_and_records_normalized_ocr_in_history() {
        let (mut shell, asked) = FakeShell::scripted(None, true);
        shell.submit("previous sentence");
        shell.dispatch(Event::ReadingChosen(Language::Es));
        shell.next();
        let picker = press_capture(&mut shell);
        // A change while the window is hiding counts, a change after the
        // picker starts does not.
        shell.dispatch(Event::ReadingChosen(Language::Ca));
        shell.next();
        shell.dispatch(Event::PickerStarted { id: picker });
        shell.dispatch(Event::ReadingChosen(Language::En));
        shell.next();
        let recognized = deliver_image(&mut shell, picker, Language::Ca);
        finish_recognition(&shell, recognized, "  Recognized words\n on two lines.  ");
        let (view, effects) = shell.next();
        assert_eq!(view.field.text, "Recognized words on two lines.");
        assert_eq!(view.field.revision, 1);
        assert_eq!(view.ocr_field_revision, Some(view.field.revision));
        assert_eq!(view.source, Source::Ocr);
        assert!(view.can_back);
        assert_eq!(view.reading_source, None);
        assert!(effects.contains(&Effect::RecordCapture {
            read: evidence::Read::Recognized,
            timing: Timing {
                capture_ms: Some(23),
                recognition_ms: Some(8),
                child_peak_rss_bytes: Some(1024),
            },
        }));
        assert!(asked.try_recv().is_err());
        shell.dispatch(Event::Back);
        assert_eq!(shell.next().0.field.text, "previous sentence");
        shell.dispatch(Event::Forward);
        assert_eq!(shell.next().0.field.text, "Recognized words on two lines.");
    }

    #[test]
    fn capture_failures_preserve_the_field_and_article_and_clear_on_edit() {
        let data = TestData::with_bank();
        let (mut shell, asked) = FakeShell::scripted(Some(data.0.clone()), true);
        shell.edit("bank");
        let (original, _) = shell.settled();
        for (failure, message) in [
            (CaptureFailure::Cancelled, "The capture was cancelled."),
            (CaptureFailure::Ended, "No snapshot was taken."),
            (
                CaptureFailure::Unavailable,
                "Screen capture is not available here.",
            ),
        ] {
            let id = press_capture(&mut shell);
            shell.dispatch(Event::AreaCaptured {
                id,
                result: Err(Ended::Shell(failure)),
                capture_ms: Some(10),
            });
            let (view, effects) = shell.next();
            assert_eq!(view.field, original.field);
            assert_eq!(view.shown, original.shown);
            assert!(view.capture_sensitive);
            assert!(view.source_failure.unwrap().starts_with(message));
            assert!(matches!(
                effects.as_slice(),
                [
                    Effect::PresentCapture { retry: true, .. },
                    Effect::RecordCapture { .. }
                ]
            ));
        }
        let picker = press_capture(&mut shell);
        let id = accept_image(&mut shell, picker, Language::En);
        finish_recognition(&shell, id, "  — ?!\n  ");
        let (view, effects) = shell.next();
        assert_eq!(view.field, original.field);
        assert_eq!(view.shown, original.shown);
        assert!(!view.looking_up);
        assert_eq!(
            view.source_failure.as_deref(),
            Some("No text was recognized.")
        );
        assert!(matches!(
            effects.as_slice(),
            [
                Effect::PresentCapture { retry: true, .. },
                Effect::RecordCapture {
                    read: evidence::Read::Empty,
                    ..
                }
            ]
        ));
        assert!(asked.try_recv().is_err());
        assert_eq!(marker(&mut shell).source_failure, view.source_failure);
        shell.edit("new words");
        assert!(shell.next().0.source_failure.is_none());
    }

    #[test]
    fn editing_during_recognition_cancels_the_effect_and_keeps_the_edit() {
        let mut shell = FakeShell::start(None);
        let picker = press_capture(&mut shell);
        let id = accept_image(&mut shell, picker, Language::En);
        shell.edit("your edit");
        let (view, effects) = shell.next();
        assert_eq!(effects, [Effect::Cancel(id)]);
        assert_eq!(view.reading_source, None);
        finish_recognition(&shell, id, "obsolete OCR");
        let view = marker(&mut shell);
        assert_eq!(view.field.text, "your edit");
        assert_eq!(view.field.revision, 0);
    }

    #[test]
    fn read_retires_either_stage_and_releases_a_late_image() {
        struct ImageDrop(mpsc::Sender<()>);
        impl Drop for ImageDrop {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }

        let mut shell = FakeShell::start(None);
        let picker = press_capture(&mut shell);
        shell.dispatch(Event::ReadPressed { sequence: 1 });
        let (view, effects) = shell.next();
        assert_eq!(view.read_sequence, 1);
        let [
            Effect::Cancel(cancelled),
            Effect::ShowPanel { read_sequence: 1 },
            Effect::ReadSelection { .. },
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(*cancelled, picker);
        let (dropped, release) = mpsc::channel();
        shell.dispatch(Event::AreaCaptured {
            id: picker,
            result: Ok(ImageRef::new(ImageDrop(dropped))),
            capture_ms: None,
        });
        release.recv_timeout(WAIT).expect("retired image released");
        assert!(marker(&mut shell).field.text.is_empty());

        let picker = press_capture(&mut shell);
        let id = accept_image(&mut shell, picker, Language::En);
        shell.dispatch(Event::ReadPressed { sequence: 2 });
        let (view, effects) = shell.next();
        assert_eq!(effects[0], Effect::Cancel(id));
        assert!(matches!(effects.last(), Some(Effect::ReadSelection { .. })));
        assert_eq!(view.read_sequence, 2);
        finish_recognition(&shell, id, "obsolete OCR");
        assert!(marker(&mut shell).field.text.is_empty());
    }

    #[test]
    fn each_capture_deadline_cancels_then_records_after_acknowledgement() {
        for recognition in [false, true] {
            let short = Duration::from_millis(15);
            let (picker_limit, recognition_limit) = if recognition {
                (deadlines::CAPTURE_LIMIT, short)
            } else {
                (short, deadlines::OCR_LIMIT)
            };
            let capture = capture::Capture::with_limits(picker_limit, recognition_limit);
            let mut shell = FakeShell::with_capture(None, Silent, true, capture);
            let picker = press_capture(&mut shell);
            let id = if recognition {
                accept_image(&mut shell, picker, Language::En)
            } else {
                picker
            };
            let (view, effects) = shell.next();
            assert!(view.capture_sensitive);
            assert_eq!(view.reading_source, None);
            let message = if recognition {
                "Text recognition took too long."
            } else {
                "No area was chosen in time. Press Capture to try again."
            };
            assert_eq!(view.source_failure.as_deref(), Some(message));
            assert_eq!(
                effects,
                [
                    Effect::Cancel(id),
                    Effect::PresentCapture {
                        read_sequence: 0,
                        retry: true
                    }
                ]
            );
            if recognition {
                finish_recognition(&shell, id, "obsolete OCR");
            } else {
                shell.dispatch(Event::AreaCaptured {
                    id,
                    result: Err(Ended::Shell(CaptureFailure::Cancelled)),
                    capture_ms: Some(500),
                });
            }
            let (after, effects) = shell.next();
            assert_eq!(after.field, view.field);
            assert_eq!(after.source_failure, view.source_failure);
            let [Effect::RecordCapture { read, timing }] = effects.as_slice() else {
                panic!("deadline evidence: {effects:?}");
            };
            if recognition {
                assert_eq!(*read, evidence::Read::OcrTimedOut);
                assert_eq!(timing.capture_ms, Some(23));
                assert_eq!(timing.recognition_ms, Some(8));
            } else {
                assert_eq!(*read, evidence::Read::CaptureTimedOut);
                assert_eq!(timing.capture_ms, Some(500));
            }
        }
    }

    fn article_definition(view: &ViewState) -> &str {
        let Some(Lookup::Article(article)) = view.shown.lookup() else {
            panic!("an article, not {:?}", view.shown.lookup());
        };
        &article.senses[0].definition
    }

    struct Silent;

    impl Translator for Silent {
        fn translate(&self, _: Request) -> Translating<'_> {
            Box::pin(std::future::pending())
        }
    }

    type Answer = Result<String, Refusal>;

    struct Asked {
        request: Request,
        answer: oneshot::Sender<Answer>,
    }

    struct Scripted(mpsc::Sender<Asked>);

    impl Translator for Scripted {
        fn translate(&self, request: Request) -> Translating<'_> {
            Box::pin(async move {
                let (answer, received) = oneshot::channel();
                if self.0.send(Asked { request, answer }).is_err() {
                    return Err(Refusal::Other);
                }
                received.await.unwrap_or(Err(Refusal::Other))
            })
        }
    }

    fn pending(view: &ViewState, kind: OnlineKind) -> bool {
        view.online
            .iter()
            .any(|row| row.kind == kind && row.state == OnlineState::Pending)
    }

    #[test]
    fn a_typed_word_renders_an_article_in_a_newer_snapshot() {
        let data = TestData::with_bank();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        shell.edit("bank");
        let (pending, _) = shell.next();
        assert_eq!(pending.field.text, "bank");
        assert_eq!(pending.shown.lookup(), Some(&Lookup::Pending));
        assert!(pending.looking_up);
        // Typing asks nothing online.
        assert!(pending.online.is_empty());
        let (completed, effects) = shell.next();
        // No revision rise for typing; no history.
        assert_eq!(completed.field.text, "bank");
        assert_eq!(completed.field.revision, 0);
        assert!(!completed.can_back && !completed.can_forward);
        assert!(!completed.looking_up);
        assert_eq!(article_definition(&completed), DEFINITION);
        assert_eq!(completed.answered, Some(Language::En));
        assert!(effects.contains(&Effect::RecordLookup(Answered::Article)));
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::ObserveIndexes { generation: 0, .. }))
        );
        assert!(completed.online.is_empty(), "no DeepL");

        let dispatcher = shell.controller.dispatcher();
        let work = shell.controller.work();
        assert!(work.upgrade().is_some());
        drop(shell);
        assert!(work.upgrade().is_none());
        assert!(!dispatcher.dispatch(Event::FieldSubmitted));
    }

    #[test]
    fn a_replaced_lookup_cannot_settle_the_new_one_and_a_refusal_asks_for_indexes() {
        let mut shell = FakeShell::start(None);
        // Hold the worker while both edits are handled on the controller.
        let (release, _gate) = shell.hold_worker();
        shell.edit("bank");
        let (old, _) = shell.next();
        assert_eq!(old.shown.lookup(), Some(&Lookup::Pending));
        shell.edit("river");
        let (current, _) = shell.next();
        assert_eq!(current.shown.lookup(), Some(&Lookup::Pending));
        assert_eq!(current.sentence, "river");
        // A worker callback may have succeeded in enqueuing before
        // cancellation: the first lookup, of the first run.
        shell
            .controller
            .dispatch
            .0
            .send(Message::LookedUp(LookedUp {
                id: RequestId(1),
                run: 1,
                generation: 0,
                language: Language::En,
                outcome: Box::new(Outcome::Panicked),
            }))
            .unwrap();
        release.send(()).unwrap();
        let (stale, _) = shell.next();
        assert_eq!(stale.shown.lookup(), Some(&Lookup::Pending), "stale");
        let (completed, _) = shell.next();
        assert_eq!(
            completed.shown.lookup(),
            Some(&Lookup::NotImported {
                word: "river".to_owned()
            })
        );

        // A missing dictionary starts no job and asks for the indexes.
        shell.dispatch(Event::KnownIndexes {
            generation: 7,
            indexes: Some(Indexes::MISSING),
        });
        shell.dispatch(Event::ReadingChosen(Language::Es));
        let (refused, effects) = shell.next();
        assert_eq!(
            refused.shown.lookup(),
            Some(&Lookup::NoDictionary(Language::Es))
        );
        assert!(!refused.looking_up);
        assert!(effects.contains(&Effect::CheckIndexes {
            index_generation: 7
        }));
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::RecordLookup(_)))
        );
    }

    #[test]
    fn typing_a_sentence_looks_nothing_up_and_records_nothing() {
        let data = TestData::with_bank();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        shell.edit("the bank of the river");
        let (view, effects) = shell.next();
        assert_eq!(view.sentence, "the bank of the river");
        assert_eq!(view.field.text, "the bank of the river");
        assert_eq!(view.field.revision, 0);
        assert_eq!(view.words.len(), 5);
        assert_eq!(view.chosen, None);
        assert_eq!(view.input, None);
        assert!(view.shown.lookup().is_none());
        assert!(!view.looking_up);
        assert!(!view.can_forward);
        // Back can go from a typed text only when history has one.
        assert!(!view.can_back);
        assert!(effects.is_empty());
    }

    #[test]
    fn clicking_two_words_looks_each_up_in_its_sentence_and_the_last_is_chosen() {
        let data = TestData::with_bank();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        shell.edit("the bank of the river");
        let (typed, _) = shell.next();
        shell.dispatch(Event::WordChosen {
            text: typed.text_number,
            index: 1,
        });
        let (view, _) = shell.next();
        assert_eq!(view.chosen, Some(1));
        assert_eq!(view.shown.lookup(), Some(&Lookup::Pending));
        assert!(pending(&view, OnlineKind::Word));
        assert!(pending(&view, OnlineKind::Sentence));
        let input = view.input.clone().expect("an input");
        assert_eq!(input.word(), "bank");
        assert_eq!(input.sentence(), Some("the bank of the river"));
        assert_eq!(view.context, Some((0, 0..view.sentence.len())));
        // A click is no new text.
        assert_eq!(view.text_number, typed.text_number);
        let (view, _) = shell.settled();
        assert_eq!(article_definition(&view), DEFINITION);

        shell.dispatch(Event::WordChosen {
            text: typed.text_number,
            index: 4,
        });
        let (view, _) = shell.next();
        assert_eq!(view.chosen, Some(4));
        assert!(pending(&view, OnlineKind::Word));
        assert!(pending(&view, OnlineKind::Sentence));
        let (view, _) = shell.settled();
        assert_eq!(view.chosen, Some(4));
        assert_eq!(view.sentence, "the bank of the river");
        assert_eq!(view.words.len(), 5);
        assert_eq!(article_definition(&view), RIVER);
        assert!(!view.can_back && !view.can_forward, "nothing recorded");

        // An invalid index is ignored.
        shell.quiet(Event::WordChosen {
            text: typed.text_number,
            index: 99,
        });
    }

    #[test]
    fn choosing_another_word_during_a_lookup_supersedes_it() {
        let data = TestData::with_bank();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        shell.edit("the bank of the river");
        let (typed, _) = shell.next();
        let (release, _gate) = shell.hold_worker();
        shell.dispatch(Event::WordChosen {
            text: typed.text_number,
            index: 1,
        });
        let (first, _) = shell.next();
        assert!(first.looking_up);
        shell.dispatch(Event::WordChosen {
            text: typed.text_number,
            index: 4,
        });
        let (second, _) = shell.next();
        assert!(second.looking_up);
        // The outcome of the first click, enqueued before it was dropped.
        shell
            .controller
            .dispatch
            .0
            .send(Message::LookedUp(LookedUp {
                id: RequestId(1),
                run: 2,
                generation: 0,
                language: Language::En,
                outcome: Box::new(Outcome::Panicked),
            }))
            .unwrap();
        release.send(()).unwrap();
        let (view, _) = shell.settled();
        assert_eq!(view.chosen, Some(4));
        assert_eq!(article_definition(&view), RIVER);
    }

    #[test]
    fn clearing_the_field_during_a_lookup_hides_it_and_raises_the_revision() {
        let data = TestData::with_bank();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        let (release, _gate) = shell.hold_worker();
        shell.edit("bank");
        let (pending, _) = shell.next();
        assert!(pending.looking_up);
        shell.dispatch(Event::FieldCleared);
        let (cleared, effects) = shell.next();
        assert_eq!(cleared.field.text, "");
        assert_eq!(cleared.field.revision, pending.field.revision + 1);
        assert_eq!(cleared.shown.lookup(), None);
        assert!(!cleared.looking_up);
        assert!(effects.is_empty());
        release.send(()).unwrap();
        // The cancelled lookup shows nothing, up to a marker sent after it.
        shell.dispatch(Event::SetReadingSource(Some(Source::Clipboard)));
        loop {
            let (view, _) = shell.next();
            assert_eq!(view.shown.lookup(), None);
            if view.reading_source.is_some() {
                break;
            }
        }
    }

    #[test]
    fn picking_a_sense_twice_re_renders_and_leaves_field_and_history() {
        let data = TestData::with_bank();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        let (before, _) = shell.submit("bank");
        shell.dispatch(Event::SenseChosen {
            article: before.article_id(),
            index: 0,
        });
        let (view, effects) = shell.next();
        assert_eq!(view.shown.choice(), Some(Choice::Sense(0)));
        assert_eq!(view.field, before.field);
        assert_eq!(
            (view.can_back, view.can_forward),
            (before.can_back, before.can_forward)
        );
        assert!(effects.is_empty(), "nothing is looked up or asked");
        // Again: GTK keeps a clicked link visited until it renders again.
        shell.dispatch(Event::SenseChosen {
            article: before.article_id(),
            index: 0,
        });
        let (again, _) = shell.next();
        assert_eq!(again.shown.choice(), Some(Choice::Sense(0)));
        assert_eq!(again.field, before.field);
        assert!(again.choice_revision > view.choice_revision);
        // Out of range is ignored, and so is a gloss of an English article.
        shell.quiet(Event::SenseChosen {
            article: before.article_id(),
            index: 999,
        });
        shell.quiet(Event::GlossChosen {
            article: before.article_id(),
            index: 0,
        });
    }

    #[test]
    fn picking_a_gloss_requests_its_definition_and_records_nothing() {
        let data = TestData::with_banco();
        let (mut shell, asked) = FakeShell::scripted(Some(data.0.clone()), false);
        shell.quiet(Event::KnownIndexes {
            generation: 0,
            indexes: Some(SPANISH),
        });
        shell.dispatch(Event::ReadingChosen(Language::Es));
        shell.next();
        shell.edit("banco");
        let (before, _) = shell.settled();
        let Some(Lookup::Grouped { language, .. }) = before.shown.lookup() else {
            panic!(
                "banco must have a Spanish article: {:?}",
                before.shown.lookup()
            );
        };
        assert_eq!(*language, Language::Es);
        shell.dispatch(Event::GlossChosen {
            article: before.article_id(),
            index: 0,
        });
        let (view, effects) = shell.next();
        assert_eq!(view.shown.choice(), Some(Choice::Gloss(0)));
        assert!(effects.is_empty());
        let Asked { request, answer } = asked.recv_timeout(WAIT).unwrap();
        assert_eq!(request.text, "bank");
        assert_eq!(request.context, None);
        assert_eq!(request.source, Language::En);
        assert_eq!(request.target, Language::Ru);
        assert_eq!(view.shown.definition(), Some(&Online::Pending));
        assert_eq!(view.field, before.field);
        assert!(!view.can_back && !view.can_forward, "nothing recorded");
        // Again: no second request while one is pending.
        shell.dispatch(Event::GlossChosen {
            article: before.article_id(),
            index: 0,
        });
        let (_, effects) = shell.next();
        assert!(effects.is_empty());
        assert!(asked.try_recv().is_err());
        answer.send(Ok("банк".into())).unwrap();
        let (answered, _) = shell.next();
        assert_eq!(
            answered.shown.definition(),
            Some(&Online::Translated("банк".into()))
        );
        // Out of range is ignored.
        shell.quiet(Event::GlossChosen {
            article: before.article_id(),
            index: 9,
        });
    }

    #[test]
    fn back_and_forward_restore_the_field_and_look_it_up_again() {
        let data = TestData::with_bank();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        let (bank, _) = shell.submit("bank");
        assert!(pending(&bank, OnlineKind::Lone));
        assert!(!bank.can_back);
        let (river, _) = shell.submit("river");
        assert!(river.can_back && !river.can_forward);
        assert_eq!(river.field.revision, 0);

        shell.dispatch(Event::Back);
        let (back, _) = shell.next();
        assert_eq!(back.field.text, "bank");
        assert_eq!(back.field.revision, 1);
        assert_eq!(back.sentence, "bank");
        assert_eq!(back.shown.lookup(), Some(&Lookup::Pending));
        assert!(!back.can_back && back.can_forward);
        assert!(pending(&back, OnlineKind::Lone));
        assert!(back.text_number > river.text_number);
        let (back, _) = shell.settled();
        assert_eq!(article_definition(&back), DEFINITION);

        shell.dispatch(Event::Forward);
        let (forward, _) = shell.next();
        assert_eq!(forward.field.text, "river");
        assert_eq!(forward.field.revision, 2);
        assert!(forward.can_back && !forward.can_forward);
        assert!(pending(&forward, OnlineKind::Lone));
        let (forward, _) = shell.settled();
        assert_eq!(article_definition(&forward), RIVER);

        // Nothing was recorded by the walk: Back leads to bank again.
        shell.dispatch(Event::Back);
        let (again, _) = shell.next();
        assert_eq!(again.field.text, "bank");
        assert!(!again.can_back);
        shell.settled();
    }

    #[test]
    fn back_and_forward_at_the_edges_do_nothing() {
        let mut shell = FakeShell::start(None);
        shell.quiet(Event::Back);
        shell.quiet(Event::Forward);
        shell.submit("bank");
        shell.quiet(Event::Back);
        shell.quiet(Event::Forward);
    }

    #[test]
    fn back_from_a_typed_text_records_it_so_forward_comes_back() {
        let data = TestData::with_bank();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        shell.submit("bank");
        shell.edit("river");
        let (typed, _) = shell.settled();
        assert!(typed.can_back && !typed.can_forward);
        shell.dispatch(Event::Back);
        let (back, _) = shell.next();
        assert_eq!(back.field.text, "bank");
        assert!(back.can_forward);
        shell.settled();
        shell.dispatch(Event::Forward);
        let (forward, _) = shell.next();
        assert_eq!(forward.field.text, "river");
        shell.settled();
    }

    #[test]
    fn a_synonym_is_set_recorded_looked_up_and_submitted() {
        let data = TestData::with_bank();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        let (bank, _) = shell.submit("bank");
        shell.dispatch(Event::SynonymChosen {
            article: bank.article_id(),
            word: "shore".to_owned(),
        });
        let (view, _) = shell.next();
        assert_eq!(view.field.text, "shore");
        assert_eq!(view.field.revision, 1);
        assert_eq!(view.sentence, "shore");
        assert_eq!(view.shown.lookup(), Some(&Lookup::Pending));
        assert!(pending(&view, OnlineKind::Lone));
        assert!(view.can_back && !view.can_forward, "recorded");
        let (view, _) = shell.settled();
        assert_eq!(article_definition(&view), DEFINITION);
    }

    #[test]
    fn a_read_text_replaces_the_field_and_is_recorded() {
        let data = TestData::with_bank();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        shell.dispatch(Event::SetReadingSource(Some(Source::Clipboard)));
        let (reading, _) = shell.next();
        assert_eq!(
            reading.window_state,
            WindowState::Waiting {
                source: Source::Clipboard
            }
        );
        shell.dispatch(Event::FieldEdited {
            text: "  the   bank ".to_owned(),
            replaced: true,
            source: Some(Source::Clipboard),
        });
        let (view, _) = shell.next();
        assert_eq!(view.field.text, "the bank");
        assert_eq!(view.field.revision, 0, "the shell set its own text");
        assert_eq!(view.source, Source::Clipboard);
        assert_eq!(view.reading_source, None);
        assert!(pending(&view, OnlineKind::Sentence));
        // Recorded: Back from a typed text goes to it.
        shell.edit("river");
        let (typed, _) = shell.settled();
        assert!(typed.can_back);
    }

    #[test]
    fn a_change_of_language_redoes_the_lookup_and_asks_again() {
        let data = TestData::with_banco();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        shell.quiet(Event::KnownIndexes {
            generation: 0,
            indexes: Some(SPANISH),
        });
        shell.edit("banco");
        let (english, _) = shell.settled();
        assert_eq!(english.answered, Some(Language::Es), "found elsewhere");
        shell.dispatch(Event::ReadingChosen(Language::Es));
        let (view, _) = shell.next();
        assert!(view.looking_up);
        assert!(
            view.online.is_empty(),
            "typing did not submit the lone word"
        );
        let (view, _) = shell.settled();
        assert!(matches!(
            view.shown.lookup(),
            Some(Lookup::Grouped {
                language: Language::Es,
                ..
            })
        ));
        assert_eq!(view.field, english.field);

        shell.dispatch(Event::MyLanguageChosen(Language::En));
        let (view, _) = shell.next();
        assert_eq!(view.shown.translating().my, Language::En);
        assert!(view.online.is_empty());
        shell.settled();
    }
}
