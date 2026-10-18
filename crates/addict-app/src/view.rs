//! What the window says about its state: the source label and the status,
//! right under them the source line when a capture or a read of Android
//! Studio gave no text, then the
//! binding row when the shortcut is not bound and the line of the
//! dictionary indexes when one is missing or unreadable, then the field
//! for a word or a sentence, the text of the field with its words as links
//! and the result of the lookup of the chosen word: what is said about the
//! word, its translations among it, and the senses of the word in a list,
//! with what is said about the sense that was clicked under it and its
//! definition translated online once that is asked for. Below the result, apart from
//! it, stand the online texts, each under a label that says what it is: the
//! chosen word translated in its sentence, the sentence itself, or a word
//! translated without a sentence.
//!
//! The texts live here and nowhere else; `addict-core` has none. [`texts`],
//! [`binding_texts`], [`indexes_text`], [`failure_text`], [`read_failure_text`], [`too_long_text`],
//! [`sentence_markup`],
//! [`result_texts`] and [`online_markup`] are pure, and
//! so is [`Shown`], the result with the sense chosen in it; they are tested
//! without a display. This module owns no
//! widget: [`View`](crate::widgets::View) puts the results into its own.

use std::{borrow::Cow, fmt::Write as _, ops::Range, path::Path};

use addict_core::{
    dictionary::Article,
    input::{Source, WindowState},
    translation::{Refusal, Request, Source as Language},
};

use addict_platform::{
    atspi::ContextFailure, foreground::Source as Foregrounded, ocr::OcrFailure,
    screenshot::CaptureFailure,
};

use crate::{
    capture::Failure,
    dictionaries::{IndexFile, Indexes, State},
};

/// The two texts of one window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Texts {
    /// Where the input comes from.
    pub source: &'static str,
    /// What the window is doing with it.
    pub status: &'static str,
}

/// The texts of `state`; `source` is where the text of the field came
/// from, which the label names while there is no input. No wildcard arm: a
/// new state or source does not compile until it is given its texts.
pub fn texts(state: WindowState, source: Source) -> Texts {
    match state {
        // Nothing is being read, so the window waits for the user.
        WindowState::Empty => Texts {
            source: source_label(source),
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
        Source::Clipboard => "selection or clipboard",
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

/// What the button of the binding row says.
pub(crate) const RETRY_LABEL: &str = "Retry";

/// The command that imports a downloaded dictionary file, as the user types
/// it.
const IMPORT_COMMAND: &str = "addict import <FILE>";

/// The start of the line of the indexes when neither is imported.
const NOT_IMPORTED: &str = "The dictionaries are not imported.";

/// What the index of the definitions and the index of the translations
/// give, in the order of [`Indexes::files`].
const INDEX_GIVES: [&str; 2] = ["English definitions and synonyms", "Russian translations"];

/// The line of the indexes: what is said about every index that is missing
/// or cannot be read, one line for each, with the command that imports its
/// file. Empty when both are open. Plain text, not markup.
pub fn indexes_text(indexes: Indexes) -> String {
    // The Spanish index is not one of the files: the line is the same with
    // it or without it.
    if indexes.files() == Indexes::OPEN.files() {
        return String::new();
    }
    let files = indexes.files();
    if let [
        (definitions, State::Missing),
        (translations, State::Missing),
    ] = files
    {
        return format!(
            "{NOT_IMPORTED} Import each downloaded file with {IMPORT_COMMAND}: {} and {}.",
            definitions.published, translations.published
        );
    }
    let mut text = String::new();
    for ((file, state), gives) in files.into_iter().zip(INDEX_GIVES) {
        let line = match state {
            State::Open => continue,
            State::Missing => format!(
                "No {gives}: {} is not imported. Import it with {IMPORT_COMMAND}.",
                file.published
            ),
            State::Unreadable => format!("No {gives}: {}", unreadable_text(file)),
        };
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&line);
    }
    text
}

/// What is said about an index that cannot be read: its file and the
/// dictionary file to import again.
fn unreadable_text(file: IndexFile) -> String {
    format!(
        "the index {} cannot be read. Import {} again with {IMPORT_COMMAND}.",
        file.index, file.published
    )
}

/// What the empty field says.
pub(crate) const FIELD_PLACEHOLDER: &str = "Type or paste a word, or a sentence to click a word in";

/// What the button of `language` in the switch beside the field says. No
/// wildcard arm: a new language does not compile until it is given its
/// label.
pub(crate) fn language_label(language: Language) -> &'static str {
    match language {
        Language::En => "EN",
        Language::Es => "ES",
        Language::Ca => "CA",
    }
}

/// The tooltip of the switch of the reading language.
pub(crate) const LANGUAGE_TOOLTIP: &str = "Язык текста, который вы читаете";

/// What the button beside the field says: it takes a screen capture and
/// puts the text recognized in it into the field.
pub(crate) const CAPTURE_LABEL: &str = "Capture";

/// The package of the Tesseract data of `language`. No wildcard arm: a new
/// language does not compile until it is given its package.
pub(crate) fn ocr_package(language: Language) -> &'static str {
    match language {
        Language::En => "tesseract-langpack-eng",
        Language::Es => "tesseract-langpack-spa",
        Language::Ca => "tesseract-langpack-cat",
    }
}

/// The name of `language` before "OCR data" in the line of missing data.
fn ocr_language(language: Language) -> &'static str {
    match language {
        Language::En => "English",
        Language::Es => "Spanish",
        Language::Ca => "Catalan",
    }
}

/// The command that installs Tesseract with the data of `language`.
pub(crate) fn install_tesseract(language: Language) -> String {
    format!("sudo dnf install tesseract {}", ocr_package(language))
}

/// The command that installs the Tesseract data of `language`.
pub(crate) fn install_language(language: Language) -> String {
    format!("sudo dnf install {}", ocr_package(language))
}

/// What the source line says when the snapshot file is not there, whether
/// the capture or the recognition found it missing.
const NO_SNAPSHOT_FILE: &str = "The snapshot file was not found.";

/// What the source line says for `failure`, a capture recognized in
/// `language` that gave no text. The portal's code 2, which GNOME also gives
/// for Esc, claims no cause; a missing Tesseract or missing data of the
/// language names the command that installs it with the package of that
/// language. No wildcard arm: a new failure does not compile until it is
/// given its line.
pub fn failure_text(failure: &Failure, language: Language) -> Cow<'static, str> {
    match failure {
        Failure::Capture(failure) => match failure {
            CaptureFailure::Cancelled => "The capture was cancelled.".into(),
            CaptureFailure::Ended => "No snapshot was taken.".into(),
            CaptureFailure::Unavailable => "Screen capture is not available here.".into(),
            CaptureFailure::Failed(detail) => format!("The screen capture failed: {detail}").into(),
            CaptureFailure::NoImage => NO_SNAPSHOT_FILE.into(),
        },
        Failure::CaptureTimedOut => "No area was chosen in time.".into(),
        Failure::Ocr(failure) => match failure {
            OcrFailure::NoImage => NO_SNAPSHOT_FILE.into(),
            OcrFailure::TesseractMissing => format!(
                "Tesseract is not installed. Install it with: {}",
                install_tesseract(language)
            )
            .into(),
            OcrFailure::LanguageMissing => format!(
                "{} OCR data is not installed. Install it with: {}",
                ocr_language(language),
                install_language(language)
            )
            .into(),
            OcrFailure::Failed(detail) => format!("Text recognition failed: {detail}").into(),
            OcrFailure::Empty => "No text was recognized.".into(),
            OcrFailure::TimedOut => "Text recognition took too long.".into(),
        },
        Failure::Unfinished => "The capture did not finish.".into(),
    }
}

/// The command that turns accessibility on for the session's toolkits.
pub(crate) const TURN_ON_ACCESSIBILITY: &str =
    "gsettings set org.gnome.desktop.interface toolkit-accessibility true";

/// What follows every line of a read that gave nothing: the manual path.
const TYPE_OR_PASTE: &str = " Type or paste the sentence.";

