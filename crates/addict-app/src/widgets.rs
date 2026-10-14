//! The widgets of the window: the source label and the status on one line,
//! right under them the capture line, then the binding row and the line of
//! the dictionary indexes, then
//! the field for a word or a sentence with the Capture button beside it,
//! the text of the field with its words as links and, last, the result,
//! which scrolls: the card of the word with its translations as pills, the
//! online texts, and the list of senses, a row for each, the chosen one
//! explained under its row.
//!
//! This module owns the widgets, their layout and their signals, and no
//! text: what the widgets say is [`view`](crate::view)'s, and [`View`] puts
//! the results of its pure functions into them.
//!
//! The field's paste is taken over here, and it is the only place where the
//! application reads the clipboard: the user's paste action, and nothing
//! else, starts one asynchronous read, and the text that comes replaces the
//! whole field as one line and is submitted, as Enter in the field submits
//! it. A text recognized in a capture is put into the field without being
//! reported as an edit of it, and is not submitted.

use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

use addict_core::{
    input::{Source, WindowState},
    sentence,
};
use gtk::{gio, glib, prelude::*};

use crate::{
    dictionaries::Indexes,
    view::{
        Binding, CAPTURE_LABEL, DEFINITION_ACTION, Explained, FIELD_PLACEHOLDER, Listed,
        OnlineTexts, RETRY_LABEL, SENSES_LABEL, SenseText, Shown, binding_texts, indexes_text,
        online_markup, result_texts, sentence_markup, texts,
    },
    wrap::Wrap,
};

/// How wide the wrapping texts ask to be, in characters.
const TEXT_WIDTH_CHARS: i32 = 56;

/// The least height of the result area, in pixels. The area takes whatever
/// height the window has left above this.
const RESULT_MIN_HEIGHT: i32 = 72;

/// Takes over the paste of `field`: Ctrl+V, Shift+Insert and Paste of the menu
/// all emit `paste-clipboard` on the text widget inside the entry. GTK's own
/// paste, which inserts at the cursor and keeps line breaks, is stopped; the
/// clipboard is read once, off the UI thread's time, and its text replaces
/// the whole field as one line and is then submitted: the field emits
/// `activate`, as it does for Enter. The paste of the primary selection with
/// the middle button does not come through this signal, stays GTK's and
/// submits nothing.
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
                // After the text is in the field: whoever hears of the
                // submit has heard of the new text first.
                entry.emit_activate();
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
    row.add_css_class("banner");
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

/// What runs with the index of a sense when its row is clicked.
type SenseChosen = Rc<dyn Fn(usize)>;

/// Takes every child out of `container`.
fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// A dim caption above a part of the result.
fn caption(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["section-label"])
        .build()
}

/// Puts `listed` into `container`: its label as a caption and its items as
/// pills of `pill_classes`, or the line of an empty list; nothing for
/// `None`.
fn fill_listed(container: &gtk::Box, listed: Option<Listed<'_>>, pill_classes: &[&str]) {
    clear(container);
    match listed {
        None => {}
        Some(Listed::Missing(line)) => {
            container.append(
                &wrapping(gtk::Label::builder().label(line))
                    .css_classes(["hint"])
                    .build(),
            );
        }
        Some(Listed::Items { label, items }) => {
            container.append(&caption(label));
            let pills = Wrap::new();
            pills.add_css_class("pills");
            for item in items {
                pills.append(
                    &gtk::Label::builder()
                        .label(item.as_str())
                        .css_classes(pill_classes)
                        .build(),
                );
            }
            container.append(&pills);
        }
    }
}

/// The row of `sense`: its number and its definition, with the mark above
/// the definition when it is the chosen one. A click runs `chosen` with its
/// index; it does not take the focus from the field.
fn sense_row(sense: &SenseText<'_>, chosen: &SenseChosen) -> gtk::Button {
    let number = gtk::Label::builder()
        .label(sense.number.to_string())
        .valign(gtk::Align::Start)
        .css_classes(["number"])
        .build();
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    if let Some(explained) = &sense.explained {
        text.append(
            &wrapping(gtk::Label::builder().label(explained.mark))
                .css_classes(["mark"])
                .build(),
        );
    }
    text.append(
        &wrapping(gtk::Label::builder().label(sense.definition))
            .css_classes(["definition-text"])
            .build(),
    );
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(&number);
    content.append(&text);
    let row = gtk::Button::builder()
        .child(&content)
        .focus_on_click(false)
        .css_classes(["sense"])
        .build();
    if sense.explained.is_some() {
        row.add_css_class("chosen");
    }
    let index = sense.number - 1;
    let chosen = chosen.clone();
    row.connect_clicked(move |_| chosen(index));
    row
}

