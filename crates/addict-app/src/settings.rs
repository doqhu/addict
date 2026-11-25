//! The Settings window, reachable from the main window and the tray.
//!
//! It tells what the application is set to: the triggers of both shortcuts,
//! which the portal owns, with a button that asks the portal for its dialog
//! to change them; on Hyprland, whose configuration binds the keys, each row
//! names what to bind and the button copies the lines for that configuration.
//! Then the DeepL key, with a masked field to enter one, the
//! file it is saved in and the state that file is in. The window writes the
//! key, on the runner, and never reads it back: the field starts empty and
//! is cleared once the key is saved. In the General section, a line says how
//! much the on-disk cache of the answers holds, with a button that clears it;
//! the cache is read and cleared on the runner. The Start at login switch asks the
//! Background portal, on the runner, and shows only what the portal last
//! confirmed: the portal cannot be asked, so that state is saved by addict, and
//! showing the window calls no portal. The texts are plain functions of the
//! state, so that they can be tested without a display.

use std::{cell::Cell, future::Future, path::Path, rc::Rc, time::Duration};

use addict_core::APP_ID;
use addict_core::failure::login_item::StartupFailure;
use addict_core::translation::{English, Source};
use addict_deepl::key::{self, KeyError};
use addict_engine::{view::failure_reason, work::Outcome};
use addict_platform::{
    desktop::LoginEntry,
    shortcut::{Bindings, KeyOwner, SCREENSHOT_ID, SHORTCUT_ID},
};
use gtk::{glib, prelude::*};

use crate::{cache::Footprint, file_import, view::Binding};

/// How long reading the state of the key file may take.
pub const KEY_CHECK_LIMIT: Duration = Duration::from_secs(2);

/// How long reading the size of the cache, or clearing it, may take.
pub const CACHE_LIMIT: Duration = Duration::from_secs(5);

/// How long reading the saved startup state may take.
pub const STARTUP_READ_LIMIT: Duration = Duration::from_secs(2);

/// What the Startup section shows when the window opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupShown {
    /// Whether the switch is on.
    pub on: bool,
    /// Whether an older build's login entry is what makes it on.
    pub legacy: bool,
}

/// What to show for the `saved` state and the login entry: the saved state
/// when there is one. With none, only a legacy entry makes the switch on: it is
/// active, and nothing changes until the user acts. `None` for `entry` means it
/// could not be read, as in a sandbox or for a foreign file.
pub fn startup_shown(saved: Option<bool>, entry: Option<LoginEntry>) -> StartupShown {
    match saved {
        Some(on) => StartupShown { on, legacy: false },
        None => {
            let legacy = entry == Some(LoginEntry::Legacy);
            StartupShown { on: legacy, legacy }
        }
    }
}

/// The line under a switch that a legacy entry keeps on.
pub const STARTUP_LEGACY: &str = "An older login entry is active and may open the window at \
     login. Turn this off and on again to replace it.";

/// The line when a change passed its limit, which covers the whole job: the
/// portal may have acted already, or may still.
pub const STARTUP_TIMED_OUT: &str = "The Background portal did not answer in time, and the \
     change may still take effect. Turn this off and on again to repair it.";

/// The line when the state could not be read in time.
pub const STARTUP_UNREAD: &str = "The saved startup state could not be read.";

/// The line when the runner of the startup jobs is gone.
pub const STARTUP_NO_RUNNER: &str = "Startup cannot be changed: the runner is not available.";

/// How a change of the switch ended. It says what the switch then shows; a
/// state saved or not is part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupEnd {
    /// The portal confirmed it and it is saved.
    Done(bool),
    /// The portal was not asked: why, naming a path if there is one.
    NotAsked(String),
    /// The portal did not do it; nothing is saved.
    Failed(StartupFailure),
    /// Enabled, the save failed, and the portal took it back.
    Revoked { save: String },
    /// Enabled, the save failed, and the portal did not take it back.
    NotRevoked {
        save: String,
        revoke: StartupFailure,
    },
    /// Disabled, but the save failed.
    DisabledUnsaved { save: String },
}

/// One change of the switch, on the runner. `check` runs first, and `ask` and
/// `save` only after it passed; with `enable`, a failed save has the portal
/// asked to disable again, as an unsaved state would not be shown as on.
pub async fn change_startup<A, F>(
    enable: bool,
    check: impl FnOnce() -> Result<(), String>,
    ask: A,
    save: impl FnOnce(bool) -> Result<(), String>,
) -> StartupEnd
where
    A: Fn(bool) -> F,
    F: Future<Output = Result<(), StartupFailure>>,
{
    if let Err(reason) = check() {
        return StartupEnd::NotAsked(reason);
    }
    if let Err(failure) = ask(enable).await {
        return StartupEnd::Failed(failure);
    }
    match (save(enable), enable) {
        (Ok(()), _) => StartupEnd::Done(enable),
        (Err(save), false) => StartupEnd::DisabledUnsaved { save },
        (Err(save), true) => match ask(false).await {
            Ok(()) => StartupEnd::Revoked { save },
            Err(revoke) => StartupEnd::NotRevoked { save, revoke },
        },
    }
}

/// The line for a failure of the portal.
fn startup_failure_text(failure: &StartupFailure) -> String {
    format!("Startup was not changed: {}.", failure_reason(failure))
}

/// What the switch shows, and the line under it, after a change that started
/// from the confirmed state `last`. Never on without a confirmation.
pub fn startup_end_text(end: &StartupEnd, last: bool) -> (bool, String) {
    match end {
        StartupEnd::Done(on) => (*on, String::new()),
        StartupEnd::NotAsked(reason) => (last, format!("Startup was not changed: {reason}.")),
        StartupEnd::Failed(failure) => (last, startup_failure_text(failure)),
        StartupEnd::Revoked { save } => (
            false,
            format!("Startup was turned back off, as its state could not be saved: {save}."),
        ),
        StartupEnd::NotRevoked { save, revoke } => (
            false,
            format!(
                "Startup could not be saved ({save}) and could not be turned back off ({}): \
                 addict may still start at login. Turn this on and off again to repair it.",
                failure_reason(revoke)
            ),
        ),
        StartupEnd::DisabledUnsaved { save } => (
            false,
            format!(
                "Startup is off, but this could not be saved: {save}. This switch may show on \
                 next time."
            ),
        ),
    }
}

/// [`startup_end_text`] for a job's outcome. `None` for a job given up, which
/// says nothing. A limit that passed is no confirmation either way.
pub fn startup_outcome(outcome: &Outcome<StartupEnd>, last: bool) -> Option<(bool, String)> {
    match outcome {
        Outcome::Finished(end) => Some(startup_end_text(end, last)),
        Outcome::TimedOut => Some((last, STARTUP_TIMED_OUT.to_owned())),
        Outcome::Panicked => Some((
            last,
            "Startup was not changed: the request did not run.".to_owned(),
        )),
        Outcome::Cancelled => None,
    }
}

/// Where the size of the cache stands, as the window says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    /// The size is being read.
    Checking,
    /// The cache holds this.
    Read(Footprint),
    /// The size could not be read: no runner, a timeout or a panic.
    Unknown,
}

/// The line of the size of the cache. No wildcard arm: a new state does not
/// compile until it is given its line.
pub fn cache_text(state: CacheState) -> String {
    match state {
        CacheState::Checking => "Checking the cache…".to_owned(),
        CacheState::Read(footprint) if footprint.rows == 0 => "The cache is empty.".to_owned(),
        CacheState::Read(footprint) => format!("The cache holds {}.", size_text(footprint.bytes)),
        CacheState::Unknown => "The size of the cache could not be read.".to_owned(),
    }
}

/// `bytes` in decimal units, one decimal from a kilobyte up: `1.2 MB`.
pub fn size_text(bytes: u64) -> String {
    const KB: u64 = 1_000;
    const MB: u64 = 1_000_000;
    // Integer arithmetic: tenths, rounded half up.
    let tenths = |unit: u64| (bytes * 10 + unit / 2) / unit;
    if bytes < KB {
        format!("{bytes} bytes")
    } else if tenths(KB) < 10_000 {
        let tenths = tenths(KB);
        format!("{}.{} kB", tenths / 10, tenths % 10)
    } else {
        let tenths = tenths(MB);
        format!("{}.{} MB", tenths / 10, tenths % 10)
    }
}

/// The line after a clear of the cache: nothing on success, else why the
/// cache was not cleared.
pub fn clear_text(result: Result<(), &str>) -> String {
    match result {
        Ok(()) => String::new(),
        Err(reason) => format!("The cache was not cleared: {reason}"),
    }
}