/// What the source line says for `failure`, a read of Android Studio that
/// gave nothing: the cause, then the manual path. Accessibility that is off
/// names the command that turns it on; the product does not run it. No
/// wildcard arm: a new failure does not compile until it is given its line.
pub fn read_failure_text(failure: &ContextFailure) -> Cow<'static, str> {
    let cause: Cow<'static, str> = match failure {
        ContextFailure::AccessibilityOff => format!(
            "Accessibility is off, so Android Studio cannot be read. Turn it on with: \
             {TURN_ON_ACCESSIBILITY}"
        )
        .into(),
        ContextFailure::NotFromStudio(source) => match source {
            Foregrounded::Other => {
                "Super+Q was pressed outside Android Studio; only its editor is read.".into()
            }
            Foregrounded::Unknown => {
                "The application in front is not known, so nothing was read.".into()
            }
            // The adapter reads when Android Studio is in front; should it
            // ever say otherwise, nothing was read all the same.
            Foregrounded::AndroidStudio => "Android Studio could not be read.".into(),
        },
        ContextFailure::AppNotFound => "Android Studio is not on the accessibility bus. Start \
             it with the AtkWrapper VM option and screen reader support."
            .into(),
        ContextFailure::NoSelection => "Nothing is selected in Android Studio.".into(),
        ContextFailure::Ambiguous(_) => "Android Studio reports more than one selection.".into(),
        ContextFailure::TimedOut => "Android Studio did not answer in time.".into(),
        ContextFailure::Unavailable(detail) => {
            format!("The accessibility bus is not available: {detail}").into()
        }
        ContextFailure::Failed(detail) => {
            format!("Android Studio could not be read: {detail}").into()
        }
    };
    format!("{cause}{TYPE_OR_PASTE}").into()
}

/// Where a text too long to take was held after a press without context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// Selected, the primary selection.
    Selection,
    /// Copied, the clipboard.
    Clipboard,
}

/// The source line for a text that was `held` and has `chars` characters as
/// one line, more than `limit`: what was too long, and how to give less.
pub fn too_long_text(held: Held, chars: usize, limit: usize) -> String {
    let (what, verb) = match held {
        Held::Selection => ("selected", "Select"),
        Held::Clipboard => ("copied", "Copy"),
    };
    format!(
        "The {what} text is too long ({chars} characters; the limit is {limit}). \
         {verb} one sentence, or type it."
    )
}

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
    /// The English dictionaries have this article of the word.
    Article(Article),
    /// The definitions index of `language`, not English, has this article
    /// of the word: its senses in that language, with their synonyms, and
    /// no translations.
    Definitions {
        language: Language,
        article: Article,
    },
    /// The dictionary has no entry for `word`.
    NoEntry { word: String },
    /// No offline dictionary of the reading language is installed, so
    /// nothing was looked up.
    NoDictionary(Language),
    /// No index is open: there is nothing to look `word` up in.
    NotImported { word: String },
    /// An open index could not be read, so the word has no answer.
    Unreadable(IndexFile),
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

/// The line of a reading language without an offline dictionary: what
/// stands before the name of the language and what stands after it.
const NO_DICTIONARY_BEFORE: &str = "Оффлайн-словаря для ";
const NO_DICTIONARY_AFTER: &str = " нет";

/// The name of `language` in the line of a missing dictionary, in the
/// genitive. No wildcard arm: a new language does not compile until it is
/// given its name.
fn dictionary_language(language: Language) -> &'static str {
    match language {
        Language::En => "английского",
        Language::Es => "испанского",
        Language::Ca => "каталанского",
    }
}

/// The line of a word that no imported dictionary could be asked about: what
/// stands before the word and what stands after it.
const NOT_IMPORTED_BEFORE: &str = "There is no imported dictionary to look “";
const NOT_IMPORTED_AFTER: &str = "” up in. Import a downloaded dictionary file with ";

/// What stands before the line of an index that cannot be read.
const UNREADABLE_BEFORE: &str = "The word was not looked up: ";

/// What stands below the list while no sense is chosen.
const CHOOSE_SENSE: &str = "Click a sense to see its short explanation and its synonyms.";

/// The mark above the definition of the chosen sense. It says what the text
/// is and claims no fit to the sentence.
const EXPLANATION_MARK: &str = "Short explanation — the dictionary definition of the sense you chose; \
     it is not checked against the sentence:";

/// What the button that sends the definition of the chosen sense says.
pub(crate) const DEFINITION_ACTION: &str = "Translate definition into Russian";

/// What stands above the online translation of the definition of the chosen
/// sense.
const DEFINITION_LABEL: &str = "Online, this definition in Russian:";

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

/// What stands above the list of senses.
pub(crate) const SENSES_LABEL: &str = "English definitions";

/// What stands above the list of senses of an article from the definitions
/// index of `language`: the kind and the language. No wildcard arm: a new
/// language does not compile until it is given its label.
fn definitions_label(language: Language) -> &'static str {
    match language {
        Language::En => SENSES_LABEL,
        Language::Es => "Толкования на испанском",
        Language::Ca => "Толкования на каталанском",
    }
}

/// The line of an article from the definitions index of `language` without
/// senses. No wildcard arm, as in [`definitions_label`].
fn no_definitions(language: Language) -> &'static str {
    match language {
        Language::En => NO_DEFINITIONS,
        Language::Es => "В испанском словаре нет толкований этого слова.",
        Language::Ca => "В каталанском словаре нет толкований этого слова.",
    }
}

/// Items under their label, or the line that says there are none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listed<'a> {
    /// The items as the dictionary gives them, never empty.
    Items {
        label: &'static str,
        items: &'a [String],
    },
    /// The line of an empty list.
    Missing(&'static str),
}

impl<'a> Listed<'a> {
    fn of(label: &'static str, items: &'a [String], missing: &'static str) -> Self {
        if items.is_empty() {
            Self::Missing(missing)
        } else {
            Self::Items { label, items }
        }
    }
}

/// An online text in a block of its own: what it is and its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block<'a> {
    pub label: &'static str,
    pub line: Cow<'a, str>,
}

/// What is said about the chosen sense, beside and under its row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explained<'a> {
    /// The mark that stands above its definition.
    pub mark: &'static str,
    /// The online translation of its definition, once asked for.
    pub definition: Option<Block<'a>>,
    /// Its synonyms.
    pub synonyms: Listed<'a>,
}

/// One row of the list of senses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenseText<'a> {
    /// Its number in the list, from 1.
    pub number: usize,
    /// Its definition as the dictionary gives it.
    pub definition: &'a str,
    /// What is said about it when it is the chosen one.
    pub explained: Option<Explained<'a>>,
}

/// The result of a lookup, in the parts the window shows apart. All of it
/// is text, never markup: the window shows it as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultTexts<'a> {
    /// The one line of a result that is not an article.
    pub line: Option<Cow<'a, str>>,
    /// What stands above the list of senses: their kind and language.
    pub senses_label: &'static str,
    /// For an article, what stands above its list while no sense is chosen
    /// or when it has none.
    pub hint: Option<&'static str>,
    /// For an article, one row for each sense, in order.
    pub senses: Vec<SenseText<'a>>,
    /// For an article, its translations, which belong to the word.
    pub translations: Option<Listed<'a>>,
}

/// The parts of `lookup` with the sense `chosen` in it; a `chosen` index the
/// article does not have is no choice. The online translation of the
/// definition, `definition`, stands under the chosen sense when its
/// definition is not blank. A refusal about the key names `key_file`, as in
/// [`online_markup`]. No wildcard arm: a new variant does not compile until
/// it is given its text.
pub fn result_texts<'a>(
    lookup: &'a Lookup,
    chosen: Option<usize>,
    definition: Option<&'a Online>,
    key_file: Option<&Path>,
) -> ResultTexts<'a> {
    let line = |text: Cow<'a, str>| ResultTexts {
        line: Some(text),
        senses_label: SENSES_LABEL,
        hint: None,
        senses: Vec::new(),
        translations: None,
    };
    match lookup {
        Lookup::Pending => line(Cow::Borrowed(PENDING)),
        Lookup::Article(article) => article_texts(article, chosen, definition, key_file),
        Lookup::Definitions { language, article } => {
            // No definition is translated online, and the article has no
            // Russian of its own: no line is said about either.
            let mut texts = article_texts(article, chosen, None, key_file);
            texts.senses_label = definitions_label(*language);
            if article.senses.is_empty() {
                texts.hint = Some(no_definitions(*language));
            }
            texts.translations = None;
            texts
        }
        Lookup::NoEntry { word } => line(Cow::Owned(format!(
            "{NO_ENTRY_BEFORE}{word}{NO_ENTRY_AFTER}"
        ))),
        Lookup::NoDictionary(language) => line(Cow::Owned(format!(
            "{NO_DICTIONARY_BEFORE}{}{NO_DICTIONARY_AFTER}",
            dictionary_language(*language)
        ))),
        Lookup::NotImported { word } => line(Cow::Owned(format!(
            "{NOT_IMPORTED_BEFORE}{word}{NOT_IMPORTED_AFTER}{IMPORT_COMMAND}."
        ))),
        Lookup::Unreadable(file) => line(Cow::Owned(format!(
            "{UNREADABLE_BEFORE}{}",
            unreadable_text(*file)
        ))),
        Lookup::Unfinished => line(Cow::Borrowed(UNFINISHED)),
    }
}