/// What stands under the chosen sense: its synonyms, the button that
/// translates its definition when `action` is shown, and the online
/// translation of the definition once asked for.
fn explained_box(explained: &Explained<'_>, action: Option<&gtk::Button>) -> gtk::Box {
    let below = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .css_classes(["explained"])
        .build();
    let synonyms = gtk::Box::new(gtk::Orientation::Vertical, 6);
    fill_listed(&synonyms, Some(explained.synonyms), &["pill", "outline"]);
    below.append(&synonyms);
    if let Some(action) = action {
        below.append(action);
    }
    if let Some(block) = &explained.definition {
        let translated = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .css_classes(["online-definition"])
            .build();
        translated.append(&caption(block.label));
        translated.append(
            &wrapping(gtk::Label::builder().label(block.line.as_ref()))
                .selectable(true)
                .build(),
        );
        below.append(&translated);
    }
    below
}

/// The result area, which scrolls: the card of the word, the online texts
/// and the list of senses.
struct ResultArea {
    area: gtk::ScrolledWindow,
    article: gtk::Box,
    headword: gtk::Label,
    line: gtk::Label,
    translations: gtk::Box,
    online: gtk::Label,
    senses: gtk::Box,
    hint: gtk::Label,
    rows: gtk::Box,
    definition_action: gtk::Button,
}

fn result_area() -> ResultArea {
    let headword = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .css_classes(["headword"])
        .visible(false)
        .build();
    let line = wrapping(gtk::Label::builder())
        .selectable(true)
        .css_classes(["result-line"])
        .visible(false)
        .build();
    let translations = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let article = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(14)
        .css_classes(["article-card"])
        .visible(false)
        .build();
    article.append(&headword);
    article.append(&line);
    article.append(&translations);
    let online = wrapping(gtk::Label::builder().use_markup(true))
        .selectable(true)
        .css_classes(["online"])
        .visible(false)
        .build();
    let hint = wrapping(gtk::Label::builder())
        .css_classes(["hint"])
        .visible(false)
        .build();
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let senses = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .visible(false)
        .build();
    senses.append(&caption(SENSES_LABEL));
    senses.append(&hint);
    senses.append(&rows);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .build();
    content.append(&article);
    content.append(&online);
    content.append(&senses);
    // Built here, not by the scrolled window: its own viewport scrolls to
    // the child that takes the focus, and a click on a sense must not move
    // the list.
    let viewport = gtk::Viewport::builder()
        .scroll_to_focus(false)
        .child(&content)
        .build();
    // The area takes the height the window has left, so the window is as
    // tall for a long list as for a short one.
    let area = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(RESULT_MIN_HEIGHT)
        .vexpand(true)
        .child(&viewport)
        .css_classes(["result"])
        .visible(false)
        .build();
    let definition_action = gtk::Button::builder()
        .label(DEFINITION_ACTION)
        .halign(gtk::Align::Start)
        .focus_on_click(false)
        .css_classes(["definition"])
        .build();
    ResultArea {
        area,
        article,
        headword,
        line,
        translations,
        online,
        senses,
        hint,
        rows,
        definition_action,
    }
}

