//! Product settings and the OS state shown beside them: the four settings
//! files, the DeepL key, the cache, the login item and the shortcut bindings.
//! The shell only runs the effects and sends back what happened (AD-18).

use std::{
    future,
    sync::{Arc, Mutex},
    task::{Poll, Waker},
    time::Duration,
};

use addict_core::{
    dirs::Dirs,
    failure::{
        Ended,
        key_store::KeyStoreFailure,
        login_item::{LoginItemShown, StartupEnd, StartupFailure},
    },
    translation::{English, Source as Language},
};
use addict_deepl::key::{self, KeyError, KeySource, Secret, WriteError};
use addict_engine::{
    at_once, cache, english, language, my_language,
    view::{
        Binding, CHANGE_UNBOUND, CacheState, KeyState, NO_KEY_PLACE, REBINDING, SAVE_UNANSWERED,
        STARTUP_LEGACY, STARTUP_NO_RUNNER, STARTUP_TIMED_OUT, STARTUP_UNREAD, change_text,
        clear_text, key_store_text, rebind_text, reopened_status, save_text, startup_end_text,
        startup_unfinished_text,
    },
    work::{Job, Outcome},
};

use crate::{
    Effect, Message, Model, RequestId,
    deadlines::{CACHE_LIMIT, KEY_SAVE_LIMIT, LANGUAGE_FILE_LIMIT, STARTUP_LIMIT},
};

/// The assignments of the two actions, as the shell reports them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bindings {
    pub clipboard: Option<String>,
    pub screenshot: Option<String>,
}

/// An unprompted report about the shortcut bindings or the configure dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutState {
    /// The first attempt bound both actions.
    Bound(Bindings),
    /// The bindings changed while the app ran.
    Changed(Bindings),
    /// No shortcut is bound; the detail is shown as the shell sent it.
    Failed(String),
    /// A new binding is running; rows keep the old assignments.
    Rebinding(Bindings),
    /// The new binding ended, with the shell's message if it had one.
    RebindEnded(Option<String>),
    /// The configure dialog was answered.
    ConfigureAnswered(Option<String>),
}

/// The part of [`crate::ViewState`] that Settings renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsView {
    pub my_language: Language,
    pub reading: Language,
    pub english: English,
    pub english_shown: bool,
    pub at_once: bool,
    /// Whether the four files have been read at least once.
    pub languages_read: bool,
    /// Rises on every choice of my language, so a shell can tell a user's
    /// choice from the language the start read found in the file.
    pub my_chosen: u64,
    pub key: KeyState,
    pub key_line: String,
    /// Rises when the shell should empty the key field.
    pub key_entry_clears: u64,
    pub cache: CacheState,
    pub cache_line: String,
    /// Where the switch's knob stands: the asked state while a change runs.
    pub login_on: bool,
    /// The state the portal last confirmed, which the switch's `state` shows;
    /// it differs from `login_on` only while a change runs.
    pub login_confirmed: bool,
    pub login_sensitive: bool,
    pub login_line: String,
    /// The shell reported no runner, so login work cannot be done.
    pub runner_missing: bool,
    pub binding: Binding,
    pub bindings: Bindings,
    pub rebinding: bool,
    pub status_line: String,
}

/// A gate the four start reads wait on, for a test that begins before the
/// files have been read. A production controller passes `None`.
#[derive(Default)]
pub(crate) struct ReadGate {
    state: Mutex<GateState>,
}

#[derive(Default)]
struct GateState {
    open: bool,
    waiting: Vec<Waker>,
}

impl ReadGate {
    /// Yields until the test opens the gate; it never blocks the runner.
    pub(crate) async fn wait(&self) {
        future::poll_fn(|context| {
            let mut state = self.state.lock().expect("read gate");
            if state.open {
                Poll::Ready(())
            } else {
                state.waiting.push(context.waker().clone());
                Poll::Pending
            }
        })
        .await
    }

    #[cfg(test)]
    pub(crate) fn is_waiting(&self) -> bool {
        !self.state.lock().expect("read gate").waiting.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("read gate");
        state.open = true;
        for waiter in state.waiting.drain(..) {
            waiter.wake();
        }
    }
}

