//! GTK's native dictionary file chooser and the Settings import row.

use std::path::PathBuf;

use addict_core::failure::file_choice::FileChoiceFailure;
use addict_engine::view::file_choice::ImportState;
use gtk::{gio, prelude::*};

use crate::settings::label;

pub const LABEL: &str = "Import file…";

fn selected_path(file: gio::File) -> Result<PathBuf, FileChoiceFailure> {
    file.path().ok_or(FileChoiceFailure::NotLocal)
}

pub async fn choose(parent: &gtk::Window) -> Result<PathBuf, FileChoiceFailure> {
    let dialog = gtk::FileDialog::builder()
        .title("Import a dictionary file")
        .accept_label(LABEL)
        .modal(true)
        .build();
    match dialog.open_future(Some(parent)).await {
        Ok(file) => selected_path(file),
        Err(error)
            if error.matches(gtk::DialogError::Cancelled)
                || error.matches(gtk::DialogError::Dismissed) =>
        {
            Err(FileChoiceFailure::Cancelled)
        }
        Err(error) => Err(FileChoiceFailure::Failed(error.to_string())),
    }
}

pub struct Row {
    pub root: gtk::Box,
    button: gtk::Button,
    status: gtk::Label,
}

impl Row {
    pub fn new() -> Self {
        let name = label("Import a downloaded dictionary", &[]);
        name.set_hexpand(true);
        name.set_valign(gtk::Align::Center);
        let button = gtk::Button::builder()
            .label(LABEL)
            .valign(gtk::Align::Center)
            .css_classes(["settings-button"])
            .build();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.append(&name);
        line.append(&button);
        let status = label("", &["hint"]);
        status.set_selectable(true);
        status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        status.set_visible(false);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.append(&line);
        root.append(&status);
        Self {
            root,
            button,
            status,
        }
    }

    pub fn connect(&self, on_import: impl Fn() + 'static) {
        self.button.connect_clicked(move |_| on_import());
    }

    pub fn render(&self, state: &ImportState) {
        self.button.set_sensitive(!state.busy());
        let text = state.text();
        self.status.set_label(&text);
        self.status.set_visible(!text.is_empty());
    }
}

#[cfg(test)]
#[test]
fn a_non_local_file_is_not_local() {
    assert_eq!(
        selected_path(gio::File::for_uri("https://example.invalid/dictionary.gz")),
        Err(FileChoiceFailure::NotLocal)
    );
    let path = PathBuf::from("/run/user/1000/doc/grant/english-wordnet-2025.xml.gz");
    assert_eq!(selected_path(gio::File::for_path(&path)), Ok(path));
}
