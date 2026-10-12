//! What the window says about its state: the source label and the status,
//! below them the binding row when the shortcut is not bound, then the field
//! for a word or a sentence, the text of the field with its words as links
//! and the result of the lookup of the chosen word: the senses of the word as
//! links in a list that scrolls, and below the list what is said about the
//! sense that was clicked and about the word.
//!
//! The texts live here and nowhere else; `addict-core` has none. [`texts`],
//! [`binding_texts`], [`sentence_markup`], [`senses_markup`] and
//! [`details_markup`] are pure, and so is [`Shown`], the result with the sense
//! chosen in it; they are tested without a display, and [`View`] puts their
//! results into its widgets.
//!
//! The field's paste is taken over here, and it is the only place where the
//! application reads the clipboard: the user's paste action, and nothing
//! else, starts one asynchronous read, and the text that comes replaces the
//! whole field as one line.

use std::{cell::Cell, fmt::Write as _, ops::Range, rc::Rc};

use addict_core::{
    dictionary::Article,
    input::{Source, WindowState},
    sentence,
};
use gtk::{gio, glib, prelude::*};

/// The two texts of one window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Texts {
    /// Where the input comes from.
    pub source: &'static str,
    /// What the window is doing with it.
    pub status: &'static str,
}

/// The texts of `state`. No wildcard arm: a new state or source does not
/// compile until it is given its texts.
pub fn texts(state: WindowState) -> Texts {
    match state {
        // Nothing is being read, so the window waits for the user.
        WindowState::Empty => Texts {
            source: source_label(Source::Manual),
            status: "waiting for input",
        },
        WindowState::Waiting { source } => Texts {
            source: source_label(source),
            status: "reading the source…",
        },
        WindowState::Dictionary { source } => Texts {
            source: source_label(source),
            status: "word without context",
        },
        WindowState::Contextual { source } => Texts {
            source: source_label(source),
            status: "word in its sentence",
        },
    }
}

fn source_label(source: Source) -> &'static str {
    match source {
        Source::Manual => "manual input",
        Source::AtSpi => "accessibility (AT-SPI)",
        Source::Ocr => "screen capture (OCR)",
    }
}

/// What is known about the global shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// An attempt to bind is running; its end is not known yet.
    Attempting,
    /// The shortcut is bound.
    Bound,
    /// The last attempt ended without a bound shortcut. `detail` is the
    /// worker's one-line message, shown as given.
    NotBound { detail: String },
}

/// The binding row of one binding state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingTexts<'a> {
    /// The window's own line. It never names a cause.
    pub text: &'static str,
    /// The worker's message, shown below the line as given.
    pub detail: Option<&'a str>,
    /// Whether Retry can be pressed.
    pub retry: bool,
}

/// The row of `binding`, or `None` when there is nothing to say. No wildcard
/// arm: a new state does not compile until it is given its row.
pub fn binding_texts(binding: &Binding) -> Option<BindingTexts<'_>> {
    match binding {
        Binding::Bound => None,
        Binding::Attempting => Some(BindingTexts {
            text: "Binding the shortcut…",
            detail: None,
            retry: false,
        }),
        Binding::NotBound { detail } => Some(BindingTexts {
            text: "The shortcut is not bound.",
            detail: Some(detail.as_str()).filter(|detail| !detail.trim().is_empty()),
            retry: true,
        }),
    }
}

const RETRY_LABEL: &str = "Retry";

/// What the empty field says.
const FIELD_PLACEHOLDER: &str = "Type or paste a word, or a sentence to click a word in";

/// How wide the wrapping texts ask to be, in characters.
const TEXT_WIDTH_CHARS: i32 = 56;

/// The least height of the list of senses, in pixels. The list takes
/// whatever height the window has left above this.
const SENSES_MIN_HEIGHT: i32 = 72;

/// The sentence as Pango markup: the text as given, with each of `words` (byte
/// ranges of `sentence`, in order) as a link whose target is the index of the
/// word, and the `selected` word bold. `&`, `<` and `>` of the text are
/// escaped. A range that does not fit the text makes no link.
pub fn sentence_markup(sentence: &str, words: &[Range<usize>], selected: Option<usize>) -> String {
    let mut markup = String::with_capacity(sentence.len() + words.len() * 24);
    let mut at = 0;
    for (index, word) in words.iter().enumerate() {
        let (Some(between), Some(text)) = (
            sentence.get(at..word.start),
            sentence.get(word.start..word.end),
        ) else {
            continue;
        };
        push_escaped(&mut markup, between);
        // Writing to a `String` does not fail.
        let _ = write!(markup, "<a href=\"{index}\">");
        if selected == Some(index) {
            markup.push_str("<b>");
            push_escaped(&mut markup, text);
            markup.push_str("</b>");
        } else {
            push_escaped(&mut markup, text);
        }
        markup.push_str("</a>");
        at = word.end;
    }
    push_escaped(&mut markup, sentence.get(at..).unwrap_or_default());
    markup
}