/// The deadlines the controller owns, injectable for the scripted tests.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub language_file: Duration,
    pub key_save: Duration,
    pub startup: Duration,
    pub cache: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            language_file: LANGUAGE_FILE_LIMIT,
            key_save: KEY_SAVE_LIMIT,
            startup: STARTUP_LIMIT,
            cache: CACHE_LIMIT,
        }
    }
}

/// One result-bearing OS request still awaited, with its deadline's handle.
struct Pending {
    id: RequestId,
    _deadline: Job,
}

/// One file's value as it was read at start. Each setting has its own job, so
/// a choice of one never cancels the read of another (as in 0.5.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRead {
    Reading(Language),
    My(Language),
    English(English),
    AtOnce(bool),
}

/// The state itself. No file is read or written on the controller thread.
pub(crate) struct Settings {
    language_file: Option<std::path::PathBuf>,
    my_language_file: Option<std::path::PathBuf>,
    english_file: Option<std::path::PathBuf>,
    at_once_file: Option<std::path::PathBuf>,
    pub(crate) source: KeySource,
    key_file: Option<std::path::PathBuf>,
    key_line: String,
    key_entry_clears: u64,
    store: Option<Pending>,
    cache: CacheState,
    cache_line: String,
    cache_job: Option<Job>,
    clear_job: Option<Job>,
    login_on: bool,
    login_sensitive: bool,
    login_line: String,
    runner_missing: bool,
    /// The switch's state before a change began, restored when no runner ran.
    login_before: Option<bool>,
    login_change: Option<Pending>,
    /// A toggle happened since Settings was last opened: a login read that
    /// was asked before it is stale and must not overwrite its answer.
    login_read_stale: bool,
    binding: Binding,
    bindings: Bindings,
    rebinding: bool,
    status_line: String,
    pub(crate) saved_reading: Language,
    pub(crate) my_language: Language,
    pub(crate) english: English,
    pub(crate) at_once: bool,
    pub(crate) languages_read: bool,
    /// One flag per file, so a cancelled read still ends the wait for it.
    reading_read: bool,
    my_read: bool,
    english_read: bool,
    at_once_read: bool,
    pub(crate) reading_chosen: bool,
    my_chosen: bool,
    /// How many times the user chose my language; shell side effects follow
    /// this and not the start read.
    my_choices: u64,
    english_chosen: bool,
    at_once_chosen: bool,
    /// The read jobs, replaced by the jobs of a chosen setting's write.
    reading_read_job: Option<Job>,
    my_read_job: Option<Job>,
    english_read_job: Option<Job>,
    at_once_read_job: Option<Job>,
    reading_write: Option<RequestId>,
    my_write: Option<RequestId>,
    english_write: Option<RequestId>,
    at_once_write: Option<RequestId>,
    limits: Limits,
    /// Held by a test that wants to act before the start reads land.
    read_gate: Option<Arc<ReadGate>>,
}

impl Settings {
    pub(crate) fn with_limits(dirs: &Dirs, source: KeySource, limits: Limits) -> Self {
        let config = dirs.config.clone();
        Self {
            language_file: language::path(config.as_deref()),
            my_language_file: my_language::path(config.as_deref()),
            english_file: english::path(config.as_deref()),
            at_once_file: at_once::path(config.as_deref()),
            key_file: key::key_path(config.as_deref()),
            source,
            key_line: String::new(),
            key_entry_clears: 0,
            store: None,
            cache: CacheState::Checking,
            cache_line: String::new(),
            cache_job: None,
            clear_job: None,
            login_on: false,
            login_sensitive: false,
            login_line: String::new(),
            runner_missing: false,
            login_before: None,
            login_change: None,
            login_read_stale: false,
            binding: Binding::Attempting,
            bindings: Bindings::default(),
            rebinding: false,
            status_line: String::new(),
            saved_reading: Language::En,
            my_language: Language::Ru,
            english: English::Us,
            at_once: at_once::DEFAULT,
            languages_read: false,
            reading_read: false,
            my_read: false,
            english_read: false,
            at_once_read: false,
            reading_chosen: false,
            my_chosen: false,
            my_choices: 0,
            english_chosen: false,
            at_once_chosen: false,
            reading_read_job: None,
            my_read_job: None,
            english_read_job: None,
            at_once_read_job: None,
            reading_write: None,
            my_write: None,
            english_write: None,
            at_once_write: None,
            limits,
            read_gate: None,
        }
    }

