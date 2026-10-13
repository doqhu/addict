//! The widgets of the window: the source label and the status, below them
//! the binding row and the line of the dictionary indexes, then the field
//! for a word or a sentence, the text of the field with its words as links,
//! the list of senses, which scrolls, what stands below the list and, last,
//! the line of the online translation.
//!
//! This module owns the widgets, their layout and their signals, and no
//! text: what the widgets say is [`view`](crate::view)'s, and [`View`] puts
//! the results of its pure functions into them.
//!
//! The field's paste is taken over here, and it is the only place where the
//! application reads the clipboard: the user's paste action, and nothing
//! else, starts one asynchronous read, and the text that comes replaces the
//! whole field as one line.

use std::{cell::Cell, ops::Range, rc::Rc};

use addict_core::{input::WindowState, sentence};
use gtk::{gio, glib, prelude::*};

use crate::{
    dictionaries::Indexes,
    view::{
        Binding, FIELD_PLACEHOLDER, Lookup, Online, RETRY_LABEL, binding_texts, details_markup,
        indexes_text, online_markup, senses_markup, sentence_markup, texts,
    },
};

/// How wide the wrapping texts ask to be, in characters.
const TEXT_WIDTH_CHARS: i32 = 56;

/// The least height of the list of senses, in pixels. The list takes
/// whatever height the window has left above this.
const SENSES_MIN_HEIGHT: i32 = 72;

/// Takes over the paste of `field`: Ctrl+V, Shift+Insert and Paste of the menu
/// all emit `paste-clipboard` on the text widget inside the entry. GTK's own
/// paste, which inserts at the cursor and keeps line breaks, is stopped; the
/// clipboard is read once, off the UI thread's time, and its text replaces
/// the whole field as one line. The paste of the primary selection with the
/// middle button does not come through this signal and stays GTK's.
///
/// Nothing happens here without that signal, and nothing waits for the read:
/// the field stays editable, and a read that ends after the field was edited
/// is dropped. A clipboard that is empty, blank, not text or cannot be read
/// changes nothing and says nothing.
fn replace_on_paste(field: &gtk::Entry) {
    let Some(text) = field
        .delegate()
        .and_then(|delegate| delegate.downcast::<gtk::Text>().ok())
    else {
        // No text widget to take the paste from: GTK's own paste stays.
        return;
    };
    // Counts the changes of the field, so a read knows whether the field is
    // still as it was when the paste was asked for.
    let edits = Rc::new(Cell::new(0_u64));
    let counted = edits.clone();
    field.connect_changed(move |_| counted.set(counted.get().wrapping_add(1)));
    let entry = field.downgrade();
    text.connect_paste_clipboard(move |text| {
        // Before the default handler: GTK's own paste does not run.
        text.stop_signal_emission_by_name("paste-clipboard");
        let requested = edits.get();
        let edits = edits.clone();
        let entry = entry.clone();
        text.clipboard()
            .read_text_async(gio::Cancellable::NONE, move |read| {
                let Some(entry) = entry.upgrade() else {
                    return;
                };
                let Some(line) = read
                    .ok()
                    .flatten()
                    .and_then(|pasted| sentence::pasted(&pasted))
                else {
                    return;
                };
                if edits.get() != requested {
                    return;
                }
                // Fires `changed` twice, for the removal of the old text and
                // for the new one, also when the two are the same text.
                entry.set_text(&line);
                entry.set_position(-1);
            });
    });
}

/// Makes the label that `label` builds wrap at [`TEXT_WIDTH_CHARS`], with
/// its text at the left.
fn wrapping(label: gtk::builders::LabelBuilder) -> gtk::builders::LabelBuilder {
    label
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .max_width_chars(TEXT_WIDTH_CHARS)
        .xalign(0.0)
}

/// Runs `chosen` with the index a link of `label` has as its target each
/// time the link is clicked.
fn connect_index_link(label: &gtk::Label, chosen: impl Fn(usize) + 'static) {
    label.connect_activate_link(move |_, target| {
        if let Ok(index) = target.parse() {
            chosen(index);
        }
        // The target is an index: GTK must not open it as a URI.
        glib::Propagation::Stop
    });
}