/// Appends `text` with the characters that open markup replaced.
fn push_escaped(markup: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => markup.push_str("&amp;"),
            '<' => markup.push_str("&lt;"),
            '>' => markup.push_str("&gt;"),
            other => markup.push(other),
        }
    }
}

/// Where the lookup of the chosen word stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// The lookup runs; its end is not known yet.
    Pending,
    /// The dictionary has this article of the word.
    Article(Article),
    /// The dictionary has no entry for `word`.
    NoEntry { word: String },
    /// The lookup ended without an answer.
    Unfinished,
}

/// The line of a lookup that still runs.
const PENDING: &str = "Looking up…";

/// The line of a lookup that ended without an answer. It names no cause.
const UNFINISHED: &str = "The lookup did not finish.";

/// The line of a word the dictionary has no entry for: what stands before the
/// word and what stands after it.
const NO_ENTRY_BEFORE: &str = "The dictionary has no entry for “";
const NO_ENTRY_AFTER: &str = "”.";

/// What stands below the list while no sense is chosen.
const CHOOSE_SENSE: &str = "Click a sense to see its short explanation and its synonyms.";

/// The mark above the definition of the chosen sense. It says what the text
/// is and claims no fit to the sentence.
const EXPLANATION_MARK: &str = "Short explanation — the dictionary definition of the sense you chose; \
     it is not checked against the sentence:";

/// What stands before the synonyms of the chosen sense.
const SYNONYMS_LABEL: &str = "Synonyms of this sense:";

/// The line of a chosen sense without synonyms.
const NO_SYNONYMS: &str = "The dictionary gives no synonyms for this sense.";

/// What stands before the translations. They belong to the word, not to a
/// sense.
const RUSSIAN_LABEL: &str = "Russian, for the word as a whole:";

/// The line of an article without translations.
const NO_TRANSLATION: &str = "The dictionary has no Russian translation of this word.";

/// The line of an article without senses.
const NO_DEFINITIONS: &str = "The dictionary has no English definitions of this word.";

/// The list of senses as Pango markup: one numbered line for each sense, its
/// definition a link whose target is the index of the sense, and the `chosen`
/// one bold. Empty when `lookup` is not an article with senses. The
/// definitions are escaped.
pub fn senses_markup(lookup: &Lookup, chosen: Option<usize>) -> String {
    let Lookup::Article(article) = lookup else {
        return String::new();
    };
    let mut markup = String::new();
    for (index, sense) in article.senses.iter().enumerate() {
        if index > 0 {
            markup.push('\n');
        }
        // Writing to a `String` does not fail.
        let _ = write!(markup, "{}. <a href=\"{index}\">", index + 1);
        if chosen == Some(index) {
            markup.push_str("<b>");
            push_escaped(&mut markup, &sense.definition);
            markup.push_str("</b>");
        } else {
            push_escaped(&mut markup, &sense.definition);
        }
        markup.push_str("</a>");
    }
    markup
}

/// What stands below the list of senses, as Pango markup. For an article:
/// the line asking to choose a sense, or the marked definition of the `chosen`
/// one with its synonyms, then the translations of the word; whatever the
/// article lacks has its own line. For the other results their one line. No
/// wildcard arm: a new variant does not compile until it is given its text.
/// The texts of the dictionary and the word are escaped.
pub fn details_markup(lookup: &Lookup, chosen: Option<usize>) -> String {
    let mut markup = String::new();
    match lookup {
        Lookup::Pending => push_escaped(&mut markup, PENDING),
        Lookup::Article(article) => push_article(&mut markup, article, chosen),
        Lookup::NoEntry { word } => {
            push_escaped(&mut markup, NO_ENTRY_BEFORE);
            push_escaped(&mut markup, word);
            push_escaped(&mut markup, NO_ENTRY_AFTER);
        }
        Lookup::Unfinished => push_escaped(&mut markup, UNFINISHED),
    }
    markup
}

/// The lines of `article` that stand below its list of senses. A `chosen`
/// index the article does not have is no choice.
fn push_article(markup: &mut String, article: &Article, chosen: Option<usize>) {
    let sense = chosen.and_then(|index| article.senses.get(index));
    if article.senses.is_empty() {
        push_escaped(markup, NO_DEFINITIONS);
    } else if let Some(sense) = sense {
        markup.push_str("<i>");
        push_escaped(markup, EXPLANATION_MARK);
        markup.push_str("</i>\n");
        push_escaped(markup, &sense.definition);
        markup.push('\n');
        if sense.synonyms.is_empty() {
            push_escaped(markup, NO_SYNONYMS);
        } else {
            push_listed(markup, SYNONYMS_LABEL, &sense.synonyms);
        }
    } else {
        push_escaped(markup, CHOOSE_SENSE);
    }
    markup.push_str("\n\n");
    if article.translations.is_empty() {
        push_escaped(markup, NO_TRANSLATION);
    } else {
        push_listed(markup, RUSSIAN_LABEL, &article.translations);
    }
}