/// Where the DeepL key stands, as the window says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// There is no directory to look in: no absolute `XDG_CONFIG_HOME` and no
    /// `HOME`.
    NoDirectory,
    /// The file holds a key of the Free plan.
    Free,
    /// The file holds a key of the Pro plan.
    Pro,
    /// The file gave no key.
    Refused(KeyError),
    /// The state could not be read in time.
    Unknown,
}

impl KeyState {
    /// Reads the state of the file at `path`. This opens the file, so it runs
    /// on the runner. The key is dropped here: only its plan is kept.
    pub fn of(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::NoDirectory;
        };
        match key::read(path) {
            Ok(key) if key.is_free() => Self::Free,
            Ok(_) => Self::Pro,
            Err(error) => Self::Refused(error),
        }
    }
}

/// The line of a key state. No wildcard arm: a new state does not compile
/// until it is given its line.
pub fn key_text(state: Option<KeyState>) -> &'static str {
    match state {
        None => "Checking…",
        Some(KeyState::NoDirectory) => {
            "No place for the key file: set an absolute XDG_CONFIG_HOME or HOME."
        }
        Some(KeyState::Free) => "A DeepL key of the Free plan is set.",
        Some(KeyState::Pro) => "A DeepL key of the Pro plan is set.",
        Some(KeyState::Refused(KeyError::Missing)) => "There is no key file.",
        Some(KeyState::Refused(KeyError::Unreadable)) => "The key file cannot be read.",
        Some(KeyState::Refused(KeyError::TooOpen)) => {
            "The key file must have permissions 0600; it was not read."
        }
        Some(KeyState::Refused(KeyError::Empty)) => "The key file holds no key.",
        Some(KeyState::Unknown) => "The key file was not checked in time.",
    }
}

/// What the window says about the shortcut: the trigger the portal bound, or
/// where the attempt stands.
pub fn trigger_text(binding: &Binding, trigger: Option<&str>) -> String {
    match (binding, trigger) {
        (Binding::Bound, Some(trigger)) if !trigger.trim().is_empty() => trigger.to_owned(),
        (Binding::Bound, _) => "Unassigned. Choose a key in Configure shortcuts…".to_owned(),
        (Binding::Attempting, _) => "Binding the shortcut…".to_owned(),
        (Binding::NotBound { detail }, _) => format!("Not bound: {detail}"),
    }
}

/// The line after the portal answered the request for its dialog.
pub fn change_text(answer: Option<&str>) -> String {
    match answer {
        None => {
            "The system's dialog configures both shortcuts. Assignments update here immediately."
                .to_owned()
        }
        Some(message) => format!("The shortcut cannot be changed from here: {message}"),
    }
}

/// Configure shortcuts needs a session, and no new binding running.
fn change_sensitive(bound: bool, rebinding: bool) -> bool {
    bound && !rebinding
}

/// The line while a new binding has the system's dialog open.
pub const REBINDING: &str = "Choose the keys in the system's dialog.";

/// The line when the window is shown again: empty, unless a new binding is
/// running, which is why the button is off.
fn reopened_status(rebinding: bool) -> &'static str {
    if rebinding { REBINDING } else { "" }
}

/// The line after a new binding ended: what the worker said, or that the keys
/// that were chosen are in place.
pub fn rebind_text(message: Option<&str>) -> String {
    message.map_or_else(|| "The new keys are set.".to_owned(), str::to_owned)
}

/// The line after a save of the key: nothing on success, else why the key
/// was not saved. The reason never holds the key.
pub fn save_text(result: Result<(), &str>) -> String {
    match result {
        Ok(()) => String::new(),
        Err(reason) => format!("The key was not saved: {reason}."),
    }
}

/// Why a key is not saved when the environment names no configuration
/// directory.
pub const NO_KEY_PLACE: &str =
    "there is no place for the key file: set an absolute XDG_CONFIG_HOME or HOME";

/// The key's line when the write of the key file did not answer in time:
/// it may still have been written, so nothing is claimed.
pub const SAVE_UNANSWERED: &str =
    "The save did not answer in time; the line above shows what the key file holds.";

/// The hint line that names where the key is saved.
pub fn saved_in_text(key_file: Option<&Path>) -> String {
    key_file.map_or_else(
        || "There is no place to save the key.".to_owned(),
        |path| format!("Saved in {}", path.display()),
    )
}

/// The line for a request made before a usable session exists.
pub const CHANGE_UNBOUND: &str =
    "Shortcut configuration needs a portal session. Retry the connection first.";

/// The line beside Configure shortcuts.
const CONFIGURE_HINT: &str = "The desktop\u{2019}s dialog sets both.";

/// The line beside Copy Hyprland lines.
const HYPRLAND_HINT: &str = "Hyprland sets both in hyprland.lua.";

/// The line once the Hyprland lines are on the clipboard.
pub const HYPRLAND_COPIED: &str =
    "Copied. Paste the lines into ~/.config/hypr/hyprland.lua; Hyprland reloads it on save.";

/// What Hyprland's `global` dispatcher names for the shortcut `id`.
pub fn hyprland_target(id: &str) -> String {
    format!("{APP_ID}:{id}")
}

/// The line and badge of a row on Hyprland, whose portal never describes a
/// key: a bound row names what the configuration must bind, as it cannot be
/// known whether it does. `None` for the rows that read as anywhere else.
pub fn hyprland_texts(
    binding: &Binding,
    trigger: Option<&str>,
    id: &str,
) -> Option<(String, (&'static str, &'static str))> {
    let described = trigger.is_some_and(|trigger| !trigger.trim().is_empty());
    (matches!(binding, Binding::Bound) && !described).then(|| {
        (
            format!("Bind in Hyprland: {}", hyprland_target(id)),
            ("In Hyprland", "pending"),
        )
    })
}

/// The lines for `hyprland.lua`: Super+Q and Super+Shift+Q for both actions,
/// and a rule that floats and centres every window of the application at the
/// size it asks for.
pub fn hyprland_snippet() -> String {
    // A Lua string holding the regular expression of the class: `\\.` is `\.`.
    let class = APP_ID.replace('.', "\\\\.");
    let clipboard = hyprland_target(SHORTCUT_ID);
    let screenshot = hyprland_target(SCREENSHOT_ID);
    format!(
        "hl.bind(\"SUPER + Q\", hl.dsp.global(\"{clipboard}\"), \
         {{ description = \"Addict: translate selection or clipboard\" }})\n\
         hl.bind(\"SUPER + SHIFT + Q\", hl.dsp.global(\"{screenshot}\"), \
         {{ description = \"Addict: capture a screen region (OCR)\" }})\n\
         hl.window_rule({{ name = \"addict\", match = {{ class = \"^{class}$\" }}, \
         float = true, center = true }})\n"
    )
}

/// The keys of a trigger the portal described, one cap each: GNOME's
/// `Press <Super>q` as an accelerator, or a label such as `Super+Q`;
/// nothing when the description is neither (another sentence, an empty
/// part), which is then shown as it is. `Ctrl++` is Ctrl and `+`.
pub fn trigger_keys(trigger: &str) -> Option<Vec<String>> {
    let trigger = trigger.trim();
    let trigger = trigger.strip_prefix("Press ").unwrap_or(trigger).trim();
    if trigger.is_empty() || trigger.contains(char::is_whitespace) {
        return None;
    }
    if trigger.starts_with('<') {
        return accelerator_keys(trigger);
    }
    let (rest, plus) = match trigger.strip_suffix("++") {
        Some(rest) => (rest, true),
        None => (trigger, false),
    };
    let mut keys: Vec<String> = if rest.is_empty() {
        Vec::new()
    } else {
        rest.split('+').map(str::to_owned).collect()
    };
    if plus {
        keys.push("+".to_owned());
    }
    if keys.is_empty() || keys.iter().any(String::is_empty) {
        return None;
    }
    Some(keys)
}

/// The caps of an accelerator in GTK's form, `<Control><Alt>t`: each
/// modifier by the name on its key, then the key, a letter in capitals.
fn accelerator_keys(accelerator: &str) -> Option<Vec<String>> {
    let mut keys = Vec::new();
    let mut rest = accelerator;
    while let Some(after) = rest.strip_prefix('<') {
        let (modifier, tail) = after.split_once('>')?;
        keys.push(
            match modifier {
                "Control" | "Ctrl" | "Primary" => "Ctrl",
                "" => return None,
                other => other,
            }
            .to_owned(),
        );
        rest = tail;
    }
    let mut chars = rest.chars();
    let first = chars.next()?;
    keys.push(first.to_uppercase().chain(chars).collect());
    Some(keys)
}

