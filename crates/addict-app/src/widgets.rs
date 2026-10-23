//! The widgets of the window: the source label and the status on one line,
//! right under them the source line, then the binding row and the line of
//! the dictionary indexes, then the field for a word or a sentence, with
//! Back and Forward left of it and the switch of the reading language,
//! EN / ES / CA, and the Capture button beside it, the text of the field
//! with its words as links and, last, the result, which scrolls: the card
//! of the word with its translations as pills, the online texts, and the
//! list of senses, a row for each, the chosen one explained under its row;
//! for a language beside English, the English glosses are rows too, and the
//! chosen one has the button that translates it under its row.
//!
//! This module owns the widgets, their layout and their signals, and no
//! text: what the widgets say is [`view`](crate::view)'s, and [`View`] puts
//! the results of its pure functions into them.
//!
//! The field's paste is taken over here: the user's paste action starts one
//! asynchronous read of the clipboard, and the text that comes replaces the
//! whole field as one line and is submitted, as Enter in the field submits
//! it. The only other read of the clipboard, and the only read of the primary
//! selection by addict (the middle-button paste stays GTK's), is the one
//! after a press of the shortcut that gave no context, which is
//! [`resident`](crate::resident)'s. A text recognized in a capture, or read
//! that way, is put into the field without being reported as an edit of it,
//! and is not submitted. Back and Forward only report their press; the text
//! they lead to is put into the field as a paste puts it, by
//! [`View::put_into_field`], and neither button takes the focus from the
//! field.

use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

use addict_core::{
    input::{Source, WindowState},
    sentence,
    translation::Source as Language,
};
use gtk::{gdk, gio, glib, prelude::*};

use crate::{
    dictionaries::Indexes,
    view::{
        ADD_KEY_LABEL, ANALYSIS_DASH, ANALYSIS_SEPARATOR, Analysis, AnalysisGroup, Answer,
        AnswerLine, BACK_TOOLTIP, Binding, Block, CAPTURE_LABEL, COPIED_LABEL, COPY_LABEL,
        ChosenGloss, DEFINITION_ACTION, Explained, FIELD_PLACEHOLDER, FORM_TAG, FORWARD_TOOLTIP,
        FormRow, GLOSS_NO_KEY, GlossRow, GlossTexts, LANGUAGE_TOOLTIP, LONE_WORD_TIP,
        LOOK_UP_ENGLISH, Listed, MORE_PRONUNCIATIONS, NO_KEY_HINT, NO_KEY_LINE, OFFLINE_SOURCE,
        OnlineRow, OnlineState, OnlineTexts, PLAY_TOOLTIP, PLAYBACK_FAILED, PRONUNCIATION_PREVIEW,
        PronunciationText, RETRY_LABEL, ResultTexts, SENSES_LABEL, SHOW_LESS, SYNONYM_PREVIEW,
        SenseText, Shown, SynonymTexts, WHOLE_WORD_LABEL, answer_texts, binding_texts,
        choice_texts, command_in, indexes_text, language_label, online_rows, sentence_markup,
        show_more, synonym_class, synonym_description, texts,
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
                put_into_field(&entry, &line);
            });
    });
}

/// Puts `text` into `field` as a paste does: it replaces the text, and the
/// field is submitted, so a lone word is also asked for online.
fn put_into_field(field: &gtk::Entry, text: &str) {
    // Fires `changed` twice, for the removal of the old text and for the new
    // one, also when the two are the same text.
    field.set_text(text);
    field.set_position(-1);
    // After the text is in the field: whoever hears of the submit has heard
    // of the new text first.
    field.emit_activate();
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

/// One dim line, the meta row: the source at the start, the status at the
/// end. Given back as the row, the source and the status.
fn meta_row() -> (gtk::Box, gtk::Label, gtk::Label) {
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
    (meta, source, status)
}

/// The switch of the reading language: one linked toggle button for each
/// language, in the order of [`Language::ALL`], English pressed. A click
/// does not take the focus from the field.
fn language_switch() -> (gtk::Box, Vec<(Language, gtk::ToggleButton)>) {
    let switch = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["linked", "language"])
        .tooltip_text(LANGUAGE_TOOLTIP)
        .valign(gtk::Align::Center)
        .build();
    let mut buttons: Vec<(Language, gtk::ToggleButton)> = Vec::new();
    for language in Language::ALL {
        let button = gtk::ToggleButton::builder()
            .label(language_label(language))
            .focus_on_click(false)
            .active(language == Language::default())
            .build();
        if let Some((_, first)) = buttons.first() {
            button.set_group(Some(first));
        }
        switch.append(&button);
        buttons.push((language, button));
    }
    (switch, buttons)
}

/// A button of the history left of the field, showing `icon`, with
/// `tooltip`: it never takes the focus from the field, and it cannot be
/// pressed until there is somewhere to go.
fn history_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .focus_on_click(false)
        .can_focus(false)
        .sensitive(false)
        .valign(gtk::Align::Center)
        .build()
}

/// The parts of the row of the field.
struct FieldRow {
    row: gtk::Box,
    field: gtk::Entry,
    back: gtk::Button,
    forward: gtk::Button,
    languages: Vec<(Language, gtk::ToggleButton)>,
    capture: gtk::Button,
}

/// The field for a word or a sentence, its paste taken over, with Back and
/// Forward left of it and the switch of the reading language and the
/// Capture button beside it.
fn field_row() -> FieldRow {
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
    let back = history_button("go-previous-symbolic", BACK_TOOLTIP);
    let forward = history_button("go-next-symbolic", FORWARD_TOOLTIP);
    let history = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["linked", "history"])
        .valign(gtk::Align::Center)
        .build();
    history.append(&back);
    history.append(&forward);
    let (switch, languages) = language_switch();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&history);
    row.append(&field);
    row.append(&switch);
    row.append(&capture);
    FieldRow {
        row,
        field,
        back,
        forward,
        languages,
        capture,
    }
}

/// What runs with the index of a sense or a gloss when its row is clicked.
type SenseChosen = Rc<dyn Fn(usize)>;

/// A handler that runs the one `slot` holds when it is called, if any.
fn forwarding(slot: &Rc<RefCell<Option<SenseChosen>>>) -> SenseChosen {
    let slot = slot.clone();
    Rc::new(move |index| {
        // Cloned out first: the handler builds the rows anew.
        let handler = slot.borrow().clone();
        if let Some(handler) = handler {
            handler(index);
        }
    })
}

/// Takes every child out of `container`.
fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// A line in a banner that wraps and is hidden until it has something to
/// say.
fn banner() -> gtk::Label {
    wrapping(gtk::Label::builder())
        .css_classes(["banner"])
        .visible(false)
        .build()
}

/// The source line: a banner whose text wraps, with a Copy button beside
/// it that is shown while the text names a runnable command.
struct SourceFailure {
    row: gtk::Box,
    line: gtk::Label,
    copy: gtk::Button,
    /// The command the line names, which Copy puts on the clipboard.
    command: Rc<RefCell<Option<String>>>,
}

