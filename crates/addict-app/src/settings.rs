//! The Settings window, which the tray's Settings entry shows.
//!
//! It tells what the application is set to: the trigger of the shortcut,
//! which the portal owns, with a button that asks the portal for its dialog
//! to change it, and the DeepL key, with a masked field to enter one, the
//! file it is saved in and the state that file is in. The window writes the
//! key, on the runner, and never reads it back: the field starts empty and
//! is cleared once the key is saved. Below the key, a line says how much the
//! on-disk cache of the answers holds, with a button that clears it; the
//! cache is read and cleared on the runner. The texts are plain functions of
//! the state, so that they can be tested without a display.

use std::{path::Path, time::Duration};

use addict_deepl::key::{self, KeyError};
use gtk::{glib, prelude::*};

use crate::{cache::Footprint, view::Binding};

/// How long reading the state of the key file may take.
pub const KEY_CHECK_LIMIT: Duration = Duration::from_secs(2);

/// How long reading the size of the cache, or clearing it, may take.
pub const CACHE_LIMIT: Duration = Duration::from_secs(5);

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
        (Binding::Bound, Some(trigger)) => trigger.to_owned(),
        (Binding::Bound, None) => "Bound.".to_owned(),
        (Binding::Attempting, _) => "Binding the shortcut…".to_owned(),
        (Binding::NotBound { detail }, _) => format!("Not bound: {detail}"),
    }
}

/// The line after the portal answered the request for its dialog.
pub fn change_text(answer: Option<&str>) -> String {
    match answer {
        None => "The system's dialog changes the shortcut; this line shows the new one \
                 after the next start."
            .to_owned(),
        Some(message) => format!("The shortcut cannot be changed from here: {message}"),
    }
}

/// The line while the stored binding is reset and the system's bind dialog
/// is about to open, for a portal that has no dialog of its own for a change.
pub const REBINDING: &str =
    "The system's dialog opens to choose the new shortcut; the old one is reset.";

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