/// Appends `label` and the `items` after it, separated by commas.
fn push_listed(markup: &mut String, label: &str, items: &[String]) {
    push_escaped(markup, label);
    for (index, item) in items.iter().enumerate() {
        markup.push_str(if index == 0 { " " } else { ", " });
        push_escaped(markup, item);
    }
}

/// The result the window shows and the sense chosen in it.
#[derive(Debug, Default)]
pub struct Shown {
    lookup: Option<Lookup>,
    sense: Option<usize>,
}

impl Shown {
    /// Another result, or none, is shown: the sense chosen in the one before
    /// is forgotten.
    pub fn show(&mut self, lookup: Option<Lookup>) {
        self.lookup = lookup;
        self.sense = None;
    }

    /// Chooses the sense with this index and says whether it is chosen now:
    /// `false`, with nothing changed, unless the shown result is an article
    /// that has the sense. Choosing the chosen sense again is `true`.
    pub fn choose(&mut self, index: usize) -> bool {
        let Some(Lookup::Article(article)) = &self.lookup else {
            return false;
        };
        if index >= article.senses.len() {
            return false;
        }
        self.sense = Some(index);
        true
    }

    /// The shown result.
    pub fn lookup(&self) -> Option<&Lookup> {
        self.lookup.as_ref()
    }

    /// The index of the chosen sense.
    pub fn sense(&self) -> Option<usize> {
        self.sense
    }
}

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

/// The source label and the status, one above the other, then the binding
/// row: its line, the detail and Retry; then the field for a word or a
/// sentence, the text of the field with its words as links and the result:
/// the list of senses, which scrolls, and below it the rest, which does not.
pub struct View {
    root: gtk::Box,
    source: gtk::Label,
    status: gtk::Label,
    binding: gtk::Box,
    binding_text: gtk::Label,
    binding_detail: gtk::Label,
    retry: gtk::Button,
    field: gtk::Entry,
    sentence: gtk::Label,
    /// The senses as links, inside `senses_area`.
    senses: gtk::Label,
    /// Scrolls the list; hidden when there is no list.
    senses_area: gtk::ScrolledWindow,
    /// What stands below the list.
    details: gtk::Label,
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

        let binding_text = gtk::Label::new(None);
        let binding_detail = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .build();
        let retry = gtk::Button::builder()
            .label(RETRY_LABEL)
            .halign(gtk::Align::Center)
            .build();
        let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
        row.append(&binding_text);
        row.append(&binding_detail);
        row.append(&retry);
        root.append(&row);