    pub(crate) fn set_read_gate(&mut self, gate: Option<Arc<ReadGate>>) {
        self.read_gate = gate;
    }

    pub(crate) fn waiting_for_writes(&self) -> bool {
        self.store.is_some() || self.login_change.is_some()
    }

    pub(crate) fn reading(&self) -> Language {
        language::reading(self.saved_reading, self.my_language)
    }

    pub(crate) fn view(&self) -> SettingsView {
        SettingsView {
            my_language: self.my_language,
            reading: self.reading(),
            english: self.english,
            english_shown: self.my_language == Language::En,
            at_once: self.at_once,
            languages_read: self.languages_read,
            my_chosen: self.my_choices,
            key: KeyState::of(&self.source),
            key_line: self.key_line.clone(),
            key_entry_clears: self.key_entry_clears,
            cache: self.cache,
            cache_line: self.cache_line.clone(),
            login_on: self.login_on,
            login_confirmed: self.login_before.unwrap_or(self.login_on),
            login_sensitive: self.login_sensitive,
            login_line: self.login_line.clone(),
            runner_missing: self.runner_missing,
            binding: self.binding.clone(),
            bindings: self.bindings.clone(),
            rebinding: self.rebinding,
            status_line: self.status_line.clone(),
        }
    }
}

impl Model {
    /// Starts one read of each settings file on the shared runner. Every
    /// outcome is reported, even a timeout or a panic: 0.5.0 defaulted each
    /// setting and always ended the wait. A choice of one setting drops only
    /// that file's read; the others carry on.
    pub(crate) fn start_settings_read(&mut self) {
        let limit = self.settings.limits.language_file;
        let send = self.send.clone();
        let gate = self.settings.read_gate.clone();
        let path = self.settings.language_file.clone();
        let reading = send.clone();
        let waited = gate.clone();
        self.settings.reading_read_job = Some(self.work.run(
            limit,
            async move {
                if let Some(gate) = &waited {
                    gate.wait().await;
                }
                language::read(path.as_deref())
            },
            move |outcome| {
                let value = match outcome {
                    Outcome::Finished(value) => value,
                    _ => Language::En,
                };
                let _ = reading.send(Message::SettingsRead(SettingsRead::Reading(value)));
            },
        ));
        let path = self.settings.my_language_file.clone();
        let my = send.clone();
        let waited = gate.clone();
        self.settings.my_read_job = Some(self.work.run(
            limit,
            async move {
                if let Some(gate) = &waited {
                    gate.wait().await;
                }
                my_language::read(path.as_deref())
            },
            move |outcome| {
                let value = match outcome {
                    Outcome::Finished(value) => value,
                    _ => Language::Ru,
                };
                let _ = my.send(Message::SettingsRead(SettingsRead::My(value)));
            },
        ));
        let path = self.settings.english_file.clone();
        let english = send.clone();
        let waited = gate.clone();
        self.settings.english_read_job = Some(self.work.run(
            limit,
            async move {
                if let Some(gate) = &waited {
                    gate.wait().await;
                }
                english::read(path.as_deref())
            },
            move |outcome| {
                let value = match outcome {
                    Outcome::Finished(value) => value,
                    _ => English::Us,
                };
                let _ = english.send(Message::SettingsRead(SettingsRead::English(value)));
            },
        ));
        let path = self.settings.at_once_file.clone();
        let waited = gate.clone();
        self.settings.at_once_read_job = Some(self.work.run(
            limit,
            async move {
                if let Some(gate) = &waited {
                    gate.wait().await;
                }
                at_once::read(path.as_deref())
            },
            move |outcome| {
                let value = match outcome {
                    Outcome::Finished(value) => value,
                    _ => addict_engine::at_once::DEFAULT,
                };
                let _ = send.send(Message::SettingsRead(SettingsRead::AtOnce(value)));
            },
        ));
    }