/// The binding row: its line, the detail and Retry, one above the other.
struct BindingRow {
    row: gtk::Box,
    text: gtk::Label,
    detail: gtk::Label,
    retry: gtk::Button,
}

fn binding_row() -> BindingRow {
    let text = gtk::Label::new(None);
    let detail = gtk::Label::builder()
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .build();
    let retry = gtk::Button::builder()
        .label(RETRY_LABEL)
        .halign(gtk::Align::Center)
        .build();
    let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
    row.append(&text);
    row.append(&detail);
    row.append(&retry);
    BindingRow {
        row,
        text,
        detail,
        retry,
    }
}

/// The result area: the list of senses inside what scrolls it, what stands
/// below the list and, below that, the line of the online translation.
struct ResultArea {
    senses: gtk::Label,
    senses_area: gtk::ScrolledWindow,
    details: gtk::Label,
    online: gtk::Label,
}

fn result_area() -> ResultArea {
    // Not selectable: a click on a link must not give the label the
    // focus.
    let senses = wrapping(gtk::Label::builder().use_markup(true))
        .valign(gtk::Align::Start)
        .build();
    // Built here, not by the scrolled window: its own viewport scrolls to
    // the child that takes the focus, and a click on a sense must not
    // move the list.
    let viewport = gtk::Viewport::builder()
        .scroll_to_focus(false)
        .child(&senses)
        .build();
    // The list takes the height the window has left, so the window is as
    // tall for a long list as for a short one.
    let senses_area = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(SENSES_MIN_HEIGHT)
        .vexpand(true)
        .child(&viewport)
        .visible(false)
        .build();
    let details = wrapping(gtk::Label::builder().use_markup(true))
        .selectable(true)
        .visible(false)
        .build();
    let online = wrapping(gtk::Label::builder().use_markup(true))
        .selectable(true)
        .visible(false)
        .build();
    ResultArea {
        senses,
        senses_area,
        details,
        online,
    }
}

/// The source label and the status, one above the other, then the binding
/// row: its line, the detail and Retry; the line of the indexes when there
/// is something to say about them; then the field for a word or a
/// sentence, the text of the field with its words as links and the result:
/// the list of senses, which scrolls, and below it the rest, which does not,
/// with the line of the online translation at the end.
pub struct View {
    root: gtk::Box,
    source: gtk::Label,
    status: gtk::Label,
    binding: gtk::Box,
    binding_text: gtk::Label,
    binding_detail: gtk::Label,
    retry: gtk::Button,
    /// What is said about the dictionary indexes; hidden when both are open.
    indexes: gtk::Label,
    field: gtk::Entry,
    sentence: gtk::Label,
    /// The senses as links, inside `senses_area`.
    senses: gtk::Label,
    /// Scrolls the list; hidden when there is no list.
    senses_area: gtk::ScrolledWindow,
    /// What stands below the list.
    details: gtk::Label,
    /// The online translation, on its own line; hidden when there is none.
    online: gtk::Label,
}

impl View {
    /// Builds the labels with the texts of `state` and the row of `binding`.
    pub fn new(state: WindowState, binding: &Binding) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        let source = gtk::Label::new(None);
        let status = gtk::Label::new(None);
        root.append(&source);
        root.append(&status);

        let binding_row = binding_row();
        root.append(&binding_row.row);

        // Hidden until the indexes were checked. Not selectable: it must not
        // take the focus from the field.
        let indexes = wrapping(gtk::Label::builder()).visible(false).build();
        root.append(&indexes);

        let field = gtk::Entry::builder()
            .placeholder_text(FIELD_PLACEHOLDER)
            .build();
        replace_on_paste(&field);
        let sentence = wrapping(gtk::Label::builder().use_markup(true))
            .visible(false)
            .build();
        let result = result_area();
        root.append(&field);
        root.append(&sentence);
        root.append(&result.senses_area);
        root.append(&result.details);
        root.append(&result.online);