        let field = gtk::Entry::builder()
            .placeholder_text(FIELD_PLACEHOLDER)
            .build();
        replace_on_paste(&field);
        let sentence = gtk::Label::builder()
            .use_markup(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .max_width_chars(TEXT_WIDTH_CHARS)
            .xalign(0.0)
            .visible(false)
            .build();
        // Not selectable: a click on a link must not give the label the
        // focus.
        let senses = gtk::Label::builder()
            .use_markup(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .max_width_chars(TEXT_WIDTH_CHARS)
            .xalign(0.0)
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
        let details = gtk::Label::builder()
            .use_markup(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .max_width_chars(TEXT_WIDTH_CHARS)
            .xalign(0.0)
            .selectable(true)
            .visible(false)
            .build();
        root.append(&field);
        root.append(&sentence);
        root.append(&senses_area);
        root.append(&details);

        let view = Self {
            root,
            source,
            status,
            binding: row,
            binding_text,
            binding_detail,
            retry,
            field,
            sentence,
            senses,
            senses_area,
            details,
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
        self.sentence.connect_activate_link(move |_, target| {
            if let Ok(index) = target.parse() {
                chosen(index);
            }
            // The target is an index: GTK must not open it as a URI.
            glib::Propagation::Stop
        });
    }

    /// Runs `chosen` with the index of a sense each time its link is clicked.
    pub fn connect_sense_chosen(&self, chosen: impl Fn(usize) + 'static) {
        self.senses.connect_activate_link(move |_, target| {
            if let Ok(index) = target.parse() {
                chosen(index);
            }
            // The target is an index: GTK must not open it as a URI.
            glib::Propagation::Stop
        });
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

    /// Puts the keyboard focus into the field, so a paste lands there. The
    /// text is not selected: a selection would become the primary selection.
    pub fn focus_field(&self) {
        self.field.grab_focus_without_selecting();
    }
}

#[cfg(test)]
mod tests {
    use addict_core::{
        dictionary::{Article, Dictionary, Sample, Sense},
        input::{Source, WindowState},
        sentence::words,
    };

    use super::{
        Binding, CHOOSE_SENSE, EXPLANATION_MARK, FIELD_PLACEHOLDER, Lookup, NO_DEFINITIONS,
        NO_SYNONYMS, NO_TRANSLATION, PENDING, RUSSIAN_LABEL, SYNONYMS_LABEL, Shown, UNFINISHED,
        binding_texts, details_markup, senses_markup, sentence_markup, texts,
    };

    const SENTENCE: &str = "I sat on the bank.";

    #[test]
    fn the_empty_field_asks_for_a_word_or_a_sentence() {
        let asked = FIELD_PLACEHOLDER.to_lowercase();
        assert!(asked.contains("word"), "{asked}");
        assert!(asked.contains("sentence"), "{asked}");
        assert_eq!(asked.lines().count(), 1);
    }

    #[test]
    fn a_word_with_an_apostrophe_is_one_link() {
        let sentence = "I don\u{2019}t know.";
        assert_eq!(
            sentence_markup(sentence, &words(sentence), Some(1)),
            "<a href=\"0\">I</a> <a href=\"1\"><b>don\u{2019}t</b></a> \
             <a href=\"2\">know</a>."
        );
    }

    #[test]
    fn a_lone_word_is_shown_chosen_with_what_stands_around_it() {
        let text = " Bank. ";
        assert_eq!(
            sentence_markup(text, &words(text), Some(0)),
            " <a href=\"0\"><b>Bank</b></a>. "
        );
    }

    #[test]
    fn every_word_of_the_sentence_is_a_link_to_its_index() {
        assert_eq!(
            sentence_markup(SENTENCE, &words(SENTENCE), None),
            "<a href=\"0\">I</a> <a href=\"1\">sat</a> <a href=\"2\">on</a> \
             <a href=\"3\">the</a> <a href=\"4\">bank</a>."
        );
    }

    #[test]
    fn only_the_selected_word_is_bold() {
        let ranges = words(SENTENCE);
        for selected in 0..ranges.len() {
            let markup = sentence_markup(SENTENCE, &ranges, Some(selected));
            assert_eq!(markup.matches("<b>").count(), 1, "{markup}");
            let word = &SENTENCE[ranges[selected].clone()];
            assert!(
                markup.contains(&format!("<a href=\"{selected}\"><b>{word}</b></a>")),
                "{markup}"
            );
        }
        // An index that names no word marks nothing.
        let markup = sentence_markup(SENTENCE, &ranges, Some(ranges.len()));
        assert_eq!(markup, sentence_markup(SENTENCE, &ranges, None));
    }

    #[test]
    fn the_markup_escapes_what_would_open_markup() {
        let sentence = "a<b & c>d \"e\"";
        assert_eq!(
            sentence_markup(sentence, &words(sentence), Some(1)),
            "<a href=\"0\">a</a>&lt;<a href=\"1\"><b>b</b></a> &amp; \
             <a href=\"2\">c</a>&gt;<a href=\"3\">d</a> \"<a href=\"4\">e</a>\""
        );
        // Without words too: the text alone is escaped.
        assert_eq!(sentence_markup("<&>", &[], None), "&lt;&amp;&gt;");
        assert_eq!(sentence_markup("&amp;", &[], None), "&amp;amp;");
    }

    #[test]
    fn the_markup_keeps_the_text_between_the_words() {
        for sentence in ["", "  ", "…", " (Café) — über!\t", "Я сидел на берегу."]
        {
            let markup = sentence_markup(sentence, &words(sentence), None);
            let mut plain = String::new();
            let mut inside = false;
            for character in markup.chars() {
                match character {
                    '<' => inside = true,
                    '>' => inside = false,
                    other if !inside => plain.push(other),
                    _ => {}
                }
            }
            assert_eq!(plain, sentence, "{markup}");
        }
    }

    #[test]
    fn a_range_that_does_not_fit_the_text_makes_no_link() {
        // Past the end, inside a character, and out of order.
        let cases: [&[(usize, usize)]; 3] = [&[(2, 9)], &[(1, 2)], &[(2, 3), (0, 1)]];
        for bounds in cases {
            let ranges: Vec<_> = bounds.iter().map(|&(start, end)| start..end).collect();
            let markup = sentence_markup("né", &ranges, None);
            assert!(markup.matches("<a ").count() <= 1, "{ranges:?}: {markup}");
            assert!(
                markup.ends_with('é') || markup.ends_with("</a>"),
                "{markup}"
            );
        }
    }

    fn article() -> Article {
        Article {
            senses: vec![
                Sense {
                    definition: "first".to_owned(),
                    synonyms: vec!["one".to_owned(), "uno".to_owned()],
                },
                Sense {
                    definition: "second".to_owned(),
                    synonyms: Vec::new(),
                },
            ],
            translations: vec!["раз".to_owned(), "два".to_owned()],
        }
    }

    /// One lookup of each kind. No wildcard arm: a new variant does not
    /// compile here until it is listed.
    fn every_lookup() -> Vec<Lookup> {
        let all = vec![
            Lookup::Pending,
            Lookup::Article(article()),
            Lookup::NoEntry {
                word: "sat".to_owned(),
            },
            Lookup::Unfinished,
        ];
        for lookup in &all {
            match lookup {
                Lookup::Pending
                | Lookup::Article(_)
                | Lookup::NoEntry { .. }
                | Lookup::Unfinished => {}
            }
        }
        all
    }

    /// The text a label shows for `markup`: the tags dropped, the escapes
    /// read back.
    fn plain(markup: &str) -> String {
        let mut text = String::new();
        let mut inside = false;
        for character in markup.chars() {
            match character {
                '<' => inside = true,
                '>' => inside = false,
                other if !inside => text.push(other),
                _ => {}
            }
        }
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    }

    /// The lines shown below the list.
    fn details(lookup: &Lookup, chosen: Option<usize>) -> Vec<String> {
        plain(&details_markup(lookup, chosen))
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn every_result_has_its_own_text() {
        let texts: Vec<_> = every_lookup()
            .iter()
            .map(|lookup| details_markup(lookup, None))
            .collect();
        for (index, text) in texts.iter().enumerate() {
            assert!(!text.trim().is_empty());
            assert!(!texts[..index].contains(text), "{text}");
        }
    }

    #[test]
    fn every_result_but_an_article_is_one_line_and_has_no_list() {
        for lookup in every_lookup() {
            let article = matches!(lookup, Lookup::Article(_));
            for chosen in [None, Some(0), Some(7)] {
                assert_eq!(details(&lookup, chosen).len() == 1, !article, "{lookup:?}");
                assert_eq!(
                    senses_markup(&lookup, chosen).is_empty(),
                    !article,
                    "{lookup:?}"
                );
            }
        }
    }

    #[test]
    fn the_pending_and_unfinished_lines_keep_their_wording() {
        assert_eq!(details_markup(&Lookup::Pending, None), "Looking up…");
        assert_eq!(
            details_markup(&Lookup::Unfinished, None),
            "The lookup did not finish."
        );
        assert_eq!(PENDING, "Looking up…");
        assert_eq!(UNFINISHED, "The lookup did not finish.");
    }

    #[test]
    fn an_article_as_it_arrives_lists_every_sense_and_chooses_none() {
        let lookup = Lookup::Article(article());
        let list = senses_markup(&lookup, None);
        assert_eq!(
            list,
            "1. <a href=\"0\">first</a>\n2. <a href=\"1\">second</a>"
        );
        assert!(!list.contains("<b>"), "{list}");
    }

    #[test]
    fn an_article_as_it_arrives_asks_to_choose_and_explains_nothing() {
        let lookup = Lookup::Article(article());
        assert_eq!(
            details(&lookup, None),
            [
                CHOOSE_SENSE.to_owned(),
                String::new(),
                format!("{RUSSIAN_LABEL} раз, два"),
            ]
        );
        let markup = details_markup(&lookup, None);
        for absent in [EXPLANATION_MARK, SYNONYMS_LABEL, NO_SYNONYMS, "one", "uno"] {
            assert!(!markup.contains(absent), "{absent:?} in {markup}");
        }
    }

    #[test]
    fn the_chosen_sense_is_bold_in_the_list_and_explained_below_it() {
        let lookup = Lookup::Article(article());
        assert_eq!(
            senses_markup(&lookup, Some(0)),
            "1. <a href=\"0\"><b>first</b></a>\n2. <a href=\"1\">second</a>"
        );
        assert_eq!(
            details(&lookup, Some(0)),
            [
                EXPLANATION_MARK.to_owned(),
                "first".to_owned(),
                format!("{SYNONYMS_LABEL} one, uno"),
                String::new(),
                format!("{RUSSIAN_LABEL} раз, два"),
            ]
        );
        assert!(!details_markup(&lookup, Some(0)).contains(CHOOSE_SENSE));
    }

    #[test]
    fn only_the_chosen_sense_is_bold() {
        let article = Sample.lookup("bank").expect("bank");
        let count = article.senses.len();
        let lookup = Lookup::Article(article.clone());
        for chosen in 0..count {
            let markup = senses_markup(&lookup, Some(chosen));
            assert_eq!(markup.matches("<b>").count(), 1, "{markup}");
            let definition = &article.senses[chosen].definition;
            assert!(
                markup.contains(&format!("<a href=\"{chosen}\"><b>{definition}</b></a>")),
                "{markup}"
            );
            assert_eq!(markup.matches("<a href=").count(), count, "{markup}");
        }
    }

    #[test]
    fn a_chosen_sense_without_synonyms_says_so() {
        let lookup = Lookup::Article(article());
        assert_eq!(
            details(&lookup, Some(1)),
            [
                EXPLANATION_MARK.to_owned(),
                "second".to_owned(),
                NO_SYNONYMS.to_owned(),
                String::new(),
                format!("{RUSSIAN_LABEL} раз, два"),
            ]
        );
        assert!(!details_markup(&lookup, Some(1)).contains(SYNONYMS_LABEL));
    }

    #[test]
    fn the_electricity_sense_of_charge_has_no_synonyms() {
        let article = Sample.lookup("charge").expect("charge");
        let index = article
            .senses
            .iter()
            .position(|sense| sense.definition == "the amount of electricity held by an object")
            .expect("the sense");
        let lines = details(&Lookup::Article(article), Some(index));
        assert!(lines.contains(&NO_SYNONYMS.to_owned()), "{lines:?}");
    }

    #[test]
    fn an_article_without_translations_says_so() {
        let mut bare = article();
        bare.translations.clear();
        let lookup = Lookup::Article(bare);
        for chosen in [None, Some(0), Some(1)] {
            let lines = details(&lookup, chosen);
            assert_eq!(lines.last().map(String::as_str), Some(NO_TRANSLATION));
            assert!(
                !details_markup(&lookup, chosen).contains(RUSSIAN_LABEL),
                "{lines:?}"
            );
        }
    }

    #[test]
    fn an_article_without_senses_says_so_and_keeps_the_russian_line() {
        let mut bare = article();
        bare.senses.clear();
        let lookup = Lookup::Article(bare);
        // A sense cannot be chosen there; an index changes nothing.
        for chosen in [None, Some(0)] {
            assert_eq!(senses_markup(&lookup, chosen), "");
            assert_eq!(
                details(&lookup, chosen),
                [
                    NO_DEFINITIONS.to_owned(),
                    String::new(),
                    format!("{RUSSIAN_LABEL} раз, два"),
                ]
            );
        }
    }

    #[test]
    fn an_article_with_nothing_has_a_line_for_each_absence() {
        let lookup = Lookup::Article(Article {
            senses: Vec::new(),
            translations: Vec::new(),
        });
        assert_eq!(
            details(&lookup, None),
            [
                NO_DEFINITIONS.to_owned(),
                String::new(),
                NO_TRANSLATION.to_owned()
            ]
        );
    }

    #[test]
    fn a_sense_the_article_does_not_have_is_no_choice() {
        let lookup = Lookup::Article(article());
        assert_eq!(
            senses_markup(&lookup, Some(2)),
            senses_markup(&lookup, None)
        );
        assert_eq!(
            details_markup(&lookup, Some(2)),
            details_markup(&lookup, None)
        );
    }

    #[test]
    fn the_texts_of_the_dictionary_are_shown_as_written() {
        let written = "a < b & c";
        let lookup = Lookup::Article(Article {
            senses: vec![Sense {
                definition: written.to_owned(),
                synonyms: vec!["<i>x</i>".to_owned(), "y & z".to_owned()],
            }],
            translations: vec!["<б>".to_owned(), "&amp;".to_owned()],
        });
        assert_eq!(
            senses_markup(&lookup, None),
            "1. <a href=\"0\">a &lt; b &amp; c</a>"
        );
        assert_eq!(
            senses_markup(&lookup, Some(0)),
            "1. <a href=\"0\"><b>a &lt; b &amp; c</b></a>"
        );
        let markup = details_markup(&lookup, Some(0));
        assert!(markup.contains("\na &lt; b &amp; c\n"), "{markup}");
        assert!(
            markup.contains("&lt;i&gt;x&lt;/i&gt;, y &amp; z"),
            "{markup}"
        );
        assert!(markup.contains("&lt;б&gt;, &amp;amp;"), "{markup}");
        // The only tags are the ones of the mark.
        assert_eq!(markup.matches('<').count(), 2, "{markup}");
        assert!(plain(&markup).contains(written));
    }

    #[test]
    fn the_sample_article_of_bank_is_shown_in_full() {
        let article = Sample.lookup("bank").expect("bank");
        let lookup = Lookup::Article(article.clone());
        let list = plain(&senses_markup(&lookup, None));
        assert_eq!(list.lines().count(), article.senses.len());
        for (index, sense) in article.senses.iter().enumerate() {
            assert!(
                list.contains(&format!("{}. {}", index + 1, sense.definition)),
                "{list}"
            );
            let text = plain(&details_markup(&lookup, Some(index)));
            assert!(text.contains(&sense.definition), "{text}");
            for synonym in &sense.synonyms {
                assert!(text.contains(synonym), "{text}");
            }
            // Of this sense only.
            for other in article.senses.iter().filter(|other| *other != sense) {
                assert!(!text.contains(&other.definition), "{text}");
            }
            for translation in &article.translations {
                assert!(text.contains(translation), "{text}");
            }
        }
    }

    #[test]
    fn the_mark_claims_no_fit_to_the_sentence() {
        let mark = EXPLANATION_MARK.to_lowercase();
        for claim in [
            "in this sentence",
            "in the sentence",
            "here means",
            "means here",
            "context",
            "fits",
            "right sense",
            "correct",
            "best",
        ] {
            assert!(!mark.contains(claim), "{claim:?} in {mark:?}");
        }
        assert!(mark.contains("dictionary definition"), "{mark}");
        assert!(mark.contains("sense you chose"), "{mark}");
        assert!(mark.contains("explanation"), "{mark}");
    }

    #[test]
    fn the_russian_label_says_the_translations_are_of_the_word() {
        let label = RUSSIAN_LABEL.to_lowercase();
        assert!(label.contains("russian"), "{label}");
        assert!(label.contains("word"), "{label}");
        assert!(!label.contains("sense"), "{label}");
    }

    #[test]
    fn the_no_entry_line_names_the_word() {
        let line = details_markup(
            &Lookup::NoEntry {
                word: "sat".to_owned(),
            },
            None,
        );
        assert_eq!(line, "The dictionary has no entry for “sat”.");
        // The word is the user's text: it is escaped too.
        let line = details_markup(
            &Lookup::NoEntry {
                word: "a<b&c".to_owned(),
            },
            None,
        );
        assert_eq!(line, "The dictionary has no entry for “a&lt;b&amp;c”.");
    }

    #[test]
    fn the_unfinished_line_names_no_cause() {
        let line = details_markup(&Lookup::Unfinished, None).to_lowercase();
        for cause in [
            "time",
            "deadline",
            "limit",
            "slow",
            "panic",
            "crash",
            "error",
            "fail",
            "cancel",
            "unavailable",
            "because",
        ] {
            assert!(!line.contains(cause), "{cause:?} in {line:?}");
        }
        assert!(line.contains("did not finish"));
    }

    #[test]
    fn nothing_is_shown_or_chosen_at_first() {
        let mut shown = Shown::default();
        assert_eq!(shown.lookup(), None);
        assert_eq!(shown.sense(), None);
        // No article shown: a click changes nothing.
        assert!(!shown.choose(0));
        assert_eq!(shown.sense(), None);
    }

    #[test]
    fn the_sense_clicked_last_is_the_chosen_one() {
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(article())));
        assert_eq!(shown.sense(), None);
        assert!(shown.choose(1));
        assert_eq!(shown.sense(), Some(1));
        assert!(shown.choose(0));
        assert_eq!(shown.sense(), Some(0));
        // Again the same one: still chosen, and shown anew.
        assert!(shown.choose(0));
        assert_eq!(shown.sense(), Some(0));
        assert_eq!(shown.lookup(), Some(&Lookup::Article(article())));
    }

    #[test]
    fn a_new_result_forgets_the_chosen_sense() {
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(article())));
        assert!(shown.choose(1));
        // The same article again is a new result too.
        shown.show(Some(Lookup::Article(article())));
        assert_eq!(shown.sense(), None);
        assert!(shown.choose(1));
        shown.show(None);
        assert_eq!(shown.lookup(), None);
        assert_eq!(shown.sense(), None);
    }

    #[test]
    fn a_click_on_a_sense_the_result_does_not_have_changes_nothing() {
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(article())));
        assert!(shown.choose(1));
        assert!(!shown.choose(2));
        assert!(!shown.choose(usize::MAX));
        assert_eq!(shown.sense(), Some(1));
        // Every result that is not an article with that sense.
        let mut bare = article();
        bare.senses.clear();
        let mut others = every_lookup();
        others.retain(|lookup| !matches!(lookup, Lookup::Article(_)));
        others.push(Lookup::Article(bare));
        for lookup in others {
            shown.show(Some(lookup.clone()));
            assert!(!shown.choose(0), "{lookup:?}");
            assert_eq!(shown.sense(), None, "{lookup:?}");
            assert_eq!(shown.lookup(), Some(&lookup));
        }
    }

    fn not_bound(detail: &str) -> Binding {
        Binding::NotBound {
            detail: detail.to_owned(),
        }
    }

    /// One state of each kind. No wildcard arm: a new state does not compile
    /// here until it is listed.
    fn every_binding() -> Vec<Binding> {
        let all = vec![
            Binding::Attempting,
            Binding::Bound,
            not_bound("binding the shortcut: some portal text"),
        ];
        for binding in &all {
            match binding {
                Binding::Attempting | Binding::Bound | Binding::NotBound { .. } => {}
            }
        }
        all
    }

    #[test]
    fn only_a_bound_shortcut_has_no_row() {
        for binding in every_binding() {
            assert_eq!(
                binding_texts(&binding).is_none(),
                binding == Binding::Bound,
                "{binding:?}"
            );
        }
    }

    #[test]
    fn a_shown_row_is_never_empty() {
        for binding in every_binding() {
            if let Some(row) = binding_texts(&binding) {
                assert!(!row.text.trim().is_empty(), "{binding:?}");
                assert!(
                    row.detail.is_none_or(|detail| !detail.trim().is_empty()),
                    "{binding:?}"
                );
            }
        }
        // A blank message leaves the row with its own line only.
        let blank = not_bound(" ");
        let row = binding_texts(&blank).expect("row");
        assert!(!row.text.trim().is_empty());
        assert_eq!(row.detail, None);
    }

    #[test]
    fn the_not_bound_line_names_no_cause() {
        // Whatever the worker said, the window's own line stays the same.
        let lines: Vec<_> = [
            "binding the shortcut: cancelled",
            "binding the shortcut: no result within 60s",
            "session bus: I/O error",
        ]
        .into_iter()
        .map(|detail| binding_texts(&not_bound(detail)).expect("row").text)
        .collect();
        assert!(lines.iter().all(|line| *line == lines[0]));
        let lowered = lines[0].to_lowercase();
        for cause in [
            "cancel",
            "dismiss",
            "denied",
            "refus",
            "reject",
            "declin",
            "clos",
            "time",
            "limit",
            "portal",
            "error",
            "fail",
            "unavailable",
            "because",
            "you ",
        ] {
            assert!(!lowered.contains(cause), "{cause:?} in {:?}", lines[0]);
        }
        assert!(lowered.contains("not bound"));
    }

    #[test]
    fn the_detail_is_the_worker_message_as_given() {
        let message = "binding the shortcut: Portal request failed: Other";
        let binding = not_bound(message);
        let row = binding_texts(&binding).expect("row");
        assert_eq!(row.detail, Some(message));
    }

    #[test]
    fn an_attempt_and_a_failure_differ() {
        let attempt = binding_texts(&Binding::Attempting).expect("row");
        let binding = not_bound("x: y");
        let failure = binding_texts(&binding).expect("row");
        assert_ne!(attempt.text, failure.text);
        assert_eq!(attempt.detail, None);
        // Retry cannot start a second attempt while one runs.
        assert!(!attempt.retry);
        assert!(failure.retry);
    }

    /// The states of one kind over `sources`. No wildcard arm: a new state
    /// does not compile here until it is listed.
    fn same_kind(state: WindowState, sources: &[Source]) -> Vec<WindowState> {
        let with = |build: fn(Source) -> WindowState| sources.iter().copied().map(build).collect();
        match state {
            WindowState::Empty => vec![WindowState::Empty],
            WindowState::Waiting { .. } => with(|source| WindowState::Waiting { source }),
            WindowState::Dictionary { .. } => with(|source| WindowState::Dictionary { source }),
            WindowState::Contextual { .. } => with(|source| WindowState::Contextual { source }),
        }
    }

    /// One state of each kind; `same_kind` spreads them over the sources.
    const KINDS: [WindowState; 4] = [
        WindowState::Empty,
        WindowState::Waiting {
            source: Source::Manual,
        },
        WindowState::Dictionary {
            source: Source::Manual,
        },
        WindowState::Contextual {
            source: Source::Manual,
        },
    ];

    fn every_state() -> Vec<WindowState> {
        KINDS
            .into_iter()
            .flat_map(|kind| same_kind(kind, &Source::ALL))
            .collect()
    }

    #[test]
    fn every_state_is_covered() {
        assert_eq!(every_state().len(), 1 + 3 * Source::ALL.len());
    }

    #[test]
    fn both_texts_are_never_empty() {
        for state in every_state() {
            let texts = texts(state);
            assert!(!texts.source.trim().is_empty(), "{state:?}");
            assert!(!texts.status.trim().is_empty(), "{state:?}");
        }
    }

    #[test]
    fn each_source_has_its_own_label() {
        for kind in &KINDS[1..] {
            let labels: Vec<_> = same_kind(*kind, &Source::ALL)
                .into_iter()
                .map(|state| texts(state).source)
                .collect();
            for (index, label) in labels.iter().enumerate() {
                assert!(!labels[..index].contains(label), "{kind:?}: {label}");
            }
        }
    }

    #[test]
    fn the_source_label_does_not_depend_on_the_state() {
        for source in Source::ALL {
            let labels: Vec<_> = KINDS[1..]
                .iter()
                .flat_map(|kind| same_kind(*kind, &[source]))
                .map(|state| texts(state).source)
                .collect();
            assert!(labels.iter().all(|label| *label == labels[0]), "{source:?}");
        }
    }

    #[test]
    fn waiting_status_differs_from_the_other_statuses() {
        for source in Source::ALL {
            let waiting = texts(WindowState::Waiting { source }).status;
            for state in every_state() {
                if !matches!(state, WindowState::Waiting { .. }) {
                    assert_ne!(texts(state).status, waiting, "{state:?}");
                }
            }
        }
    }

    #[test]
    fn empty_asks_for_manual_input() {
        let empty = texts(WindowState::Empty);
        assert_eq!(empty.source, "manual input");
        assert_eq!(empty.status, "waiting for input");
        assert_eq!(
            empty.source,
            texts(WindowState::Waiting {
                source: Source::Manual
            })
            .source
        );
    }
}