    /// A late read applies only to the setting the user has not chosen since.
    /// The four flags make `languages_read` true even when a read was given
    /// up by a choice, so the shell can use the value it now has.
    pub(crate) fn settings_read(&mut self, read: SettingsRead, effects: &mut Vec<Effect>) -> bool {
        let reading_before = self.settings.reading();
        let mut my = None;
        let mut english = None;
        {
            let settings = &mut self.settings;
            match read {
                SettingsRead::Reading(value) => {
                    settings.reading_read = true;
                    if !settings.reading_chosen && settings.saved_reading != value {
                        settings.saved_reading = value;
                    }
                }
                SettingsRead::My(value) => {
                    settings.my_read = true;
                    if !settings.my_chosen && settings.my_language != value {
                        settings.my_language = value;
                        my = Some(value);
                    }
                }
                SettingsRead::English(value) => {
                    settings.english_read = true;
                    if !settings.english_chosen && settings.english != value {
                        settings.english = value;
                        english = Some(value);
                    }
                }
                SettingsRead::AtOnce(value) => {
                    settings.at_once_read = true;
                    if !settings.at_once_chosen && settings.at_once != value {
                        settings.at_once = value;
                    }
                }
            }
            settings.languages_read = settings.reading_read
                && settings.my_read
                && settings.english_read
                && settings.at_once_read;
            let (english, at_once) = (settings.english, settings.at_once);
            self.online.english = english;
            self.online.at_once = at_once;
        }
        if let Some(value) = my {
            self.shown.set_my_language(value);
        }
        if let Some(value) = english {
            self.shown.set_english(value);
        }
        // What a read re-asks is what the same change asked in 0.5.0: my
        // language always (`set_my_language`), the reading language only when
        // the effective one moved (`set_reading`), the spelling only while
        // English is mine, after giving up its definition (`SetEnglish`), and
        // sentence at once never (`SetAtOnce`).
        self.update_reading();
        if my.is_some() || self.settings.reading() != reading_before {
            self.ask_again(effects);
        } else if english.is_some() && self.settings.my_language == Language::En {
            self.cancel_definition();
            self.ask_again(effects);
        }
        // A file read is a state update even when its value matched the
        // default: the shell learns that another file has been read.
        true
    }

    pub(crate) fn update_reading(&mut self) {
        self.shown.set_pressed(self.settings.reading());
    }

    /// The reading language was chosen by a switch press or an English gloss.
    /// The choice wins over a read that arrives later.
    pub(crate) fn choose_reading(&mut self, language: Language, effects: &mut Vec<Effect>) -> bool {
        let previous = self.settings.reading();
        self.settings.saved_reading = language;
        self.settings.reading_chosen = true;
        self.update_reading();
        self.write_reading();
        if self.settings.reading() == previous {
            return true;
        }
        self.ask_again(effects)
    }

    pub(crate) fn choose_my_language(
        &mut self,
        language: Language,
        effects: &mut Vec<Effect>,
    ) -> bool {
        self.settings.my_language = language;
        self.settings.my_chosen = true;
        self.settings.my_choices += 1;
        self.shown.set_my_language(language);
        self.update_reading();
        self.write_my_language();
        self.ask_again(effects)
    }

    pub(crate) fn choose_english(&mut self, english: English, effects: &mut Vec<Effect>) -> bool {
        let changed = self.settings.english != english;
        self.settings.english = english;
        self.settings.english_chosen = true;
        self.online.english = english;
        self.write_english();
        if changed && self.settings.my_language == Language::En {
            self.cancel_definition();
            return self.ask_again(effects);
        }
        true
    }

    pub(crate) fn choose_at_once(&mut self, at_once: bool) -> bool {
        self.settings.at_once = at_once;
        self.settings.at_once_chosen = true;
        self.online.at_once = at_once;
        self.write_at_once();
        // Every choice renders the full snapshot (AD-5).
        true
    }

    pub(crate) fn write_reading(&mut self) {
        // The choice owns this file now: only its read is given up, not the
        // reads of the other three files.
        self.settings.reading_read_job = None;
        let path = self.settings.language_file.clone();
        let language = self.settings.saved_reading;
        self.settings.reading_write = Some(self.write_setting(
            self.settings.reading_write,
            "the reading language",
            async move { language::write(path.as_deref(), language) },
        ));
    }

    fn write_my_language(&mut self) {
        self.settings.my_read_job = None;
        let path = self.settings.my_language_file.clone();
        let language = self.settings.my_language;
        self.settings.my_write = Some(self.write_setting(
            self.settings.my_write,
            "my language",
            async move { my_language::write(path.as_deref(), language) },
        ));
    }