/// The source line, hidden until it has something to say. Neither the line
/// nor Copy takes the focus from the field: the line is not selectable and
/// the button is never focused.
fn source_failure() -> SourceFailure {
    let line = wrapping(gtk::Label::builder()).hexpand(true).build();
    let copy = gtk::Button::builder()
        .label(COPY_LABEL)
        .css_classes(["copy"])
        .focus_on_click(false)
        .can_focus(false)
        .valign(gtk::Align::Start)
        .visible(false)
        .build();
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .css_classes(["banner"])
        .visible(false)
        .build();
    row.append(&line);
    row.append(&copy);
    let command: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let held = command.clone();
    copy.connect_clicked(move |button| {
        if let Some(command) = held.borrow().as_deref() {
            button.clipboard().set_text(command);
            button.set_label(COPIED_LABEL);
        }
    });
    SourceFailure {
        row,
        line,
        copy,
        command,
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
/// pills of `pill_classes`, the first `preview` of them shown and the rest
/// behind the toggle at the end, or the line of an empty list; nothing for
/// `None`. With a `field`, each pill is a link that puts its item into the
/// field as a paste does, and keeps the focus in the field.
fn fill_listed(
    container: &gtk::Box,
    listed: Option<&Listed<'_>>,
    pill_classes: &[&str],
    field: Option<&gtk::Entry>,
    preview: usize,
) {
    clear(container);
    match listed {
        None => {}
        Some(Listed::Missing(line)) => {
            container.append(
                &wrapping(gtk::Label::builder().label(*line))
                    .css_classes(["hint"])
                    .build(),
            );
        }
        Some(Listed::Items { label, items }) => {
            container.append(&caption(label));
            container.append(&pills(items, pill_classes, field, preview, label));
        }
    }
}

/// `items` as pills of `classes` in lines, the first `preview` shown and
/// the rest behind a toggle at the end with `tooltip`; see
/// [`synonym_pill`].
fn pills(
    items: &[String],
    classes: &[&str],
    field: Option<&gtk::Entry>,
    preview: usize,
    tooltip: &str,
) -> Wrap {
    let pills = Wrap::new();
    pills.add_css_class("pills");
    let mut extra = Vec::new();
    for (number, item) in items.iter().enumerate() {
        let pill = synonym_pill(item, classes, field);
        if number >= preview {
            pill.set_visible(false);
            extra.push(pill.downgrade());
        }
        pills.append(&pill);
    }
    if !extra.is_empty() {
        let hidden = extra.len();
        pills.append(&show_more_toggle(extra, hidden, tooltip));
    }
    pills
}

/// Shared pill behavior for flat translations and independent synonym meanings.
fn synonym_pill(item: &str, classes: &[&str], field: Option<&gtk::Entry>) -> gtk::Widget {
    let pill = gtk::Label::builder()
        .label(item)
        .css_classes(classes)
        .build();
    let Some(field) = field else {
        return pill.upcast();
    };
    let link = gtk::Button::builder()
        .child(&pill)
        .css_classes(["pill-link"])
        .focus_on_click(false)
        .build();
    link.set_cursor_from_name(Some("pointer"));
    let field = field.downgrade();
    let word = item.to_owned();
    link.connect_clicked(move |_| {
        if let Some(field) = field.upgrade() {
            put_into_field(&field, &word);
        }
    });
    link.upcast()
}

/// Puts the synonym meanings `texts` into `container`: behind a disclosure
/// with their label and how many they are, open while `open` is, a card
/// each; or the line that there are none; nothing for `None`.
fn fill_synonym_groups(
    container: &gtk::Box,
    texts: Option<&SynonymTexts<'_>>,
    field: &gtk::Entry,
    open: &Rc<Cell<bool>>,
) {
    clear(container);
    match texts {
        None => {}
        Some(SynonymTexts::Missing(line)) => {
            container.append(
                &wrapping(gtk::Label::builder().label(*line))
                    .css_classes(["hint"])
                    .build(),
            );
        }
        Some(SynonymTexts::Groups {
            label,
            count,
            groups,
        }) => {
            let list = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(8)
                .visible(open.get())
                .build();
            for group in groups {
                list.append(&meaning_card(group, field));
            }
            container.append(&disclosure(label, count, &list, open));
            container.append(&list);
        }
    }
}

/// The button that shows `list` while it is active: a chevron, `label` and
/// `count`; its state is kept in `open`.
fn disclosure(
    label: &str,
    count: &str,
    list: &gtk::Box,
    open: &Rc<Cell<bool>>,
) -> gtk::ToggleButton {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.append(&gtk::Image::from_icon_name("pan-end-symbolic"));
    content.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .css_classes(["section-label", "disclosure-label"])
            .build(),
    );
    content.append(
        &gtk::Label::builder()
            .label(count)
            .hexpand(true)
            .xalign(1.0)
            .css_classes(["dmeta"])
            .build(),
    );
    let toggle = gtk::ToggleButton::builder()
        .child(&content)
        .active(open.get())
        .focus_on_click(false)
        .css_classes(["disclosure"])
        .build();
    toggle.update_state(&[gtk::accessible::State::Expanded(Some(open.get()))]);
    let (list, open) = (list.downgrade(), open.clone());
    toggle.connect_toggled(move |toggle| {
        let expanded = toggle.is_active();
        open.set(expanded);
        if let Some(list) = list.upgrade() {
            list.set_visible(expanded);
        }
        toggle.update_state(&[gtk::accessible::State::Expanded(Some(expanded))]);
    });
    toggle
}

/// The card of one synonym meaning: its headword, its class as a tag and
/// what the source says it is, with how many synonyms it has at the end;
/// under them its synonyms as links, the first [`SYNONYM_PREVIEW`] shown.
/// The source's whole label is the tooltip of the head.
fn meaning_card(group: &addict_dictionary::index::SynonymGroup, field: &gtk::Entry) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .css_classes(["meaning"])
        .build();
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.append(
        &gtk::Label::builder()
            .label(group.headword.as_str())
            .valign(gtk::Align::Baseline)
            .css_classes(["mword"])
            .build(),
    );
    if let Some(class) = synonym_class(group) {
        head.append(
            &gtk::Label::builder()
                .label(class)
                .valign(gtk::Align::Center)
                .css_classes(["pos"])
                .build(),
        );
    }
    let description =
        wrapping(gtk::Label::builder().label(synonym_description(group).unwrap_or_default()))
            .hexpand(true)
            .valign(gtk::Align::Baseline)
            .css_classes(["mgloss"])
            .build();
    head.append(&description);
    head.append(
        &gtk::Label::builder()
            .label(match group.synonyms.len() {
                1 => "1 synonym".to_owned(),
                count => format!("{count} synonyms"),
            })
            .valign(gtk::Align::Baseline)
            .css_classes(["mcount"])
            .build(),
    );
    if let Some(metadata) = &group.metadata {
        head.set_tooltip_text(Some(metadata.label()));
    }
    card.append(&head);
    card.append(&pills(
        &group.synonyms,
        &["pill", "outline"],
        Some(field),
        SYNONYM_PREVIEW,
        &group.headword,
    ));
    card
}

/// The toggle that shows the hidden widgets `extra` while it is active:
/// how many items, `hidden`, stand behind it, or [`SHOW_LESS`], by its
/// state, beside a chevron, with `tooltip`.
fn show_more_toggle(
    extra: Vec<glib::WeakRef<gtk::Widget>>,
    hidden: usize,
    tooltip: &str,
) -> gtk::ToggleButton {
    let more = show_more(hidden);
    let label = gtk::Label::new(Some(&more));
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    content.append(&label);
    content.append(&gtk::Image::from_icon_name("pan-down-symbolic"));
    let toggle = gtk::ToggleButton::builder()
        .child(&content)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .css_classes(["more"])
        .build();
    toggle.update_state(&[gtk::accessible::State::Expanded(Some(false))]);
    toggle.update_property(&[gtk::accessible::Property::Label(&more)]);
    toggle.set_tooltip_text(Some(tooltip));
    toggle.connect_toggled(move |toggle| {
        let expanded = toggle.is_active();
        for widget in &extra {
            if let Some(widget) = widget.upgrade() {
                widget.set_visible(expanded);
            }
        }
        let text = if expanded { SHOW_LESS } else { more.as_str() };
        label.set_label(text);
        toggle.update_property(&[gtk::accessible::Property::Label(text)]);
        toggle.update_state(&[gtk::accessible::State::Expanded(Some(expanded))]);
    });
    toggle
}

/// The stream that plays now, if any: one at a time.
type Player = Rc<RefCell<Option<gtk::MediaFile>>>;

/// Stops the stream of `player`, if one plays, and lets it go.
fn stop(player: &Player) {
    if let Some(stream) = player.borrow_mut().take() {
        stream.set_playing(false);
        stream.clear();
    }
}

/// Puts `pronunciations` in rows: a play button when it has a recording,
/// the headword of its entry when it is not `word`, the transcription and
/// its labels, dim. The first [`PRONUNCIATION_PREVIEW`] go into `lead`,
/// beside the headword, with the toggle that shows the others; those go
/// into `rest`, under it, hidden until then.
fn fill_pronunciations(
    lead: &gtk::Box,
    rest: &gtk::Box,
    pronunciations: &[PronunciationText<'_>],
    word: &str,
    player: &Player,
) {
    clear(lead);
    clear(rest);
    rest.set_visible(false);
    for (index, pronunciation) in pronunciations.iter().enumerate() {
        let row = pronunciation_row(pronunciation, word, player);
        if index < PRONUNCIATION_PREVIEW {
            lead.append(&row);
        } else {
            rest.append(&row);
        }
    }
    let hidden = pronunciations.len().saturating_sub(PRONUNCIATION_PREVIEW);
    if hidden > 0 {
        let extra = vec![rest.upcast_ref::<gtk::Widget>().downgrade()];
        lead.append(&show_more_toggle(extra, hidden, MORE_PRONUNCIATIONS));
    }
}

/// The row of one pronunciation; see [`fill_pronunciations`].
fn pronunciation_row(
    pronunciation: &PronunciationText<'_>,
    word: &str,
    player: &Player,
) -> gtk::Box {
    let row = gtk::Box::builder()
        .spacing(8)
        .css_classes(["pronunciation"])
        .build();
    let part = |text: &str, class: &str| {
        wrapping(gtk::Label::builder().label(text))
            .valign(gtk::Align::Center)
            .css_classes([class])
            .build()
    };
    if let Some(url) = pronunciation.audio {
        row.append(&play_button(url, player));
    }
    if let Some(headword) = pronunciation.prefix(word) {
        row.append(&part(headword, "pronunciation-headword"));
    }
    if let Some(transcription) = pronunciation.transcription {
        row.append(&part(transcription, "transcription"));
    }
    if !pronunciation.labels.is_empty() {
        row.append(&part(
            &pronunciation.labels.join(", "),
            "pronunciation-labels",
        ));
    }
    row
}

/// The button that plays the recording at `url` through `player`: the
/// stream that played before stops. A failed playback says why in the
/// button's tooltip.
fn play_button(url: &str, player: &Player) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text(PLAY_TOOLTIP)
        .focus_on_click(false)
        .valign(gtk::Align::Center)
        .css_classes(["play"])
        .build();
    button.update_property(&[gtk::accessible::Property::Label(PLAY_TOOLTIP)]);
    let (url, player) = (url.to_owned(), player.clone());
    button.connect_clicked(move |button| {
        stop(&player);
        button.set_tooltip_text(Some(PLAY_TOOLTIP));
        button.remove_css_class("failed");
        let stream = gtk::MediaFile::for_file(&gio::File::for_uri(&url));
        let failed = {
            let button = button.downgrade();
            move |stream: &gtk::MediaFile| {
                let Some(button) = button.upgrade().filter(|_| stream.error().is_some()) else {
                    return;
                };
                button.set_tooltip_text(Some(PLAYBACK_FAILED));
                button.add_css_class("failed");
            }
        };
        failed(&stream);
        stream.connect_error_notify(failed);
        stream.play();
        player.replace(Some(stream));
    });
    button
}