/// The badge beside the trigger, and its class.
pub fn badge(binding: &Binding, trigger: Option<&str>) -> (&'static str, &'static str) {
    match binding {
        Binding::Bound if trigger.is_some_and(|trigger| !trigger.trim().is_empty()) => {
            ("Assigned", "good")
        }
        Binding::Bound => ("Unassigned", "pending"),
        Binding::Attempting => ("Binding…", "pending"),
        Binding::NotBound { .. } => ("Not bound", "bad"),
    }
}

/// The class of the block of a key state: good when a key is set, pending
/// while it is checked, bad otherwise. No wildcard arm, as in `key_text`.
pub fn key_tone(state: Option<KeyState>) -> &'static str {
    match state {
        None => "pending",
        Some(KeyState::Free | KeyState::Pro) => "good",
        Some(KeyState::NoDirectory | KeyState::Refused(_) | KeyState::Unknown) => "bad",
    }
}

/// What the empty key field asks for: a set key is replaced, not entered.
pub fn key_placeholder(state: Option<KeyState>) -> &'static str {
    match key_tone(state) {
        "good" => "Paste a new key to replace it",
        _ => "Paste a DeepL key",
    }
}

/// The classes a tone replaces on a widget.
const TONES: [&str; 3] = ["good", "pending", "bad"];

fn set_tone(widget: &impl IsA<gtk::Widget>, tone: &str) {
    for other in TONES {
        widget.remove_css_class(other);
    }
    widget.add_css_class(tone);
}

/// One independently assigned action in the portal session.
struct ShortcutRow {
    /// The shortcut's ID in generation 0, the one Hyprland binds.
    id: &'static str,
    owner: KeyOwner,
    root: gtk::Box,
    keys: gtk::Box,
    trigger: gtk::Label,
    badge: gtk::Box,
    badge_text: gtk::Label,
}

impl ShortcutRow {
    fn new(name: &str, id: &'static str, owner: KeyOwner) -> Self {
        let keys = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        // Without key caps, the line of what stands in their way, below.
        let trigger = label("", &["trigger", "hint"]);
        // The badge: a dot in the text's colour, then its word. Shown only
        // when something is wrong: the key caps say the rest.
        let badge_text = gtk::Label::new(None);
        let badge = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk::Align::Center)
            .css_classes(["badge"])
            .build();
        badge.append(
            &gtk::Box::builder()
                .valign(gtk::Align::Center)
                .css_classes(["dot"])
                .build(),
        );
        badge.append(&badge_text);
        let name = label(name, &[]);
        name.set_hexpand(true);
        name.set_valign(gtk::Align::Center);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.append(&name);
        line.append(&keys);
        line.append(&badge);
        let root = row(&[line.upcast_ref(), trigger.upcast_ref()]);
        Self {
            id,
            owner,
            root,
            keys,
            trigger,
            badge,
            badge_text,
        }
    }

    fn render(&self, binding: &Binding, trigger: Option<&str>) {
        while let Some(cap) = self.keys.first_child() {
            self.keys.remove(&cap);
        }
        let bound = matches!(binding, Binding::Bound);
        let caps = trigger.filter(|_| bound).and_then(trigger_keys);
        if let Some(caps) = &caps {
            for (index, key) in caps.iter().enumerate() {
                if index > 0 {
                    self.keys.append(&label("+", &["hint"]));
                }
                self.keys.append(&label(key, &["keycap"]));
            }
        }
        self.keys.set_visible(caps.is_some());
        let hyprland = (self.owner == KeyOwner::Hyprland)
            .then(|| hyprland_texts(binding, trigger, self.id))
            .flatten();
        let (line, (text, tone)) = match hyprland {
            Some(texts) => texts,
            None if caps.is_some() => (String::new(), badge(binding, trigger)),
            None => (trigger_text(binding, trigger), badge(binding, trigger)),
        };
        say_in(&self.trigger, &line);
        set_tone(&self.trigger, tone);
        self.badge_text.set_label(text);
        self.badge.set_visible(tone != "good");
        set_tone(&self.badge, tone);
    }
}

/// The window and the labels that change.
pub struct Settings {
    window: gtk::Window,
    clipboard: ShortcutRow,
    screenshot: ShortcutRow,
    /// The block of the key's state: its icon and its line.
    key_block: gtk::Box,
    key_icon: gtk::Image,
    key_state: gtk::Label,
    key_entry: gtk::PasswordEntry,
    change: gtk::Button,
    /// Who sets the keys: on Hyprland the button copies lines and is always on.
    owner: KeyOwner,
    status: gtk::Label,
    /// The line of the key's saves, apart from the shortcut's.
    key_status: gtk::Label,
    /// The line of the size of the cache.
    cache_size: gtk::Label,
    /// The line of the clears of the cache.
    cache_status: gtk::Label,
    /// The Start at login switch.
    startup: gtk::Switch,
    /// Set while the code, not the user, moves the switch.
    startup_rendering: Rc<Cell<bool>>,
    /// Whether the shortcuts are bound, as last rendered.
    bound: Cell<bool>,
    /// A new binding is running, so Configure shortcuts would do nothing.
    rebinding: Cell<bool>,
    /// The line of the startup state.
    startup_status: gtk::Label,
    /// The Translate sentence at once switch.
    at_once: gtk::Switch,
    /// Set while the code, not the user, moves the switch.
    at_once_rendering: Rc<Cell<bool>>,
    /// The My language choice.
    my_language: gtk::DropDown,
    /// Set while the code, not the user, moves the choice.
    my_language_rendering: Rc<Cell<bool>>,
    /// The English spelling row, shown only while English is my language.
    english_row: gtk::Box,
    /// The English spelling choice.
    english: gtk::DropDown,
    /// Set while the code, not the user, moves the choice.
    english_rendering: Rc<Cell<bool>>,
    download: gtk::Button,
    download_status: gtk::Label,
}

/// The name of a language in the my-language row. No wildcard arm: a new
/// language does not compile until it is named.
pub fn my_language_name(language: Source) -> &'static str {
    match language {
        Source::En => "English",
        Source::Es => "Spanish",
        Source::Ca => "Catalan",
        Source::Ru => "Russian",
    }
}

/// The names of the my-language row, in the order of its choices.
pub fn my_language_names() -> Vec<&'static str> {
    Source::ALL.into_iter().map(my_language_name).collect()
}

/// The name of a spelling of English in the English spelling row. No
/// wildcard arm: a new variant does not compile until it is named.
pub fn english_name(english: English) -> &'static str {
    match english {
        English::Us => "US",
        English::Gb => "British",
    }
}

/// The name of the switch that translates a copied sentence at once.
const AT_ONCE_NAME: &str = "Translate sentence at once";

/// The line under it: what the switch does.
const AT_ONCE_HINT: &str = "A sentence read by the shortcut is translated online without a \
     click on a word.";

/// The names of the English spelling row, in the order of its choices.
pub fn english_names() -> Vec<&'static str> {
    English::ALL.into_iter().map(english_name).collect()
}

pub(crate) fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .css_classes(classes.to_vec())
        .build()
}

/// A section: its label above a card holding `rows`.
fn section(name: &str, rows: &[&gtk::Widget]) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .css_classes(["settings-card"])
        .build();
    for row in rows {
        card.append(*row);
    }
    titled(name, &card)
}

/// A section whose card is a list of [`row`]s, parted by hairlines.
pub(crate) fn list_section(name: &str, rows: &[&gtk::Box]) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["settings-list"])
        .build();
    for row in rows {
        card.append(*row);
    }
    titled(name, &card)
}

/// One row of a list: its `lines`, one under the other.
pub(crate) fn row(lines: &[&gtk::Widget]) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .css_classes(["settings-row"])
        .build();
    for line in lines {
        row.append(*line);
    }
    row
}

/// `card` under its label `name`.
fn titled(name: &str, card: &gtk::Box) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    // Not wrapped: the label is measured before it is set in capitals,
    // and a wrapped one breaks where the capitals do not fit.
    let name = label(name, &["section-label", "settings-section"]);
    name.set_wrap(false);
    section.append(&name);
    section.append(card);
    section
}

/// A line that hides itself while it is empty.
fn say_in(line: &gtk::Label, text: &str) {
    line.set_label(text);
    line.set_visible(!text.is_empty());
}