    fn write_english(&mut self) {
        self.settings.english_read_job = None;
        let path = self.settings.english_file.clone();
        let english = self.settings.english;
        self.settings.english_write = Some(self.write_setting(
            self.settings.english_write,
            "the English spelling",
            async move { english::write(path.as_deref(), english) },
        ));
    }

    fn write_at_once(&mut self) {
        self.settings.at_once_read_job = None;
        let path = self.settings.at_once_file.clone();
        let at_once = self.settings.at_once;
        self.settings.at_once_write = Some(self.write_setting(
            self.settings.at_once_write,
            "Translate sentence at once",
            async move { at_once::write(path.as_deref(), at_once) },
        ));
    }

    /// Keep even a superseded job until it reports its outcome: a write
    /// that started before cancellation may still be committing.
    fn write_setting(
        &mut self,
        previous: Option<RequestId>,
        name: &'static str,
        write: impl Future<Output = std::io::Result<()>> + Send + 'static,
    ) -> RequestId {
        if let Some(job) = previous.and_then(|id| self.lifecycle.writes.get(&id)) {
            job.cancel();
        }
        let id = self.next_request();
        let send = self.send.clone();
        let job = self
            .work
            .run(self.settings.limits.language_file, write, move |outcome| {
                match outcome {
                    Outcome::Finished(Ok(())) | Outcome::Cancelled => {}
                    Outcome::Finished(Err(error)) => {
                        eprintln!("addict: {name} was not kept: {error}")
                    }
                    Outcome::TimedOut | Outcome::Panicked => {
                        eprintln!("addict: {name} was not kept")
                    }
                }
                let _ = send.send(Message::SettingWritten(id));
            });
        self.lifecycle.writes.insert(id, job);
        id
    }

    /// Settings was opened: clear the lines, empty the key field, ask for the
    /// login item's state and read the cache's size again. A login change that
    /// is already running keeps its line, its switch and its answer.
    pub(crate) fn settings_opened(&mut self, effects: &mut Vec<Effect>) -> bool {
        let settings = &mut self.settings;
        settings.key_line.clear();
        settings.cache_line.clear();
        settings.status_line = reopened_status(settings.rebinding).to_owned();
        settings.key_entry_clears += 1;
        settings.login_read_stale = false;
        if settings.login_change.is_none() {
            settings.login_sensitive = false;
            settings.login_line.clear();
            effects.push(Effect::ReadLoginItem);
        }
        self.check_cache();
        true
    }

    fn check_cache(&mut self) {
        self.settings.cache = CacheState::Checking;
        let store = self.cache.clone();
        let limit = self.settings.limits.cache;
        let send = self.send.clone();
        self.settings.cache_job = Some(self.work.run(
            limit,
            async move { store.footprint() },
            move |outcome| {
                let found = match outcome {
                    Outcome::Finished(footprint) => Some(footprint),
                    Outcome::Cancelled => return,
                    Outcome::TimedOut | Outcome::Panicked => None,
                };
                let _ = send.send(Message::CacheFootprint(found));
            },
        ));
    }

    pub(crate) fn cache_read(&mut self, found: Option<cache::Footprint>) -> bool {
        self.settings.cache = found.map_or(CacheState::Unknown, CacheState::Read);
        true
    }

    /// Clear every row on the runner, then read the size again.
    pub(crate) fn clear_cache_pressed(&mut self) -> bool {
        self.settings.cache_line.clear();
        self.settings.cache = CacheState::Checking;
        let store = self.cache.clone();
        let limit = self.settings.limits.cache;
        let send = self.send.clone();
        self.settings.clear_job = Some(self.work.run(
            limit,
            async move { store.clear() },
            move |outcome| {
                let reason = match outcome {
                    Outcome::Finished(Ok(())) => None,
                    Outcome::Finished(Err(e)) => Some(e),
                    Outcome::TimedOut => Some("the clear did not answer in time".to_owned()),
                    Outcome::Panicked => Some("the clear did not run".to_owned()),
                    Outcome::Cancelled => return,
                };
                let _ = send.send(Message::CacheCleared(reason));
            },
        ));
        true
    }