/// Puts `analysis` into `container`: a line per lemma, which wraps between
/// its groups, never inside one; the lemma a link that puts it into `field`
/// as a paste does, then a dash and the groups, each its persons as a tag,
/// its tense and its mood, dimmer, with a dot between two; then the note,
/// dim, on a line of its own.
fn fill_analysis(container: &gtk::Box, analysis: &Analysis, field: &gtk::Entry) {
    clear(container);
    for line in &analysis.lines {
        let row = Wrap::new();
        row.add_css_class("analysis-line");
        row.update_property(&[gtk::accessible::Property::Label(&line.text())]);
        row.append(&lemma_link(&line.lemma, field));
        append_analysis_groups(&row, &line.groups);
        container.append(&row);
    }
    if let Some(note) = &analysis.note {
        container.append(
            &wrapping(gtk::Label::builder().label(note.as_str()))
                .selectable(true)
                .css_classes(["hint"])
                .build(),
        );
    }
}

/// A plain label of `class`, a part of an analysis line.
fn analysis_part(text: &str, class: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes([class])
        .build()
}

/// The lemma as a serif link in the accent that puts it into `field` as a
/// paste does.
fn lemma_link(lemma: &str, field: &gtk::Entry) -> gtk::Button {
    let link = gtk::Button::builder()
        .child(&analysis_part(lemma, "lemma"))
        .css_classes(["lemma-link"])
        .focus_on_click(false)
        .build();
    link.set_cursor_from_name(Some("pointer"));
    let held = field.downgrade();
    let word = lemma.to_owned();
    link.connect_clicked(move |_| {
        if let Some(field) = held.upgrade() {
            put_into_field(&field, &word);
        }
    });
    link
}

/// Appends the dash and `groups` to `row`: each its persons as a tag, its
/// tense and its mood, with a dot between two.
fn append_analysis_groups(row: &Wrap, groups: &[AnalysisGroup]) {
    row.append(&analysis_part(ANALYSIS_DASH, "analysis-mark"));
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            row.append(&analysis_part(ANALYSIS_SEPARATOR, "analysis-mark"));
        }
        let parts = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        if let Some(persons) = &group.persons {
            let tag = analysis_part(persons, "persons");
            tag.set_valign(gtk::Align::Center);
            parts.append(&tag);
        }
        if let Some(tense) = group.tense {
            parts.append(&analysis_part(tense, "tense"));
        }
        if let Some(mood) = group.mood {
            parts.append(&analysis_part(mood, "mood"));
        }
        row.append(&parts);
    }
}

/// Runs with the English word of a gloss when its pill is clicked.
type LookUp = Rc<dyn Fn(&str)>;

/// Puts the English glosses `texts` into `container`: the label with how
/// many there are, the hint, then a group per part of speech, its tag in
/// the left column and a row per gloss, which a click chooses, and the
/// forms last; or the line that there are none; nothing for `None`. Under
/// the `chosen` gloss stand its online translation, or the line that a key
/// is needed, and its English word, which `look_up` looks up.
fn fill_glosses(
    container: &gtk::Box,
    texts: Option<&GlossTexts<'_>>,
    chosen: Option<&ChosenGloss<'_>>,
    clicked: &SenseChosen,
    look_up: &LookUp,
    field: &gtk::Entry,
) {
    clear(container);
    let glosses = match texts {
        None => return,
        Some(GlossTexts::Missing(line)) => {
            container.append(
                &wrapping(gtk::Label::builder().label(*line))
                    .css_classes(["hint"])
                    .build(),
            );
            return;
        }
        Some(GlossTexts::Glosses(glosses)) => glosses,
    };
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    head.append(&caption(glosses.label));
    head.append(
        &gtk::Label::builder()
            .label(glosses.count.as_str())
            .hexpand(true)
            .xalign(1.0)
            .css_classes(["dmeta"])
            .build(),
    );
    container.append(&head);
    container.append(
        &wrapping(gtk::Label::builder().label(glosses.hint))
            .css_classes(["hint"])
            .build(),
    );
    for group in &glosses.groups {
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for row in &group.rows {
            let button = gloss_row(row);
            let (clicked, index) = (clicked.clone(), row.index);
            button.connect_clicked(move |_| clicked(index));
            rows.append(&button);
            let Some(chosen) = chosen.filter(|chosen| chosen.index == row.index) else {
                continue;
            };
            button.add_css_class("chosen");
            rows.append(&gloss_explained(chosen, row.lookup, look_up));
        }
        container.append(&gloss_group(group.pos, &rows));
    }
    if !glosses.forms.is_empty() {
        let rows = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .css_classes(["gloss-forms"])
            .build();
        for form in &glosses.forms {
            rows.append(&form_row(form, field));
        }
        container.append(&gloss_group(Some(FORM_TAG), &rows));
    }
}

/// One group of glosses: a hairline above, the tag `pos` in the left
/// column and `rows` beside it.
fn gloss_group(pos: Option<&str>, rows: &gtk::Box) -> gtk::Box {
    let group = gtk::Box::builder()
        .spacing(8)
        .css_classes(["gloss-group"])
        .build();
    let column = gtk::Box::builder()
        .width_request(84)
        .valign(gtk::Align::Start)
        .css_classes(["gloss-pos"])
        .build();
    if let Some(pos) = pos {
        column.append(
            &gtk::Label::builder()
                .label(pos)
                .css_classes(["pos"])
                .build(),
        );
    }
    rows.set_hexpand(true);
    group.append(&column);
    group.append(rows);
    group
}

/// The row of one gloss: its number, what it says, bold, and what the
/// source adds, dim. It does not take the focus from the field.
fn gloss_row(row: &GlossRow<'_>) -> gtk::Button {
    let number = gtk::Label::builder()
        .label(row.number.to_string())
        .valign(gtk::Align::Start)
        .css_classes(["number"])
        .build();
    let text = Wrap::new();
    text.set_hexpand(true);
    text.append(
        &wrapping(gtk::Label::builder().label(row.head))
            .css_classes(["gloss-head"])
            .build(),
    );
    if let Some(qualifier) = row.qualifier {
        text.append(
            &wrapping(gtk::Label::builder().label(qualifier))
                .css_classes(["gloss-qualifier"])
                .build(),
        );
    }
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(&number);
    content.append(&text);
    gtk::Button::builder()
        .child(&content)
        .focus_on_click(false)
        .css_classes(["sense"])
        .build()
}

/// What stands under the chosen gloss: its online translation, or the line
/// that a key is needed for it, and its English word as a pill that
/// `look_up` looks up.
fn gloss_explained(chosen: &ChosenGloss<'_>, lookup: Option<&str>, look_up: &LookUp) -> gtk::Box {
    let below = explained_tail(None, chosen.definition.as_ref());
    if chosen.no_key {
        below.append(
            &wrapping(gtk::Label::builder().label(GLOSS_NO_KEY))
                .css_classes(["hint"])
                .build(),
        );
    }
    if let Some(word) = lookup {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.append(
            &gtk::Label::builder()
                .label(LOOK_UP_ENGLISH)
                .css_classes(["hint"])
                .build(),
        );
        let pill = gtk::Button::builder()
            .child(
                &gtk::Label::builder()
                    .label(word)
                    .css_classes(["pill", "outline"])
                    .build(),
            )
            .css_classes(["pill-link"])
            .focus_on_click(false)
            .build();
        pill.set_cursor_from_name(Some("pointer"));
        let (look_up, word) = (look_up.clone(), word.to_owned());
        pill.connect_clicked(move |_| look_up(&word));
        line.append(&pill);
        below.append(&line);
    }
    below
}

/// The row of a gloss that names a form: its number, the description, dim,
/// the lemma as a link into `field`, then the analysis of the word for it
/// or what the source adds, dim.
fn form_row(form: &FormRow<'_>, field: &gtk::Entry) -> Wrap {
    let row = Wrap::new();
    row.add_css_class("analysis-line");
    let number = analysis_part(&form.number.to_string(), "number");
    number.set_valign(gtk::Align::Center);
    row.append(&number);
    row.append(
        &wrapping(gtk::Label::builder().label(form.description.as_str()))
            .css_classes(["hint"])
            .build(),
    );
    row.append(&lemma_link(form.lemma, field));
    match (&form.analysis, form.qualifier) {
        (Some(analysis), _) => append_analysis_groups(&row, &analysis.groups),
        (None, Some(qualifier)) => {
            row.append(
                &wrapping(gtk::Label::builder().label(qualifier))
                    .css_classes(["gloss-qualifier"])
                    .build(),
            );
        }
        (None, None) => {}
    }
    row
}