/// The line for a request made while the shortcut is not bound.
pub const CHANGE_UNBOUND: &str = "The shortcut can be changed once it is bound.";

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
pub fn badge(binding: &Binding) -> (&'static str, &'static str) {
    match binding {
        Binding::Bound => ("Bound", "good"),
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

/// The classes a tone replaces on a widget.
const TONES: [&str; 3] = ["good", "pending", "bad"];

fn set_tone(widget: &impl IsA<gtk::Widget>, tone: &str) {
    for other in TONES {
        widget.remove_css_class(other);
    }
    widget.add_css_class(tone);
}

/// The window and the labels that change.
pub struct Settings {
    window: gtk::Window,
    /// The caps of the bound trigger.
    keys: gtk::Box,
    /// The trigger when it is not shown as caps: where the attempt stands.
    trigger: gtk::Label,
    badge: gtk::Box,
    badge_text: gtk::Label,
    /// The block of the key's state: its icon and its line.
    key_block: gtk::Box,
    key_icon: gtk::Image,
    key_state: gtk::Label,
    key_entry: gtk::PasswordEntry,
    change: gtk::Button,
    status: gtk::Label,
    /// The line of the key's saves, apart from the shortcut's.
    key_status: gtk::Label,
    /// The line of the size of the cache.
    cache_size: gtk::Label,
    /// The line of the clears of the cache.
    cache_status: gtk::Label,
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
pub(crate) fn section(name: &str, rows: &[&gtk::Widget]) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .css_classes(["settings-card"])
        .build();
    for row in rows {
        card.append(*row);
    }
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    // Not wrapped: the label is measured before it is set in capitals,
    // and a wrapped one breaks where the capitals do not fit.
    let name = label(name, &["section-label", "settings-section"]);
    name.set_wrap(false);
    section.append(&name);
    section.append(&card);
    section
}

/// A line that hides itself while it is empty.
fn say_in(line: &gtk::Label, text: &str) {
    line.set_label(text);
    line.set_visible(!text.is_empty());
}

impl Settings {
    /// Builds the window, hidden, above `parent`. `on_change` runs when
    /// Change shortcut is pressed; `on_save` runs with the text of the key
    /// field, trimmed and not empty, when Save key or Enter in the field is
    /// pressed; `on_clear` runs when Clear cache is pressed.
    pub fn new(
        parent: &impl IsA<gtk::Window>,
        key_file: Option<&Path>,
        on_change: impl Fn() + 'static,
        on_save: impl Fn(String) + 'static,
        on_clear: impl Fn() + 'static,
    ) -> Self {
        let keys = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        let trigger = label("", &["trigger"]);
        trigger.set_hexpand(true);
        // The badge: a dot in the text's colour, then its word.
        let badge_text = gtk::Label::new(None);
        let badge = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::End)
            .hexpand(true)
            .css_classes(["badge"])
            .build();
        badge.append(
            &gtk::Box::builder()
                .valign(gtk::Align::Center)
                .css_classes(["dot"])
                .build(),
        );
        badge.append(&badge_text);
        let trigger_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        trigger_row.append(&keys);
        trigger_row.append(&trigger);
        trigger_row.append(&badge);
        let change = gtk::Button::builder()
            .label("Change shortcut…")
            .halign(gtk::Align::Start)
            .css_classes(["settings-button"])
            .build();
        change.connect_clicked(move |_| on_change());
        let status = label("", &["hint"]);
        status.set_visible(false);

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
            .placeholder_text("Paste a DeepL key")
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
        // The place of the file below a hairline: the path in monospace, and
        // selectable, so that it can be copied.
        let key_file_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .css_classes(["key-file"])
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

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .build();
        content.append(&section(
            "Shortcut",
            &[
                trigger_row.upcast_ref(),
                change.upcast_ref(),
                status.upcast_ref(),
            ],
        ));
        content.append(&section(
            "DeepL key",
            &[
                key_block.upcast_ref(),
                key_row.upcast_ref(),
                key_status.upcast_ref(),
                key_file_row.upcast_ref(),
                cache_row.upcast_ref(),
                cache_status.upcast_ref(),
            ],
        ));
        let footer = label(
            "Esc closes the window. The key is never shown back here.",
            &["hint", "settings-footer"],
        );
        content.append(&footer);

        let window = gtk::Window::builder()
            .title("Addict settings")
            .transient_for(parent)
            .default_width(440)
            .hide_on_close(true)
            .titlebar(&crate::resident::header_bar("Addict settings"))
            .child(&content)
            .css_classes(["addict", "settings"])
            .build();
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
            keys,
            trigger,
            badge,
            badge_text,
            key_block,
            key_icon,
            key_state,
            key_entry,
            change,
            status,
            key_status,
            cache_size,
            cache_status,
        };
        settings.render_binding(&Binding::Attempting, None);
        settings.render_key(None);
        settings
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Shows where the shortcut stands: a bound trigger as key caps, else
    /// the line of `trigger_text`; the button works only when it is bound.
    pub fn render_binding(&self, binding: &Binding, trigger: Option<&str>) {
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
        // Bound with no trigger to show: the badge says it alone.
        let line = match (bound, &caps, trigger) {
            (true, Some(_), _) | (true, None, None) => String::new(),
            _ => trigger_text(binding, trigger),
        };
        say_in(&self.trigger, &line);
        set_tone(&self.trigger, badge(binding).1);
        let (text, tone) = badge(binding);
        self.badge_text.set_label(text);
        set_tone(&self.badge, tone);
        self.change.set_sensitive(bound);
    }

    pub fn render_key(&self, state: Option<KeyState>) {
        self.key_state.set_label(key_text(state));
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
}

#[cfg(test)]
mod tests {
    use addict_deepl::key::KeyError;

    use super::{
        CacheState, KeyState, NO_KEY_PLACE, REBINDING, badge, cache_text, change_text, clear_text,
        key_text, key_tone, save_text, saved_in_text, size_text, trigger_keys, trigger_text,
    };
    use crate::{cache::Footprint, view::Binding};

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
    fn the_rebind_line_names_the_dialog_and_the_reset() {
        assert!(REBINDING.contains("dialog"));
        assert!(REBINDING.contains("reset"));
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
        assert_eq!(badge(&Binding::Bound), ("Bound", "good"));
        assert_eq!(badge(&Binding::Attempting).1, "pending");
        let not_bound = Binding::NotBound {
            detail: "x".to_owned(),
        };
        assert_eq!(badge(&not_bound), ("Not bound", "bad"));
        assert_eq!(key_tone(None), "pending");
        assert_eq!(key_tone(Some(KeyState::Pro)), "good");
        assert_eq!(key_tone(Some(KeyState::Refused(KeyError::TooOpen))), "bad");
    }
}