/// The source label and the status on one line, the capture line when a
/// capture gave no text, then the binding row: its
/// line, the detail and Retry; the line of the indexes when there is
/// something to say about them; then the field for a word or a sentence,
/// the text of the field with its words as links, and the result, which
/// scrolls: the card of the word with its translations, the online texts
/// and the list of senses, the chosen one explained under its row.
pub struct View {
    root: gtk::Box,
    source: gtk::Label,
    status: gtk::Label,
    /// Why the last capture gave no text; hidden until one did, and again
    /// once something new happens in the window.
    capture_failure: gtk::Label,
    binding: gtk::Box,
    binding_text: gtk::Label,
    binding_detail: gtk::Label,
    retry: gtk::Button,
    /// What is said about the dictionary indexes; hidden when both are open.
    indexes: gtk::Label,
    field: gtk::Entry,
    /// The handler that reports the changes of the field; blocked while a
    /// recognized text is put in.
    sentence_handler: RefCell<Option<glib::SignalHandlerId>>,
    /// Beside the field: takes a screen capture.
    capture: gtk::Button,
    sentence: gtk::Label,
    result: ResultArea,
    /// Runs with the index of a sense when its row is clicked; the rows are
    /// built anew for each result and each choice.
    sense_chosen: Rc<RefCell<Option<SenseChosen>>>,
}