/// The row of `sense`: its number, its part of speech as a dim tag when
/// known, and its definition, with the mark above
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
    let definition = wrapping(gtk::Label::builder().label(sense.definition))
        .css_classes(["definition-text"])
        .hexpand(true)
        .build();
    if let Some(pos) = sense.pos {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.append(
            &gtk::Label::builder()
                .label(pos)
                .valign(gtk::Align::Start)
                .css_classes(["pos"])
                .build(),
        );
        line.append(&definition);
        text.append(&line);
    } else {
        text.append(&definition);
    }
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(&number);
    content.append(&text);
    if let Some(count) = &sense.count {
        content.append(
            &gtk::Label::builder()
                .label(count.as_str())
                .valign(gtk::Align::Start)
                .css_classes(["scount"])
                .build(),
        );
    }
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

/// What stands under the chosen sense: its synonyms, each a link that puts
/// it into `field`, the button that translates its definition when `action`
/// is shown, and the online translation of the definition once asked for.
fn explained_box(
    explained: &Explained<'_>,
    action: Option<&gtk::Button>,
    field: &gtk::Entry,
) -> gtk::Box {
    let below = explained_tail(action, explained.definition.as_ref());
    let synonyms = gtk::Box::new(gtk::Orientation::Vertical, 6);
    fill_listed(
        &synonyms,
        Some(&explained.synonyms),
        &["pill", "outline"],
        Some(field),
        SYNONYM_PREVIEW,
    );
    below.prepend(&synonyms);
    below
}

/// What stands under a chosen sense or gloss without its synonyms: the
/// button that translates it when `action` is shown, and the online
/// translation `definition` once asked for.
fn explained_tail(action: Option<&gtk::Button>, definition: Option<&Block<'_>>) -> gtk::Box {
    let below = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .css_classes(["explained"])
        .build();
    if let Some(action) = action {
        below.append(action);
    }
    if let Some(block) = definition {
        below.append(&definition_block(block));
    }
    below
}

/// The online translation of a definition or a gloss: its label and, under
/// it, its line.
fn definition_block(block: &Block<'_>) -> gtk::Box {
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
    translated
}

/// The result area, which scrolls: the card of the word, with the answer
/// first, the tip for a lone word, the list of senses and, for a language
/// beside English, the synonyms of the word no sense took and its English
/// glosses.
struct ResultArea {
    area: gtk::ScrolledWindow,
    /// The raised surface; shown while the article or the answer is.
    card: gtk::Box,
    article: gtk::Box,
    headword: gtk::Label,
    /// Beside the headword: its first pronunciation and the toggle that
    /// shows the others.
    lead: gtk::Box,
    /// Under the headword: the other pronunciations, a row each.
    pronunciations: gtk::Box,
    line: gtk::Label,
    /// The answer: what it is, the Russian, large, the sentence under it,
    /// the dictionary's other translations and where it came from.
    answer: AnswerBox,
    /// Without a DeepL key: what a key would give, and the button that adds
    /// one.
    no_key: gtk::Box,
    add_key: gtk::Button,
    /// For a verb form, its analyses, a line per lemma.
    analysis: gtk::Box,
    /// For a word translated in its sentence: the dictionary's translations
    /// of the word as a whole, beside their label.
    whole: gtk::Box,
    whole_pills: gtk::Box,
    /// Under the card of a word looked up without its sentence.
    tip: gtk::Box,
    senses: gtk::Box,
    /// What stands above the list: the kind and the language of the senses,
    /// set for each result.
    senses_caption: gtk::Label,
    hint: gtk::Label,
    rows: gtk::Box,
    /// Under the senses, for a language beside English: the synonym
    /// meanings of the word that no sense took, behind a disclosure.
    word_synonyms: gtk::Box,
    /// Under them, for a language beside English: the English glosses, as
    /// rows to choose.
    glosses: gtk::Box,
    definition_action: gtk::Button,
}

/// The widgets of the answer; see [`ResultArea::answer`].
struct AnswerBox {
    root: gtk::Box,
    label: gtk::Label,
    word: gtk::Label,
    word_spinner: gtk::Spinner,
    sentence: gtk::Label,
    sentence_spinner: gtk::Spinner,
    pills: gtk::Box,
    missing: gtk::Label,
    source_icon: gtk::Image,
    source: gtk::Label,
}

/// A line of the answer with the spinner shown while it is pending.
fn answer_line(class: &str) -> (gtk::Box, gtk::Label, gtk::Spinner) {
    let row = gtk::Box::builder().spacing(8).build();
    let spinner = gtk::Spinner::builder()
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    let label = wrapping(gtk::Label::builder())
        .selectable(true)
        .hexpand(true)
        .css_classes([class])
        .build();
    row.append(&spinner);
    row.append(&label);
    (row, label, spinner)
}

fn answer_box() -> AnswerBox {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .css_classes(["answer"])
        .visible(false)
        .build();
    let label = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["answer-label"])
        .build();
    let (word_row, word, word_spinner) = answer_line("answer-word");
    let (sentence_row, sentence, sentence_spinner) = answer_line("answer-sentence");
    let pills = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pills.add_css_class("answer-pills");
    let missing = wrapping(gtk::Label::builder())
        .css_classes(["hint"])
        .build();
    let source_row = gtk::Box::builder()
        .spacing(6)
        .css_classes(["online-source"])
        .build();
    let source_icon = gtk::Image::from_icon_name("web-browser-symbolic");
    let source = gtk::Label::new(None);
    source_row.append(&source_icon);
    source_row.append(&source);
    root.append(&label);
    root.append(&word_row);
    root.append(&sentence_row);
    root.append(&pills);
    root.append(&missing);
    root.append(&source_row);
    AnswerBox {
        root,
        label,
        word,
        word_spinner,
        sentence,
        sentence_spinner,
        pills,
        missing,
        source_icon,
        source,
    }
}

/// Puts `line` into `label`, styled by how it reads, with `spinner` turning
/// while it is pending; hides the row without a line.
fn fill_answer_line(label: &gtk::Label, spinner: &gtk::Spinner, line: Option<&AnswerLine<'_>>) {
    let row = label.parent().expect("the row of the line");
    row.set_visible(line.is_some());
    let Some((text, state)) = line else {
        label.set_text("");
        spinner.set_spinning(false);
        return;
    };
    label.set_text(text);
    label.set_selectable(*state != OnlineState::Pending);
    for (class, on) in [
        ("online-pending", *state == OnlineState::Pending),
        ("online-failed", *state == OnlineState::Failed),
    ] {
        if on {
            label.add_css_class(class);
        } else {
            label.remove_css_class(class);
        }
    }
    spinner.set_visible(*state == OnlineState::Pending);
    spinner.set_spinning(*state == OnlineState::Pending);
}

fn result_area() -> ResultArea {
    let headword = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .css_classes(["headword"])
        .build();
    let lead = gtk::Box::builder()
        .spacing(8)
        .valign(gtk::Align::Center)
        .css_classes(["lead"])
        .build();
    let wordrow = Wrap::new();
    wordrow.add_css_class("wordrow");
    wordrow.append(&headword);
    wordrow.append(&lead);
    let pronunciations = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .css_classes(["pronunciations"])
        .visible(false)
        .build();
    let line = wrapping(gtk::Label::builder())
        .selectable(true)
        .css_classes(["result-line"])
        .visible(false)
        .build();
    let answer = answer_box();
    let add_key = gtk::Button::builder()
        .label(ADD_KEY_LABEL)
        .valign(gtk::Align::Center)
        .focus_on_click(false)
        .css_classes(["settings-button"])
        .build();
    let no_key = gtk::Box::builder()
        .spacing(12)
        .css_classes(["offline-note"])
        .visible(false)
        .build();
    no_key.append(&gtk::Image::from_icon_name("network-offline-symbolic"));
    let no_key_text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    no_key_text.append(&wrapping(gtk::Label::builder().label(NO_KEY_LINE)).build());
    no_key_text.append(
        &wrapping(gtk::Label::builder().label(NO_KEY_HINT))
            .css_classes(["hint"])
            .build(),
    );
    no_key.append(&no_key_text);
    no_key.append(&add_key);
    let analysis = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .css_classes(["analysis"])
        .visible(false)
        .build();
    let whole_pills = gtk::Box::builder().hexpand(true).build();
    let whole = gtk::Box::builder()
        .spacing(10)
        .css_classes(["whole"])
        .visible(false)
        .build();
    whole.append(
        &gtk::Label::builder()
            .label(WHOLE_WORD_LABEL)
            .valign(gtk::Align::Start)
            .css_classes(["hint", "whole-label"])
            .build(),
    );
    whole.append(&whole_pills);
    let article = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(14)
        .visible(false)
        .build();
    article.append(&wordrow);
    article.append(&pronunciations);
    article.append(&line);
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(14)
        .css_classes(["article-card"])
        .visible(false)
        .build();
    card.append(&article);
    card.append(&answer.root);
    card.append(&no_key);
    card.append(&analysis);
    card.append(&whole);
    let tip = gtk::Box::builder()
        .spacing(10)
        .css_classes(["tip"])
        .visible(false)
        .build();
    tip.append(&gtk::Image::from_icon_name("format-justify-left-symbolic"));
    tip.append(
        &wrapping(gtk::Label::builder().label(LONE_WORD_TIP))
            .hexpand(true)
            .build(),
    );
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
    let senses_caption = caption(SENSES_LABEL);
    senses.append(&senses_caption);
    senses.append(&hint);
    senses.append(&rows);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .css_classes(["result-content"])
        .build();
    let word_synonyms = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .visible(false)
        .build();
    let glosses = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .visible(false)
        .build();
    content.append(&card);
    content.append(&tip);
    content.append(&senses);
    content.append(&word_synonyms);
    content.append(&glosses);
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
        card,
        article,
        headword,
        lead,
        pronunciations,
        line,
        answer,
        no_key,
        add_key,
        analysis,
        whole,
        whole_pills,
        tip,
        senses,
        senses_caption,
        hint,
        rows,
        word_synonyms,
        glosses,
        definition_action,
    }
}