impl Settings {
    /// Builds the window, hidden, above `parent`. `owner` says who sets the
    /// keys: on Hyprland the button copies the lines for its configuration
    /// instead. `on_change` runs when Configure shortcuts is pressed; `on_save` runs with the text of the key
    /// field, trimmed and not empty, when Save key or Enter in the field is
    /// pressed; `on_clear` runs when Clear cache is pressed; `on_startup` runs
    /// with the state asked for when the user moves the Start at login switch;
    /// `on_my_language` runs with the language the user chooses in My language;
    /// `on_english` with the spelling the user chooses in English spelling;
    /// `on_at_once` with the state the user moves Translate sentence at once
    /// to.
    /// `on_imported` refreshes resident dictionary states before success is shown.
    #[allow(clippy::too_many_arguments)]
    pub fn new<F: Future<Output = Result<(), String>> + 'static>(
        parent: &impl IsA<gtk::Window>,
        key_file: Option<&Path>,
        owner: KeyOwner,
        on_change: impl Fn() + 'static,
        on_save: impl Fn(String) + 'static,
        on_clear: impl Fn() + 'static,
        on_startup: impl Fn(bool) + 'static,
        on_my_language: impl Fn(Source) + 'static,
        on_english: impl Fn(English) + 'static,
        on_at_once: impl Fn(bool) + 'static,
        on_imported: impl Fn() -> F + 'static,
        on_download: impl Fn() + 'static,
    ) -> Self {
        let clipboard = ShortcutRow::new("Selection or clipboard", SHORTCUT_ID, owner);
        let screenshot = ShortcutRow::new("Screen region (OCR)", SCREENSHOT_ID, owner);
        let hyprland = owner == KeyOwner::Hyprland;
        let change = gtk::Button::builder()
            .label(if hyprland {
                "Copy Hyprland lines"
            } else {
                "Configure shortcuts…"
            })
            .valign(gtk::Align::Center)
            .css_classes(["settings-button"])
            .build();
        let configure_hint = label(
            if hyprland {
                HYPRLAND_HINT
            } else {
                CONFIGURE_HINT
            },
            &["hint"],
        );
        configure_hint.set_hexpand(true);
        configure_hint.set_valign(gtk::Align::Center);
        let configure_line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        configure_line.append(&configure_hint);
        configure_line.append(&change);
        let status = label("", &["hint"]);
        status.set_visible(false);
        if hyprland {
            let copied = status.clone();
            change.connect_clicked(move |button| {
                button.clipboard().set_text(&hyprland_snippet());
                say_in(&copied, HYPRLAND_COPIED);
            });
        } else {
            change.connect_clicked(move |_| on_change());
        }

        let key_icon = gtk::Image::builder().valign(gtk::Align::Center).build();
        let key_state = label(key_text(None), &["key-state"]);
        key_state.set_hexpand(true);
        key_state.set_valign(gtk::Align::Center);
        let key_block = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .css_classes(["key-block"])
            .build();
        key_block.append(&key_icon);
        key_block.append(&key_state);
        let key_entry = gtk::PasswordEntry::builder()
            .show_peek_icon(true)
            .placeholder_text(key_placeholder(None))
            .hexpand(true)
            .css_classes(["key"])
            .build();
        let save = gtk::Button::builder()
            .label("Save key")
            .sensitive(false)
            .css_classes(["save"])
            .build();
        let key_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        key_row.append(&key_entry);
        key_row.append(&save);
        let saving = save.downgrade();
        key_entry.connect_changed(move |entry| {
            if let Some(save) = saving.upgrade() {
                save.set_sensitive(!entry.text().trim().is_empty());
            }
        });
        let on_save = std::rc::Rc::new(move |entry: &gtk::PasswordEntry| {
            let text = entry.text();
            let text = text.trim();
            if !text.is_empty() {
                on_save(text.to_owned());
            }
        });
        let pressed = on_save.clone();
        let entry = key_entry.downgrade();
        save.connect_clicked(move |_| {
            if let Some(entry) = entry.upgrade() {
                pressed(&entry);
            }
        });
        key_entry.connect_activate(move |entry| on_save(entry));
        let key_status = label("", &["hint"]);
        key_status.set_visible(false);
        // The place of the file: the path in monospace, and selectable, so
        // that it can be copied.
        let key_file_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        match key_file {
            Some(path) => {
                key_file_row.append(&label("Saved in", &["hint"]));
                let path = label(&path.display().to_string(), &["path"]);
                path.set_selectable(true);
                path.set_wrap_mode(gtk::pango::WrapMode::Char);
                path.set_hexpand(true);
                key_file_row.append(&path);
            }
            None => key_file_row.append(&label(&saved_in_text(None), &["hint"])),
        }
        // The cache of the answers: its size, and the button that clears it.
        let cache_size = label(&cache_text(CacheState::Checking), &["hint"]);
        cache_size.set_hexpand(true);
        cache_size.set_valign(gtk::Align::Center);
        let clear = gtk::Button::builder()
            .label("Clear cache")
            .valign(gtk::Align::Center)
            .css_classes(["settings-button"])
            .build();
        clear.connect_clicked(move |_| on_clear());
        let cache_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        cache_row.append(&cache_size);
        cache_row.append(&clear);
        let cache_status = label("", &["hint"]);
        cache_status.set_visible(false);

        // Stopping `state-set` keeps the state off until the portal confirms.
        let startup = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .sensitive(false)
            .build();
        startup.update_property(&[gtk::accessible::Property::Label("Start at login")]);
        let startup_rendering = Rc::new(Cell::new(false));
        let rendering = startup_rendering.clone();
        startup.connect_state_set(move |_, wanted| {
            if !rendering.get() {
                on_startup(wanted);
            }
            glib::Propagation::Stop
        });
        let startup_name = label("Start at login", &[]);
        startup_name.set_hexpand(true);
        startup_name.set_valign(gtk::Align::Center);
        let startup_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        startup_row.append(&startup_name);
        startup_row.append(&startup);
        let startup_status = label("", &["hint"]);
        startup_status.set_visible(false);

        // Before the handler is connected: the start is not a choice.
        let at_once = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .active(addict_engine::at_once::DEFAULT)
            .build();
        at_once.update_property(&[gtk::accessible::Property::Label(AT_ONCE_NAME)]);
        let at_once_rendering = Rc::new(Cell::new(false));
        let rendering = at_once_rendering.clone();
        at_once.connect_active_notify(move |switch| {
            if !rendering.get() {
                on_at_once(switch.is_active());
            }
        });
        let at_once_name = label(AT_ONCE_NAME, &[]);
        at_once_name.set_hexpand(true);
        at_once_name.set_valign(gtk::Align::Center);
        let at_once_line = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        at_once_line.append(&at_once_name);
        at_once_line.append(&at_once);
        let at_once_hint = label(AT_ONCE_HINT, &["hint"]);

        let my_language = gtk::DropDown::from_strings(&my_language_names());
        my_language.set_valign(gtk::Align::Center);
        my_language.update_property(&[gtk::accessible::Property::Label("My language")]);
        // Before the handler is connected: the start is not a choice.
        my_language.set_selected(
            Source::ALL
                .iter()
                .position(|language| *language == Source::Ru)
                .map_or(0, |index| index as u32),
        );
        let my_language_rendering = Rc::new(Cell::new(false));
        let rendering = my_language_rendering.clone();
        my_language.connect_selected_notify(move |dropdown| {
            if !rendering.get()
                && let Some(chosen) = Source::ALL.get(dropdown.selected() as usize)
            {
                on_my_language(*chosen);
            }
        });
        let my_language_name = label("My language", &[]);
        my_language_name.set_hexpand(true);
        my_language_name.set_valign(gtk::Align::Center);
        let my_language_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        my_language_row.append(&my_language_name);
        my_language_row.append(&my_language);

        let english = gtk::DropDown::from_strings(&english_names());
        english.set_valign(gtk::Align::Center);
        english.update_property(&[gtk::accessible::Property::Label("English spelling")]);
        // Before the handler is connected: the start is not a choice.
        english.set_selected(
            English::ALL
                .iter()
                .position(|one| *one == English::default())
                .map_or(0, |index| index as u32),
        );
        let english_rendering = Rc::new(Cell::new(false));
        let rendering = english_rendering.clone();
        english.connect_selected_notify(move |dropdown| {
            if !rendering.get()
                && let Some(chosen) = English::ALL.get(dropdown.selected() as usize)
            {
                on_english(*chosen);
            }
        });
        let english_name = label("English spelling", &[]);
        english_name.set_hexpand(true);
        english_name.set_valign(gtk::Align::Center);
        let english_line = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        english_line.append(&english_name);
        english_line.append(&english);
        // The row of the list, not its line, is hidden: a hidden row is no
        // last child, so My language above it keeps no hairline.
        let english_row = row(&[english_line.upcast_ref()]);
        english_row.set_visible(false);

        let key_note = label("The key is never shown back here.", &["hint"]);
        let import_file = file_import::Row::new();
        let download = gtk::Button::builder()
            .label("Download dictionaries")
            .halign(gtk::Align::Start)
            .css_classes(["settings-button"])
            .build();
        download.connect_clicked(move |_| on_download());
        let download_status = label("", &["hint"]);
        download_status.set_visible(false);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .build();
        content.append(&list_section(
            "Shortcuts",
            &[
                &clipboard.root,
                &screenshot.root,
                &row(&[configure_line.upcast_ref(), status.upcast_ref()]),
            ],
        ));
        content.append(&section(
            "DeepL key",
            &[
                key_block.upcast_ref(),
                key_row.upcast_ref(),
                key_status.upcast_ref(),
                key_file_row.upcast_ref(),
                key_note.upcast_ref(),
            ],
        ));
        content.append(&list_section(
            "Dictionaries",
            &[
                &row(&[download.upcast_ref(), download_status.upcast_ref()]),
                &row(&[import_file.root.upcast_ref()]),
            ],
        ));
        content.append(&list_section(
            "General",
            &[
                &row(&[cache_row.upcast_ref(), cache_status.upcast_ref()]),
                &row(&[startup_row.upcast_ref(), startup_status.upcast_ref()]),
                &row(&[at_once_line.upcast_ref(), at_once_hint.upcast_ref()]),
                &row(&[my_language_row.upcast_ref()]),
                &english_row,
            ],
        ));

        // Let refusal details grow inside the scroller rather than making
        // lower settings unreachable on a 768-pixel-high desktop.
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(600)
            .child(&content)
            .build();
        let window = gtk::Window::builder()
            .title("Addict settings")
            .transient_for(parent)
            .default_width(520)
            .hide_on_close(true)
            .titlebar(&crate::resident::header_bar("Addict settings"))
            .child(&scroll)
            .css_classes(["addict", "settings"])
            .build();
        import_file.connect(&window, on_imported);
        let escape = gtk::EventControllerKey::new();
        let esc = window.downgrade();
        escape.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                if let Some(window) = esc.upgrade() {
                    window.close();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(escape);
        let settings = Self {
            window,
            clipboard,
            screenshot,
            key_block,
            key_icon,
            key_state,
            key_entry,
            change,
            owner,
            status,
            key_status,
            cache_size,
            cache_status,
            startup,
            startup_rendering,
            bound: Cell::new(false),
            rebinding: Cell::new(false),
            startup_status,
            at_once,
            at_once_rendering,
            my_language,
            my_language_rendering,
            english_row,
            english,
            english_rendering,
            download,
            download_status,
        };
        settings.render_binding(&Binding::Attempting, &Bindings::default());
        settings.render_key(None);
        settings
    }

    pub fn window(&self) -> &gtk::Window {
        &self.window
    }

    /// Disable only while preparing; active progress remains reachable.
    pub fn render_download(&self, busy: bool, text: &str) {
        self.download.set_sensitive(!busy);
        say_in(&self.download_status, text);
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// A session can be configured even when neither action has a key.
    pub fn render_binding(&self, binding: &Binding, bindings: &Bindings) {
        self.clipboard
            .render(binding, bindings.clipboard.as_deref());
        self.screenshot
            .render(binding, bindings.screenshot.as_deref());
        self.bound.set(matches!(binding, Binding::Bound));
        self.render_change();
    }

    /// Whether a new binding is running: Configure shortcuts waits for its end.
    pub fn set_rebinding(&self, rebinding: bool) {
        self.rebinding.set(rebinding);
        self.render_change();
    }

    fn render_change(&self) {
        self.change.set_sensitive(
            self.owner == KeyOwner::Hyprland
                || change_sensitive(self.bound.get(), self.rebinding.get()),
        );
    }

    pub fn render_key(&self, state: Option<KeyState>) {
        self.key_state.set_label(key_text(state));
        self.key_entry
            .set_property("placeholder-text", key_placeholder(state));
        let tone = key_tone(state);
        set_tone(&self.key_block, tone);
        self.key_icon.set_icon_name(Some(match tone {
            "good" => "object-select-symbolic",
            "pending" => "content-loading-symbolic",
            _ => "dialog-warning-symbolic",
        }));
    }

    /// Empties the key field, after the key in it was saved or when the
    /// window is shown again.
    pub fn clear_key_entry(&self) {
        self.key_entry.set_text("");
    }

    /// Clears the line when the window is shown again, keeping what still holds.
    pub fn reset_status(&self) {
        self.say(reopened_status(self.rebinding.get()));
    }

    pub fn say(&self, text: &str) {
        say_in(&self.status, text);
    }

    /// Sets the line of the key's saves.
    pub fn say_key(&self, text: &str) {
        say_in(&self.key_status, text);
    }

    /// Shows the size of the cache.
    pub fn render_cache(&self, state: CacheState) {
        self.cache_size.set_label(&cache_text(state));
    }

    /// Sets the line of the clears of the cache.
    pub fn say_cache(&self, text: &str) {
        say_in(&self.cache_status, text);
    }

    /// Moves the switch to `on`, as confirmed, without asking anything.
    pub fn render_startup(&self, on: bool) {
        self.startup_rendering.set(true);
        self.startup.set_active(on);
        self.startup.set_state(on);
        self.startup_rendering.set(false);
    }

    /// Whether the switch can be moved: not while a request or a read runs.
    pub fn set_startup_sensitive(&self, sensitive: bool) {
        self.startup.set_sensitive(sensitive);
    }

    /// The line under Download dictionaries.
    #[cfg(test)]
    pub(crate) fn download_text(&self) -> String {
        self.download_status.label().into()
    }

    /// The language the My language row shows.
    #[cfg(test)]
    pub(crate) fn shown_my_language(&self) -> Option<Source> {
        Source::ALL
            .get(self.my_language.selected() as usize)
            .copied()
    }

    /// Shows `language` in the My language row without asking anything.
    pub fn render_my_language(&self, language: Source) {
        let Some(index) = Source::ALL.iter().position(|one| *one == language) else {
            return;
        };
        self.my_language_rendering.set(true);
        self.my_language.set_selected(index as u32);
        self.my_language_rendering.set(false);
    }

    /// Shows `english` in the English spelling row without asking anything.
    pub fn render_english(&self, english: English) {
        let Some(index) = English::ALL.iter().position(|one| *one == english) else {
            return;
        };
        self.english_rendering.set(true);
        self.english.set_selected(index as u32);
        self.english_rendering.set(false);
    }

    /// Shows `at_once` in the Translate sentence at once switch without
    /// asking anything.
    pub fn render_at_once(&self, at_once: bool) {
        self.at_once_rendering.set(true);
        self.at_once.set_active(at_once);
        self.at_once_rendering.set(false);
    }

    /// Shows the English spelling row, or hides it: it is shown only while
    /// English is my language.
    pub fn show_english(&self, shown: bool) {
        self.english_row.set_visible(shown);
    }

    /// Whether the English spelling row is shown, and the spelling it shows.
    #[cfg(test)]
    pub(crate) fn english_row(&self) -> (bool, English) {
        let english = English::ALL
            .get(self.english.selected() as usize)
            .copied()
            .unwrap_or_default();
        (self.english_row.get_visible(), english)
    }

    /// Sets the line of the startup state.
    pub fn say_startup(&self, text: &str) {
        say_in(&self.startup_status, text);
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, future::ready};

    use addict_core::failure::login_item::StartupFailure;
    use addict_deepl::key::KeyError;
    use addict_engine::work::Outcome;
    use addict_platform::desktop::LoginEntry;

    use super::{
        CacheState, KeyState, NO_KEY_PLACE, REBINDING, StartupEnd, StartupShown, badge, cache_text,
        change_sensitive, change_startup, change_text, clear_text, key_placeholder, key_text,
        key_tone, rebind_text, reopened_status, save_text, saved_in_text, size_text,
        startup_end_text, startup_outcome, startup_shown, trigger_keys, trigger_text,
    };
    use crate::{cache::Footprint, view::Binding};

    /// Polls a job whose fakes never wait.
    fn finish<F: std::future::Future>(job: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(job)
    }

    /// One change with a portal that gives `answers` in turn and a save that
    /// gives `saved`; what happened comes with its end.
    fn change(
        enable: bool,
        checked: Result<(), String>,
        answers: Vec<Result<(), StartupFailure>>,
        saved: Result<(), String>,
    ) -> (StartupEnd, Vec<String>) {
        let log = RefCell::new(Vec::new());
        let answers = RefCell::new(answers.into_iter());
        let end = finish(change_startup(
            enable,
            || {
                log.borrow_mut().push("check".to_owned());
                checked
            },
            |asked| {
                log.borrow_mut().push(format!("ask {asked}"));
                ready(answers.borrow_mut().next().expect("an answer"))
            },
            |state| {
                log.borrow_mut().push(format!("save {state}"));
                saved
            },
        ));
        (end, log.into_inner())
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn dictionary_download_action_shows_preparation_feedback_and_keeps_retry_available() {
        use gtk::prelude::*;
        gtk::init().expect("a display");
        let parent = gtk::Window::new();
        let presses = std::rc::Rc::new(std::cell::Cell::new(0));
        let pressed = presses.clone();
        let settings = super::Settings::new(
            &parent,
            None,
            super::KeyOwner::Portal,
            || {},
            |_| {},
            || {},
            |_| {},
            |_| {},
            |_| {},
            |_| {},
            || async { Ok(()) },
            move || pressed.set(pressed.get() + 1),
        );
        settings.present();
        assert_eq!(
            settings.download.label().as_deref(),
            Some("Download dictionaries")
        );
        settings.download.emit_clicked();
        assert_eq!(presses.get(), 1);
        settings.render_download(true, crate::first_run::FETCHING);
        assert!(!settings.download.is_sensitive());
        assert!(settings.download_status.is_visible());
        assert_eq!(settings.download_status.label(), crate::first_run::FETCHING);
        for status in [
            crate::first_run::nothing_text(super::Source::Ru),
            crate::first_run::recovery_failed_text("cannot read the index manifest: malformed"),
        ] {
            settings.render_download(false, &status);
            assert!(settings.download.is_sensitive());
            assert!(settings.download_status.is_visible());
            assert_eq!(settings.download_status.label(), status);
            settings.download.emit_clicked();
        }
        assert_eq!(
            presses.get(),
            3,
            "empty/error results keep the action retryable"
        );
        let scroll = settings
            .window
            .child()
            .unwrap()
            .downcast::<gtk::ScrolledWindow>()
            .unwrap();
        assert_eq!(scroll.max_content_height(), 600);
        settings.window.destroy();
        parent.destroy();
    }

    #[test]
    fn the_my_language_row_names_the_four_languages_in_english() {
        assert_eq!(
            super::my_language_names(),
            ["English", "Spanish", "Catalan", "Russian"]
        );
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn the_my_language_row_asks_only_for_the_users_choice() {
        use addict_core::translation::Source;
        gtk::init().expect("a display");
        let parent = gtk::Window::new();
        let asked = std::rc::Rc::new(RefCell::new(Vec::new()));
        let told = asked.clone();
        let settings = super::Settings::new(
            &parent,
            None,
            super::KeyOwner::Portal,
            || {},
            |_| {},
            || {},
            |_| {},
            move |chosen| told.borrow_mut().push(chosen),
            |_| {},
            |_| {},
            || async { Ok(()) },
            || {},
        );
        assert_eq!(settings.my_language.selected(), 3, "Russian at first");
        settings.render_my_language(Source::En);
        assert_eq!(settings.my_language.selected(), 0);
        assert!(asked.borrow().is_empty(), "a render asks nothing");
        settings.my_language.set_selected(1);
        assert_eq!(*asked.borrow(), [Source::Es]);
    }

    #[test]
    fn the_english_spelling_row_names_the_two_spellings_in_english() {
        assert_eq!(super::english_names(), ["US", "British"]);
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn the_english_spelling_row_asks_only_for_the_users_choice_and_starts_hidden() {
        use addict_core::translation::English;
        use gtk::prelude::*;
        gtk::init().expect("a display");
        let parent = gtk::Window::new();
        let asked = std::rc::Rc::new(RefCell::new(Vec::new()));
        let told = asked.clone();
        let settings = super::Settings::new(
            &parent,
            None,
            super::KeyOwner::Portal,
            || {},
            |_| {},
            || {},
            |_| {},
            |_| {},
            move |chosen| told.borrow_mut().push(chosen),
            |_| {},
            || async { Ok(()) },
            || {},
        );
        assert_eq!(settings.english.selected(), 0, "US at first");
        assert!(!settings.english_row.get_visible(), "hidden at first");
        assert!(
            settings.english_row.has_css_class("settings-row"),
            "the row of the list is the one hidden"
        );
        settings.render_english(English::Gb);
        assert_eq!(settings.english.selected(), 1);
        settings.render_english(English::Us);
        assert_eq!(settings.english.selected(), 0);
        assert!(asked.borrow().is_empty(), "a render asks nothing");
        settings.english.set_selected(1);
        assert_eq!(*asked.borrow(), [English::Gb]);
        settings.show_english(true);
        assert!(settings.english_row.get_visible());
        settings.show_english(false);
        assert!(!settings.english_row.get_visible());
        assert_eq!(*asked.borrow(), [English::Gb], "showing asks nothing");
        settings.window.destroy();
        parent.destroy();
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn the_switch_asks_and_stays_off_until_it_is_told() {
        use gtk::prelude::*;
        gtk::init().expect("a display");
        let parent = gtk::Window::new();
        let asked = std::rc::Rc::new(RefCell::new(Vec::new()));
        let told = asked.clone();
        let settings = super::Settings::new(
            &parent,
            None,
            super::KeyOwner::Portal,
            || {},
            |_| {},
            || {},
            move |wanted| told.borrow_mut().push(wanted),
            |_| {},
            |_| {},
            |_| {},
            || async { Ok(()) },
            || {},
        );
        assert!(!settings.startup.is_sensitive());
        settings.set_startup_sensitive(true);

        settings.startup.set_active(true);
        assert_eq!(*asked.borrow(), [true]);
        assert!(!settings.startup.state(), "not on before a confirmation");

        // A refusal: back off, and the code asks nothing.
        settings.render_startup(false);
        assert_eq!(*asked.borrow(), [true]);
        assert!(!settings.startup.is_active() && !settings.startup.state());

        // A confirmation, and the state of a window that opens.
        settings.render_startup(true);
        assert_eq!(*asked.borrow(), [true]);
        assert!(settings.startup.is_active() && settings.startup.state());

        settings.startup.set_active(false);
        assert_eq!(*asked.borrow(), [true, false]);
        assert!(settings.startup.state(), "on until the portal confirms");
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_long_import_refusal_keeps_lower_controls_reachable_at_768px() {
        use gtk::prelude::*;
        gtk::init().expect("a display");
        let parent = gtk::Window::new();
        let settings = super::Settings::new(
            &parent,
            None,
            super::KeyOwner::Portal,
            || {},
            |_| {},
            || {},
            |_| {},
            |_| {},
            |_| {},
            |_| {},
            || async { Ok(()) },
            || {},
        );
        let scroll = settings
            .window
            .child()
            .unwrap()
            .downcast::<gtk::ScrolledWindow>()
            .unwrap();
        assert_eq!(scroll.max_content_height(), 600);
        assert_eq!(scroll.hscrollbar_policy(), gtk::PolicyType::Never);

        fn import_button(widget: &gtk::Widget) -> Option<gtk::Button> {
            if let Some(button) = widget.downcast_ref::<gtk::Button>()
                && button.label().as_deref() == Some(crate::file_import::LABEL)
            {
                return Some(button.clone());
            }
            let mut child = widget.first_child();
            while let Some(current) = child {
                if let Some(found) = import_button(&current) {
                    return Some(found);
                }
                child = current.next_sibling();
            }
            None
        }
        let button = import_button(scroll.upcast_ref()).expect("mounted import action");
        let status = button
            .parent()
            .unwrap()
            .next_sibling()
            .unwrap()
            .downcast::<gtk::Label>()
            .unwrap();
        // Use an ordinary refusal, including the full list of pinned files.
        let path =
            std::env::temp_dir().join(format!("addict-settings-refusal-{}", std::process::id()));
        std::fs::write(&path, b"unrecognized download").unwrap();
        let refusal = addict_dictionary::import::import(
            &path,
            &path.with_extension("data"),
            addict_dictionary::import::Origin::Manual,
        )
        .unwrap_err()
        .to_string();
        std::fs::remove_file(path).unwrap();
        assert!(
            refusal.len() > 400,
            "exercise the long production error: {refusal}"
        );
        assert!(refusal.contains("english-wordnet-2025.xml.gz"));
        status.set_label(&refusal);
        status.set_visible(true);
        settings.window.set_default_size(520, 680);
        settings.present();
        let context = gtk::glib::MainContext::default();
        context.block_on(gtk::glib::timeout_future(std::time::Duration::from_millis(
            200,
        )));
        assert!(
            settings.window.height() <= 720,
            "fits a 768px display with shell chrome"
        );
        let adjustment = scroll.vadjustment();
        assert!(
            adjustment.upper() > adjustment.page_size(),
            "refusal causes scrolling"
        );
        adjustment.set_value(adjustment.upper() - adjustment.page_size());
        // Changing the adjustment queues allocation of the viewport's
        // translated child. A standalone display test may not receive a
        // compositor frame, so settle that allocation at the existing
        // window size before measuring the controls' actual coordinates.
        settings
            .window
            .allocate(settings.window.width(), settings.window.height(), -1, None);
        assert_eq!(
            adjustment.value(),
            adjustment.upper() - adjustment.page_size()
        );
        let clear_cache = settings
            .cache_size
            .next_sibling()
            .expect("Clear cache button");
        for widget in [&clear_cache, settings.startup.upcast_ref::<gtk::Widget>()] {
            let bounds = widget
                .compute_bounds(&scroll)
                .expect("lower control bounds");
            assert!(
                bounds.y() >= 0.0 && bounds.y() + bounds.height() <= scroll.height() as f32,
                "lower control reachable after scrolling: {bounds:?}"
            );
        }
        settings.window.close();
    }

    #[test]
    fn a_confirmed_change_is_saved_and_shown() {
        let (end, log) = change(true, Ok(()), vec![Ok(())], Ok(()));
        assert_eq!(end, StartupEnd::Done(true));
        assert_eq!(log, ["check", "ask true", "save true"]);
        assert_eq!(startup_end_text(&end, false), (true, String::new()));
        let (end, log) = change(false, Ok(()), vec![Ok(())], Ok(()));
        assert_eq!(end, StartupEnd::Done(false));
        assert_eq!(log, ["check", "ask false", "save false"]);
        assert_eq!(startup_end_text(&end, true), (false, String::new()));
    }

    #[test]
    fn a_refusal_saves_nothing_and_never_shows_on() {
        let (end, log) = change(true, Ok(()), vec![Err(StartupFailure::Refused)], Ok(()));
        assert_eq!(log, ["check", "ask true"]);
        let (on, line) = startup_end_text(&end, false);
        assert!(!on);
        assert!(line.contains("refused"), "{line}");
    }

    #[test]
    fn no_portal_returns_to_the_last_confirmed_state_and_says_why() {
        for (failure, word) in [
            (StartupFailure::Unavailable, "not available"),
            (StartupFailure::NoAnswer, "in time"),
            (StartupFailure::Failed("step: broke".to_owned()), "broke"),
        ] {
            for last in [false, true] {
                let (end, log) = change(!last, Ok(()), vec![Err(failure.clone())], Ok(()));
                assert_eq!(log.len(), 2, "nothing saved");
                let (on, line) = startup_end_text(&end, last);
                assert_eq!(on, last);
                assert!(line.contains(word), "{line}");
            }
        }
    }

    #[test]
    fn a_failed_check_asks_nothing() {
        let (end, log) = change(false, Err("/x is foreign".to_owned()), vec![], Ok(()));
        assert_eq!(log, ["check"]);
        let (on, line) = startup_end_text(&end, true);
        assert!(on);
        assert!(line.contains("/x is foreign"), "{line}");
    }

    #[test]
    fn a_failed_save_after_enable_revokes_and_says_why() {
        let (end, log) = change(true, Ok(()), vec![Ok(()), Ok(())], Err("disk".to_owned()));
        assert_eq!(log, ["check", "ask true", "save true", "ask false"]);
        let (on, line) = startup_end_text(&end, false);
        assert!(!on);
        assert!(line.contains("disk"), "{line}");
    }

    #[test]
    fn a_failed_revoke_is_stated_too() {
        let (end, _) = change(
            true,
            Ok(()),
            vec![Ok(()), Err(StartupFailure::Refused)],
            Err("disk".to_owned()),
        );
        let (on, line) = startup_end_text(&end, false);
        assert!(!on);
        assert!(line.contains("disk") && line.contains("refused"), "{line}");
        assert!(line.contains("may still start"), "{line}");
    }

    #[test]
    fn a_failed_save_after_disable_stays_off_and_says_so() {
        let (end, log) = change(false, Ok(()), vec![Ok(())], Err("disk".to_owned()));
        assert_eq!(log.len(), 3, "no second request");
        let (on, line) = startup_end_text(&end, true);
        assert!(!on);
        assert!(line.contains("disk"), "{line}");
    }

    #[test]
    fn only_a_finished_confirmation_moves_the_switch() {
        for outcome in [Outcome::TimedOut, Outcome::Panicked] {
            for last in [false, true] {
                let (on, line) = startup_outcome(&outcome, last).expect("shown");
                assert_eq!(on, last);
                assert!(!line.is_empty());
            }
        }
        let (_, line) = startup_outcome(&Outcome::TimedOut, false).expect("shown");
        assert!(!line.contains("not changed"), "{line}");
        assert!(line.contains("did not answer in time"), "{line}");
        assert!(line.contains("may still take effect"), "{line}");
        assert!(line.contains("off and on"), "{line}");
        assert_eq!(startup_outcome(&Outcome::Cancelled, true), None);
        assert_eq!(
            startup_outcome(&Outcome::Finished(StartupEnd::Done(true)), false),
            Some((true, String::new()))
        );
    }

    #[test]
    fn the_saved_state_wins_and_only_a_legacy_entry_shows_on_without_one() {
        let shown = |on, legacy| StartupShown { on, legacy };
        assert_eq!(startup_shown(Some(true), None), shown(true, false));
        assert_eq!(
            startup_shown(Some(false), Some(LoginEntry::Legacy)),
            shown(false, false)
        );
        assert_eq!(
            startup_shown(None, Some(LoginEntry::Legacy)),
            shown(true, true)
        );
        assert_eq!(
            startup_shown(None, Some(LoginEntry::Portal)),
            shown(false, false)
        );
        assert_eq!(
            startup_shown(None, Some(LoginEntry::None)),
            shown(false, false)
        );
        assert_eq!(startup_shown(None, None), shown(false, false));
    }

    #[test]
    fn the_cache_line_says_its_size_or_that_it_is_empty() {
        let read = |rows, bytes| CacheState::Read(Footprint { rows, bytes });
        assert_eq!(cache_text(read(0, 0)), "The cache is empty.");
        // The file of an empty cache still has its pages.
        assert_eq!(cache_text(read(0, 20_480)), "The cache is empty.");
        assert_eq!(cache_text(read(3, 1_234_567)), "The cache holds 1.2 MB.");
        assert_eq!(cache_text(read(1, 24_576)), "The cache holds 24.6 kB.");
        assert_eq!(
            cache_text(CacheState::Unknown),
            "The size of the cache could not be read."
        );
        let lines: std::collections::HashSet<_> = [
            CacheState::Checking,
            read(0, 0),
            read(1, 1),
            CacheState::Unknown,
        ]
        .into_iter()
        .map(cache_text)
        .collect();
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn a_size_is_in_decimal_units_with_one_decimal() {
        assert_eq!(size_text(0), "0 bytes");
        assert_eq!(size_text(999), "999 bytes");
        assert_eq!(size_text(1_000), "1.0 kB");
        assert_eq!(size_text(1_049), "1.0 kB");
        assert_eq!(size_text(1_050), "1.1 kB");
        assert_eq!(size_text(999_949), "999.9 kB");
        assert_eq!(size_text(999_950), "1.0 MB");
        assert_eq!(size_text(1_200_000), "1.2 MB");
        assert_eq!(size_text(10_000_000), "10.0 MB");
    }

    #[test]
    fn a_clear_says_nothing_when_it_worked_and_why_when_it_did_not() {
        assert_eq!(clear_text(Ok(())), "");
        assert_eq!(
            clear_text(Err("Permission denied (os error 13)")),
            "The cache was not cleared: Permission denied (os error 13)"
        );
    }

    #[test]
    fn every_key_state_has_a_distinct_line() {
        let states = [
            None,
            Some(KeyState::NoDirectory),
            Some(KeyState::Free),
            Some(KeyState::Pro),
            Some(KeyState::Refused(KeyError::Missing)),
            Some(KeyState::Refused(KeyError::Unreadable)),
            Some(KeyState::Refused(KeyError::TooOpen)),
            Some(KeyState::Refused(KeyError::Empty)),
            Some(KeyState::Unknown),
        ];
        let lines: std::collections::HashSet<_> = states.into_iter().map(key_text).collect();
        assert_eq!(lines.len(), states.len());
    }

    #[test]
    fn the_field_asks_to_replace_only_a_key_that_is_set() {
        assert!(key_placeholder(Some(KeyState::Pro)).contains("replace"));
        assert!(key_placeholder(Some(KeyState::Free)).contains("replace"));
        assert!(!key_placeholder(None).contains("replace"));
        assert!(!key_placeholder(Some(KeyState::Refused(KeyError::Missing))).contains("replace"));
    }

    #[test]
    fn the_unsafe_mode_names_0600() {
        assert!(key_text(Some(KeyState::Refused(KeyError::TooOpen))).contains("0600"));
    }

    #[test]
    fn a_state_of_a_missing_path_or_file() {
        assert_eq!(KeyState::of(None), KeyState::NoDirectory);
        let path = std::path::Path::new("/nonexistent/addict/deepl.key");
        assert_eq!(
            KeyState::of(Some(path)),
            KeyState::Refused(KeyError::Missing)
        );
    }

    /// The state of a key file written with `content` and `mode`.
    fn state_of(name: &str, content: &str, mode: u32) -> KeyState {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("addict-settings-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("deepl.key");
        std::fs::write(&path, content).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("mode");
        let state = KeyState::of(Some(&path));
        std::fs::remove_dir_all(&dir).expect("cleanup");
        state
    }

    #[test]
    fn the_key_is_never_in_its_state() {
        let state = state_of("free", "secret-key:fx\n", 0o600);
        assert_eq!(state, KeyState::Free);
        assert!(!format!("{state:?}").contains("secret"));
        assert!(!key_text(Some(state)).contains("secret"));
    }

    #[test]
    fn a_state_of_each_kind_of_file() {
        assert_eq!(state_of("pro", "secret-key\n", 0o600), KeyState::Pro);
        assert_eq!(
            state_of("open", "secret-key:fx\n", 0o644),
            KeyState::Refused(KeyError::TooOpen)
        );
        assert_eq!(
            state_of("empty", " \n", 0o600),
            KeyState::Refused(KeyError::Empty)
        );
    }

    #[test]
    fn the_trigger_is_shown_only_when_bound() {
        let not_bound = Binding::NotBound {
            detail: "x".to_owned(),
        };
        assert_eq!(trigger_text(&Binding::Bound, Some("Super+Q")), "Super+Q");
        assert_eq!(
            trigger_text(&Binding::Attempting, Some("Super+Q")),
            "Binding the shortcut…"
        );
        assert_eq!(trigger_text(&not_bound, Some("Super+Q")), "Not bound: x");
    }

    #[test]
    fn on_hyprland_a_bound_row_names_what_its_configuration_binds() {
        use super::{hyprland_target, hyprland_texts};
        assert_eq!(hyprland_target("invoke"), "io.github.doqhu.Addict:invoke");
        let (line, badge) = hyprland_texts(&Binding::Bound, Some(""), "invoke").unwrap();
        assert_eq!(line, "Bind in Hyprland: io.github.doqhu.Addict:invoke");
        assert_eq!(badge, ("In Hyprland", "pending"));
        assert!(hyprland_texts(&Binding::Bound, None, "screenshot").is_some());
        // A key the portal does describe, and a session that is not there,
        // read as anywhere else.
        assert_eq!(
            hyprland_texts(&Binding::Bound, Some("Super+Q"), "invoke"),
            None
        );
        assert_eq!(hyprland_texts(&Binding::Attempting, None, "invoke"), None);
        let not_bound = Binding::NotBound {
            detail: "no portal".to_owned(),
        };
        assert_eq!(hyprland_texts(&not_bound, None, "invoke"), None);
    }

    #[test]
    fn the_hyprland_lines_bind_both_fixed_ids_and_float_the_application() {
        let snippet = super::hyprland_snippet();
        assert_eq!(snippet.lines().count(), 3, "{snippet}");
        assert!(
            snippet.contains(
                r#"hl.bind("SUPER + Q", hl.dsp.global("io.github.doqhu.Addict:invoke"), "#
            )
        );
        assert!(snippet.contains(
            r#"hl.bind("SUPER + SHIFT + Q", hl.dsp.global("io.github.doqhu.Addict:screenshot"), "#
        ));
        // `\\.` in a Lua string: a literal dot in the class's expression.
        assert!(snippet.contains(r#"class = "^io\\.github\\.doqhu\\.Addict$""#));
        assert!(snippet.contains("float = true, center = true"));
        // Settings and About share the class: their own sizes are kept.
        assert!(!snippet.contains("size"));
    }

    #[test]
    fn unassigned_actions_and_a_new_binding_explain_configuration_without_resetting_keys() {
        assert!(trigger_text(&Binding::Bound, None).starts_with("Unassigned"));
        assert!(trigger_text(&Binding::Bound, Some("")).starts_with("Unassigned"));
        assert!(REBINDING.contains("dialog"));
        assert!(!rebind_text(None).contains("unchanged"));
        assert_eq!(
            rebind_text(Some("The keys are unchanged.")),
            "The keys are unchanged."
        );
        assert!(!change_text(None).contains("next start"));
    }

    #[test]
    fn a_save_says_nothing_when_it_worked_and_why_when_it_did_not() {
        assert_eq!(save_text(Ok(())), "");
        assert_eq!(
            save_text(Err(
                "a DeepL key is one word of printable ASCII, at most 1023 characters"
            )),
            "The key was not saved: a DeepL key is one word of printable ASCII, at most 1023 \
             characters."
        );
        assert_eq!(
            save_text(Err("permission denied")),
            "The key was not saved: permission denied."
        );
    }

    #[test]
    fn the_hint_names_where_the_key_is_saved() {
        let path = std::path::Path::new("/home/u/.config/addict/deepl.key");
        assert_eq!(
            saved_in_text(Some(path)),
            "Saved in /home/u/.config/addict/deepl.key"
        );
        assert!(!saved_in_text(None).is_empty());
    }

    #[test]
    fn a_save_without_a_directory_says_what_to_set() {
        let line = save_text(Err(NO_KEY_PLACE));
        assert!(line.starts_with("The key was not saved"), "{line}");
        assert!(
            line.contains("XDG_CONFIG_HOME") && line.contains("HOME"),
            "{line}"
        );
    }

    #[test]
    fn reopening_keeps_only_the_line_of_a_running_new_binding() {
        assert_eq!(reopened_status(true), REBINDING);
        assert_eq!(reopened_status(false), "");
    }

    #[test]
    fn configure_is_offered_only_when_bound_and_not_during_a_new_binding() {
        assert!(change_sensitive(true, false));
        assert!(!change_sensitive(true, true));
        assert!(!change_sensitive(false, false));
        assert!(!change_sensitive(false, true));
    }

    #[test]
    fn a_refused_change_says_why() {
        assert!(change_text(Some("version 1")).ends_with("version 1"));
        assert!(!change_text(None).contains("cannot"));
    }

    #[test]
    fn a_trigger_is_split_into_its_keys() {
        let keys = |t| trigger_keys(t).map(|k| k.join(" "));
        assert_eq!(keys("Super+Q").as_deref(), Some("Super Q"));
        assert_eq!(keys("Ctrl+Alt+T").as_deref(), Some("Ctrl Alt T"));
        assert_eq!(keys("Ctrl++").as_deref(), Some("Ctrl +"));
        assert_eq!(keys("F9").as_deref(), Some("F9"));
        assert_eq!(keys("Press <Super>q").as_deref(), Some("Super Q"));
        assert_eq!(keys("<Control><Alt>t").as_deref(), Some("Ctrl Alt T"));
        assert_eq!(keys("<Primary>F9").as_deref(), Some("Ctrl F9"));
        assert_eq!(keys("<Super>"), None);
        assert_eq!(keys("<>q"), None);
        assert_eq!(keys("Press the key"), None);
        assert_eq!(keys("Super+"), None);
        assert_eq!(keys(""), None);
    }

    #[test]
    fn the_badge_and_tones_follow_the_state() {
        assert_eq!(
            badge(&Binding::Bound, Some("Super+Q")),
            ("Assigned", "good")
        );
        assert_eq!(badge(&Binding::Bound, None), ("Unassigned", "pending"));
        assert_eq!(badge(&Binding::Attempting, None).1, "pending");
        let not_bound = Binding::NotBound {
            detail: "x".to_owned(),
        };
        assert_eq!(badge(&not_bound, None), ("Not bound", "bad"));
        assert_eq!(key_tone(None), "pending");
        assert_eq!(key_tone(Some(KeyState::Pro)), "good");
        assert_eq!(key_tone(Some(KeyState::Refused(KeyError::TooOpen))), "bad");
    }
}