        let view = Self {
            root,
            source,
            status,
            binding: binding_row.row,
            binding_text: binding_row.text,
            binding_detail: binding_row.detail,
            retry: binding_row.retry,
            indexes,
            field,
            sentence,
            senses: result.senses,
            senses_area: result.senses_area,
            details: result.details,
            online: result.online,
        };
        view.render(state);
        view.render_binding(binding);
        view
    }

    /// The box holding everything the window shows.
    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    /// Shows the texts of `state`.
    pub fn render(&self, state: WindowState) {
        let texts = texts(state);
        self.source.set_text(texts.source);
        self.status.set_text(texts.status);
    }

    /// Shows the row of `binding`, or hides it when there is none.
    pub fn render_binding(&self, binding: &Binding) {
        let Some(texts) = binding_texts(binding) else {
            self.binding.set_visible(false);
            return;
        };
        self.binding_text.set_text(texts.text);
        self.binding_detail
            .set_text(texts.detail.unwrap_or_default());
        self.binding_detail.set_visible(texts.detail.is_some());
        self.retry.set_sensitive(texts.retry);
        self.binding.set_visible(true);
        if texts.retry {
            // Otherwise the selectable detail, the first focusable widget,
            // takes the focus and selects its text.
            self.retry.grab_focus();
        }
    }

    /// Shows what is said about `indexes`, or hides the line when both are
    /// open.
    pub fn render_indexes(&self, indexes: Indexes) {
        let text = indexes_text(indexes);
        self.indexes.set_text(&text);
        self.indexes.set_visible(!text.is_empty());
    }

    /// Runs `retry` each time Retry is pressed.
    pub fn connect_retry(&self, retry: impl Fn() + 'static) {
        self.retry.connect_clicked(move |_| retry());
    }

    /// Runs `changed` with the text of the field each time it changes.
    pub fn connect_sentence_changed(&self, changed: impl Fn(&str) + 'static) {
        self.field
            .connect_changed(move |field| changed(field.text().as_str()));
    }

    /// Runs `chosen` with the index of a word each time its link is clicked.
    pub fn connect_word_chosen(&self, chosen: impl Fn(usize) + 'static) {
        connect_index_link(&self.sentence, chosen);
    }

    /// Runs `chosen` with the index of a sense each time its link is clicked.
    pub fn connect_sense_chosen(&self, chosen: impl Fn(usize) + 'static) {
        connect_index_link(&self.senses, chosen);
    }

    /// The text the field holds now.
    pub fn field_text(&self) -> String {
        self.field.text().into()
    }

    /// Shows `sentence` with its `words` as links and the `selected` one
    /// bold; a text without a word hides the line.
    pub fn render_sentence(&self, sentence: &str, words: &[Range<usize>], selected: Option<usize>) {
        if words.is_empty() {
            self.sentence.set_visible(false);
            self.sentence.set_markup("");
            return;
        }
        // Empty first: GTK carries the visited flag of a clicked link over to
        // a link with the same target in the next markup, and the targets are
        // indices, so another word would be shown as visited.
        self.sentence.set_markup("");
        self.sentence
            .set_markup(&sentence_markup(sentence, words, selected));
        self.sentence.set_visible(true);
    }

    /// Shows `lookup` with the sense `chosen` in it, or hides the result
    /// when there is none. With no sense chosen the list is put at its top:
    /// that is how a result arrives. A choice leaves the list where it is.
    pub fn render_result(&self, lookup: Option<&Lookup>, chosen: Option<usize>) {
        // Empty first, as for the sentence: the targets are indices, and GTK
        // would show the same-numbered sense of the next markup as visited.
        self.senses.set_markup("");
        let Some(lookup) = lookup else {
            self.senses_area.set_visible(false);
            self.details.set_visible(false);
            self.details.set_markup("");
            return;
        };
        let list = senses_markup(lookup, chosen);
        self.senses.set_markup(&list);
        self.senses_area.set_visible(!list.is_empty());
        if chosen.is_none() {
            self.senses_area.vadjustment().set_value(0.0);
        }
        self.details.set_markup(&details_markup(lookup, chosen));
        self.details.set_visible(true);
    }

    /// Shows the line of `online`, or hides it when there is none.
    pub fn render_online(&self, online: Option<&Online>) {
        let markup = online.map(online_markup).unwrap_or_default();
        self.online.set_markup(&markup);
        self.online.set_visible(online.is_some());
    }

    /// Puts the keyboard focus into the field, so a paste lands there. The
    /// text is not selected: a selection would become the primary selection.
    pub fn focus_field(&self) {
        self.field.grab_focus_without_selecting();
    }
}