/// Puts `answer` into the answer of `result`, the note without a key and
/// the row of the word as a whole; hides them for `None`. Each translation
/// is a link that puts it into `field`.
fn fill_answer(result: &ResultArea, answer: Option<&Answer<'_>>, field: &gtk::Entry) {
    let widgets = &result.answer;
    let shown = answer.is_some_and(|answer| {
        answer.word.is_some() || answer.sentence.is_some() || answer.missing.is_some()
    });
    widgets.root.set_visible(shown);
    result
        .no_key
        .set_visible(answer.is_some_and(|answer| answer.no_key));
    clear(&widgets.pills);
    clear(&result.whole_pills);
    let Some(answer) = answer else {
        result.whole.set_visible(false);
        return;
    };
    widgets.label.set_text(answer.label);
    fill_answer_line(&widgets.word, &widgets.word_spinner, answer.word.as_ref());
    fill_answer_line(
        &widgets.sentence,
        &widgets.sentence_spinner,
        answer.sentence.as_ref(),
    );
    if !answer.pills.is_empty() {
        widgets.pills.append(&pills(
            answer.pills,
            &["pill", "small"],
            Some(field),
            usize::MAX,
            answer.label,
        ));
    }
    widgets.pills.set_visible(!answer.pills.is_empty());
    widgets.missing.set_text(answer.missing.unwrap_or_default());
    widgets.missing.set_visible(answer.missing.is_some());
    widgets.source.set_text(answer.source);
    widgets
        .source_icon
        .set_icon_name(Some(if answer.source == OFFLINE_SOURCE {
            "accessories-dictionary-symbolic"
        } else {
            "web-browser-symbolic"
        }));
    match &answer.whole {
        Some(Listed::Items { items, .. }) => {
            result.whole_pills.append(&pills(
                items,
                &["pill", "small"],
                Some(field),
                usize::MAX,
                WHOLE_WORD_LABEL,
            ));
            result.whole.set_visible(true);
        }
        Some(Listed::Missing(line)) => {
            result.whole_pills.append(
                &wrapping(gtk::Label::builder().label(*line))
                    .css_classes(["hint"])
                    .build(),
            );
            result.whole.set_visible(true);
        }
        None => result.whole.set_visible(false),
    }
}

/// The source label and the status on one line, the source line when a
/// capture or a read of Android Studio gave no text, then the binding row:
/// its line, the detail and Retry; the line of the indexes when there is
/// something to say about them; then the field for a word or a sentence, the text of the field with
/// its words as links, and the result, which scrolls: the card of the word
/// with its translations, the online texts and the list of senses, the
/// chosen one explained under its row.
pub struct View {
    root: gtk::Box,
    source: gtk::Label,
    status: gtk::Label,
    /// Why the last capture or read of Android Studio gave no text, shared
    /// by both: they never run together. Hidden until one gave nothing, and
    /// again once something new happens in the window.
    source_failure: SourceFailure,
    binding: gtk::Box,
    binding_text: gtk::Label,
    binding_detail: gtk::Label,
    retry: gtk::Button,
    /// What is said about the dictionary indexes; hidden when both are open.
    indexes: gtk::Label,
    field: gtk::Entry,
    /// Left of the field: puts the text before into it.
    back: gtk::Button,
    /// Left of the field, after Back: puts the text after into it.
    forward: gtk::Button,
    /// The handler that reports the changes of the field; blocked while a
    /// recognized text is put in.
    sentence_handler: RefCell<Option<glib::SignalHandlerId>>,
    /// Beside the field: the switch of the reading language, a button for
    /// each language.
    languages: Vec<(Language, gtk::ToggleButton)>,
    /// Set while [`View::set_language`] presses a button, so that the
    /// change is not reported as the user's.
    setting_language: Rc<Cell<bool>>,
    /// Beside the field: takes a screen capture.
    capture: gtk::Button,
    sentence: gtk::Label,
    result: ResultArea,
    /// Runs with the index of a sense when its row is clicked; the rows are
    /// built anew for each result and each choice.
    sense_chosen: Rc<RefCell<Option<SenseChosen>>>,
    /// Runs with the index of an English gloss when its row is clicked.
    gloss_chosen: Rc<RefCell<Option<SenseChosen>>>,
    /// Runs with the English word of the chosen gloss when its pill is
    /// clicked.
    look_up_english: Rc<RefCell<Option<LookUp>>>,
    /// The recording that plays now; a new result stops it.
    player: Player,
    /// The [`Shown::generation`] of the result whose pronunciations are
    /// shown: a choice in the same result leaves them, and their playback,
    /// as they are.
    pronounced: Cell<Option<u64>>,
    /// The dictionary's translations of the shown result and the online
    /// rows, kept apart because they come apart: the answer is made of
    /// both.
    translations: RefCell<Option<Listed<'static>>>,
    online: RefCell<Vec<OnlineRow<'static>>>,
    /// Whether the synonyms of the word that no sense took are shown; a new
    /// result closes them, a choice in the same one keeps them.
    synonyms_open: Rc<Cell<bool>>,
    /// The text of the field is one word, with no sentence around it.
    lone_word: Cell<bool>,
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
        let (meta, source, status) = meta_row();
        root.append(&meta);

        // Wraps, so an install command fits. Not selectable, like the line
        // of the indexes: it must not take the focus from the field.
        let source_failure = source_failure();
        root.append(&source_failure.row);

        let binding_row = binding_row();
        root.append(&binding_row.row);

        // Hidden until the indexes were checked. Not selectable: it must not
        // take the focus from the field.
        let indexes = banner();
        root.append(&indexes);