/// The parts of `article`; see [`result_texts`].
fn article_texts<'a>(
    article: &'a Article,
    chosen: Option<usize>,
    definition: Option<&'a Online>,
    key_file: Option<&Path>,
) -> ResultTexts<'a> {
    let chosen = chosen.filter(|index| *index < article.senses.len());
    let hint = if article.senses.is_empty() {
        Some(NO_DEFINITIONS)
    } else if chosen.is_none() {
        Some(CHOOSE_SENSE)
    } else {
        None
    };
    let senses = article
        .senses
        .iter()
        .enumerate()
        .map(|(index, sense)| SenseText {
            number: index + 1,
            definition: &sense.definition,
            explained: (chosen == Some(index)).then(|| Explained {
                mark: EXPLANATION_MARK,
                definition: definition
                    .filter(|_| !sense.definition.trim().is_empty())
                    .map(|online| Block {
                        label: DEFINITION_LABEL,
                        line: online_line(online, key_file),
                    }),
                synonyms: Listed::of(SYNONYMS_LABEL, &sense.synonyms, NO_SYNONYMS),
            }),
        })
        .collect();
    ResultTexts {
        line: None,
        senses_label: SENSES_LABEL,
        hint,
        senses,
        translations: Some(Listed::of(
            RUSSIAN_LABEL,
            &article.translations,
            NO_TRANSLATION,
        )),
    }
}

/// Where one online request stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Online {
    /// The request runs; its end is not known yet.
    Pending,
    /// The Russian text the translator gave.
    Translated(String),
    /// The request gave no translation.
    Refused(Refusal),
}

/// Where the translation of the sentence itself stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Whole {
    /// The sentence was asked for.
    Asked(Online),
    /// The text is too long to be sent as a sentence: nothing was asked.
    TooLong,
}

/// The online texts the window shows, each in a block of its own; `None`
/// where there is no block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OnlineTexts<'a> {
    /// The chosen word, translated in its sentence.
    pub word: Option<&'a Online>,
    /// The sentence itself.
    pub sentence: Option<&'a Whole>,
    /// The only word of the text, translated without a sentence.
    pub lone: Option<&'a Online>,
}

/// What stands above the online translation of the chosen word in its
/// sentence.
const ONLINE_WORD_LABEL: &str = "Online, this word in the sentence:";

/// What stands above the online translation of a word that has no sentence.
const ONLINE_LONE_LABEL: &str = "Online, this word without a sentence:";

/// What stands above the online translation of the sentence itself.
const ONLINE_SENTENCE_LABEL: &str = "Online, the whole sentence:";

/// The line of an online request that still runs.
const ONLINE_PENDING: &str = "Translating online…";

/// The fallback line for an unclassified failure.
const ONLINE_UNAVAILABLE: &str = "Online translation is not available.";

/// The line of a text that is not sent as a sentence because of its length.
const ONLINE_TOO_LONG: &str = "The sentence is too long to translate.";

/// The online texts as Pango markup: a block for the word in its sentence,
/// one for the word without a sentence and one for the sentence, in this
/// order, with an empty line between two of them. A block is its label and,
/// under it, the pending line, the translation as it came, escaped, the line
/// of a refusal or the line of a text that is too long. Empty when there is
/// no block. A refusal about the key names `key_file`, or says how to make
/// it known when there is none.
pub fn online_markup(texts: &OnlineTexts<'_>, key_file: Option<&Path>) -> String {
    let mut markup = String::new();
    if let Some(online) = texts.word {
        push_online_block(
            &mut markup,
            ONLINE_WORD_LABEL,
            &online_line(online, key_file),
        );
    }
    if let Some(online) = texts.lone {
        push_online_block(
            &mut markup,
            ONLINE_LONE_LABEL,
            &online_line(online, key_file),
        );
    }
    if let Some(whole) = texts.sentence {
        let line = match whole {
            Whole::Asked(online) => online_line(online, key_file),
            Whole::TooLong => Cow::Borrowed(ONLINE_TOO_LONG),
        };
        push_online_block(&mut markup, ONLINE_SENTENCE_LABEL, &line);
    }
    markup
}

/// Appends a block: the `label` and the `line` under it, both escaped, after
/// an empty line when a block stands before it.
fn push_online_block(markup: &mut String, label: &str, line: &str) {
    if !markup.is_empty() {
        markup.push_str("\n\n");
    }
    markup.push_str("<i>");
    push_escaped(markup, label);
    markup.push_str("</i>\n");
    push_escaped(markup, line);
}

/// The line of one online request: the pending line, the translation as it
/// came or the line of a refusal. No wildcard arm: a new state does not
/// compile until it is given its line.
fn online_line<'a>(online: &'a Online, key_file: Option<&Path>) -> Cow<'a, str> {
    match online {
        Online::Pending => Cow::Borrowed(ONLINE_PENDING),
        Online::Translated(text) => Cow::Borrowed(text),
        Online::Refused(refusal) => Cow::Owned(refusal_text(*refusal, key_file)),
    }
}

/// The line of `refusal`, which says what failed. A refusal about the key
/// names `key_file`, or says how to make it known when there is none. No
/// wildcard arm: a new refusal does not compile until it is given its line.
fn refusal_text(refusal: Refusal, key_file: Option<&Path>) -> String {
    let line = match refusal {
        Refusal::Key => "A DeepL key is required.",
        Refusal::UnreadableKey => "Cannot read the DeepL key file.",
        Refusal::UnsafeKey => "The DeepL key file must have permissions 0600.",
        Refusal::RejectedKey => "DeepL rejected the key.",
        Refusal::Network => "Cannot connect to DeepL.",
        Refusal::Quota => "The DeepL quota is exhausted.",
        Refusal::UnsupportedLanguage => {
            "DeepL does not support translation from this language into Russian."
        }
        Refusal::TimedOut => "Online translation timed out.",
        Refusal::Other => ONLINE_UNAVAILABLE,
    };
    if matches!(
        refusal,
        Refusal::Key | Refusal::UnreadableKey | Refusal::UnsafeKey | Refusal::RejectedKey
    ) {
        match key_file {
            Some(path) => format!("{line} Key file: {}", path.display()),
            None => format!(
                "{line} Set an absolute XDG_CONFIG_HOME or HOME to locate addict/deepl.key."
            ),
        }
    } else {
        line.to_owned()
    }
}

/// The result the window shows and the sense chosen in it.
#[derive(Debug, Default)]
pub struct Shown {
    lookup: Option<Lookup>,
    sense: Option<usize>,
    definition: Option<(u64, Online)>,
    next_definition: u64,
}

impl Shown {
    /// Another result, or none, is shown: the sense chosen in the one before
    /// is forgotten.
    pub fn show(&mut self, lookup: Option<Lookup>) {
        self.lookup = lookup;
        self.sense = None;
        self.definition = None;
    }

    /// Chooses the sense with this index and says whether it is chosen now:
    /// `false`, with nothing changed, unless the shown result is an article
    /// that has the sense. Choosing the chosen sense again is `true`.
    pub fn choose(&mut self, index: usize) -> bool {
        let (Some(Lookup::Article(article)) | Some(Lookup::Definitions { article, .. })) =
            &self.lookup
        else {
            return false;
        };
        if index >= article.senses.len() {
            return false;
        }
        if self.sense != Some(index) {
            self.definition = None;
        }
        self.sense = Some(index);
        true
    }

    /// Visibility and sensitivity of the explicit definition action.
    pub fn definition_action(&self) -> (bool, bool) {
        let available = self.selected_definition().is_some();
        let enabled = available && matches!(self.definition(), None | Some(Online::Refused(_)));
        (available, enabled)
    }

    /// The definition of the chosen sense, as the dictionary gives it, or
    /// `None` when no sense is chosen or its definition is blank. A sense
    /// of another language's definitions has none to send: its request
    /// would go as English.
    fn selected_definition(&self) -> Option<&str> {
        let Lookup::Article(article) = self.lookup.as_ref()? else {
            return None;
        };
        let text = &article.senses.get(self.sense?)?.definition;
        (!text.trim().is_empty()).then_some(text.as_str())
    }