    pub(crate) fn cache_cleared(&mut self, reason: Option<String>) -> bool {
        self.settings.cache_line = match reason {
            None => clear_text(Ok(())),
            Some(reason) => clear_text(Err(&reason)),
        };
        self.check_cache();
        true
    }

    fn store_timer(&self, id: RequestId) -> Job {
        let send = self.send.clone();
        self.work.run(
            self.settings.limits.key_save,
            future::pending::<()>(),
            move |outcome| {
                if outcome == Outcome::TimedOut {
                    let _ = send.send(Message::KeySaveDeadline(id));
                }
            },
        )
    }

    fn login_timer(&self, id: RequestId) -> Job {
        let send = self.send.clone();
        self.work.run(
            self.settings.limits.startup,
            future::pending::<()>(),
            move |outcome| {
                if outcome == Outcome::TimedOut {
                    let _ = send.send(Message::LoginDeadline(id));
                }
            },
        )
    }

    /// Save key pressed. An invalid key or a missing key file needs no OS
    /// work; it says why in the key's line, as in 0.5.0.
    pub(crate) fn save_key(
        &mut self,
        key: Result<Secret, WriteError>,
        effects: &mut Vec<Effect>,
    ) -> bool {
        // 0.5.0 refused a save with no place for the file before it looked
        // at the key: every save there says where the file would go.
        if self.settings.key_file.is_none() {
            self.settings.key_line = save_text(Err(NO_KEY_PLACE));
            return true;
        }
        let key = match key {
            Ok(key) => key,
            Err(error) => {
                let detail = error.to_string();
                self.settings.key_line = save_text(Err(&detail));
                return true;
            }
        };
        let id = self.next_request();
        self.settings.key_line = "Saving the key…".to_owned();
        // A newer save supersedes the pending one (AD-6).
        if let Some(old) = self.settings.store.take() {
            effects.push(Effect::Cancel(old.id));
        }
        let deadline = self.store_timer(id);
        self.settings.store = Some(Pending {
            id,
            _deadline: deadline,
        });
        effects.push(Effect::StoreKey { id, key });
        true
    }

    /// The shell published a new active key. Only this event changes the key
    /// in memory after startup (AD-10).
    pub(crate) fn key_changed(&mut self, result: Result<Secret, KeyError>) -> bool {
        self.settings.source.replace_result(result);
        true
    }

    pub(crate) fn key_stored(
        &mut self,
        id: RequestId,
        result: Result<(), Ended<KeyStoreFailure>>,
    ) -> bool {
        let Some(store) = self.settings.store.take() else {
            return false;
        };
        if store.id != id {
            self.settings.store = Some(store);
            return false;
        }
        match result {
            Ok(()) => {
                self.settings.key_line.clear();
                self.settings.key_entry_clears += 1;
            }
            Err(failure) => self.settings.key_line = key_store_text(&failure),
        }
        true
    }

    pub(crate) fn key_save_deadline(&mut self, id: RequestId, effects: &mut Vec<Effect>) -> bool {
        let Some(store) = self.settings.store.take() else {
            return false;
        };
        if store.id != id {
            self.settings.store = Some(store);
            return false;
        }
        effects.push(Effect::Cancel(id));
        self.settings.key_line = SAVE_UNANSWERED.to_owned();
        true
    }

    /// The switch was moved: ask the shell's portal adapter, with a deadline.
    /// A second move while one runs is ignored, as `toggle_startup` did, and
    /// the switch shows the asked state until the answer comes.
    pub(crate) fn login_toggled(&mut self, on: bool, effects: &mut Vec<Effect>) -> bool {
        if self.settings.login_change.is_some() {
            return false;
        }
        let id = self.next_request();
        let deadline = self.login_timer(id);
        let settings = &mut self.settings;
        settings.login_change = Some(Pending {
            id,
            _deadline: deadline,
        });
        settings.login_before = Some(settings.login_on);
        settings.login_on = on;
        settings.login_read_stale = true;
        settings.runner_missing = false;
        settings.login_sensitive = false;
        settings.login_line = "Asking the desktop…".to_owned();
        effects.push(Effect::SetLoginItem { id, on });
        true
    }