        let FieldRow {
            row: field_row,
            field,
            back,
            forward,
            languages,
            capture,
        } = field_row();
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
            source_failure,
            binding: binding_row.row,
            binding_text: binding_row.text,
            binding_detail: binding_row.detail,
            retry: binding_row.retry,
            indexes,
            field,
            back,
            forward,
            sentence_handler: RefCell::new(None),
            languages,
            setting_language: Rc::new(Cell::new(false)),
            capture,
            sentence,
            result,
            sense_chosen: Rc::new(RefCell::new(None)),
            gloss_chosen: Rc::new(RefCell::new(None)),
            look_up_english: Rc::new(RefCell::new(None)),
            player: Rc::new(RefCell::new(None)),
            pronounced: Cell::new(None),
            translations: RefCell::new(None),
            online: RefCell::new(Vec::new()),
            synonyms_open: Rc::new(Cell::new(false)),
            lone_word: Cell::new(false),
        };
        view.render(state, Source::Manual);
        view.render_binding(binding);
        view
    }

    /// The box holding everything the window shows.
    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    /// Shows the texts of `state` for a text from `source`. The source line
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

    /// Shows `line`, why a capture or a read of Android Studio gave no
    /// text, under the source and the status. [`View::render`] leaves it;
    /// [`View::clear_source_failure`] hides it.
    /// A line that names a runnable command shows Copy beside it.
    pub fn render_source_failure(&self, line: &str) {
        self.set_source_failure(line);
        self.source_failure.row.set_visible(true);
    }

    /// Hides the source line.
    pub fn clear_source_failure(&self) {
        self.source_failure.row.set_visible(false);
        self.set_source_failure("");
    }

    /// Puts `line` into the source line and gives Copy the command it
    /// names, if any, with Copy's label back to its own.
    fn set_source_failure(&self, line: &str) {
        let failure = &self.source_failure;
        failure.line.set_text(line);
        let command = command_in(line);
        failure.copy.set_visible(command.is_some());
        failure.copy.set_label(COPY_LABEL);
        failure.command.replace(command);
    }

    /// Runs `changed` with the text of the field each time the user changes
    /// it; [`View::set_field_text`] does not run it. Called once.
    pub fn connect_sentence_changed(&self, changed: impl Fn(&str) + 'static) {
        let handler = self
            .field
            .connect_changed(move |field| changed(field.text().as_str()));
        self.sentence_handler.replace(Some(handler));
    }

    /// Runs `chosen` with the language each time the user presses the button
    /// of another language in the switch; [`View::set_language`] does not
    /// run it. Called once.
    pub fn connect_language_chosen(&self, chosen: impl Fn(Language) + 'static) {
        let chosen = Rc::new(chosen);
        for (language, button) in &self.languages {
            let (language, chosen) = (*language, chosen.clone());
            let setting = self.setting_language.clone();
            button.connect_toggled(move |button| {
                // The button let go in a group is toggled too.
                if button.is_active() && !setting.get() {
                    chosen(language);
                }
            });
        }
    }

    /// Presses the button of `language` in the switch, without running the
    /// handler of [`View::connect_language_chosen`].
    pub fn set_language(&self, language: Language) {
        self.setting_language.set(true);
        for (shown, button) in &self.languages {
            if *shown == language {
                button.set_active(true);
            }
        }
        self.setting_language.set(false);
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

    /// Runs `back` each time Back is pressed.
    pub fn connect_back(&self, back: impl Fn() + 'static) {
        self.back.connect_clicked(move |_| back());
    }

    /// Runs `forward` each time Forward is pressed.
    pub fn connect_forward(&self, forward: impl Fn() + 'static) {
        self.forward.connect_clicked(move |_| forward());
    }

    /// Whether Back and Forward can be pressed.
    pub fn set_history(&self, can_back: bool, can_forward: bool) {
        self.back.set_sensitive(can_back);
        self.forward.set_sensitive(can_forward);
    }

    /// The hotkeys of Back and Forward, Alt+Left and Alt+Right, for the
    /// window to take in its capture phase, before a focused child: each
    /// presses its button when that can be pressed, and is left to the
    /// child otherwise. Ctrl+Left and Ctrl+Right stay the field's.
    pub fn history_keys(&self) -> gtk::EventControllerKey {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (back, forward) = (self.back.clone(), self.forward.clone());
        keys.connect_key_pressed(move |_, key, _, state| {
            let held = state
                & (gdk::ModifierType::SHIFT_MASK
                    | gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SUPER_MASK
                    | gdk::ModifierType::HYPER_MASK);
            if held != gdk::ModifierType::ALT_MASK {
                return glib::Propagation::Proceed;
            }
            let button = match key {
                gdk::Key::Left | gdk::Key::KP_Left => &back,
                gdk::Key::Right | gdk::Key::KP_Right => &forward,
                _ => return glib::Propagation::Proceed,
            };
            if !button.is_sensitive() {
                return glib::Propagation::Proceed;
            }
            button.emit_clicked();
            glib::Propagation::Stop
        });
        keys
    }

    /// Puts `text` into the field as a paste does: it replaces the text,
    /// the handler of [`View::connect_sentence_changed`] runs, and then the
    /// field is submitted.
    pub fn put_into_field(&self, text: &str) {
        put_into_field(&self.field, text);
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

    /// Runs `chosen` with the index of an English gloss each time its row is
    /// clicked.
    pub fn connect_gloss_chosen(&self, chosen: impl Fn(usize) + 'static) {
        self.gloss_chosen.replace(Some(Rc::new(chosen)));
    }

    /// Runs `look_up` with the English word of the chosen gloss each time
    /// its pill is clicked.
    pub fn connect_look_up_english(&self, look_up: impl Fn(&str) + 'static) {
        self.look_up_english.replace(Some(Rc::new(look_up)));
    }

    /// Runs `action` each time the button that translates the definition of
    /// the chosen sense or gloss is clicked.
    pub fn connect_definition_action(&self, action: impl Fn() + 'static) {
        self.result
            .definition_action
            .connect_clicked(move |_| action());
    }

    /// Runs `add` each time Add DeepL key… is pressed in the card.
    pub fn connect_add_key(&self, add: impl Fn() + 'static) {
        self.result.add_key.connect_clicked(move |_| add());
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
        self.release_focus();
        let headword = selected
            .and_then(|index| words.get(index))
            .and_then(|word| sentence.get(word.clone()))
            .unwrap_or_default();
        self.result.headword.set_text(headword);
        self.result.headword.set_visible(!headword.is_empty());
        self.lone_word.set(words.len() == 1);
        if words.len() != 1 {
            self.result.tip.set_visible(false);
        }
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
        self.release_focus();
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
            self.hide_result();
            return;
        };
        let texts = choice_texts(
            lookup,
            shown.choice(),
            shown.definition(),
            key_file,
            shown.elsewhere(),
        );
        // A result that is not an article has its line; an article of
        // another language than the chosen one has the line that says so,
        // in the same place, above the article.
        let line = texts.line.as_deref().or(texts.elsewhere.as_deref());
        result.line.set_text(line.unwrap_or_default());
        result.line.set_visible(line.is_some());
        if self.pronounced.replace(Some(shown.generation())) != Some(shown.generation()) {
            stop(&self.player);
            self.synonyms_open.set(false);
            fill_pronunciations(
                &result.lead,
                &result.pronunciations,
                &texts.pronunciations,
                &result.headword.text(),
                &self.player,
            );
        }
        fill_analysis(&result.analysis, &texts.analysis, &self.field);
        result.analysis.set_visible(!texts.analysis.is_empty());
        self.translations
            .replace(texts.translations.clone().map(Listed::into_owned));
        result.article.set_visible(true);
        result
            .tip
            .set_visible(self.lone_word.get() && texts.senses.len() > 1);

        self.render_senses(&texts, visible);
        // Hidden for English, which has neither.
        fill_synonym_groups(
            &result.word_synonyms,
            texts.word_synonyms.as_ref(),
            &self.field,
            &self.synonyms_open,
        );
        result
            .word_synonyms
            .set_visible(texts.word_synonyms.is_some());
        fill_glosses(
            &result.glosses,
            texts.glosses.as_ref(),
            texts.gloss.as_ref(),
            &forwarding(&self.gloss_chosen),
            &{
                let slot = self.look_up_english.clone();
                Rc::new(move |word: &str| {
                    // Cloned out first: the handler builds the rows anew.
                    let handler = slot.borrow().clone();
                    if let Some(handler) = handler {
                        handler(word);
                    }
                }) as LookUp
            },
            &self.field,
        );
        result.glosses.set_visible(texts.glosses.is_some());
        if shown.choice().is_none() {
            result.area.vadjustment().set_value(0.0);
        }
        self.show_area();
    }

    /// Hides what [`render_result`](Self::render_result) shows when there
    /// is no lookup: the article, the senses, the word-level synonyms and
    /// the glosses.
    fn hide_result(&self) {
        let result = &self.result;
        stop(&self.player);
        self.pronounced.set(None);
        clear(&result.lead);
        clear(&result.pronunciations);
        result.pronunciations.set_visible(false);
        result.article.set_visible(false);
        result.analysis.set_visible(false);
        result.tip.set_visible(false);
        result.senses.set_visible(false);
        self.translations.replace(None);
        clear(&result.word_synonyms);
        result.word_synonyms.set_visible(false);
        clear(&result.glosses);
        result.glosses.set_visible(false);
        self.show_area();
    }

    /// Shows the senses of `texts`: their caption, the hint, and a row per
    /// sense with what stands under the chosen one, the definition action
    /// among it when `visible`.
    fn render_senses(&self, texts: &ResultTexts<'_>, visible: bool) {
        let result = &self.result;
        result.senses_caption.set_text(texts.senses_label);
        result.hint.set_text(texts.hint.unwrap_or_default());
        result.hint.set_visible(texts.hint.is_some());
        let chosen = forwarding(&self.sense_chosen);
        for sense in &texts.senses {
            result.rows.append(&sense_row(sense, &chosen));
            if let Some(explained) = &sense.explained {
                let action = visible.then_some(&result.definition_action);
                result
                    .rows
                    .append(&explained_box(explained, action, &self.field));
            }
        }
        result
            .senses
            .set_visible(!texts.senses.is_empty() || texts.hint.is_some());
    }

    /// Shows the online texts `online` in the answer, with the
    /// dictionary's translations of the shown result.
    pub fn render_online(&self, online: &OnlineTexts<'_>, key_file: Option<&std::path::Path>) {
        self.release_focus();
        self.online.replace(
            online_rows(online, key_file)
                .into_iter()
                .map(OnlineRow::into_owned)
                .collect(),
        );
        self.show_area();
    }

    /// Fills the answer from what is kept of the result and the online
    /// texts, then shows the card and the result area while they have
    /// something in them.
    fn show_area(&self) {
        let result = &self.result;
        {
            let translations = self.translations.borrow();
            let online = self.online.borrow();
            fill_answer(
                result,
                answer_texts(translations.as_ref(), &online).as_ref(),
                &self.field,
            );
        }
        // The flags of their own: `is_visible` would also ask the hidden
        // area itself.
        let shown = result.article.get_visible()
            || result.answer.root.get_visible()
            || result.no_key.get_visible();
        result.card.set_visible(shown);
        result.area.set_visible(shown);
    }

    /// Puts the focus back into the field when it is in the result area,
    /// before the area's widgets are removed or hidden. Otherwise GTK moves
    /// it on its own after the next frame, onto a selectable label, and
    /// warns that the widget that had it is gone (a `gtk_widget_is_ancestor`
    /// critical).
    fn release_focus(&self) {
        let focus = self.root.root().and_then(|root| root.focus());
        if focus.is_some_and(|focus| focus.is_ancestor(&self.result.area)) {
            self.focus_field();
        }
    }

    /// Puts the keyboard focus into the field, so a paste lands there. The
    /// text is not selected: a selection would become the primary selection.
    pub fn focus_field(&self) {
        self.field.grab_focus_without_selecting();
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use addict_core::{
        dictionary::{
            Article, Audio, Dictionary, EntryIdentity, LexicalEntry, PartOfSpeech, Pronunciation,
            Sample,
        },
        input::WindowState,
        sentence::words,
    };
    use gtk::{glib, prelude::*};

    use super::{View, fill_listed};
    use crate::view::{
        BACK_TOOLTIP, COPIED_LABEL, COPY_LABEL, FORWARD_TOOLTIP, Listed, PLAY_TOOLTIP,
        PLAYBACK_FAILED, SHOW_LESS,
    };
    use crate::{
        style,
        view::{Binding, Lookup, Online, OnlineTexts, Shown},
    };

    /// Draws the window with the sample article of "bank" in a sentence,
    /// its first sense chosen, into the PNG file `ADDICT_SHOT` names. Run
    /// by hand, and alone, to see the look: it opens a window on the
    /// display, and GTK starts once per process, on one thread.
    /// `ADDICT_SHOT_SCHEME=dark` or `light` draws it in that scheme instead
    /// of the desktop's.
    #[test]
    #[ignore = "opens a window; run alone with ADDICT_SHOT=<file.png>"]
    fn draws_the_window_into_a_png() {
        let path = std::env::var("ADDICT_SHOT").expect("ADDICT_SHOT");
        gtk::init().expect("a display");
        style::install();
        let scheme = match std::env::var("ADDICT_SHOT_SCHEME").as_deref() {
            Ok("dark") => Some(gtk::InterfaceColorScheme::Dark),
            Ok("light") => Some(gtk::InterfaceColorScheme::Light),
            _ => None,
        };
        if let Some(scheme) = scheme {
            gtk::Settings::default()
                .expect("settings")
                .set_gtk_interface_color_scheme(scheme);
        }
        let view = View::new(WindowState::Empty, &Binding::Bound);
        let sentence = "I sat on the bank of the river.";
        let words = words(sentence);
        let bank = words
            .iter()
            .position(|word| &sentence[word.clone()] == "bank")
            .expect("bank");
        view.set_field_text(sentence);
        view.render_sentence(sentence, &words, Some(bank));
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(pronounced_bank())));
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
            .titlebar(&crate::resident::header_bar("Addict"))
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

    /// The sample article of "bank" with a part of speech on each sense and
    /// two pronunciations, one with a recording.
    fn pronounced_bank() -> Article {
        let mut article = Sample.lookup("bank").expect("the sample article");
        for (sense, pos) in article.senses.iter_mut().zip(["n", "n", "v"]) {
            sense.part_of_speech = PartOfSpeech::from_label(pos);
        }
        let pronunciation = |label: &str, audio: Vec<Audio>| Pronunciation {
            text: Some("bæŋk".to_owned()),
            notation: Some("IPA".to_owned()),
            labels: vec![label.to_owned()],
            audio,
            ..Pronunciation::default()
        };
        article.entries = vec![LexicalEntry {
            identity: EntryIdentity {
                source: "test".to_owned(),
                headword: "bank".to_owned(),
                id: "1".to_owned(),
            },
            part_of_speech: None,
            pronunciations: vec![
                pronunciation(
                    "US",
                    vec![Audio {
                        url: "https://example.org/bank.ogg".to_owned(),
                        format: Some("ogg".to_owned()),
                    }],
                ),
                pronunciation("GB", Vec::new()),
            ],
        }];
        article
    }

    /// The buttons with `class` under `widget`, at any depth.
    fn buttons(widget: &gtk::Widget, class: &str) -> Vec<gtk::Button> {
        let mut found = Vec::new();
        let mut next = widget.first_child();
        while let Some(child) = next {
            if child.has_css_class(class)
                && let Ok(button) = child.clone().downcast::<gtk::Button>()
            {
                found.push(button);
            }
            found.extend(buttons(&child, class));
            next = child.next_sibling();
        }
        found
    }

    /// What a "more" toggle says: the label inside it.
    fn toggle_text(toggle: &gtk::ToggleButton) -> String {
        toggle
            .child()
            .and_then(|content| content.first_child())
            .and_downcast::<gtk::Label>()
            .expect("the label of the toggle")
            .label()
            .into()
    }

    /// The children of `widget`, in order.
    fn children(widget: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
        let mut children = Vec::new();
        let mut next = widget.first_child();
        while let Some(child) = next {
            next = child.next_sibling();
            children.push(child);
        }
        children
    }

    /// A pronunciation with a recording has a play button and one without
    /// has none; a new result stops what plays and takes the rows away,
    /// a choice in the same result keeps them. Run alone: GTK starts once
    /// per process, on one thread.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn only_a_recording_has_a_play_button() {
        gtk::init().expect("a display");
        let view = View::new(WindowState::Empty, &Binding::Bound);
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(pronounced_bank())));
        view.render_result(&shown, None);
        // The first stands beside the headword, the second under it, behind
        // the toggle.
        let lead = view.result.lead.clone().upcast::<gtk::Widget>();
        let rest = view.result.pronunciations.clone().upcast::<gtk::Widget>();
        let leading = children(&lead);
        assert_eq!(leading.len(), 2);
        assert_eq!(buttons(&leading[0], "play").len(), 1);
        let toggle = leading[1]
            .clone()
            .downcast::<gtk::ToggleButton>()
            .expect("the toggle");
        assert_eq!(toggle_text(&toggle), "1 more");
        let under = children(&rest);
        assert_eq!(under.len(), 1);
        assert!(buttons(&under[0], "play").is_empty());
        assert!(!rest.is_visible());
        toggle.set_active(true);
        assert!(rest.is_visible());
        let play = &buttons(&leading[0], "play")[0];
        assert_eq!(play.tooltip_text().as_deref(), Some(PLAY_TOOLTIP));
        assert!(!play.gets_focus_on_click());
        // A file that is not there: the stream fails, and says so in the
        // tooltip, never in a dialog.
        let missing = std::env::temp_dir().join("addict-no-such-recording.ogg");
        let mut article = pronounced_bank();
        article.entries[0].pronunciations[0].audio[0].url =
            gtk::gio::File::for_path(&missing).uri().into();
        shown.show(Some(Lookup::Article(article)));
        view.render_result(&shown, None);
        let play = buttons(&lead, "play").remove(0);
        play.emit_clicked();
        assert!(view.player.borrow().is_some());
        let context = glib::MainContext::default();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !play.has_css_class("failed") && std::time::Instant::now() < until {
            context.iteration(false);
        }
        assert!(play.has_css_class("failed"));
        assert_eq!(play.tooltip_text().as_deref(), Some(PLAYBACK_FAILED));
        // A choice keeps the rows and the stream; a new result stops it.
        shown.choose(0);
        view.render_result(&shown, None);
        assert!(view.player.borrow().is_some());
        assert!(play.parent().is_some());
        shown.show(Some(Lookup::Article(
            Sample.lookup("bank").expect("the sample article"),
        )));
        view.render_result(&shown, None);
        assert!(view.player.borrow().is_none());
        assert!(lead.first_child().is_none());
        assert!(rest.first_child().is_none());
        assert!(!rest.is_visible());
        // Five variants: one shown, four behind the toggle.
        let mut article = pronounced_bank();
        article.entries[0].pronunciations = (1..=5)
            .map(|n| Pronunciation {
                text: Some(format!("v{n}")),
                ..Pronunciation::default()
            })
            .collect();
        shown.show(Some(Lookup::Article(article)));
        view.render_result(&shown, None);
        let leading = children(&lead);
        assert_eq!(leading.len(), 2);
        assert_eq!(children(&rest).len(), 4);
        assert!(!rest.is_visible());
        let toggle = leading[1]
            .clone()
            .downcast::<gtk::ToggleButton>()
            .expect("the toggle");
        assert_eq!(toggle_text(&toggle), "4 more");
        toggle.set_active(true);
        assert!(rest.is_visible());
        assert_eq!(toggle_text(&toggle), SHOW_LESS);
    }

    #[test]
    #[ignore = "needs a display; run alone"]
    fn synonym_meanings_expand_wrap_collapse_and_submit_revealed_members() {
        use crate::view::SynonymTexts;
        use addict_dictionary::{format::SenseMetadata, index::SynonymGroup};
        gtk::init().expect("a display");
        let field = gtk::Entry::new();
        let submitted = Rc::new(Cell::new(false));
        let heard = submitted.clone();
        field.connect_activate(move |_| heard.set(true));
        let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let group = SynonymGroup {
            headword: "fer".to_owned(),
            definition: String::new(),
            metadata: SenseMetadata::from_label("(v; beure)"),
            synonyms: (1..=9).map(|n| format!("synonym {n}")).collect(),
        };
        let open = Rc::new(Cell::new(false));
        super::fill_synonym_groups(
            &container,
            Some(&SynonymTexts::Groups {
                label: "More Catalan synonyms",
                count: "1 meaning not matched to a sense".to_owned(),
                groups: vec![&group],
            }),
            &field,
            &open,
        );
        // Closed at first: the disclosure opens the cards and keeps its
        // state in `open`.
        let disclosure = container
            .first_child()
            .and_downcast::<gtk::ToggleButton>()
            .expect("the disclosure");
        let list = container.last_child().expect("the list");
        assert!(!list.is_visible());
        disclosure.set_active(true);
        assert!(list.is_visible());
        assert!(open.get());
        let meaning = list.first_child().expect("meaning");
        let head = meaning.first_child().expect("head");
        let texts: Vec<String> = children(&head)
            .into_iter()
            .filter_map(|child| child.downcast::<gtk::Label>().ok())
            .map(|label| label.label().into())
            .collect();
        assert_eq!(texts, ["fer", "verb", "beure", "9 synonyms"]);
        let pills = head.next_sibling().expect("pills");
        let mut members = children(&pills);
        let toggle = members
            .pop()
            .and_downcast::<gtk::ToggleButton>()
            .expect("toggle");
        assert!(toggle.is_focusable());
        assert_eq!(toggle_text(&toggle), "3 more");
        assert_eq!(members.len(), 9);
        assert_eq!(members.iter().filter(|child| child.is_visible()).count(), 6);
        let preview_height = pills.measure(gtk::Orientation::Vertical, 140).1;
        toggle.set_active(true);
        assert_eq!(toggle_text(&toggle), SHOW_LESS);
        assert!(members.iter().all(|child| child.is_visible()));
        assert!(pills.measure(gtk::Orientation::Vertical, 140).1 > preview_height);
        let last = members[8].clone().downcast::<gtk::Button>().expect("link");
        assert!(!last.gets_focus_on_click());
        last.emit_clicked();
        assert_eq!(field.text(), "synonym 9");
        assert!(submitted.get());
        toggle.set_active(false);
        assert_eq!(toggle_text(&toggle), "3 more");
        assert_eq!(members.iter().filter(|child| child.is_visible()).count(), 6);
        assert_eq!(
            pills.measure(gtk::Orientation::Vertical, 140).1,
            preview_height
        );
        toggle.set_active(true);
        assert!(members[8].is_visible());
    }

    /// A synonym is a link: a click puts it into the field and submits the
    /// field, as a paste does; without a field the pills are plain. Run
    /// alone: GTK starts once per process, on one thread.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_synonym_click_puts_it_into_the_field() {
        gtk::init().expect("a display");
        let field = gtk::Entry::new();
        field.set_text("I sat on the bank of the river.");
        let submitted = std::rc::Rc::new(std::cell::Cell::new(false));
        let heard = submitted.clone();
        field.connect_activate(move |_| heard.set(true));
        let items = ["shore".to_owned(), "riverside".to_owned()];
        let listed = || Listed::Items {
            label: "SYNONYMS",
            items: std::borrow::Cow::Borrowed(&items[..]),
        };
        let synonyms = gtk::Box::new(gtk::Orientation::Vertical, 6);
        fill_listed(
            &synonyms,
            Some(&listed()),
            &["pill"],
            Some(&field),
            usize::MAX,
        );
        let pills = synonyms.last_child().expect("the pills");
        let link = pills
            .last_child()
            .and_downcast::<gtk::Button>()
            .expect("a link");
        assert!(!link.gets_focus_on_click());
        link.emit_clicked();
        assert_eq!(field.text(), "riverside");
        assert!(submitted.get());

        let plain = gtk::Box::new(gtk::Orientation::Vertical, 6);
        fill_listed(&plain, Some(&listed()), &["pill"], None, usize::MAX);
        let pills = plain.last_child().expect("the pills");
        assert!(pills.first_child().and_downcast::<gtk::Label>().is_some());
    }

    /// A translation of an article is a link too: a click puts it into the
    /// field and submits the field, as a paste does. Run alone: GTK starts
    /// once per process, on one thread.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_translation_click_puts_it_into_the_field() {
        gtk::init().expect("a display");
        let view = View::new(WindowState::Empty, &Binding::Bound);
        view.set_field_text("bank");
        let submitted = std::rc::Rc::new(std::cell::Cell::new(false));
        let heard = submitted.clone();
        view.field.connect_activate(move |_| heard.set(true));
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(
            Sample.lookup("bank").expect("the sample article"),
        )));
        view.render_result(&shown, None);
        // Offline, the first translation is the answer and the others are
        // pills under it.
        let pills = view.result.answer.pills.first_child().expect("the pills");
        let link = pills
            .first_child()
            .and_downcast::<gtk::Button>()
            .expect("a link");
        assert!(!link.gets_focus_on_click());
        let word = link
            .child()
            .and_downcast::<gtk::Label>()
            .expect("the pill")
            .label();
        link.emit_clicked();
        assert_eq!(view.field.text(), word);
        assert!(submitted.get());
    }

    /// Back and Forward never take the focus from the field, cannot be
    /// pressed at first, name their hotkeys, and put a text into the field
    /// as a paste does. Run alone: GTK starts once per process, on one
    /// thread.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn the_history_buttons_follow_the_history() {
        gtk::init().expect("a display");
        let view = View::new(WindowState::Empty, &Binding::Bound);
        for (button, tooltip) in [(&view.back, BACK_TOOLTIP), (&view.forward, FORWARD_TOOLTIP)] {
            assert!(!button.can_focus());
            assert!(!button.gets_focus_on_click());
            assert!(!button.is_sensitive());
            assert_eq!(button.tooltip_text().as_deref(), Some(tooltip));
        }
        view.set_history(true, false);
        assert!(view.back.is_sensitive());
        assert!(!view.forward.is_sensitive());
        let submitted = Rc::new(Cell::new(false));
        let heard = submitted.clone();
        view.connect_submitted(move || heard.set(true));
        view.put_into_field("bank");
        assert_eq!(view.field_text(), "bank");
        assert!(submitted.get());
    }

    /// A result that replaces the one holding the focus puts the focus
    /// back into the field, so GTK never has to move it off a removed
    /// widget. Run alone: GTK starts once per process, on one thread.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn a_new_result_gives_the_focus_back_to_the_field() {
        gtk::init().expect("a display");
        let view = View::new(WindowState::Empty, &Binding::Bound);
        let window = gtk::Window::builder().child(view.root()).build();
        window.present();
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(pronounced_bank())));
        view.render_result(&shown, None);
        let play = buttons(&view.result.lead.clone().upcast(), "play").remove(0);
        assert!(play.grab_focus());
        assert_eq!(
            gtk::prelude::GtkWindowExt::focus(&window).as_ref(),
            Some(play.upcast_ref::<gtk::Widget>())
        );
        shown.show(Some(Lookup::Article(
            Sample.lookup("bank").expect("the sample article"),
        )));
        view.render_result(&shown, None);
        assert!(play.parent().is_none());
        let focus = gtk::prelude::GtkWindowExt::focus(&window).expect("a focus");
        assert!(
            focus == view.field.clone().upcast::<gtk::Widget>() || focus.is_ancestor(&view.field)
        );
        window.destroy();
    }

    /// Copy shows only beside a line that names a command, says "Copied"
    /// once pressed, and is back to "Copy" for every new line. Run alone:
    /// GTK starts once per process, on one thread.
    #[test]
    #[ignore = "needs a display; run alone"]
    fn copy_follows_the_source_line() {
        gtk::init().expect("a display");
        let view = View::new(WindowState::Empty, &Binding::Bound);
        let copy = &view.source_failure.copy;
        assert!(!copy.can_focus());
        assert!(!copy.gets_focus_on_click());
        let missing = "Catalan OCR data is not installed. Install it with: \
                       sudo dnf install tesseract-langpack-cat";
        view.render_source_failure(missing);
        assert!(copy.is_visible());
        assert_eq!(
            view.source_failure.command.borrow().as_deref(),
            Some("sudo dnf install tesseract-langpack-cat")
        );
        copy.emit_clicked();
        assert_eq!(copy.label().as_deref(), Some(COPIED_LABEL));

        view.render_source_failure("The capture was cancelled.");
        assert!(!copy.is_visible());
        assert_eq!(view.source_failure.command.borrow().as_deref(), None);

        view.render_source_failure(missing);
        assert_eq!(copy.label().as_deref(), Some(COPY_LABEL));
        view.clear_source_failure();
        assert!(!view.source_failure.row.is_visible());
        assert!(!copy.is_visible());
    }
}