    /// The request for the definition of the chosen sense, with the number
    /// its answer must carry, when the definition action is enabled; the
    /// definition is then pending. The text is the dictionary's, unchanged,
    /// and is sent without context, from English whatever the reading
    /// language: the definitions are OEWN's. Only the definition action
    /// calls this.
    pub fn request_definition(&mut self) -> Option<(u64, Request)> {
        if !self.definition_action().1 {
            return None;
        }
        let request = Request {
            text: self.selected_definition()?.to_owned(),
            context: None,
            source: Language::En,
        };
        self.next_definition += 1;
        let number = self.next_definition;
        self.definition = Some((number, Online::Pending));
        Some((number, request))
    }

    /// Keeps `online` as the translation of the definition when the request
    /// with `number` is the one still pending, and says whether it was kept.
    /// The answer of a request that was replaced, by another sense, another
    /// result or a retry, is not.
    pub fn answer_definition(&mut self, number: u64, online: Online) -> bool {
        if !matches!(self.definition, Some((current, Online::Pending)) if current == number) {
            return false;
        }
        self.definition = Some((number, online));
        true
    }

    /// Where the translation of the definition of the chosen sense stands;
    /// `None` until it is asked for.
    pub fn definition(&self) -> Option<&Online> {
        self.definition.as_ref().map(|(_, online)| online)
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

#[cfg(test)]
mod tests {
    use addict_core::{
        dictionary::{Article, Dictionary, Sample, Sense},
        input::{Source, WindowState},
        sentence::words,
        translation::{Refusal, Source as Language},
    };

    use super::{
        Binding, CAPTURE_LABEL, CHOOSE_SENSE, EXPLANATION_MARK, FIELD_PLACEHOLDER, Listed, Lookup,
        NO_DEFINITIONS, NO_SYNONYMS, NO_TRANSLATION, ONLINE_LONE_LABEL, ONLINE_PENDING,
        ONLINE_SENTENCE_LABEL, ONLINE_TOO_LONG, ONLINE_UNAVAILABLE, ONLINE_WORD_LABEL, Online,
        OnlineTexts, PENDING, RUSSIAN_LABEL, ResultTexts, SYNONYMS_LABEL, Shown, UNFINISHED, Whole,
        binding_texts, indexes_text, online_markup, result_texts, sentence_markup, texts,
    };
    use addict_platform::{ocr::OcrFailure, screenshot::CaptureFailure};

    use super::{
        Held, TURN_ON_ACCESSIBILITY, failure_text, install_language, install_tesseract,
        read_failure_text, too_long_text,
    };
    use crate::{
        capture::Failure,
        dictionaries::{
            Indexes, State,
            tests::{FREEDICT, OEWN},
        },
    };
    use addict_platform::{atspi::ContextFailure, foreground::Source as Foregrounded};

    fn definition_shown() -> Shown {
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(Article {
            senses: vec![
                Sense {
                    definition: "  land <near> & water  ".to_owned(),
                    synonyms: vec!["shore".to_owned()],
                },
                Sense {
                    definition: "financial institution".to_owned(),
                    synonyms: vec![],
                },
                Sense {
                    definition: " \n\t ".to_owned(),
                    synonyms: vec![],
                },
            ],
            translations: vec!["берег".to_owned()],
        })));
        shown
    }

    #[test]
    fn definition_action_requires_a_selected_nonblank_definition() {
        let mut shown = Shown::default();
        assert_eq!(shown.definition_action(), (false, false));
        assert!(shown.request_definition().is_none());
        shown = definition_shown();
        assert_eq!(shown.definition_action(), (false, false));
        assert!(!shown.choose(99));
        assert!(shown.choose(2));
        assert_eq!(shown.definition_action(), (false, false));
        assert!(shown.request_definition().is_none());
        assert!(shown.choose(0));
        assert_eq!(shown.definition_action(), (true, true));
        let (number, request) = shown.request_definition().expect("explicit action");
        assert_eq!(request.text, "  land <near> & water  ");
        assert_eq!(request.context, None);
        assert_eq!(shown.definition_action(), (true, false));
        assert!(shown.request_definition().is_none());
        shown.choose(0);
        assert_eq!(shown.definition(), Some(&Online::Pending));
        assert!(shown.answer_definition(number, translated("берег")));
        assert!(shown.request_definition().is_none());
        shown.choose(0);
        assert_eq!(shown.definition(), Some(&translated("берег")));
    }

    #[test]
    fn selection_round_trips_retries_and_article_replacement_reject_old_answers() {
        let mut shown = definition_shown();
        shown.choose(0);
        let (old, _) = shown.request_definition().expect("first");
        shown.choose(1);
        assert_eq!(shown.definition(), None);
        shown.choose(0);
        let (new, _) = shown.request_definition().expect("round trip");
        assert_ne!(new, old);
        assert!(!shown.answer_definition(old, translated("stale")));
        assert!(shown.answer_definition(new, Online::Refused(Refusal::Network)));
        assert_eq!(shown.definition_action(), (true, true));
        let (retry, _) = shown.request_definition().expect("retry");
        assert_ne!(retry, new);
        assert!(!shown.answer_definition(new, translated("stale retry")));
        assert!(shown.answer_definition(retry, translated("fresh")));
        let article = shown.lookup().cloned();
        shown.show(article);
        assert_eq!(shown.definition(), None);
        shown.choose(0);
        let (replacement, _) = shown.request_definition().expect("new article");
        assert!(!shown.answer_definition(retry, translated("stale article")));
        shown.show(None);
        assert!(!shown.answer_definition(replacement, translated("cleared")));
        assert_eq!(shown.definition_action(), (false, false));
    }

    #[test]
    fn definition_outcomes_stand_under_the_definition_before_synonyms_and_translations() {
        let mut shown = definition_shown();
        shown.choose(0);
        let path = std::path::Path::new("/key/<file>&");
        for online in [
            Online::Pending,
            translated("<b>русский</b> &"),
            Online::Refused(Refusal::Key),
            Online::Refused(Refusal::UnreadableKey),
            Online::Refused(Refusal::UnsafeKey),
            Online::Refused(Refusal::RejectedKey),
            Online::Refused(Refusal::Network),
            Online::Refused(Refusal::Quota),
            Online::Refused(Refusal::UnsupportedLanguage),
            Online::Refused(Refusal::TimedOut),
            Online::Refused(Refusal::Other),
        ] {
            let texts = result_texts(
                shown.lookup().expect("article"),
                shown.sense(),
                Some(&online),
                Some(path),
            );
            // Shown as written: the translation and the path are text.
            assert_eq!(
                read(&texts),
                [
                    EXPLANATION_MARK.to_owned(),
                    "  land <near> & water  ".to_owned(),
                    super::DEFINITION_LABEL.to_owned(),
                    super::online_line(&online, Some(path)).into_owned(),
                    format!("{SYNONYMS_LABEL} shore"),
                    String::new(),
                    format!("{RUSSIAN_LABEL} берег"),
                ]
            );
        }
        let dictionary_only = details(shown.lookup().expect("article"), shown.sense());
        assert!(!dictionary_only.contains(&super::DEFINITION_LABEL.to_owned()));
        // A blank definition is never sent, so it has no block.
        shown.choose(2);
        let texts = result_texts(
            shown.lookup().expect("article"),
            shown.sense(),
            Some(&Online::Pending),
            None,
        );
        assert!(!read(&texts).contains(&super::DEFINITION_LABEL.to_owned()));
    }

    const SENTENCE: &str = "I sat on the bank.";

    fn translated(text: &str) -> Online {
        Online::Translated(text.to_owned())
    }

    /// The block of `label` with `line` under it.
    fn block(label: &str, line: &str) -> String {
        format!("<i>{label}</i>\n{line}")
    }

    #[test]
    fn without_an_online_text_there_is_no_block() {
        assert_eq!(online_markup(&OnlineTexts::default(), None), "");
    }

    #[test]
    fn the_word_and_its_sentence_stand_under_their_own_labels() {
        let word = translated("берег");
        let sentence = Whole::Asked(translated("Я сидел на берегу реки."));
        let markup = online_markup(
            &OnlineTexts {
                word: Some(&word),
                sentence: Some(&sentence),
                lone: None,
            },
            None,
        );
        assert_eq!(
            markup,
            format!(
                "{}\n\n{}",
                block(ONLINE_WORD_LABEL, "берег"),
                block(ONLINE_SENTENCE_LABEL, "Я сидел на берегу реки.")
            )
        );
        assert!(!markup.contains(ONLINE_LONE_LABEL), "{markup}");
    }

