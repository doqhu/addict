//! The Settings window, which the tray's Settings entry shows.
//!
//! It tells what the application is set to and changes nothing of its own:
//! the trigger of the shortcut, which the portal owns, with a button that
//! asks the portal for its dialog to change it, and the file of the DeepL
//! key, with the state it is in. The key itself is never read into the
//! window. The texts are plain functions of the state, so that they can be
//! tested without a display.

use std::{path::Path, time::Duration};

use addict_deepl::key::{self, KeyError};
use gtk::{glib, prelude::*};

use crate::view::Binding;

/// How long reading the state of the key file may take.
pub const KEY_CHECK_LIMIT: Duration = Duration::from_secs(2);

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

/// The line for a request made while the shortcut is not bound.
pub const CHANGE_UNBOUND: &str = "The shortcut can be changed once it is bound.";

/// The window and the labels that change.
pub struct Settings {
    window: gtk::Window,
    trigger: gtk::Label,
    key_state: gtk::Label,
    change: gtk::Button,
    status: gtk::Label,
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .css_classes(classes.to_vec())
        .build()
}

impl Settings {
    /// Builds the window, hidden, above `parent`. `on_change` runs when the
    /// button is pressed.
    pub fn new(
        parent: &impl IsA<gtk::Window>,
        key_file: Option<&Path>,
        on_change: impl Fn() + 'static,
    ) -> Self {
        let trigger = label("", &[]);
        let change = gtk::Button::builder()
            .label("Change shortcut…")
            .halign(gtk::Align::Start)
            .build();
        change.connect_clicked(move |_| on_change());
        let status = label("", &["hint"]);
        let key_file_label = label(
            &key_file.map_or_else(|| "—".to_owned(), |path| path.display().to_string()),
            &[],
        );
        key_file_label.set_selectable(true);
        let key_state = label(key_text(None), &["hint"]);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .build();
        content.append(&label("Shortcut", &["section-label"]));
        content.append(&trigger);
        content.append(&change);
        content.append(&status);
        content.append(&label("DeepL key file", &["section-label"]));
        content.append(&key_file_label);
        content.append(&key_state);

        let window = gtk::Window::builder()
            .title("Addict settings")
            .transient_for(parent)
            .default_width(420)
            .hide_on_close(true)
            .child(&content)
            .css_classes(["addict"])
            .build();
        let keys = gtk::EventControllerKey::new();
        let esc = window.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                if let Some(window) = esc.upgrade() {
                    window.close();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(keys);
        Self {
            window,
            trigger,
            key_state,
            change,
            status,
        }
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Shows where the shortcut stands; the button works only when it is
    /// bound.
    pub fn render_binding(&self, binding: &Binding, trigger: Option<&str>) {
        self.trigger.set_label(&trigger_text(binding, trigger));
        self.change.set_sensitive(matches!(binding, Binding::Bound));
    }

    pub fn render_key(&self, state: Option<KeyState>) {
        self.key_state.set_label(key_text(state));
    }

    pub fn say(&self, text: &str) {
        self.status.set_label(text);
    }
}

#[cfg(test)]
mod tests {
    use addict_deepl::key::KeyError;

    use super::{KeyState, REBINDING, change_text, key_text, trigger_text};
    use crate::view::Binding;

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
    fn a_refused_change_says_why() {
        assert!(change_text(Some("version 1")).ends_with("version 1"));
        assert!(!change_text(None).contains("cannot"));
    }
}