    pub(crate) fn login_set(
        &mut self,
        id: RequestId,
        result: Result<StartupEnd, Ended<StartupFailure>>,
        runner_missing: bool,
    ) -> bool {
        let Some(change) = self.settings.login_change.take() else {
            return false;
        };
        if change.id != id {
            self.settings.login_change = Some(change);
            return false;
        }
        let settings = &mut self.settings;
        settings.login_sensitive = true;
        settings.runner_missing = runner_missing;
        // Every end is computed from the state confirmed before the toggle,
        // never from the state the switch showed while the request ran. A
        // refused, unasked or timed-out change leaves the switch as it was
        // (`startup_changed` in 0.5.0).
        let last = settings.login_before.take().unwrap_or(settings.login_on);
        let (on, line) = if runner_missing {
            (last, STARTUP_NO_RUNNER.to_owned())
        } else {
            match result {
                Ok(end) => startup_end_text(&end, last),
                Err(Ended::Shell(failure)) => startup_end_text(&StartupEnd::Failed(failure), last),
                Err(Ended::Unfinished) => (last, startup_unfinished_text()),
                // A shell deadline is as inconclusive as the controller's.
                Err(Ended::DeadlinePassed) => (last, STARTUP_TIMED_OUT.to_owned()),
            }
        };
        settings.login_on = on;
        settings.login_line = line;
        true
    }

    pub(crate) fn login_deadline(&mut self, id: RequestId, effects: &mut Vec<Effect>) -> bool {
        let Some(change) = self.settings.login_change.take() else {
            return false;
        };
        if change.id != id {
            self.settings.login_change = Some(change);
            return false;
        }
        effects.push(Effect::Cancel(id));
        self.settings.login_sensitive = true;
        // The change was never confirmed: show the confirmed state again.
        self.settings.login_on = self
            .settings
            .login_before
            .take()
            .unwrap_or(self.settings.login_on);
        self.settings.login_line = STARTUP_TIMED_OUT.to_owned();
        true
    }

    pub(crate) fn login_read(
        &mut self,
        shown: Option<LoginItemShown>,
        runner_missing: bool,
    ) -> bool {
        // A read asked before a toggle reports the state before it.
        if self.settings.login_change.is_some() || self.settings.login_read_stale {
            return false;
        }
        let settings = &mut self.settings;
        settings.runner_missing = runner_missing;
        if runner_missing {
            settings.login_sensitive = false;
            settings.login_line = STARTUP_NO_RUNNER.to_owned();
            return true;
        }
        settings.login_sensitive = true;
        match shown {
            Some(shown) => {
                settings.login_on = shown.on;
                settings.login_line = if shown.legacy {
                    STARTUP_LEGACY.to_owned()
                } else {
                    String::new()
                };
            }
            None => {
                settings.login_line = STARTUP_UNREAD.to_owned();
            }
        }
        true
    }

    pub(crate) fn shortcut_changed(&mut self, state: ShortcutState) -> bool {
        let settings = &mut self.settings;
        match state {
            ShortcutState::Bound(bindings) | ShortcutState::Changed(bindings) => {
                settings.binding = Binding::Bound;
                settings.bindings = bindings;
            }
            ShortcutState::Failed(detail) => {
                settings.binding = Binding::NotBound { detail };
                settings.bindings = Bindings::default();
                settings.rebinding = false;
                settings.status_line.clear();
            }
            ShortcutState::Rebinding(bindings) => {
                settings.binding = Binding::Bound;
                settings.bindings = bindings;
                settings.rebinding = true;
                settings.status_line = REBINDING.to_owned();
            }
            ShortcutState::RebindEnded(message) => {
                settings.rebinding = false;
                settings.status_line = rebind_text(message.as_deref());
            }
            ShortcutState::ConfigureAnswered(message) => {
                settings.status_line = change_text(message.as_deref());
            }
        }
        true
    }

    pub(crate) fn retry_pressed(&mut self, effects: &mut Vec<Effect>) -> bool {
        if !matches!(self.settings.binding, Binding::NotBound { .. }) {
            return false;
        }
        self.settings.binding = Binding::Attempting;
        effects.push(Effect::BindShortcuts);
        true
    }

    pub(crate) fn configure_pressed(&mut self, effects: &mut Vec<Effect>) -> bool {
        if self.settings.binding == Binding::Bound {
            effects.push(Effect::ConfigureShortcuts);
        } else {
            self.settings.status_line = CHANGE_UNBOUND.to_owned();
        }
        true
    }
}