impl View {
    /// Builds the labels with the texts of `state` and the row of `binding`.
    pub fn new(state: WindowState, binding: &Binding) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(20)
            .margin_end(20)
            .build();
        // One dim line: the source at the start, the status at the end.
        let source = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["meta"])
            .build();
        let status = gtk::Label::builder()
            .xalign(1.0)
            .css_classes(["meta"])
            .build();
        let meta = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        meta.append(&source);
        meta.append(&status);
        root.append(&meta);

        // Wraps, so an install command fits. Not selectable, like the line
        // of the indexes: it must not take the focus from the field.
        let capture_failure = wrapping(gtk::Label::builder())
            .css_classes(["banner"])
            .visible(false)
            .build();
        root.append(&capture_failure);

        let binding_row = binding_row();
        root.append(&binding_row.row);

        // Hidden until the indexes were checked. Not selectable: it must not
        // take the focus from the field.
        let indexes = wrapping(gtk::Label::builder())
            .css_classes(["banner"])
            .visible(false)
            .build();
        root.append(&indexes);

        let field = gtk::Entry::builder()
            .placeholder_text(FIELD_PLACEHOLDER)
            .css_classes(["lookup"])
            .build();
        replace_on_paste(&field);
        field.set_hexpand(true);
        let capture = gtk::Button::builder()
            .label(CAPTURE_LABEL)
            .css_classes(["capture"])
            .build();
        let field_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        field_row.append(&field);
        field_row.append(&capture);
        let sentence = wrapping(gtk::Label::builder().use_markup(true))
            .css_classes(["sentence"])
            .visible(false)
            .build();
        let result = result_area();
        root.append(&field_row);
        root.append(&sentence);
        root.append(&result.area);

        let view = Self {
            root,
            source,
            status,
            capture_failure,
            binding: binding_row.row,
            binding_text: binding_row.text,
            binding_detail: binding_row.detail,
            retry: binding_row.retry,
            indexes,
            field,
            sentence_handler: RefCell::new(None),
            capture,
            sentence,
            result,
            sense_chosen: Rc::new(RefCell::new(None)),
        };
        view.render(state, Source::Manual);
        view.render_binding(binding);
        view
    }

    /// The box holding everything the window shows.
    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    /// Shows the texts of `state` for a text from `source`. The capture line
    /// is left as it is: showing the window again keeps it.
    pub fn render(&self, state: WindowState, source: Source) {
        let texts = texts(state, source);
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

    /// Shows `line`, why a capture gave no text, under the source and the
    /// status. [`View::render`] leaves it; [`View::clear_capture_failure`]
    /// hides it.
    pub fn render_capture_failure(&self, line: &str) {
        self.capture_failure.set_text(line);
        self.capture_failure.set_visible(true);
    }

    /// Hides the capture line.
    pub fn clear_capture_failure(&self) {
        self.capture_failure.set_visible(false);
        self.capture_failure.set_text("");
    }

    /// Runs `changed` with the text of the field each time the user changes
    /// it; [`View::set_field_text`] does not run it. Called once.
    pub fn connect_sentence_changed(&self, changed: impl Fn(&str) + 'static) {
        let handler = self
            .field
            .connect_changed(move |field| changed(field.text().as_str()));
        self.sentence_handler.replace(Some(handler));
    }

    /// Runs `capture` each time Capture is pressed.
    pub fn connect_capture(&self, capture: impl Fn() + 'static) {
        self.capture.connect_clicked(move |_| capture());
    }

    /// Whether Capture can be pressed.
    pub fn set_capture_sensitive(&self, sensitive: bool) {
        self.capture.set_sensitive(sensitive);
    }

    /// Puts `text` into the field with the caret at its end, without running
    /// the handler of [`View::connect_sentence_changed`]. The other handlers
    /// of the field, the paste's count of its changes among them, see it.
    pub fn set_field_text(&self, text: &str) {
        let handler = self.sentence_handler.borrow();
        if let Some(handler) = handler.as_ref() {
            self.field.block_signal(handler);
        }
        self.field.set_text(text);
        self.field.set_position(-1);
        if let Some(handler) = handler.as_ref() {
            self.field.unblock_signal(handler);
        }
    }

    /// Runs `submitted` each time the text of the field is submitted: by
    /// Enter in the field, and by a paste after it has replaced the text.
    pub fn connect_submitted(&self, submitted: impl Fn() + 'static) {
        self.field.connect_activate(move |_| submitted());
    }

    /// Runs `chosen` with the index of a word each time its link is clicked.
    pub fn connect_word_chosen(&self, chosen: impl Fn(usize) + 'static) {
        connect_index_link(&self.sentence, chosen);
    }

    /// Runs `chosen` with the index of a sense each time its row is clicked.
    pub fn connect_sense_chosen(&self, chosen: impl Fn(usize) + 'static) {
        self.sense_chosen.replace(Some(Rc::new(chosen)));
    }

    /// Runs `action` each time the button that translates the definition of
    /// the chosen sense is clicked.
    pub fn connect_definition_action(&self, action: impl Fn() + 'static) {
        self.result
            .definition_action
            .connect_clicked(move |_| action());
    }

    /// The text the field holds now.
    pub fn field_text(&self) -> String {
        self.field.text().into()
    }

    /// Shows `sentence` with its `words` as links and the `selected` one
    /// bold, and the selected word as the headword of the result. A text
    /// without a word hides the line, and so does a lone word: the headword
    /// says it.
    pub fn render_sentence(&self, sentence: &str, words: &[Range<usize>], selected: Option<usize>) {
        let headword = selected
            .and_then(|index| words.get(index))
            .and_then(|word| sentence.get(word.clone()))
            .unwrap_or_default();
        self.result.headword.set_text(headword);
        self.result.headword.set_visible(!headword.is_empty());
        if words.len() < 2 {
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

    /// Shows the result of `shown` with the sense chosen in it, or hides it
    /// when there is none. With no sense chosen the area is put at its top:
    /// that is how a result arrives. A choice leaves it where it is.
    pub fn render_result(&self, shown: &Shown, key_file: Option<&std::path::Path>) {
        let result = &self.result;
        let (visible, sensitive) = shown.definition_action();
        result.definition_action.set_sensitive(sensitive);
        // Out of the old rows before they go: it is put under the chosen
        // one again below.
        if let Some(parent) = result.definition_action.parent().and_downcast::<gtk::Box>() {
            parent.remove(&result.definition_action);
        }
        clear(&result.rows);
        let Some(lookup) = shown.lookup() else {
            result.article.set_visible(false);
            result.senses.set_visible(false);
            fill_listed(&result.translations, None, &[]);
            self.show_area();
            return;
        };
        let texts = result_texts(lookup, shown.sense(), shown.definition(), key_file);
        result
            .line
            .set_text(texts.line.as_deref().unwrap_or_default());
        result.line.set_visible(texts.line.is_some());
        fill_listed(&result.translations, texts.translations, &["pill"]);
        result
            .translations
            .set_visible(texts.translations.is_some());
        result.article.set_visible(true);

        result.hint.set_text(texts.hint.unwrap_or_default());
        result.hint.set_visible(texts.hint.is_some());
        let chosen = self.sense_chosen.clone();
        let chosen: SenseChosen = Rc::new(move |index| {
            // Cloned out first: the handler builds the rows anew.
            let handler = chosen.borrow().clone();
            if let Some(handler) = handler {
                handler(index);
            }
        });
        for sense in &texts.senses {
            result.rows.append(&sense_row(sense, &chosen));
            if let Some(explained) = &sense.explained {
                let action = visible.then_some(&result.definition_action);
                result.rows.append(&explained_box(explained, action));
            }
        }
        result
            .senses
            .set_visible(!texts.senses.is_empty() || texts.hint.is_some());
        if shown.sense().is_none() {
            result.area.vadjustment().set_value(0.0);
        }
        self.show_area();
    }

    /// Shows the blocks of `online`, or hides them when there is none.
    pub fn render_online(&self, online: &OnlineTexts<'_>, key_file: Option<&std::path::Path>) {
        let markup = online_markup(online, key_file);
        self.result.online.set_markup(&markup);
        self.result.online.set_visible(!markup.is_empty());
        self.show_area();
    }

    /// Shows the result area while it has something in it.
    fn show_area(&self) {
        let result = &self.result;
        result
            .area
            // The flags of their own: `is_visible` would also ask the
            // hidden area itself.
            .set_visible(result.article.get_visible() || result.online.get_visible());
    }

    /// Puts the keyboard focus into the field, so a paste lands there. The
    /// text is not selected: a selection would become the primary selection.
    pub fn focus_field(&self) {
        self.field.grab_focus_without_selecting();
    }
}

#[cfg(test)]
mod tests {
    use addict_core::{
        dictionary::{Dictionary, Sample},
        input::WindowState,
        sentence::words,
    };
    use gtk::{glib, prelude::*};

    use super::View;
    use crate::{
        style,
        view::{Binding, Lookup, Online, OnlineTexts, Shown},
    };

    /// Draws the window with the sample article of "bank" in a sentence,
    /// its first sense chosen, into the PNG file `ADDICT_SHOT` names. Run
    /// by hand, and alone, to see the look: it opens a window on the
    /// display, and GTK starts once per process, on one thread.
    #[test]
    #[ignore = "opens a window; run alone with ADDICT_SHOT=<file.png>"]
    fn draws_the_window_into_a_png() {
        let path = std::env::var("ADDICT_SHOT").expect("ADDICT_SHOT");
        gtk::init().expect("a display");
        style::install();
        let view = View::new(WindowState::Empty, &Binding::Bound);
        let sentence = "I sat on the bank of the river.";
        let words = words(sentence);
        let bank = words
            .iter()
            .position(|word| &sentence[word.clone()] == "bank")
            .expect("bank");
        view.set_field_text(sentence);
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(
            Sample.lookup("bank").expect("the sample article"),
        )));
        shown.choose(0);
        view.render_result(&shown, None);
        let word = Online::Translated("берег".to_owned());
        let whole =
            crate::view::Whole::Asked(Online::Translated("Я сидел на берегу реки.".to_owned()));
        view.render_online(
            &OnlineTexts {
                word: Some(&word),
                sentence: Some(&whole),
                lone: None,
            },
            None,
        );
        let window = gtk::Window::builder()
            .title("Addict")
            .default_width(480)
            .default_height(860)
            .child(view.root())
            .css_classes(["addict"])
            .titlebar(&crate::resident::header_bar())
            .build();
        window.present();
        let context = glib::MainContext::default();
        let until = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        while std::time::Instant::now() < until {
            context.iteration(false);
        }
        // As in the application: the sentence is shown as it is typed, in a
        // shown window, and then a word of it is chosen.
        for end in 1..=sentence.len() {
            let typed = &sentence[..end];
            view.render_sentence(typed, &addict_core::sentence::words(typed), None);
            context.iteration(false);
        }
        view.render_sentence(sentence, &words, Some(bank));
        let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while std::time::Instant::now() < until {
            context.iteration(false);
        }
        let (width, height) = (window.width(), window.height());
        let paintable = gtk::WidgetPaintable::new(Some(&window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
        let node = snapshot.to_node().expect("something drawn");
        let renderer = window
            .native()
            .expect("a native")
            .renderer()
            .expect("a renderer");
        let texture = renderer.render_texture(&node, None);
        texture.save_to_png(&path).expect("the PNG");
        window.destroy();
    }
}
