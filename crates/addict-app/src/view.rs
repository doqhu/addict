//! What the window says about its state: the source label and the status,
//! below them the binding row when the shortcut is not bound, then the field
//! for a word or a sentence, the text of the field with its words as links
//! and the result of the lookup of the chosen word.
//!
//! The texts live here and nowhere else; `addict-core` has none. [`texts`],
//! [`binding_texts`], [`sentence_markup`] and [`result_text`] are pure, so
//! they are tested without a display; [`View`] puts their results into its
//! widgets.
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

/// The result as plain text. No wildcard arm: a new variant does not compile
/// until it is given its text. The line of an unfinished lookup names no
/// cause.
pub fn result_text(lookup: &Lookup) -> String {
    match lookup {
        Lookup::Pending => "Looking up…".to_owned(),
        Lookup::Article(article) => article_text(article),
        Lookup::NoEntry { word } => format!("The dictionary has no entry for “{word}”."),
        Lookup::Unfinished => "The lookup did not finish.".to_owned(),
    }
}

/// The numbered senses, each with its synonyms when it has any, then the
/// translations.
fn article_text(article: &Article) -> String {
    let mut lines = Vec::new();
    for (index, sense) in article.senses.iter().enumerate() {
        lines.push(format!("{}. {}", index + 1, sense.definition));
        if !sense.synonyms.is_empty() {
            lines.push(format!("    synonyms: {}", sense.synonyms.join(", ")));
        }
    }
    if !article.translations.is_empty() {
        lines.push(format!("Russian: {}", article.translations.join(", ")));
    }
    lines.join("\n")
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
/// sentence, the text of the field with its words as links and the result.
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
    result: gtk::Label,
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
        let result = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .max_width_chars(TEXT_WIDTH_CHARS)
            .xalign(0.0)
            .selectable(true)
            .visible(false)
            .build();
        root.append(&field);
        root.append(&sentence);
        root.append(&result);

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
            result,
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

    /// Shows the text of `lookup`, or hides the result when there is none.
    pub fn render_result(&self, lookup: Option<&Lookup>) {
        let Some(lookup) = lookup else {
            self.result.set_visible(false);
            self.result.set_text("");
            return;
        };
        self.result.set_text(&result_text(lookup));
        self.result.set_visible(true);
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
        Binding, FIELD_PLACEHOLDER, Lookup, binding_texts, result_text, sentence_markup, texts,
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

    #[test]
    fn every_result_has_its_own_text() {
        let texts: Vec<_> = every_lookup().iter().map(result_text).collect();
        for (index, text) in texts.iter().enumerate() {
            assert!(!text.trim().is_empty());
            assert!(!texts[..index].contains(text), "{text}");
        }
    }

    #[test]
    fn every_result_but_an_article_is_one_line() {
        for lookup in every_lookup() {
            let lines = result_text(&lookup).lines().count();
            assert_eq!(
                lines == 1,
                !matches!(lookup, Lookup::Article(_)),
                "{lookup:?}"
            );
        }
    }

    #[test]
    fn an_article_lists_senses_synonyms_and_translations() {
        assert_eq!(
            result_text(&Lookup::Article(article())),
            "1. first\n    synonyms: one, uno\n2. second\nRussian: раз, два"
        );
    }

    #[test]
    fn an_article_without_translations_has_no_line_for_them() {
        let mut bare = article();
        bare.translations.clear();
        assert_eq!(
            result_text(&Lookup::Article(bare)),
            "1. first\n    synonyms: one, uno\n2. second"
        );
    }

    #[test]
    fn the_sample_article_of_bank_is_shown_in_full() {
        let article = Sample.lookup("bank").expect("bank");
        let text = result_text(&Lookup::Article(article.clone()));
        for sense in &article.senses {
            assert!(text.contains(&sense.definition), "{text}");
            for synonym in &sense.synonyms {
                assert!(text.contains(synonym), "{text}");
            }
        }
        for translation in &article.translations {
            assert!(text.contains(translation), "{text}");
        }
    }

    #[test]
    fn the_no_entry_line_names_the_word() {
        let line = result_text(&Lookup::NoEntry {
            word: "sat".to_owned(),
        });
        assert!(line.contains("sat"), "{line}");
        assert!(line.contains("no entry"), "{line}");
    }

    #[test]
    fn the_unfinished_line_names_no_cause() {
        let line = result_text(&Lookup::Unfinished).to_lowercase();
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