    #[test]
    fn a_word_without_a_sentence_has_its_own_label_and_no_sentence_block() {
        let lone = translated("банк");
        let markup = online_markup(
            &OnlineTexts {
                word: None,
                sentence: None,
                lone: Some(&lone),
            },
            None,
        );
        assert_eq!(markup, block(ONLINE_LONE_LABEL, "банк"));
    }

    #[test]
    fn the_sentence_stays_alone_when_the_word_has_no_online_text() {
        let sentence = Whole::Asked(translated("Я сидел."));
        let markup = online_markup(
            &OnlineTexts {
                word: None,
                sentence: Some(&sentence),
                lone: None,
            },
            None,
        );
        assert_eq!(markup, block(ONLINE_SENTENCE_LABEL, "Я сидел."));
    }

    #[test]
    fn the_online_labels_say_what_each_text_is() {
        assert_eq!(ONLINE_WORD_LABEL, "Online, this word in the sentence:");
        assert_eq!(ONLINE_LONE_LABEL, "Online, this word without a sentence:");
        assert_eq!(ONLINE_SENTENCE_LABEL, "Online, the whole sentence:");
        // Told apart from the dictionary texts too.
        for label in [ONLINE_WORD_LABEL, ONLINE_LONE_LABEL, ONLINE_SENTENCE_LABEL] {
            assert_ne!(label, RUSSIAN_LABEL);
            assert_ne!(label, EXPLANATION_MARK);
            assert_eq!(label.lines().count(), 1);
        }
    }

    #[test]
    fn an_online_translation_is_text_and_never_markup() {
        let hostile = translated("<b>берег</b> & банк");
        let escaped = "&lt;b&gt;берег&lt;/b&gt; &amp; банк";
        let whole = Whole::Asked(hostile.clone());
        assert_eq!(
            online_markup(
                &OnlineTexts {
                    word: Some(&hostile),
                    sentence: Some(&whole),
                    lone: None,
                },
                None
            ),
            format!(
                "{}\n\n{}",
                block(ONLINE_WORD_LABEL, escaped),
                block(ONLINE_SENTENCE_LABEL, escaped)
            )
        );
        assert_eq!(
            online_markup(
                &OnlineTexts {
                    word: None,
                    sentence: None,
                    lone: Some(&hostile),
                },
                None
            ),
            block(ONLINE_LONE_LABEL, escaped)
        );
    }

    #[test]
    fn a_pending_online_request_has_its_own_line_under_its_label() {
        let pending = Whole::Asked(Online::Pending);
        assert_eq!(
            online_markup(
                &OnlineTexts {
                    word: Some(&Online::Pending),
                    sentence: Some(&pending),
                    lone: None,
                },
                None
            ),
            format!(
                "{}\n\n{}",
                block(ONLINE_WORD_LABEL, ONLINE_PENDING),
                block(ONLINE_SENTENCE_LABEL, ONLINE_PENDING)
            )
        );
        assert_eq!(
            online_markup(
                &OnlineTexts {
                    word: None,
                    sentence: None,
                    lone: Some(&Online::Pending),
                },
                None
            ),
            block(ONLINE_LONE_LABEL, ONLINE_PENDING)
        );
        assert_ne!(ONLINE_PENDING, PENDING);
        assert_ne!(ONLINE_PENDING, ONLINE_UNAVAILABLE);
    }

    #[test]
    fn every_refusal_has_its_line_under_its_label() {
        assert_eq!(ONLINE_UNAVAILABLE, "Online translation is not available.");
        for refusal in [
            Refusal::Key,
            Refusal::UnreadableKey,
            Refusal::UnsafeKey,
            Refusal::Network,
            Refusal::UnsupportedLanguage,
            Refusal::RejectedKey,
            Refusal::Quota,
            Refusal::TimedOut,
            Refusal::Other,
        ] {
            let line = super::refusal_text(refusal, None);
            let refused = Online::Refused(refusal);
            let whole = Whole::Asked(refused.clone());
            assert_eq!(
                online_markup(
                    &OnlineTexts {
                        word: Some(&refused),
                        sentence: Some(&whole),
                        lone: None,
                    },
                    None
                ),
                format!(
                    "{}\n\n{}",
                    block(ONLINE_WORD_LABEL, &line),
                    block(ONLINE_SENTENCE_LABEL, &line)
                ),
                "{refusal:?}"
            );
            assert_eq!(
                online_markup(
                    &OnlineTexts {
                        word: None,
                        sentence: None,
                        lone: Some(&refused),
                    },
                    None
                ),
                block(ONLINE_LONE_LABEL, &line),
                "{refusal:?}"
            );
        }
    }

    #[test]
    fn a_text_that_is_too_long_says_so_under_the_label_of_the_sentence() {
        assert_eq!(ONLINE_TOO_LONG, "The sentence is too long to translate.");
        assert_ne!(ONLINE_TOO_LONG, ONLINE_UNAVAILABLE);
        let word = translated("берег");
        assert_eq!(
            online_markup(
                &OnlineTexts {
                    word: Some(&word),
                    sentence: Some(&Whole::TooLong),
                    lone: None,
                },
                None
            ),
            format!(
                "{}\n\n{}",
                block(ONLINE_WORD_LABEL, "берег"),
                block(ONLINE_SENTENCE_LABEL, ONLINE_TOO_LONG)
            )
        );
    }

    #[test]
    fn the_non_key_online_lines_are_one_line_each_and_name_no_path() {
        for line in [ONLINE_PENDING, ONLINE_UNAVAILABLE, ONLINE_TOO_LONG] {
            assert_eq!(line.lines().count(), 1, "{line}");
            assert!(!line.contains('/'), "{line}");
            assert!(!line.contains("deepl"), "{line}");
        }
        let refused = Online::Refused(Refusal::Other);
        let markup = online_markup(
            &OnlineTexts {
                word: Some(&refused),
                sentence: None,
                lone: None,
            },
            None,
        );
        assert_eq!(markup.lines().count(), 2, "{markup}");
        assert!(!markup.contains("deepl"), "{markup}");
    }

    #[test]
    fn the_capture_texts_are_one_line_each_and_name_no_cause() {
        assert_eq!(CAPTURE_LABEL, "Capture");
        let empty = failure_text(&Failure::Ocr(OcrFailure::Empty), Language::En);
        assert_eq!(empty, "No text was recognized.");
        for text in [CAPTURE_LABEL, &empty] {
            assert_eq!(text.lines().count(), 1, "{text}");
        }
        // The status of a capture being read is the waiting one, from OCR.
        let waiting = texts(
            WindowState::Waiting {
                source: Source::Ocr,
            },
            Source::Manual,
        );
        assert_eq!(waiting.source, "screen capture (OCR)");
        assert_ne!(waiting.status, empty);
    }

    /// Every failure of a capture, each once.
    fn every_failure() -> Vec<Failure> {
        vec![
            Failure::Capture(CaptureFailure::Cancelled),
            Failure::Capture(CaptureFailure::Ended),
            Failure::Capture(CaptureFailure::Unavailable),
            Failure::Capture(CaptureFailure::Failed("no portal answer".to_owned())),
            Failure::Capture(CaptureFailure::NoImage),
            Failure::CaptureTimedOut,
            Failure::Ocr(OcrFailure::NoImage),
            Failure::Ocr(OcrFailure::TesseractMissing),
            Failure::Ocr(OcrFailure::LanguageMissing),
            Failure::Ocr(OcrFailure::Failed(
                "tesseract exit status 1: bad".to_owned(),
            )),
            Failure::Ocr(OcrFailure::Empty),
            Failure::Ocr(OcrFailure::TimedOut),
            Failure::Unfinished,
        ]
    }

    #[test]
    fn the_capture_lines_are_as_agreed() {
        let lines: Vec<String> = every_failure()
            .iter()
            .map(|failure| failure_text(failure, Language::En).into_owned())
            .collect();
        assert_eq!(
            lines,
            [
                "The capture was cancelled.",
                "No snapshot was taken.",
                "Screen capture is not available here.",
                "The screen capture failed: no portal answer",
                "The snapshot file was not found.",
                "No area was chosen in time.",
                "The snapshot file was not found.",
                "Tesseract is not installed. Install it with: sudo dnf install tesseract tesseract-langpack-eng",
                "English OCR data is not installed. Install it with: sudo dnf install tesseract-langpack-eng",
                "Text recognition failed: tesseract exit status 1: bad",
                "No text was recognized.",
                "Text recognition took too long.",
                "The capture did not finish.",
            ]
        );
    }

    #[test]
    fn every_capture_failure_has_a_line_of_its_own() {
        let failures = every_failure();
        for (i, a) in failures.iter().enumerate() {
            for b in &failures[i + 1..] {
                // The missing snapshot file is one line, whoever found it.
                let same_file = matches!(
                    (a, b),
                    (
                        Failure::Capture(CaptureFailure::NoImage),
                        Failure::Ocr(OcrFailure::NoImage)
                    )
                );
                if !same_file {
                    assert_ne!(
                        failure_text(a, Language::En),
                        failure_text(b, Language::En),
                        "{a:?} and {b:?}"
                    );
                }
            }
        }
        for failure in &failures {
            let text = failure_text(failure, Language::En);
            assert_eq!(text.lines().count(), 1, "{text}");
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn code_2_claims_no_cause() {
        let ended =
            failure_text(&Failure::Capture(CaptureFailure::Ended), Language::En).to_lowercase();
        assert!(!ended.contains("cancel"), "{ended}");
        assert!(!ended.contains("esc"), "{ended}");
    }

    #[test]
    fn the_install_commands_stand_only_in_their_lines() {
        for language in Language::ALL {
            for failure in every_failure() {
                let text = failure_text(&failure, language);
                assert_eq!(
                    text.contains(&install_tesseract(language)),
                    failure == Failure::Ocr(OcrFailure::TesseractMissing),
                    "{text}"
                );
                assert_eq!(
                    text.contains(&install_language(language)),
                    failure == Failure::Ocr(OcrFailure::LanguageMissing),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn missing_ocr_data_names_the_package_of_the_language() {
        let missing = Failure::Ocr(OcrFailure::LanguageMissing);
        assert_eq!(
            failure_text(&missing, Language::Es),
            "Spanish OCR data is not installed. Install it with: sudo dnf install tesseract-langpack-spa"
        );
        assert_eq!(
            failure_text(&missing, Language::Ca),
            "Catalan OCR data is not installed. Install it with: sudo dnf install tesseract-langpack-cat"
        );
        assert_eq!(
            failure_text(&Failure::Ocr(OcrFailure::TesseractMissing), Language::Ca),
            "Tesseract is not installed. Install it with: sudo dnf install tesseract tesseract-langpack-cat"
        );
        // Every other line is the same in every language.
        for failure in every_failure() {
            if !matches!(
                failure,
                Failure::Ocr(OcrFailure::TesseractMissing | OcrFailure::LanguageMissing)
            ) {
                for language in Language::ALL {
                    assert_eq!(
                        failure_text(&failure, language),
                        failure_text(&failure, Language::En)
                    );
                }
            }
        }
    }

    /// Every failure of a read, each once.
    fn every_read_failure() -> Vec<ContextFailure> {
        vec![
            ContextFailure::AccessibilityOff,
            ContextFailure::NotFromStudio(Foregrounded::Other),
            ContextFailure::NotFromStudio(Foregrounded::Unknown),
            ContextFailure::NotFromStudio(Foregrounded::AndroidStudio),
            ContextFailure::AppNotFound,
            ContextFailure::NoSelection,
            ContextFailure::Ambiguous(2),
            ContextFailure::TimedOut,
            ContextFailure::Unavailable("org.a11y.Bus GetAddress: no reply".to_owned()),
            ContextFailure::Failed("GetText failed".to_owned()),
        ]
    }

    #[test]
    fn the_read_lines_are_as_agreed() {
        let lines: Vec<String> = every_read_failure()
            .iter()
            .map(|failure| read_failure_text(failure).into_owned())
            .collect();
        assert_eq!(
            lines,
            [
                "Accessibility is off, so Android Studio cannot be read. Turn it on with: gsettings set org.gnome.desktop.interface toolkit-accessibility true Type or paste the sentence.",
                "Super+Q was pressed outside Android Studio; only its editor is read. Type or paste the sentence.",
                "The application in front is not known, so nothing was read. Type or paste the sentence.",
                "Android Studio could not be read. Type or paste the sentence.",
                "Android Studio is not on the accessibility bus. Start it with the AtkWrapper VM option and screen reader support. Type or paste the sentence.",
                "Nothing is selected in Android Studio. Type or paste the sentence.",
                "Android Studio reports more than one selection. Type or paste the sentence.",
                "Android Studio did not answer in time. Type or paste the sentence.",
                "The accessibility bus is not available: org.a11y.Bus GetAddress: no reply Type or paste the sentence.",
                "Android Studio could not be read: GetText failed Type or paste the sentence.",
            ]
        );
    }

    #[test]
    fn every_read_line_is_one_line_and_ends_in_the_manual_path() {
        let failures = every_read_failure();
        for (i, a) in failures.iter().enumerate() {
            for b in &failures[i + 1..] {
                assert_ne!(
                    read_failure_text(a),
                    read_failure_text(b),
                    "{a:?} and {b:?}"
                );
            }
        }
        for failure in &failures {
            let text = read_failure_text(failure);
            assert_eq!(text.lines().count(), 1, "{text}");
            assert!(text.ends_with(" Type or paste the sentence."), "{text}");
            assert_eq!(
                text.contains(TURN_ON_ACCESSIBILITY),
                *failure == ContextFailure::AccessibilityOff,
                "{text}"
            );
        }
    }

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
            Lookup::NoDictionary(Language::Es),
            Lookup::NotImported {
                word: "sat".to_owned(),
            },
            Lookup::Unreadable(OEWN),
            Lookup::Unfinished,
        ];
        for lookup in &all {
            match lookup {
                Lookup::Pending
                | Lookup::Article(_)
                | Lookup::Definitions { .. }
                | Lookup::NoEntry { .. }
                | Lookup::NoDictionary(_)
                | Lookup::NotImported { .. }
                | Lookup::Unreadable(_)
                | Lookup::Unfinished => {}
            }
        }
        all
    }

    /// `listed` as one line: its label and the items after it, separated by
    /// commas, or the line of an empty list.
    fn listed_line(listed: Listed<'_>) -> String {
        match listed {
            Listed::Items { label, items } => format!("{label} {}", items.join(", ")),
            Listed::Missing(line) => line.to_owned(),
        }
    }

    /// The parts of a result as the lines they read in, top to bottom: the
    /// line of a result that is not an article; otherwise the hint, or the
    /// chosen sense with its mark, its definition, the block of its online
    /// translation and its synonyms; then an empty line and the
    /// translations.
    fn read(texts: &ResultTexts<'_>) -> Vec<String> {
        if let Some(line) = &texts.line {
            return vec![line.to_string()];
        }
        let mut lines = Vec::new();
        lines.extend(texts.hint.map(str::to_owned));
        for sense in &texts.senses {
            if let Some(explained) = &sense.explained {
                lines.push(explained.mark.to_owned());
                lines.push(sense.definition.to_owned());
                if let Some(block) = &explained.definition {
                    lines.push(block.label.to_owned());
                    lines.push(block.line.to_string());
                }
                lines.push(listed_line(explained.synonyms));
            }
        }
        lines.push(String::new());
        lines.extend(texts.translations.map(listed_line));
        lines
    }

    /// The lines of `lookup` with the sense `chosen` in it.
    fn details(lookup: &Lookup, chosen: Option<usize>) -> Vec<String> {
        read(&result_texts(lookup, chosen, None, None))
    }

    /// The rows of the list: the number, the definition and whether the row
    /// is the chosen one.
    fn rows(lookup: &Lookup, chosen: Option<usize>) -> Vec<(usize, String, bool)> {
        result_texts(lookup, chosen, None, None)
            .senses
            .iter()
            .map(|sense| {
                (
                    sense.number,
                    sense.definition.to_owned(),
                    sense.explained.is_some(),
                )
            })
            .collect()
    }

    #[test]
    fn every_result_has_its_own_text() {
        let texts: Vec<_> = every_lookup()
            .iter()
            .map(|lookup| details(lookup, None).join("\n"))
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
                assert_eq!(rows(&lookup, chosen).is_empty(), !article, "{lookup:?}");
            }
        }
    }

    #[test]
    fn the_pending_and_unfinished_lines_keep_their_wording() {
        assert_eq!(details(&Lookup::Pending, None), ["Looking up…"]);
        assert_eq!(
            details(&Lookup::Unfinished, None),
            ["The lookup did not finish."]
        );
        assert_eq!(PENDING, "Looking up…");
        assert_eq!(UNFINISHED, "The lookup did not finish.");
    }

    #[test]
    fn an_article_as_it_arrives_lists_every_sense_and_chooses_none() {
        let lookup = Lookup::Article(article());
        assert_eq!(
            rows(&lookup, None),
            [
                (1, "first".to_owned(), false),
                (2, "second".to_owned(), false)
            ]
        );
    }

    #[test]
    fn an_article_as_it_arrives_asks_to_choose_and_explains_nothing() {
        let lookup = Lookup::Article(article());
        let lines = details(&lookup, None);
        assert_eq!(
            lines,
            [
                CHOOSE_SENSE.to_owned(),
                String::new(),
                format!("{RUSSIAN_LABEL} раз, два"),
            ]
        );
        let text = lines.join("\n");
        for absent in [EXPLANATION_MARK, SYNONYMS_LABEL, NO_SYNONYMS, "one", "uno"] {
            assert!(!text.contains(absent), "{absent:?} in {text}");
        }
    }

    #[test]
    fn the_chosen_sense_is_marked_in_the_list_and_explained_under_it() {
        let lookup = Lookup::Article(article());
        assert_eq!(
            rows(&lookup, Some(0)),
            [
                (1, "first".to_owned(), true),
                (2, "second".to_owned(), false)
            ]
        );
        let lines = details(&lookup, Some(0));
        assert_eq!(
            lines,
            [
                EXPLANATION_MARK.to_owned(),
                "first".to_owned(),
                format!("{SYNONYMS_LABEL} one, uno"),
                String::new(),
                format!("{RUSSIAN_LABEL} раз, два"),
            ]
        );
        assert!(!lines.contains(&CHOOSE_SENSE.to_owned()));
    }

    #[test]
    fn only_the_chosen_sense_is_chosen() {
        let article = Sample.lookup("bank").expect("bank");
        let count = article.senses.len();
        let lookup = Lookup::Article(article.clone());
        for chosen in 0..count {
            let rows = rows(&lookup, Some(chosen));
            assert_eq!(rows.len(), count);
            let picked: Vec<_> = rows.iter().filter(|(_, _, picked)| *picked).collect();
            assert_eq!(
                picked,
                [&(chosen + 1, article.senses[chosen].definition.clone(), true)]
            );
        }
    }

    #[test]
    fn a_chosen_sense_without_synonyms_says_so() {
        let lookup = Lookup::Article(article());
        let lines = details(&lookup, Some(1));
        assert_eq!(
            lines,
            [
                EXPLANATION_MARK.to_owned(),
                "second".to_owned(),
                NO_SYNONYMS.to_owned(),
                String::new(),
                format!("{RUSSIAN_LABEL} раз, два"),
            ]
        );
        assert!(!lines.join("\n").contains(SYNONYMS_LABEL));
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
            assert!(!lines.join("\n").contains(RUSSIAN_LABEL), "{lines:?}");
        }
    }

    #[test]
    fn an_article_without_senses_says_so_and_keeps_the_russian_line() {
        let mut bare = article();
        bare.senses.clear();
        let lookup = Lookup::Article(bare);
        // A sense cannot be chosen there; an index changes nothing.
        for chosen in [None, Some(0)] {
            assert!(rows(&lookup, chosen).is_empty());
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
        assert_eq!(rows(&lookup, Some(2)), rows(&lookup, None));
        assert_eq!(details(&lookup, Some(2)), details(&lookup, None));
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
        assert_eq!(rows(&lookup, None), [(1, written.to_owned(), false)]);
        assert_eq!(rows(&lookup, Some(0)), [(1, written.to_owned(), true)]);
        assert_eq!(
            details(&lookup, Some(0)),
            [
                EXPLANATION_MARK.to_owned(),
                written.to_owned(),
                format!("{SYNONYMS_LABEL} <i>x</i>, y & z"),
                String::new(),
                format!("{RUSSIAN_LABEL} <б>, &amp;"),
            ]
        );
    }

    #[test]
    fn the_sample_article_of_bank_is_shown_in_full() {
        let article = Sample.lookup("bank").expect("bank");
        let lookup = Lookup::Article(article.clone());
        let rows = rows(&lookup, None);
        assert_eq!(rows.len(), article.senses.len());
        for (index, sense) in article.senses.iter().enumerate() {
            assert_eq!(rows[index], (index + 1, sense.definition.clone(), false));
            let text = details(&lookup, Some(index)).join("\n");
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
    fn the_no_dictionary_line_names_the_language_in_russian() {
        assert_eq!(
            details(&Lookup::NoDictionary(Language::Es), None),
            ["Оффлайн-словаря для испанского нет"]
        );
        assert_eq!(
            details(&Lookup::NoDictionary(Language::Ca), None),
            ["Оффлайн-словаря для каталанского нет"]
        );
        // A line, not an article: no list, no translations.
        let texts = result_texts(&Lookup::NoDictionary(Language::Ca), Some(0), None, None);
        assert!(texts.senses.is_empty());
        assert_eq!(texts.translations, None);
        assert_eq!(texts.hint, None);
    }

    #[test]
    fn the_unsupported_language_line_names_no_pair() {
        let line = super::refusal_text(Refusal::UnsupportedLanguage, None);
        assert!(!line.contains("EN"), "{line}");
        assert!(line.contains("DeepL"), "{line}");
    }

    #[test]
    fn a_definition_is_sent_from_english() {
        let mut shown = definition_shown();
        shown.choose(1);
        let (_, request) = shown.request_definition().expect("action");
        assert_eq!(request.source, Language::En);
    }

    #[test]
    fn the_no_entry_line_names_the_word() {
        let line = details(
            &Lookup::NoEntry {
                word: "sat".to_owned(),
            },
            None,
        );
        assert_eq!(line, ["The dictionary has no entry for “sat”."]);
        // The word is the user's text: it is shown as written.
        let line = details(
            &Lookup::NoEntry {
                word: "a<b&c".to_owned(),
            },
            None,
        );
        assert_eq!(line, ["The dictionary has no entry for “a<b&c”."]);
    }

    #[test]
    fn the_not_imported_line_names_the_word_and_the_command() {
        let line = details(
            &Lookup::NotImported {
                word: "a<b".to_owned(),
            },
            None,
        );
        assert_eq!(
            line,
            ["There is no imported dictionary to look “a<b” up in. \
             Import a downloaded dictionary file with addict import <FILE>."]
        );
    }

    #[test]
    fn the_unreadable_line_names_the_index_its_file_and_the_command() {
        for file in [OEWN, FREEDICT] {
            let line = details(&Lookup::Unreadable(file), None).join("\n");
            assert!(line.contains(file.index), "{line}");
            assert!(line.contains(file.published), "{line}");
            assert!(line.contains("cannot be read"), "{line}");
            assert!(line.contains("again"), "{line}");
            assert!(line.contains("addict import <FILE>"), "{line}");
            assert!(!line.contains("did not finish"), "{line}");
        }
    }

    const STATES: [State; 3] = [State::Open, State::Missing, State::Unreadable];

    #[test]
    fn the_line_of_the_indexes_is_empty_only_when_both_are_open() {
        for definitions in STATES {
            for translations in STATES {
                let indexes = Indexes {
                    definitions,
                    translations,
                    spanish: State::Missing,
                };
                assert_eq!(
                    indexes_text(indexes).is_empty(),
                    indexes == Indexes::OPEN,
                    "{indexes:?}"
                );
            }
        }
    }

    #[test]
    fn with_nothing_imported_the_line_says_so_and_names_both_files() {
        let line = indexes_text(Indexes {
            definitions: State::Missing,
            translations: State::Missing,
            spanish: State::Missing,
        });
        assert_eq!(
            line,
            "The dictionaries are not imported. Import each downloaded file with \
             addict import <FILE>: english-wordnet-2025.xml.gz and \
             freedict-eng-rus-2025.11.23.src.tar.xz."
        );
    }

    #[test]
    fn the_line_of_the_indexes_names_each_one_that_is_not_open() {
        for definitions in STATES {
            for translations in STATES {
                let text = indexes_text(Indexes {
                    definitions,
                    translations,
                    spanish: State::Missing,
                });
                let named = [(OEWN, definitions), (FREEDICT, translations)];
                let lines = named.iter().filter(|(_, s)| *s != State::Open).count();
                if lines > 0 && (definitions, translations) != (State::Missing, State::Missing) {
                    assert_eq!(text.lines().count(), lines, "{text}");
                }
                for (file, state) in named {
                    // The file to import is named exactly for an index that
                    // is not open, and its index file when it is unreadable.
                    assert_eq!(
                        text.contains(file.published),
                        state != State::Open,
                        "{text}"
                    );
                    assert_eq!(
                        text.contains(file.index),
                        state == State::Unreadable,
                        "{text}"
                    );
                }
                if !text.is_empty() {
                    assert!(text.contains("addict import <FILE>"), "{text}");
                }
            }
        }
    }

    #[test]
    fn one_index_alone_says_what_the_other_would_give() {
        let text = indexes_text(Indexes {
            definitions: State::Open,
            translations: State::Missing,
            spanish: State::Missing,
        });
        assert_eq!(
            text,
            "No Russian translations: freedict-eng-rus-2025.11.23.src.tar.xz is not \
             imported. Import it with addict import <FILE>."
        );
        let text = indexes_text(Indexes {
            definitions: State::Unreadable,
            translations: State::Open,
            spanish: State::Missing,
        });
        assert_eq!(
            text,
            "No English definitions and synonyms: the index oewn.index cannot be read. \
             Import english-wordnet-2025.xml.gz again with addict import <FILE>."
        );
    }

    #[test]
    fn the_unfinished_line_names_no_cause() {
        let line = details(&Lookup::Unfinished, None).join("\n").to_lowercase();
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

    fn spanish() -> Lookup {
        Lookup::Definitions {
            language: Language::Es,
            article: Article {
                senses: vec![
                    Sense {
                        definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                        synonyms: Vec::new(),
                    },
                    Sense {
                        definition: "Mueble alargado para sentarse.".to_owned(),
                        synonyms: vec!["banca".to_owned()],
                    },
                ],
                translations: Vec::new(),
            },
        }
    }

    #[test]
    fn a_spanish_article_is_labelled_in_russian_by_its_kind_and_language() {
        let lookup = spanish();
        let texts = result_texts(&lookup, None, None, None);
        assert_eq!(texts.senses_label, "Толкования на испанском");
        assert_eq!(texts.line, None);
        // No line about Russian: missing kinds are said later.
        assert_eq!(texts.translations, None);
        assert_eq!(texts.hint, Some(CHOOSE_SENSE));
        assert_eq!(
            rows(&lookup, None),
            [
                (
                    1,
                    "Empresa que ofrece servicios monetarios.".to_owned(),
                    false
                ),
                (2, "Mueble alargado para sentarse.".to_owned(), false),
            ]
        );
        // The synonyms of the chosen sense, as for English.
        let chosen = result_texts(&lookup, Some(1), None, None);
        let explained = chosen.senses[1].explained.as_ref().expect("explained");
        assert_eq!(
            explained.synonyms,
            Listed::Items {
                label: SYNONYMS_LABEL,
                items: &["banca".to_owned()],
            }
        );
        assert_eq!(explained.definition, None);
        let none = result_texts(&lookup, Some(0), None, None);
        assert_eq!(
            none.senses[0]
                .explained
                .as_ref()
                .expect("explained")
                .synonyms,
            Listed::Missing(NO_SYNONYMS)
        );
        // Without senses, the line says so in Russian, about Spanish.
        let bare = Lookup::Definitions {
            language: Language::Es,
            article: Article {
                senses: Vec::new(),
                translations: Vec::new(),
            },
        };
        assert_eq!(
            result_texts(&bare, None, None, None).hint,
            Some("В испанском словаре нет толкований этого слова.")
        );
    }

    #[test]
    fn an_english_article_keeps_its_label_and_every_other_result_has_it_too() {
        for lookup in every_lookup() {
            assert_eq!(
                result_texts(&lookup, None, None, None).senses_label,
                "English definitions",
                "{lookup:?}"
            );
        }
    }

    #[test]
    fn a_chosen_spanish_sense_offers_no_definition_translation() {
        let mut shown = Shown::default();
        shown.show(Some(spanish()));
        assert!(shown.choose(1));
        assert!(!shown.choose(2));
        assert_eq!(shown.sense(), Some(1));
        assert_eq!(shown.definition_action(), (false, false));
        assert_eq!(shown.request_definition(), None);
        assert_eq!(shown.definition(), None);
        // The English article still offers it.
        shown.show(Some(Lookup::Article(article())));
        assert!(shown.choose(0));
        assert_eq!(shown.definition_action(), (true, true));
        let (_, request) = shown.request_definition().expect("request");
        assert_eq!(request.source, Language::En);
    }

    #[test]
    fn the_line_of_the_indexes_does_not_change_with_the_spanish_index() {
        for definitions in STATES {
            for translations in STATES {
                let english = Indexes {
                    definitions,
                    translations,
                    spanish: State::Missing,
                };
                for spanish in STATES {
                    assert_eq!(
                        indexes_text(Indexes { spanish, ..english }),
                        indexes_text(english),
                        "{english:?} {spanish:?}"
                    );
                }
            }
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
            let texts = texts(state, Source::Manual);
            assert!(!texts.source.trim().is_empty(), "{state:?}");
            assert!(!texts.status.trim().is_empty(), "{state:?}");
        }
    }

    #[test]
    fn each_source_has_its_own_label() {
        for kind in &KINDS[1..] {
            let labels: Vec<_> = same_kind(*kind, &Source::ALL)
                .into_iter()
                .map(|state| texts(state, Source::Manual).source)
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
                .map(|state| texts(state, Source::Manual).source)
                .collect();
            assert!(labels.iter().all(|label| *label == labels[0]), "{source:?}");
        }
    }

    #[test]
    fn waiting_status_differs_from_the_other_statuses() {
        for source in Source::ALL {
            let waiting = texts(WindowState::Waiting { source }, Source::Manual).status;
            for state in every_state() {
                if !matches!(state, WindowState::Waiting { .. }) {
                    assert_ne!(texts(state, Source::Manual).status, waiting, "{state:?}");
                }
            }
        }
    }

    #[test]
    fn empty_names_the_source_of_the_text() {
        let empty = texts(WindowState::Empty, Source::Ocr);
        assert_eq!(empty.source, "screen capture (OCR)");
        assert_eq!(empty.status, "waiting for input");
    }

    #[test]
    fn empty_asks_for_manual_input() {
        let empty = texts(WindowState::Empty, Source::Manual);
        assert_eq!(empty.source, "manual input");
        assert_eq!(empty.status, "waiting for input");
        assert_eq!(
            empty.source,
            texts(
                WindowState::Waiting {
                    source: Source::Manual,
                },
                Source::Manual,
            )
            .source
        );
    }
    #[test]
    fn refusal_lines_are_distinct_and_key_paths_are_escaped() {
        let refusals = [
            Refusal::Key,
            Refusal::UnreadableKey,
            Refusal::UnsafeKey,
            Refusal::RejectedKey,
            Refusal::Network,
            Refusal::Quota,
            Refusal::UnsupportedLanguage,
            Refusal::TimedOut,
            Refusal::Other,
        ];
        let mut lines = std::collections::BTreeSet::new();
        for refusal in refusals {
            let line = super::refusal_text(refusal, None);
            assert!(lines.insert(line.clone()), "{line}");
            let online = Online::Refused(refusal);
            let markup = super::online_markup(
                &OnlineTexts {
                    lone: Some(&online),
                    ..OnlineTexts::default()
                },
                Some(std::path::Path::new("/config/<key>&/deepl.key")),
            );
            if matches!(
                refusal,
                Refusal::Key | Refusal::UnreadableKey | Refusal::UnsafeKey | Refusal::RejectedKey
            ) {
                assert!(
                    markup.contains("/config/&lt;key&gt;&amp;/deepl.key"),
                    "{markup}"
                );
                assert!(
                    line.contains("XDG_CONFIG_HOME") && line.contains("HOME"),
                    "{line}"
                );
            } else {
                assert!(!markup.contains("deepl.key"), "{markup}");
            }
        }
    }

    #[test]
    fn a_too_long_text_names_where_it_was_and_its_length() {
        assert_eq!(
            too_long_text(Held::Selection, 1234, 1000),
            "The selected text is too long (1234 characters; the limit is 1000). \
             Select one sentence, or type it."
        );
        assert_eq!(
            too_long_text(Held::Clipboard, 54828, 1000),
            "The copied text is too long (54828 characters; the limit is 1000). \
             Copy one sentence, or type it."
        );
    }
}
