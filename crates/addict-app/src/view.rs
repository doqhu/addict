//! What the window says about its state: the source label and the status,
//! right under them the source line when a capture or a read of Android
//! Studio gave no text, then the
//! binding row when the shortcut is not bound and the line of the
//! dictionary indexes when one is missing or unreadable, then the field
//! for a word or a sentence, the text of the field with its words as links
//! and the result of the lookup of the chosen word: what is said about the
//! word, its translations among it, and the senses of the word in a list,
//! with what is said about the sense that was clicked under it and its
//! definition translated online once that is asked for, and, for a language
//! beside English, its English glosses, one of which can be chosen and
//! translated instead. In the card of the word, under its translations,
//! stand the online texts, each beside a label that says what it is: the
//! chosen word translated in its sentence, the sentence itself, or a word
//! translated without a sentence; a dim line under them names DeepL.
//!
//! The texts live here and nowhere else; `addict-core` has none. [`texts`],
//! [`binding_texts`], [`indexes_text`], [`failure_text`], [`read_failure_text`], [`too_long_text`],
//! [`sentence_markup`],
//! [`choice_texts`] and [`online_rows`] are pure, and
//! so is [`Shown`], the result with the sense chosen in it; they are tested
//! without a display. This module owns no
//! widget: [`View`](crate::widgets::View) puts the results into its own.

use std::{borrow::Cow, fmt::Write as _, ops::Range, path::Path};

use addict_core::{
    dictionary::{Article, LexicalEntry, PartOfSpeech, Pos, Pronunciation, Sense},
    input::{Source, WindowState},
    translation::{Refusal, Request, Source as Language},
};

use addict_platform::{
    atspi::ContextFailure, foreground::Source as Foregrounded, ocr::OcrFailure,
    screenshot::CaptureFailure,
};

use addict_dictionary::{
    analysis::{Analyses, Number, Person, Shape, TenseMood},
    lookup::Grouped,
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
pub(crate) const LANGUAGE_TOOLTIP: &str = "The language of the text you read";

/// What the button beside the field says: it takes a screen capture and
/// puts the text recognized in it into the field.
pub(crate) const CAPTURE_LABEL: &str = "Capture";

/// The tooltip of the button left of the field that puts the text before
/// into it, naming its hotkey.
pub(crate) const BACK_TOOLTIP: &str = "Back (Alt+Left)";

/// The tooltip of the button left of the field that puts the text after
/// into it, naming its hotkey.
pub(crate) const FORWARD_TOOLTIP: &str = "Forward (Alt+Right)";

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
        // Only a read whose application in front is not known looks for
        // Android Studio on the bus: say that first, as for an unknown one,
        // and how to make Android Studio readable after it.
        ContextFailure::AppNotFound => "The application in front is not known, so nothing was \
             read. To read Android Studio, start it with the AtkWrapper VM option and screen \
             reader support."
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

/// What the button beside a source line that names a command says: it puts
/// that command on the clipboard.
pub(crate) const COPY_LABEL: &str = "Copy";

/// What that button says once pressed, until the line changes.
pub(crate) const COPIED_LABEL: &str = "Copied";

/// The runnable command `line` names, built by the same functions that
/// wrote it into the line: the install of Tesseract for a language, then the
/// install of a language's data, then the command that turns accessibility
/// on. `None` for a line that names none.
pub(crate) fn command_in(line: &str) -> Option<String> {
    let tesseract = Language::ALL.into_iter().map(install_tesseract);
    let data = Language::ALL.into_iter().map(install_language);
    tesseract
        .chain(data)
        .chain(std::iter::once(TURN_ON_ACCESSIBILITY.to_owned()))
        .find(|command| line.contains(command.as_str()))
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
    /// The indexes of `language`, not English, gave this for the word, by
    /// kind: one of them at least has it.
    Grouped {
        language: Language,
        grouped: Grouped,
    },
    /// The Russian index of `language` gave these translations of a
    /// Russian word, never empty; the article is this list alone.
    FromRussian {
        language: Language,
        translations: Vec<String>,
    },
    /// The dictionary has no entry for `word`.
    NoEntry { word: String },
    /// No offline dictionary of the reading language is installed, so
    /// nothing was looked up.
    NoDictionary(Language),
    /// No index is open: there is nothing to look `word` up in.
    NotImported { word: String },
    /// No Russian index of `language` is imported, so a Russian word has
    /// nothing to be looked up in.
    NoRussianDictionary(Language),
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
const NO_DICTIONARY_BEFORE: &str = "There is no offline dictionary for ";
const NO_DICTIONARY_AFTER: &str = ".";

/// The name of `language` in the line of a missing dictionary. No wildcard arm: a new language does not compile until it is
/// given its name.
fn dictionary_language(language: Language) -> &'static str {
    match language {
        Language::En => "English",
        Language::Es => "Spanish",
        Language::Ca => "Catalan",
    }
}

/// The line above an article of another language than the chosen one,
/// which had no entry for the word: what stands before the chosen
/// language, between it and the language that answered, and after that.
const ELSEWHERE_BEFORE: &str = "Not in the ";
const ELSEWHERE_BETWEEN: &str = " dictionaries: the ";
const ELSEWHERE_AFTER: &str = " article.";

/// The line above an article that `found` gave for a word the `chosen`
/// language had no entry for.
fn elsewhere_text(chosen: Language, found: Language) -> String {
    format!(
        "{ELSEWHERE_BEFORE}{}{ELSEWHERE_BETWEEN}{}{ELSEWHERE_AFTER}",
        dictionary_language(chosen),
        dictionary_language(found)
    )
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

/// What stands before the translations of a Russian word into `language`,
/// as [`RUSSIAN_LABEL`] stands before those of a word into Russian. No
/// wildcard arm, as in [`definitions_label`].
fn from_russian_label(language: Language) -> &'static str {
    match language {
        Language::En => "English, for the word as a whole:",
        Language::Es => "Spanish, for the word as a whole:",
        Language::Ca => "Catalan, for the word as a whole:",
    }
}

/// The line of a Russian word read with no Russian index of `language`
/// imported, as [`missing_russian`] is for the other way. No wildcard arm,
/// as in [`definitions_label`].
fn missing_from_russian(language: Language) -> &'static str {
    match language {
        Language::En => "No Russian-English dictionary is imported.",
        Language::Es => "No Russian-Spanish dictionary is imported.",
        Language::Ca => "No Russian-Catalan dictionary is imported.",
    }
}

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
        Language::Es => "Spanish definitions",
        Language::Ca => "Catalan definitions",
    }
}

/// The line of an article from the definitions index of `language` without
/// senses. No wildcard arm, as in [`definitions_label`].
fn no_definitions(language: Language) -> &'static str {
    match language {
        Language::En => NO_DEFINITIONS,
        Language::Es => "The dictionary has no Spanish definitions of this word.",
        Language::Ca => "The dictionary has no Catalan definitions of this word.",
    }
}

/// The line of a language beside English with no Russian index imported.
/// No wildcard arm, as in [`definitions_label`].
fn missing_russian(language: Language) -> &'static str {
    match language {
        Language::En => "No English-Russian dictionary is imported.",
        Language::Es => "No Spanish-Russian dictionary is imported.",
        Language::Ca => "No Catalan-Russian dictionary is imported.",
    }
}

/// The line of a language with no definitions index imported. No wildcard
/// arm, as in [`definitions_label`].
fn missing_definitions(language: Language) -> &'static str {
    match language {
        Language::En => "No dictionary of English definitions is imported.",
        Language::Es => "No dictionary of Spanish definitions is imported.",
        Language::Ca => "No dictionary of Catalan definitions is imported.",
    }
}

/// What stands before the synonyms of the word as a whole, from a synonyms
/// index of `language`. No wildcard arm, as in [`definitions_label`].
fn word_synonyms_label(language: Language) -> &'static str {
    match language {
        Language::En => "English synonyms of the word",
        Language::Es => "Spanish synonyms of the word",
        Language::Ca => "Catalan synonyms of the word",
    }
}

/// The line of a language with no synonyms index imported. No wildcard arm,
/// as in [`definitions_label`].
fn missing_synonyms(language: Language) -> &'static str {
    match language {
        Language::En => "No dictionary of English synonyms is imported.",
        Language::Es => "No dictionary of Spanish synonyms is imported.",
        Language::Ca => "No dictionary of Catalan synonyms is imported.",
    }
}

/// The line of a word that neither its senses nor a synonyms index give a
/// synonym for.
const NO_WORD_SYNONYMS: &str = "The dictionaries give no synonyms for this word.";

/// What stands above the English glosses of a word of another language. They
/// are English, whatever the language of the word.
const ENGLISH_LABEL: &str = "English glosses";

/// The line of a word the English glosses index has no gloss for.
const NO_GLOSS: &str = "The dictionary has no English gloss of this word.";

/// Between the lemma of an analysis line and its groups.
pub const ANALYSIS_DASH: &str = "—";

/// Between the groups of one analysis line.
pub const ANALYSIS_SEPARATOR: &str = "·";

/// The line under the analysis of a Catalan form of `anar` that is also the
/// auxiliary of the periphrastic past, around the word as picked.
const PERIPHRASTIC_BEFORE: &str = "Also the auxiliary of the periphrastic past: ";
const PERIPHRASTIC_AFTER: &str = " + infinitive = past.";

/// What opens the article of a verb form: a line per lemma of its analyses,
/// then, for a Catalan auxiliary of the periphrastic past, the note that
/// says so. Empty without analyses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    pub lines: Vec<AnalysisLine>,
    pub note: Option<String>,
}

impl Analysis {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.note.is_none()
    }
}

/// One lemma and what the form is of it: the lemma, which the window shows
/// as a link to its own article, then a group per tense and mood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisLine {
    pub lemma: String,
    pub groups: Vec<AnalysisGroup>,
}

/// One group of an analysis line, in the parts the window styles apart:
/// the persons and numbers as a tag, the tense, then the mood, dimmer.
/// A past participle or a gerund has no persons and no mood, and an
/// imperative or a conditional has only the one word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisGroup {
    pub persons: Option<String>,
    pub tense: Option<&'static str>,
    pub mood: Option<&'static str>,
}

impl AnalysisLine {
    /// The line as plain text, as it reads on screen:
    /// `hablar — 1st/3rd sg present subjunctive · usted/3rd sg imperative`.
    pub fn text(&self) -> String {
        let groups: Vec<String> = self.groups.iter().map(AnalysisGroup::text).collect();
        format!(
            "{} {ANALYSIS_DASH} {}",
            self.lemma,
            groups.join(&format!(" {ANALYSIS_SEPARATOR} "))
        )
    }
}

impl AnalysisGroup {
    /// The group as plain text: its parts joined by spaces.
    pub fn text(&self) -> String {
        [self.persons.as_deref(), self.tense, self.mood]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The analysis that opens the article of a verb form; empty without one.
fn analysis(language: Language, grouped: &Grouped) -> Analysis {
    let Some(verb) = &grouped.verb else {
        return Analysis::default();
    };
    Analysis {
        lines: verb
            .analyses
            .iter()
            .map(|analyses| analysis_line(language, analyses))
            .collect(),
        note: verb
            .periphrastic
            .as_ref()
            .map(|word| format!("{PERIPHRASTIC_BEFORE}{word}{PERIPHRASTIC_AFTER}")),
    }
}

/// The lemma and a group per tense and mood, or a past participle or a
/// gerund, in the order of the shapes, which are sorted.
fn analysis_line(language: Language, analyses: &Analyses) -> AnalysisLine {
    let mut groups = Vec::new();
    let mut shapes = analyses.shapes.iter().copied().peekable();
    while let Some(shape) = shapes.next() {
        let Shape::Finite {
            tense_mood,
            number,
            person,
        } = shape
        else {
            groups.push(AnalysisGroup {
                persons: None,
                tense: Some(nonfinite_text(shape)),
                mood: None,
            });
            continue;
        };
        let mut persons = vec![(number, person)];
        while let Some(Shape::Finite {
            tense_mood: next,
            number,
            person,
        }) = shapes.peek().copied()
        {
            if next != tense_mood {
                break;
            }
            persons.push((number, person));
            shapes.next();
        }
        let (tense, mood) = tense_mood_text(tense_mood);
        groups.push(AnalysisGroup {
            persons: Some(persons_text(language, &persons)),
            tense,
            mood,
        });
    }
    AnalysisLine {
        lemma: analyses.lemma.clone(),
        groups,
    }
}

/// The persons of one tense and mood, sorted: those of the singular, then
/// those of the plural, joined by `/`; in each the persons joined by `/`
/// and the number after them, unless every one of them is named by a
/// pronoun that says it (`usted/3rd sg`, `vos`, `1st sg/2nd pl`).
fn persons_text(language: Language, persons: &[(Number, Person)]) -> String {
    let mut chunks = Vec::new();
    for number in [Number::Singular, Number::Plural] {
        let of: Vec<Person> = persons
            .iter()
            .filter(|(own, _)| *own == number)
            .map(|(_, person)| *person)
            .collect();
        if of.is_empty() {
            continue;
        }
        let names: Vec<&str> = of
            .iter()
            .map(|person| person_text(language, *person, number))
            .collect();
        let mut chunk = names.join("/");
        if !of
            .iter()
            .all(|person| matches!(person, Person::Vos | Person::Formal))
        {
            chunk.push(' ');
            chunk.push_str(match number {
                Number::Singular => "sg",
                Number::Plural => "pl",
            });
        }
        chunks.push(chunk);
    }
    chunks.join("/")
}

/// A person as the analysis line names it; the formal second person by the
/// pronoun of `language`. No wildcard arm: a new language does not compile
/// until it is given its pronouns.
fn person_text(language: Language, person: Person, number: Number) -> &'static str {
    match person {
        Person::First => "1st",
        Person::Second => "2nd",
        Person::Third => "3rd",
        Person::Vos => "vos",
        Person::Formal => match (language, number) {
            (Language::Es, Number::Singular) => "usted",
            (Language::Es, Number::Plural) => "ustedes",
            (Language::Ca, Number::Singular) => "vostè",
            (Language::Ca, Number::Plural) => "vostès",
            // English has no analyses: the words of the tags.
            (Language::En, Number::Singular) => "formal sg",
            (Language::En, Number::Plural) => "formal pl",
        },
    }
}

/// A tense and mood as the analysis line names them: the tense, then the
/// mood, each when it has a word of its own.
fn tense_mood_text(tense_mood: TenseMood) -> (Option<&'static str>, Option<&'static str>) {
    const INDICATIVE: Option<&str> = Some("indicative");
    const SUBJUNCTIVE: Option<&str> = Some("subjunctive");
    match tense_mood {
        TenseMood::PresentIndicative => (Some("present"), INDICATIVE),
        TenseMood::PreteriteIndicative => (Some("preterite"), INDICATIVE),
        TenseMood::ImperfectIndicative => (Some("imperfect"), INDICATIVE),
        TenseMood::FutureIndicative => (Some("future"), INDICATIVE),
        TenseMood::Conditional => (Some("conditional"), None),
        TenseMood::PresentSubjunctive => (Some("present"), SUBJUNCTIVE),
        TenseMood::ImperfectSubjunctive => (Some("imperfect"), SUBJUNCTIVE),
        TenseMood::FutureSubjunctive => (Some("future"), SUBJUNCTIVE),
        TenseMood::Imperative => (None, Some("imperative")),
    }
}

/// A shape without a person as the analysis line names it; a finite one
/// has its group instead.
fn nonfinite_text(shape: Shape) -> &'static str {
    match shape {
        Shape::PastParticiple => "past participle",
        Shape::Gerund => "gerund",
        Shape::Finite { .. } => "",
    }
}

/// The line of a language beside English with no English glosses index
/// imported. No wildcard arm, as in [`definitions_label`].
fn missing_english(language: Language) -> &'static str {
    match language {
        Language::En => "No dictionary of English glosses is imported.",
        Language::Es => "No Spanish-English dictionary is imported.",
        Language::Ca => "No Catalan-English dictionary is imported.",
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

/// Separate synonym meanings, or the existing missing-source/result line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynonymTexts<'a> {
    Groups {
        label: &'static str,
        groups: &'a [addict_dictionary::index::SynonymGroup],
    },
    Missing(&'static str),
}

impl SynonymTexts<'_> {
    /// The section as one line: the label and the synonyms of every
    /// meaning, joined, or the missing line.
    #[cfg(test)]
    pub fn line(&self) -> String {
        match self {
            Self::Groups { label, groups } => format!(
                "{label} {}",
                groups
                    .iter()
                    .flat_map(|group| &group.synonyms)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Missing(line) => (*line).to_owned(),
        }
    }
}

pub(crate) const SYNONYM_PREVIEW: usize = 6;
pub(crate) const SHOW_MORE: &str = "Show more";
pub(crate) const SHOW_LESS: &str = "Show less";

/// Uses only explicit source information, with an ordinal for unlabeled
/// meanings. The complete original grammatical label remains available.
pub(crate) fn synonym_heading(
    group: &addict_dictionary::index::SynonymGroup,
    number: usize,
) -> String {
    use addict_dictionary::format::GrammaticalClass;
    let mut text = group.headword.clone();
    if let Some(metadata) = &group.metadata {
        let class = match metadata.class() {
            Some(GrammaticalClass::Verb) => "Verb",
            Some(GrammaticalClass::Adjective) => "Adjective",
            Some(GrammaticalClass::Noun) => "Noun",
            Some(GrammaticalClass::Adverb) => "Adverb",
            None => "",
        };
        if !class.is_empty() {
            write!(text, " · {class}").expect("write string");
        }
    }
    write!(text, " · Meaning {number}").expect("write string");
    let description = group
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.description())
        .or_else(|| (!group.definition.trim().is_empty()).then_some(group.definition.as_str()));
    if let Some(description) = description {
        write!(text, " — {description}").expect("write string");
    }
    text
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
    /// Its part of speech, as [`pos_label`] names it: its own, or the one
    /// its entries agree on; `None` when neither is known.
    pub pos: Option<&'a str>,
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
    /// For an article of a verb form, what opens it: a line per lemma with
    /// the analyses of the form, then any note under them.
    pub analysis: Analysis,
    /// For an article of another language than the chosen one, which had no
    /// entry for the word, the line that names both.
    pub elsewhere: Option<String>,
    /// What stands above the list of senses: their kind and language.
    pub senses_label: &'static str,
    /// For an article, what stands above its list while no sense is chosen
    /// or when it has none.
    pub hint: Option<&'static str>,
    /// For an article, one row for each sense, in order.
    pub senses: Vec<SenseText<'a>>,
    /// For an article, its translations, which belong to the word.
    pub translations: Option<Listed<'a>>,
    /// For an article of a language beside English, the synonyms of the
    /// word as a whole, shown under its definitions; `None` when there is
    /// nothing to say.
    pub word_synonyms: Option<SynonymTexts<'a>>,
    /// For an article of a language beside English, its English glosses:
    /// rows to choose, one at a time with the senses.
    pub glosses: Option<Listed<'a>>,
    /// The chosen English gloss, when one of [`ResultTexts::glosses`] is.
    pub gloss: Option<ChosenGloss<'a>>,
    /// For an article, the pronunciations of its entries in source order,
    /// each once; the window shows [`PRONUNCIATION_PREVIEW`] of them and the
    /// rest behind [`SHOW_MORE`].
    pub pronunciations: Vec<PronunciationText<'a>>,
}

/// How many pronunciations the card shows before [`SHOW_MORE`].
pub(crate) const PRONUNCIATION_PREVIEW: usize = 3;

/// The tooltip of the toggle that shows the pronunciations past
/// [`PRONUNCIATION_PREVIEW`].
pub(crate) const MORE_PRONUNCIATIONS: &str = "Pronunciations of the word";

/// What the play button of a pronunciation says when hovered.
pub(crate) const PLAY_TOOLTIP: &str = "Play the pronunciation";

/// The tooltip of a play button whose playback failed. It names no cause:
/// the media backend's message may be translated.
pub(crate) const PLAYBACK_FAILED: &str = "Playback failed.";

/// One pronunciation of an article: plain texts, never markup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PronunciationText<'a> {
    /// The headword of the entry it belongs to; see
    /// [`PronunciationText::prefix`].
    pub headword: &'a str,
    /// The transcription; `None` for a record that has a recording only.
    pub transcription: Option<&'a str>,
    /// What stands beside it: the source's labels, such as a region, and
    /// the notation when it is not IPA.
    pub labels: Vec<&'a str>,
    /// The recording to play: an ogg one when the record has one, else its
    /// first; `None` when it has none.
    pub audio: Option<&'a str>,
}

impl<'a> PronunciationText<'a> {
    /// What stands before the pronunciation when the window shows `word`:
    /// the headword of its entry when it is another word, as the lemma of
    /// a verb form is.
    pub fn prefix(&self, word: &str) -> Option<&'a str> {
        let word = word.trim();
        (!word.is_empty()
            && !self.headword.trim().is_empty()
            && self.headword.trim().to_lowercase() != word.to_lowercase())
        .then_some(self.headword)
    }
}

/// `text` as compared with another transcription: without the slashes around
/// a phonemic transcription or the brackets around a phonetic one, and
/// without syllable dots, so `kʌʃ`, `/kʌʃ/` and `[kʌʃ]` are one, and
/// `ˈkʌʃɪŋ` is `/ˈkʌ.ʃɪŋ/`; a phonetic transcription that says more, such
/// as `[kʰʌʃ]`, stays apart.
fn transcription_key(text: &str) -> String {
    let text = text.trim();
    let text = [('/', '/'), ('[', ']')]
        .into_iter()
        .find_map(|(open, close)| text.strip_prefix(open)?.strip_suffix(close))
        .unwrap_or(text);
    text.chars().filter(|c| *c != '.').collect()
}

/// The notation of `record` as compared with another one's: `None` for IPA,
/// which is not named on a row, as for a record that names no notation.
fn notation_key(record: &Pronunciation) -> Option<String> {
    record
        .notation
        .as_deref()
        .map(str::trim)
        .filter(|notation| !notation.is_empty() && !notation.eq_ignore_ascii_case("IPA"))
        .map(str::to_lowercase)
}

/// The pronunciations of `entries`, in their order, a record with the same
/// headword, transcription, notation and labels as one before it skipped (a
/// record without a transcription also needs the same recordings); a skipped
/// record lends its recording to the kept one when that has none.
/// Transcriptions are compared by [`transcription_key`] and notations by
/// [`notation_key`]; the kept row shows the first record's text. A
/// transcription without labels is dropped when the same headword has it
/// with labels, as that row says more. The rows of
/// one headword are then grouped, the headwords in the order they first
/// appear and each one's rows in source order.
fn pronunciation_texts(entries: &[LexicalEntry]) -> Vec<PronunciationText<'_>> {
    let mut kept: Vec<(&Pronunciation, PronunciationText<'_>)> = Vec::new();
    for entry in entries {
        for record in &entry.pronunciations {
            let audio = record
                .audio
                .iter()
                .find(|audio| {
                    audio
                        .format
                        .as_deref()
                        .is_some_and(|format| format.eq_ignore_ascii_case("ogg"))
                        || audio.url.to_lowercase().ends_with(".ogg")
                })
                .or(record.audio.first())
                .map(|audio| audio.url.as_str());
            let transcription = record
                .text
                .as_deref()
                .filter(|text| !text.trim().is_empty());
            if transcription.is_none() && audio.is_none() {
                continue;
            }
            let headword = entry.identity.headword.as_str();
            let key = transcription.map(transcription_key);
            let notation = notation_key(record);
            if let Some((_, earlier)) = kept.iter_mut().find(|(earlier, text)| {
                text.headword == headword
                    && text.transcription.map(transcription_key) == key
                    && notation_key(earlier) == notation
                    && earlier.labels == record.labels
                    // Each recording without a transcription keeps its row.
                    && (transcription.is_some() || earlier.audio == record.audio)
            }) {
                earlier.audio = earlier.audio.or(audio);
                continue;
            }
            let mut labels: Vec<&str> = record
                .labels
                .iter()
                .map(String::as_str)
                .filter(|label| !label.trim().is_empty())
                .collect();
            if let Some(notation) = record.notation.as_deref().filter(|notation| {
                transcription.is_some()
                    && !notation.trim().is_empty()
                    && !notation.trim().eq_ignore_ascii_case("IPA")
            }) {
                labels.push(notation);
            }
            kept.push((
                record,
                PronunciationText {
                    headword: &entry.identity.headword,
                    transcription,
                    labels,
                    audio,
                },
            ));
        }
    }
    // The rows of a headword stand together, in the order of its first row,
    // so that one word's transcriptions from two sources are side by side;
    // the order is taken before a row gives way, so a word keeps its place.
    let mut headwords: Vec<&str> = Vec::new();
    for (_, text) in &kept {
        if !headwords.contains(&text.headword) {
            headwords.push(text.headword);
        }
    }
    // A transcription without labels says less than the same one with
    // labels, so it gives way to that row and lends it its recording.
    let labelled =
        |record: &Pronunciation| record.labels.iter().any(|label| !label.trim().is_empty());
    let mut index = 0;
    while index < kept.len() {
        let (record, text) = &kept[index];
        let (headword, audio) = (text.headword, text.audio);
        let key = text.transcription.map(transcription_key);
        let notation = notation_key(record);
        let fuller = (key.is_some() && !labelled(record))
            .then(|| {
                kept.iter().position(|(other, text)| {
                    labelled(other)
                        && text.headword == headword
                        && text.transcription.map(transcription_key) == key
                        && notation_key(other) == notation
                })
            })
            .flatten();
        match fuller {
            Some(fuller) => {
                let text = &mut kept[fuller].1;
                text.audio = text.audio.or(audio);
                kept.remove(index);
            }
            None => index += 1,
        }
    }
    let mut texts: Vec<_> = kept.into_iter().map(|(_, text)| text).collect();
    texts.sort_by_key(|text| headwords.iter().position(|word| *word == text.headword));
    texts
}

/// The name of the part of speech `pos`: a fixed English one for a known
/// category, the source's label as written for another.
pub fn pos_label(pos: &PartOfSpeech) -> &str {
    let Some(category) = pos.category else {
        return &pos.label;
    };
    match category {
        Pos::Noun => "noun",
        Pos::Verb => "verb",
        Pos::Adjective | Pos::AdjectiveSatellite => "adjective",
        Pos::Adverb => "adverb",
        Pos::Pronoun => "pronoun",
        Pos::Determiner => "determiner",
        Pos::Article => "article",
        Pos::Preposition => "preposition",
        Pos::Postposition => "postposition",
        Pos::Conjunction => "conjunction",
        Pos::Interjection => "interjection",
        Pos::Numeral => "numeral",
        Pos::Particle => "particle",
        Pos::ProperNoun => "proper noun",
        Pos::Auxiliary => "auxiliary verb",
        Pos::Affix => "affix",
        Pos::Prefix => "prefix",
        Pos::Suffix => "suffix",
        Pos::Infix => "infix",
        Pos::Circumfix => "circumfix",
        Pos::Contraction => "contraction",
        Pos::Abbreviation => "abbreviation",
        Pos::Acronym => "acronym",
        Pos::Symbol => "symbol",
        Pos::Letter => "letter",
        Pos::Phrase => "phrase",
        Pos::Proverb => "proverb",
        Pos::Idiom => "idiom",
        Pos::Punctuation => "punctuation",
    }
}

/// The part of speech of `sense`: its own or, without one, the one its
/// entries among `entries` agree on; `None` when they disagree or say
/// nothing.
fn sense_pos<'a>(sense: &'a Sense, entries: &'a [LexicalEntry]) -> Option<&'a str> {
    if let Some(pos) = &sense.part_of_speech {
        return Some(pos_label(pos));
    }
    let mut labels = entries
        .iter()
        .filter(|entry| sense.entries.contains(&entry.identity))
        .filter_map(|entry| entry.part_of_speech.as_ref().map(pos_label));
    let first = labels.next()?;
    labels.all(|label| label == first).then_some(first)
}

/// The English gloss chosen in an article of a language beside English.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChosenGloss<'a> {
    /// Its index among the glosses.
    pub index: usize,
    /// The online translation of the gloss, once asked for; never for a
    /// blank gloss.
    pub definition: Option<Block<'a>>,
}

/// What is chosen in the shown result: a sense of its definitions or, in an
/// article of a language beside English, one of its English glosses. One at
/// a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The sense with this index.
    Sense(usize),
    /// The English gloss with this index.
    Gloss(usize),
}

/// The parts of `lookup` with the sense `chosen` in it, as
/// [`choice_texts`] gives them; for the tests.
#[cfg(test)]
pub fn result_texts<'a>(
    lookup: &'a Lookup,
    chosen: Option<usize>,
    definition: Option<&'a Online>,
    key_file: Option<&Path>,
) -> ResultTexts<'a> {
    choice_texts(
        lookup,
        chosen.map(Choice::Sense),
        definition,
        key_file,
        None,
    )
}

/// The parts of `lookup` with `choice` in it; a choice the result does not
/// have is no choice. The online translation of the definition,
/// `definition`, stands under the chosen sense or gloss when its text is not
/// blank. A refusal about the key names `key_file`, as in
/// [`online_rows`]. An article that the language `elsewhere.1` gave for a
/// word the chosen language `elsewhere.0` had no entry for has the line
/// that names both. No wildcard arm: a new variant does not compile until
/// it is given its text.
pub fn choice_texts<'a>(
    lookup: &'a Lookup,
    choice: Option<Choice>,
    definition: Option<&'a Online>,
    key_file: Option<&Path>,
    elsewhere: Option<(Language, Language)>,
) -> ResultTexts<'a> {
    let elsewhere = elsewhere
        .filter(|(chosen, found)| chosen != found)
        .map(|(chosen, found)| elsewhere_text(chosen, found));
    let texts = lookup_texts(lookup, choice, definition, key_file);
    match lookup {
        Lookup::Article(_) | Lookup::Grouped { .. } => ResultTexts { elsewhere, ..texts },
        // A Russian word is answered by the chosen language only.
        Lookup::FromRussian { .. }
        | Lookup::Pending
        | Lookup::NoEntry { .. }
        | Lookup::NoDictionary(_)
        | Lookup::NotImported { .. }
        | Lookup::NoRussianDictionary(_)
        | Lookup::Unreadable(_)
        | Lookup::Unfinished => texts,
    }
}

/// The parts of `lookup` with `choice` in it, with no line about another
/// language; see [`choice_texts`].
fn lookup_texts<'a>(
    lookup: &'a Lookup,
    choice: Option<Choice>,
    definition: Option<&'a Online>,
    key_file: Option<&Path>,
) -> ResultTexts<'a> {
    let chosen = match choice {
        Some(Choice::Sense(index)) => Some(index),
        Some(Choice::Gloss(_)) | None => None,
    };
    let line = |text: Cow<'a, str>| ResultTexts {
        line: Some(text),
        analysis: Analysis::default(),
        elsewhere: None,
        senses_label: SENSES_LABEL,
        hint: None,
        senses: Vec::new(),
        translations: None,
        word_synonyms: None,
        glosses: None,
        gloss: None,
        pronunciations: Vec::new(),
    };
    match lookup {
        Lookup::Pending => line(Cow::Borrowed(PENDING)),
        Lookup::Article(article) => article_texts(article, chosen, definition, key_file),
        Lookup::Grouped { language, grouped } => {
            grouped_texts(*language, grouped, choice, definition, key_file)
        }
        // The list of translations alone: no senses, synonyms or glosses.
        Lookup::FromRussian {
            language,
            translations,
        } => ResultTexts {
            line: None,
            translations: Some(Listed::Items {
                label: from_russian_label(*language),
                items: translations,
            }),
            ..line(Cow::Borrowed(""))
        },
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
        Lookup::NoRussianDictionary(language) => {
            line(Cow::Borrowed(missing_from_russian(*language)))
        }
        Lookup::Unreadable(file) => line(Cow::Owned(format!(
            "{UNREADABLE_BEFORE}{}",
            unreadable_text(*file)
        ))),
        Lookup::Unfinished => line(Cow::Borrowed(UNFINISHED)),
    }
}

/// The parts of `article`; see [`choice_texts`].
fn article_texts<'a>(
    article: &'a Article,
    chosen: Option<usize>,
    definition: Option<&'a Online>,
    key_file: Option<&Path>,
) -> ResultTexts<'a> {
    let (hint, senses) = sense_texts(
        &article.senses,
        &article.entries,
        chosen,
        definition,
        key_file,
    );
    ResultTexts {
        line: None,
        analysis: Analysis::default(),
        elsewhere: None,
        senses_label: SENSES_LABEL,
        hint: hint.or(article.senses.is_empty().then_some(NO_DEFINITIONS)),
        senses,
        translations: Some(Listed::of(
            RUSSIAN_LABEL,
            &article.translations,
            NO_TRANSLATION,
        )),
        word_synonyms: None,
        glosses: None,
        gloss: None,
        pronunciations: pronunciation_texts(&article.entries),
    }
}

/// The parts of what the indexes of `language`, not English, gave, each
/// kind in its own part, a kind with no index or with nothing for the word
/// said by its own line. `choice` is a sense of the definitions or an
/// English gloss, and the online translation `definition` stands under it.
fn grouped_texts<'a>(
    language: Language,
    grouped: &'a Grouped,
    choice: Option<Choice>,
    definition: Option<&'a Online>,
    key_file: Option<&Path>,
) -> ResultTexts<'a> {
    let senses = grouped.definitions.as_deref().unwrap_or_default();
    let chosen = match choice {
        Some(Choice::Sense(index)) => Some(index),
        Some(Choice::Gloss(_)) | None => None,
    };
    let (hint, rows) = sense_texts(senses, &grouped.entries, chosen, definition, key_file);
    let hint = match &grouped.definitions {
        None => Some(missing_definitions(language)),
        Some(senses) if senses.is_empty() => Some(no_definitions(language)),
        Some(_) => hint.filter(|_| !matches!(choice, Some(Choice::Gloss(_)))),
    };
    let translations = match &grouped.russian {
        None => Listed::Missing(missing_russian(language)),
        Some(russian) => Listed::of(RUSSIAN_LABEL, russian, NO_TRANSLATION),
    };
    let in_senses = senses.iter().any(|sense| !sense.synonyms.is_empty());
    let word_synonyms = match &grouped.synonyms {
        // The senses have their own: nothing to say about the word.
        None if in_senses => None,
        None => Some(SynonymTexts::Missing(missing_synonyms(language))),
        Some(synonyms) if synonyms.is_empty() && in_senses => None,
        Some(synonyms) if synonyms.is_empty() => Some(SynonymTexts::Missing(NO_WORD_SYNONYMS)),
        Some(groups) => Some(SynonymTexts::Groups {
            label: word_synonyms_label(language),
            groups,
        }),
    };
    let glosses = match &grouped.english {
        None => Listed::Missing(missing_english(language)),
        Some(english) => Listed::of(ENGLISH_LABEL, english, NO_GLOSS),
    };
    let gloss = match choice {
        Some(Choice::Gloss(index)) => grouped
            .english
            .as_deref()
            .and_then(|english| english.get(index))
            .map(|text| ChosenGloss {
                index,
                definition: definition
                    .filter(|_| !text.trim().is_empty())
                    .map(|online| Block {
                        label: DEFINITION_LABEL,
                        line: online_line(online, key_file),
                    }),
            }),
        Some(Choice::Sense(_)) | None => None,
    };
    ResultTexts {
        line: None,
        analysis: analysis(language, grouped),
        elsewhere: None,
        senses_label: definitions_label(language),
        hint,
        senses: rows,
        translations: Some(translations),
        word_synonyms,
        glosses: Some(glosses),
        gloss,
        pronunciations: pronunciation_texts(&grouped.entries),
    }
}

/// The rows of `senses` with the sense `chosen` explained, and the hint that
/// stands above them while none is chosen; no hint for an empty list. A
/// sense's part of speech comes from `entries` when it has none of its own.
fn sense_texts<'a>(
    senses: &'a [Sense],
    entries: &'a [LexicalEntry],
    chosen: Option<usize>,
    definition: Option<&'a Online>,
    key_file: Option<&Path>,
) -> (Option<&'static str>, Vec<SenseText<'a>>) {
    let chosen = chosen.filter(|index| *index < senses.len());
    let hint = (!senses.is_empty() && chosen.is_none()).then_some(CHOOSE_SENSE);
    let rows = senses
        .iter()
        .enumerate()
        .map(|(index, sense)| SenseText {
            number: index + 1,
            pos: sense_pos(sense, entries),
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
    (hint, rows)
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

/// What stands beside the online translation of the chosen word in its
/// sentence.
const ONLINE_WORD_LABEL: &str = "In this sentence";

/// What stands beside the online translation of a word that has no sentence.
const ONLINE_LONE_LABEL: &str = "Without a sentence";

/// What stands beside the online translation of the sentence itself.
const ONLINE_SENTENCE_LABEL: &str = "Whole sentence";

/// The dim line under the online rows: where they came from.
pub(crate) const ONLINE_SOURCE: &str = "Online · DeepL";

/// The line of an online request that still runs.
const ONLINE_PENDING: &str = "Translating online…";

/// The fallback line for an unclassified failure.
const ONLINE_UNAVAILABLE: &str = "Online translation is not available.";

/// The line of a text that is not sent as a sentence because of its length.
const ONLINE_TOO_LONG: &str = "The sentence is too long to translate.";

/// How the line of an online row reads, so the window styles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnlineState {
    /// The translation as it came.
    Translated,
    /// The request still runs.
    Pending,
    /// No translation: a refusal, or a text too long to send.
    Failed,
}

/// One online text in a row of its own: what it is, its line and how the
/// line reads. The line is text, never markup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlineRow<'a> {
    pub label: &'static str,
    pub line: Cow<'a, str>,
    pub state: OnlineState,
    /// The translation of the word, which stands out from the sentence.
    pub word: bool,
}

/// The online texts as rows: the word in its sentence, the word without a
/// sentence and the sentence, in this order; empty when there is none. A
/// row's line is the pending line, the translation as it came, the line of
/// a refusal or the line of a text that is too long. A refusal about the
/// key names `key_file`, or says how to make it known when there is none.
pub fn online_rows<'a>(texts: &OnlineTexts<'a>, key_file: Option<&Path>) -> Vec<OnlineRow<'a>> {
    let mut rows = Vec::new();
    if let Some(online) = texts.word {
        rows.push(online_row(ONLINE_WORD_LABEL, online, key_file, true));
    }
    if let Some(online) = texts.lone {
        rows.push(online_row(ONLINE_LONE_LABEL, online, key_file, true));
    }
    if let Some(whole) = texts.sentence {
        rows.push(match whole {
            Whole::Asked(online) => online_row(ONLINE_SENTENCE_LABEL, online, key_file, false),
            Whole::TooLong => OnlineRow {
                label: ONLINE_SENTENCE_LABEL,
                line: Cow::Borrowed(ONLINE_TOO_LONG),
                state: OnlineState::Failed,
                word: false,
            },
        });
    }
    rows
}

/// The row of `online` beside `label`.
fn online_row<'a>(
    label: &'static str,
    online: &'a Online,
    key_file: Option<&Path>,
    word: bool,
) -> OnlineRow<'a> {
    let state = match online {
        Online::Pending => OnlineState::Pending,
        Online::Translated(_) => OnlineState::Translated,
        Online::Refused(_) => OnlineState::Failed,
    };
    OnlineRow {
        label,
        line: online_line(online, key_file),
        state,
        word,
    }
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

/// The result the window shows and the sense or gloss chosen in it.
#[derive(Debug, Default)]
pub struct Shown {
    lookup: Option<Lookup>,
    /// The chosen language and the one that gave the shown result, when
    /// the chosen one had no entry for the word.
    elsewhere: Option<(Language, Language)>,
    choice: Option<Choice>,
    definition: Option<(u64, Online)>,
    next_definition: u64,
    /// Counts the results shown, so the window knows a new one from the
    /// same one shown again.
    generation: u64,
}

impl Shown {
    /// Another result, or none, is shown: the sense or gloss chosen in the
    /// one before is forgotten.
    pub fn show(&mut self, lookup: Option<Lookup>) {
        self.lookup = lookup;
        self.generation = self.generation.wrapping_add(1);
        self.elsewhere = None;
        self.choice = None;
        self.definition = None;
    }

    /// As [`Shown::show`], for a result that the language `found` gave for
    /// a word read in `chosen`: when they differ, the chosen one had no
    /// entry for it, and an article says so.
    pub fn show_elsewhere(&mut self, lookup: Option<Lookup>, chosen: Language, found: Language) {
        self.show(lookup);
        self.elsewhere = (chosen != found).then_some((chosen, found));
    }

    /// The chosen language and the one that gave the shown result, when
    /// the chosen one had no entry for the word.
    pub fn elsewhere(&self) -> Option<(Language, Language)> {
        self.elsewhere
    }

    /// Makes `choice` the chosen one; the translation of the definition of
    /// another one is forgotten.
    fn select(&mut self, choice: Choice) {
        if self.choice != Some(choice) {
            self.definition = None;
        }
        self.choice = Some(choice);
    }

    /// Chooses the sense with this index and says whether it is chosen now:
    /// `false`, with nothing changed, unless the shown result is an article
    /// that has the sense. Choosing the chosen sense again is `true`.
    pub fn choose(&mut self, index: usize) -> bool {
        let senses = match &self.lookup {
            Some(Lookup::Article(article)) => &article.senses,
            // The senses of the definitions kind; the glosses are no senses.
            Some(Lookup::Grouped { grouped, .. }) => match &grouped.definitions {
                Some(senses) => senses,
                None => return false,
            },
            _ => return false,
        };
        if index >= senses.len() {
            return false;
        }
        self.select(Choice::Sense(index));
        true
    }

    /// Chooses the English gloss with this index, instead of any sense, and
    /// says whether it is chosen now: `false`, with nothing changed, unless
    /// the shown result is an article of a language beside English that has
    /// the gloss. Choosing the chosen gloss again is `true`.
    pub fn choose_gloss(&mut self, index: usize) -> bool {
        let Some(Lookup::Grouped { grouped, .. }) = &self.lookup else {
            return false;
        };
        if index >= grouped.english.as_ref().map_or(0, Vec::len) {
            return false;
        }
        self.select(Choice::Gloss(index));
        true
    }

    /// Visibility and sensitivity of the explicit definition action.
    pub fn definition_action(&self) -> (bool, bool) {
        let available = self.selected_definition().is_some();
        let enabled = available && matches!(self.definition(), None | Some(Online::Refused(_)));
        (available, enabled)
    }

    /// The definition of the chosen sense or the chosen English gloss, as
    /// the dictionary gives it, with the language it is in, or `None` when
    /// nothing is chosen or the text is blank. A sense of an English article
    /// is OEWN's, so English; a sense of another language's definitions is
    /// in that language; a gloss is English.
    fn selected_definition(&self) -> Option<(&str, Language)> {
        let (text, language) = match (self.lookup.as_ref()?, self.choice?) {
            (Lookup::Article(article), Choice::Sense(index)) => {
                (&article.senses.get(index)?.definition, Language::En)
            }
            (Lookup::Grouped { language, grouped }, Choice::Sense(index)) => (
                &grouped.definitions.as_ref()?.get(index)?.definition,
                *language,
            ),
            (Lookup::Grouped { grouped, .. }, Choice::Gloss(index)) => {
                (grouped.english.as_ref()?.get(index)?, Language::En)
            }
            _ => return None,
        };
        (!text.trim().is_empty()).then_some((text.as_str(), language))
    }

    /// The request for the definition of the chosen sense or gloss, with
    /// the number its answer must carry, when the definition action is
    /// enabled; the definition is then pending. The text is the
    /// dictionary's, unchanged, and is sent without context, from the
    /// language it is in: English for OEWN and for a gloss, the article's
    /// language for its own definitions. Only the definition action calls
    /// this.
    pub fn request_definition(&mut self) -> Option<(u64, Request)> {
        if !self.definition_action().1 {
            return None;
        }
        let (text, source) = self.selected_definition()?;
        let request = Request {
            text: text.to_owned(),
            context: None,
            source,
        };
        self.next_definition += 1;
        let number = self.next_definition;
        self.definition = Some((number, Online::Pending));
        Some((number, request))
    }

    /// Keeps `online` as the translation of the definition when the request
    /// with `number` is the one still pending, and says whether it was kept.
    /// The answer of a request that was replaced, by another sense or gloss,
    /// another result or a retry, is not.
    pub fn answer_definition(&mut self, number: u64, online: Online) -> bool {
        if !matches!(self.definition, Some((current, Online::Pending)) if current == number) {
            return false;
        }
        self.definition = Some((number, online));
        true
    }

    /// Where the translation of the definition of the chosen sense or gloss
    /// stands; `None` until it is asked for.
    pub fn definition(&self) -> Option<&Online> {
        self.definition.as_ref().map(|(_, online)| online)
    }

    /// A number that changes each time another result, or none, is shown,
    /// and not when a sense or gloss is chosen in it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The shown result.
    pub fn lookup(&self) -> Option<&Lookup> {
        self.lookup.as_ref()
    }

    /// The index of the chosen sense.
    #[cfg(test)]
    pub fn sense(&self) -> Option<usize> {
        match self.choice? {
            Choice::Sense(index) => Some(index),
            Choice::Gloss(_) => None,
        }
    }

    /// The index of the chosen English gloss.
    #[cfg(test)]
    pub fn gloss(&self) -> Option<usize> {
        match self.choice? {
            Choice::Gloss(index) => Some(index),
            Choice::Sense(_) => None,
        }
    }

    /// What is chosen in the shown result.
    pub fn choice(&self) -> Option<Choice> {
        self.choice
    }
}

#[cfg(test)]
mod tests {
    use addict_core::{
        dictionary::{Article, Dictionary, Sample, Sense},
        input::{Source, WindowState},
        sentence::words,
        translation::{Refusal, Request, Source as Language},
    };

    use super::{
        AnalysisGroup, AnalysisLine, Binding, CAPTURE_LABEL, CHOOSE_SENSE, ENGLISH_LABEL,
        EXPLANATION_MARK, FIELD_PLACEHOLDER, Listed, Lookup, NO_DEFINITIONS, NO_GLOSS, NO_SYNONYMS,
        NO_TRANSLATION, NO_WORD_SYNONYMS, ONLINE_LONE_LABEL, ONLINE_PENDING, ONLINE_SENTENCE_LABEL,
        ONLINE_SOURCE, ONLINE_TOO_LONG, ONLINE_UNAVAILABLE, ONLINE_WORD_LABEL, Online, OnlineState,
        OnlineTexts, PENDING, RUSSIAN_LABEL, ResultTexts, SYNONYMS_LABEL, Shown, SynonymTexts,
        UNFINISHED, Whole, binding_texts, indexes_text, online_rows, result_texts, sentence_markup,
        texts,
    };
    use addict_platform::{ocr::OcrFailure, screenshot::CaptureFailure};

    use super::{
        Held, TURN_ON_ACCESSIBILITY, command_in, failure_text, install_language, install_tesseract,
        read_failure_text, too_long_text,
    };
    use crate::{
        capture::Failure,
        dictionaries::{
            Indexes, State,
            tests::{FREEDICT, OEWN},
        },
    };
    use addict_dictionary::lookup::{Grouped, VerbForm};
    use addict_platform::{atspi::ContextFailure, foreground::Source as Foregrounded};

    fn definition_shown() -> Shown {
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(Article {
            sense_metadata: Vec::new(),
            entries: Vec::new(),
            senses: vec![
                Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "  land <near> & water  ".to_owned(),
                    synonyms: vec!["shore".to_owned()],
                },
                Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "financial institution".to_owned(),
                    synonyms: vec![],
                },
                Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
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

    /// The labels and lines of the rows of `texts`, with no key file.
    fn online(texts: &OnlineTexts<'_>) -> Vec<(&'static str, String, OnlineState, bool)> {
        online_rows(texts, None)
            .into_iter()
            .map(|row| (row.label, row.line.into_owned(), row.state, row.word))
            .collect()
    }

    #[test]
    fn without_an_online_text_there_is_no_row() {
        assert!(online_rows(&OnlineTexts::default(), None).is_empty());
    }

    #[test]
    fn the_word_and_its_sentence_stand_beside_their_own_labels() {
        let word = translated("берег");
        let sentence = Whole::Asked(translated("Я сидел на берегу реки."));
        assert_eq!(
            online(&OnlineTexts {
                word: Some(&word),
                sentence: Some(&sentence),
                lone: None,
            }),
            [
                (
                    ONLINE_WORD_LABEL,
                    "берег".to_owned(),
                    OnlineState::Translated,
                    true
                ),
                (
                    ONLINE_SENTENCE_LABEL,
                    "Я сидел на берегу реки.".to_owned(),
                    OnlineState::Translated,
                    false
                ),
            ]
        );
    }

    #[test]
    fn a_word_without_a_sentence_has_its_own_label_and_no_sentence_row() {
        let lone = translated("банк");
        assert_eq!(
            online(&OnlineTexts {
                word: None,
                sentence: None,
                lone: Some(&lone),
            }),
            [(
                ONLINE_LONE_LABEL,
                "банк".to_owned(),
                OnlineState::Translated,
                true
            )]
        );
    }

    #[test]
    fn the_sentence_stays_alone_when_the_word_has_no_online_text() {
        let sentence = Whole::Asked(translated("Я сидел."));
        assert_eq!(
            online(&OnlineTexts {
                word: None,
                sentence: Some(&sentence),
                lone: None,
            }),
            [(
                ONLINE_SENTENCE_LABEL,
                "Я сидел.".to_owned(),
                OnlineState::Translated,
                false
            )]
        );
    }

    #[test]
    fn the_online_labels_say_what_each_text_is() {
        assert_eq!(ONLINE_WORD_LABEL, "In this sentence");
        assert_eq!(ONLINE_LONE_LABEL, "Without a sentence");
        assert_eq!(ONLINE_SENTENCE_LABEL, "Whole sentence");
        assert_eq!(ONLINE_SOURCE, "Online · DeepL");
        // Told apart from the dictionary texts too.
        for label in [ONLINE_WORD_LABEL, ONLINE_LONE_LABEL, ONLINE_SENTENCE_LABEL] {
            assert_ne!(label, RUSSIAN_LABEL);
            assert_ne!(label, EXPLANATION_MARK);
            assert_eq!(label.lines().count(), 1);
        }
    }

    #[test]
    fn an_online_translation_is_kept_as_it_came() {
        // The window shows it as text: nothing is escaped or parsed.
        let hostile = translated("<b>берег</b> & банк");
        let whole = Whole::Asked(hostile.clone());
        for row in online_rows(
            &OnlineTexts {
                word: Some(&hostile),
                sentence: Some(&whole),
                lone: None,
            },
            None,
        ) {
            assert_eq!(row.line, "<b>берег</b> & банк");
        }
    }

    #[test]
    fn a_pending_online_request_has_its_own_line_beside_its_label() {
        let pending = Whole::Asked(Online::Pending);
        assert_eq!(
            online(&OnlineTexts {
                word: Some(&Online::Pending),
                sentence: Some(&pending),
                lone: None,
            }),
            [
                (
                    ONLINE_WORD_LABEL,
                    ONLINE_PENDING.to_owned(),
                    OnlineState::Pending,
                    true
                ),
                (
                    ONLINE_SENTENCE_LABEL,
                    ONLINE_PENDING.to_owned(),
                    OnlineState::Pending,
                    false
                ),
            ]
        );
        assert_eq!(
            online(&OnlineTexts {
                word: None,
                sentence: None,
                lone: Some(&Online::Pending),
            }),
            [(
                ONLINE_LONE_LABEL,
                ONLINE_PENDING.to_owned(),
                OnlineState::Pending,
                true
            )]
        );
        assert_ne!(ONLINE_PENDING, PENDING);
        assert_ne!(ONLINE_PENDING, ONLINE_UNAVAILABLE);
    }

    #[test]
    fn every_refusal_has_its_line_beside_its_label() {
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
                online(&OnlineTexts {
                    word: Some(&refused),
                    sentence: Some(&whole),
                    lone: None,
                }),
                [
                    (ONLINE_WORD_LABEL, line.clone(), OnlineState::Failed, true),
                    (
                        ONLINE_SENTENCE_LABEL,
                        line.clone(),
                        OnlineState::Failed,
                        false
                    ),
                ],
                "{refusal:?}"
            );
            assert_eq!(
                online(&OnlineTexts {
                    word: None,
                    sentence: None,
                    lone: Some(&refused),
                }),
                [(ONLINE_LONE_LABEL, line.clone(), OnlineState::Failed, true)],
                "{refusal:?}"
            );
        }
    }

    #[test]
    fn a_text_that_is_too_long_says_so_beside_the_label_of_the_sentence() {
        assert_eq!(ONLINE_TOO_LONG, "The sentence is too long to translate.");
        assert_ne!(ONLINE_TOO_LONG, ONLINE_UNAVAILABLE);
        let word = translated("берег");
        assert_eq!(
            online(&OnlineTexts {
                word: Some(&word),
                sentence: Some(&Whole::TooLong),
                lone: None,
            }),
            [
                (
                    ONLINE_WORD_LABEL,
                    "берег".to_owned(),
                    OnlineState::Translated,
                    true
                ),
                (
                    ONLINE_SENTENCE_LABEL,
                    ONLINE_TOO_LONG.to_owned(),
                    OnlineState::Failed,
                    false
                ),
            ]
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
        let shown = online(&OnlineTexts {
            word: Some(&refused),
            sentence: None,
            lone: None,
        });
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].1.lines().count(), 1, "{shown:?}");
        assert!(!shown[0].1.contains("deepl"), "{shown:?}");
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

    #[test]
    fn a_line_naming_an_install_gives_exactly_that_command() {
        for language in Language::ALL {
            for failure in every_failure() {
                let text = failure_text(&failure, language);
                let expected = match failure {
                    Failure::Ocr(OcrFailure::TesseractMissing) => Some(install_tesseract(language)),
                    Failure::Ocr(OcrFailure::LanguageMissing) => Some(install_language(language)),
                    _ => None,
                };
                assert_eq!(command_in(&text), expected, "{text}");
            }
        }
        assert_eq!(
            command_in(&failure_text(
                &Failure::Ocr(OcrFailure::LanguageMissing),
                Language::Ca
            ))
            .as_deref(),
            Some("sudo dnf install tesseract-langpack-cat")
        );
        assert_eq!(
            command_in(&failure_text(
                &Failure::Ocr(OcrFailure::TesseractMissing),
                Language::En
            ))
            .as_deref(),
            Some("sudo dnf install tesseract tesseract-langpack-eng")
        );
    }

    #[test]
    fn a_read_line_gives_the_accessibility_command_without_the_manual_path() {
        for failure in &every_read_failure() {
            let text = read_failure_text(failure);
            let expected = (*failure == ContextFailure::AccessibilityOff)
                .then(|| TURN_ON_ACCESSIBILITY.to_owned());
            assert_eq!(command_in(&text), expected, "{text}");
        }
        assert_eq!(
            command_in(&read_failure_text(&ContextFailure::AccessibilityOff)).as_deref(),
            Some("gsettings set org.gnome.desktop.interface toolkit-accessibility true")
        );
    }

    #[test]
    fn a_too_long_line_gives_no_command() {
        assert_eq!(
            command_in(&too_long_text(Held::Selection, 1234, 1000)),
            None
        );
        assert_eq!(
            command_in(&too_long_text(Held::Clipboard, 54828, 1000)),
            None
        );
        assert_eq!(command_in(""), None);
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
                "The application in front is not known, so nothing was read. To read Android Studio, start it with the AtkWrapper VM option and screen reader support. Type or paste the sentence.",
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
            sense_metadata: Vec::new(),
            entries: Vec::new(),
            senses: vec![
                Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "first".to_owned(),
                    synonyms: vec!["one".to_owned(), "uno".to_owned()],
                },
                Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
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
            Lookup::NoRussianDictionary(Language::Ca),
        ];
        for lookup in &all {
            match lookup {
                Lookup::Pending
                | Lookup::Article(_)
                | Lookup::Grouped { .. }
                | Lookup::FromRussian { .. }
                | Lookup::NoEntry { .. }
                | Lookup::NoDictionary(_)
                | Lookup::NotImported { .. }
                | Lookup::NoRussianDictionary(_)
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
    fn a_russian_word_shows_its_translations_alone_under_the_language() {
        for (language, label) in [
            (Language::En, "English, for the word as a whole:"),
            (Language::Es, "Spanish, for the word as a whole:"),
            (Language::Ca, "Catalan, for the word as a whole:"),
        ] {
            let lookup = Lookup::FromRussian {
                language,
                translations: vec!["perro".to_owned(), "cachorro".to_owned()],
            };
            let texts = result_texts(&lookup, None, None, None);
            assert_eq!(texts.line, None);
            assert_eq!(texts.hint, None);
            assert!(texts.senses.is_empty());
            assert_eq!(texts.word_synonyms, None);
            assert_eq!(texts.glosses, None);
            assert_eq!(texts.gloss, None);
            assert_eq!(
                details(&lookup, None),
                ["", &format!("{label} perro, cachorro")]
            );
            // No sense to choose, and no line about another language.
            let mut shown = Shown::default();
            shown.show_elsewhere(Some(lookup.clone()), language, language);
            assert!(!shown.choose(0));
            assert_eq!(shown_texts(&shown), texts);
        }
        assert_eq!(
            details(&Lookup::NoRussianDictionary(Language::Ca), None),
            ["No Russian-Catalan dictionary is imported."]
        );
        assert_eq!(
            details(&Lookup::NoRussianDictionary(Language::Es), None),
            ["No Russian-Spanish dictionary is imported."]
        );
        assert_eq!(
            details(&Lookup::NoRussianDictionary(Language::En), None),
            ["No Russian-English dictionary is imported."]
        );
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
            sense_metadata: Vec::new(),
            entries: Vec::new(),
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
            sense_metadata: Vec::new(),
            entries: Vec::new(),
            senses: vec![Sense {
                part_of_speech: None,
                entries: Vec::new(),
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
    fn the_no_dictionary_line_names_the_language() {
        assert_eq!(
            details(&Lookup::NoDictionary(Language::Es), None),
            ["There is no offline dictionary for Spanish."]
        );
        assert_eq!(
            details(&Lookup::NoDictionary(Language::Ca), None),
            ["There is no offline dictionary for Catalan."]
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
                    catalan: State::Missing,
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
            catalan: State::Missing,
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
                    catalan: State::Missing,
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
            catalan: State::Missing,
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
            catalan: State::Missing,
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

    fn spanish_senses() -> Vec<Sense> {
        vec![
            Sense {
                part_of_speech: None,
                entries: Vec::new(),
                definition: "Empresa que ofrece servicios monetarios.".to_owned(),
                synonyms: Vec::new(),
            },
            Sense {
                part_of_speech: None,
                entries: Vec::new(),
                definition: "Mueble alargado para sentarse.".to_owned(),
                synonyms: vec!["banca".to_owned()],
            },
        ]
    }

    /// `banco` with only a definitions index asked, as the tracer's import
    /// gives it.
    fn spanish() -> Lookup {
        Lookup::Grouped {
            language: Language::Es,
            grouped: Grouped {
                definitions: Some(spanish_senses()),
                ..Grouped::default()
            },
        }
    }

    /// `banco` with an index of every kind asked and each having it.
    fn spanish_all() -> Lookup {
        Lookup::Grouped {
            language: Language::Es,
            grouped: Grouped {
                entries: Vec::new(),
                sense_metadata: Vec::new(),
                definitions: Some(spanish_senses()),
                russian: Some(vec!["банк".to_owned(), "скамейка".to_owned()]),
                synonyms: Some(vec![addict_dictionary::index::SynonymGroup {
                    headword: "banco".to_owned(),
                    definition: String::new(),
                    metadata: None,
                    synonyms: vec!["entidad".to_owned(), "asiento".to_owned()],
                }]),
                verb: None,
                english: Some(vec![
                    "bank (financial institution)".to_owned(),
                    "bench".to_owned(),
                ]),
            },
        }
    }

    /// A grouped article read in `language` with `analyses`, each a lemma
    /// and its shapes, and the word `periphrastic` names.
    fn analysed(
        language: Language,
        analyses: &[(&str, &[addict_dictionary::analysis::Shape])],
        periphrastic: Option<&str>,
    ) -> Vec<String> {
        let lookup = Lookup::Grouped {
            language,
            grouped: Grouped {
                definitions: Some(spanish_senses()),
                verb: (!analyses.is_empty()).then(|| {
                    Box::new(VerbForm {
                        analyses: analyses
                            .iter()
                            .map(|(lemma, shapes)| addict_dictionary::analysis::Analyses {
                                lemma: (*lemma).to_owned(),
                                shapes: shapes.to_vec(),
                            })
                            .collect(),
                        periphrastic: periphrastic.map(str::to_owned),
                    })
                }),
                ..Grouped::default()
            },
        };
        let analysis = result_texts(&lookup, None, None, None).analysis;
        analysis
            .lines
            .iter()
            .map(AnalysisLine::text)
            .chain(analysis.note)
            .collect()
    }

    #[test]
    fn a_verb_form_opens_its_article_with_a_line_per_lemma() {
        use addict_dictionary::analysis::{
            Number::{Plural, Singular},
            Person::{self, First, Formal, Second, Third, Vos},
            Shape::{self, Gerund, PastParticiple},
            TenseMood::{self, *},
        };
        let at = |tense_mood: TenseMood, person: Person, number| Shape::Finite {
            tense_mood,
            number,
            person,
        };
        assert_eq!(
            analysed(
                Language::Es,
                &[(
                    "hablar",
                    &[
                        at(PresentSubjunctive, First, Singular),
                        at(PresentSubjunctive, Third, Singular),
                        at(Imperative, Formal, Singular),
                        at(Imperative, Third, Singular),
                    ]
                )],
                None
            ),
            ["hablar — 1st/3rd sg present subjunctive · usted/3rd sg imperative"]
        );
        let preterite = [at(PreteriteIndicative, Third, Singular)];
        assert_eq!(
            analysed(
                Language::Es,
                &[("ir", &preterite), ("ser", &preterite)],
                None
            ),
            [
                "ir — 3rd sg preterite indicative",
                "ser — 3rd sg preterite indicative"
            ]
        );
        assert_eq!(
            analysed(Language::Es, &[("decir", &[PastParticiple])], None),
            ["decir — past participle"]
        );
        assert_eq!(
            analysed(
                Language::Es,
                &[("decir", &[at(Imperative, Second, Singular)])],
                None
            ),
            ["decir — 2nd sg imperative"]
        );
        // A person a pronoun names has no number after it.
        assert_eq!(
            analysed(
                Language::Es,
                &[(
                    "hablar",
                    &[
                        at(PresentIndicative, Vos, Singular),
                        at(Imperative, Formal, Plural),
                        at(Imperative, Third, Plural),
                        Gerund,
                    ]
                )],
                None
            ),
            ["hablar — vos present indicative · ustedes/3rd pl imperative · gerund"]
        );
        // Two numbers in one tense and mood; the Catalan formal person.
        assert_eq!(
            analysed(
                Language::Ca,
                &[(
                    "parlar",
                    &[
                        at(PresentSubjunctive, First, Singular),
                        at(PresentSubjunctive, Second, Plural),
                        at(Imperative, Formal, Singular),
                        at(Imperative, Formal, Plural),
                    ]
                )],
                None
            ),
            ["parlar — 1st sg/2nd pl present subjunctive · vostè/vostès imperative"]
        );
    }

    #[test]
    fn an_analysis_group_has_its_persons_tense_and_mood_apart() {
        use addict_dictionary::analysis::{
            Analyses,
            Number::Singular,
            Person::{Second, Third},
            Shape::{self, Gerund},
            TenseMood::{Imperative, PreteriteIndicative},
        };
        let lookup = Lookup::Grouped {
            language: Language::Es,
            grouped: Grouped {
                definitions: Some(spanish_senses()),
                verb: Some(Box::new(VerbForm {
                    analyses: vec![Analyses {
                        lemma: "decir".to_owned(),
                        shapes: vec![
                            Shape::Finite {
                                tense_mood: PreteriteIndicative,
                                number: Singular,
                                person: Third,
                            },
                            Shape::Finite {
                                tense_mood: Imperative,
                                number: Singular,
                                person: Second,
                            },
                            Gerund,
                        ],
                    }],
                    periphrastic: None,
                })),
                ..Grouped::default()
            },
        };
        let analysis = result_texts(&lookup, None, None, None).analysis;
        assert_eq!(
            analysis.lines,
            [AnalysisLine {
                lemma: "decir".to_owned(),
                groups: vec![
                    AnalysisGroup {
                        persons: Some("3rd sg".to_owned()),
                        tense: Some("preterite"),
                        mood: Some("indicative"),
                    },
                    AnalysisGroup {
                        persons: Some("2nd sg".to_owned()),
                        tense: None,
                        mood: Some("imperative"),
                    },
                    AnalysisGroup {
                        persons: None,
                        tense: Some("gerund"),
                        mood: None,
                    },
                ],
            }]
        );
        assert_eq!(analysis.note, None);
    }

    #[test]
    fn a_catalan_auxiliary_has_its_note_under_its_analysis() {
        use addict_dictionary::analysis::{Number::Singular, Person::Third, Shape, TenseMood};
        let va = [Shape::Finite {
            tense_mood: TenseMood::PresentIndicative,
            number: Singular,
            person: Third,
        }];
        assert_eq!(
            analysed(Language::Ca, &[("anar", &va)], Some("va")),
            [
                "anar — 3rd sg present indicative",
                "Also the auxiliary of the periphrastic past: va + infinitive = past."
            ]
        );
    }

    #[test]
    fn an_article_without_analyses_has_no_analysis_line() {
        assert!(analysed(Language::Es, &[], None).is_empty());
        let english = Lookup::Article(Sample.lookup("bank").expect("bank"));
        assert!(result_texts(&english, None, None, None).analysis.is_empty());
        let none = Lookup::NoEntry {
            word: "xyz".to_owned(),
        };
        assert!(result_texts(&none, None, None, None).analysis.is_empty());
    }

    /// The sections of a result beside its senses, top to bottom: the
    /// Russian, the synonyms of the word and the English glosses, each as
    /// one line, or nothing where there is no section.
    fn sections(texts: &ResultTexts<'_>) -> [Option<String>; 3] {
        [
            texts.translations.map(listed_line),
            texts.word_synonyms.map(|synonyms| synonyms.line()),
            texts.glosses.map(listed_line),
        ]
    }

    /// The texts of what `shown` shows with what is chosen in it.
    fn shown_texts(shown: &Shown) -> ResultTexts<'_> {
        super::choice_texts(
            shown.lookup().expect("a result"),
            shown.choice(),
            shown.definition(),
            None,
            shown.elsewhere(),
        )
    }

    #[test]
    fn an_article_of_another_language_names_the_chosen_one_and_its_own() {
        // Catalan chosen, a Spanish article.
        let mut shown = Shown::default();
        shown.show_elsewhere(Some(spanish_all()), Language::Ca, Language::Es);
        let texts = shown_texts(&shown);
        assert_eq!(
            texts.elsewhere.as_deref(),
            Some("Not in the Catalan dictionaries: the Spanish article.")
        );
        assert_eq!(texts.line, None);
        // Spanish chosen, an English article.
        let english = Lookup::Article(Sample.lookup("bank").expect("bank"));
        shown.show_elsewhere(Some(english.clone()), Language::Es, Language::En);
        assert_eq!(
            shown_texts(&shown).elsewhere.as_deref(),
            Some("Not in the Spanish dictionaries: the English article.")
        );
        // The rest of the article is as it is without the line.
        assert_eq!(
            ResultTexts {
                elsewhere: None,
                ..shown_texts(&shown)
            },
            result_texts(&english, None, None, None)
        );
        // The next result forgets it.
        shown.show(Some(english.clone()));
        assert_eq!(shown.elsewhere(), None);
        assert_eq!(shown_texts(&shown).elsewhere, None);
    }

    #[test]
    fn the_chosen_language_s_own_result_has_no_line_about_another() {
        let mut shown = Shown::default();
        let english = Lookup::Article(Sample.lookup("bank").expect("bank"));
        for (lookup, language) in [(spanish_all(), Language::Es), (english, Language::En)] {
            shown.show_elsewhere(Some(lookup.clone()), language, language);
            assert_eq!(shown.elsewhere(), None);
            let texts = shown_texts(&shown);
            assert_eq!(texts.elsewhere, None);
            assert_eq!(texts, result_texts(&lookup, None, None, None));
        }
        // A result that is no article has its own line only.
        for lookup in [
            Lookup::NoEntry {
                word: "zzz".to_owned(),
            },
            Lookup::Unfinished,
            Lookup::Pending,
        ] {
            shown.show_elsewhere(Some(lookup.clone()), Language::Ca, Language::Es);
            let texts = shown_texts(&shown);
            assert_eq!(texts.elsewhere, None);
            assert_eq!(texts, result_texts(&lookup, None, None, None));
        }
    }

    #[test]
    fn a_definition_of_a_spanish_article_shown_for_catalan_is_sent_from_spanish() {
        let mut shown = Shown::default();
        shown.show_elsewhere(Some(spanish_all()), Language::Ca, Language::Es);
        assert!(shown.choose(0));
        let (_, request) = shown.request_definition().expect("definition");
        assert_eq!(request.source, Language::Es);
        assert_eq!(request.context, None);
        // Its English gloss goes as English.
        assert!(shown.choose_gloss(1));
        let (_, request) = shown.request_definition().expect("gloss");
        assert_eq!(request.source, Language::En);
        assert_eq!(request.text, "bench");
    }

    #[test]
    fn a_spanish_article_of_every_kind_shows_each_kind_labelled() {
        let lookup = spanish_all();
        let texts = result_texts(&lookup, None, None, None);
        assert_eq!(texts.line, None);
        assert_eq!(texts.senses_label, "Spanish definitions");
        assert_eq!(texts.hint, Some(CHOOSE_SENSE));
        assert_eq!(texts.senses.len(), 2);
        assert_eq!(
            sections(&texts),
            [
                Some(format!("{RUSSIAN_LABEL} банк, скамейка")),
                Some("Spanish synonyms of the word entidad, asiento".to_owned()),
                Some(format!(
                    "{ENGLISH_LABEL} bank (financial institution), bench"
                )),
            ]
        );
        assert_eq!(ENGLISH_LABEL, "English glosses");
        // A sense keeps its own synonyms.
        let chosen = result_texts(&lookup, Some(1), None, None);
        let explained = chosen.senses[1].explained.as_ref().expect("explained");
        assert_eq!(
            explained.synonyms,
            Listed::Items {
                label: SYNONYMS_LABEL,
                items: &["banca".to_owned()],
            }
        );
    }

    #[test]
    fn a_catalan_definitions_index_alone_names_every_other_kind_as_missing() {
        let lookup = Lookup::Grouped {
            language: Language::Ca,
            grouped: Grouped {
                definitions: Some(vec![Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "Conjunt de coses.".to_owned(),
                    // No synonym on the sense: the missing index is named.
                    synonyms: Vec::new(),
                }]),
                ..Grouped::default()
            },
        };
        let texts = result_texts(&lookup, None, None, None);
        assert_eq!(texts.senses_label, "Catalan definitions");
        assert_eq!(texts.senses.len(), 1);
        assert_eq!(
            sections(&texts),
            [
                Some("No Catalan-Russian dictionary is imported.".to_owned()),
                Some("No dictionary of Catalan synonyms is imported.".to_owned()),
                Some("No Catalan-English dictionary is imported.".to_owned()),
            ]
        );
    }

    #[test]
    fn a_kind_without_the_word_says_so_and_one_not_imported_says_that() {
        let lookup = Lookup::Grouped {
            language: Language::Es,
            grouped: Grouped {
                entries: Vec::new(),
                sense_metadata: Vec::new(),
                definitions: Some(spanish_senses()),
                russian: Some(Vec::new()),
                synonyms: Some(Vec::new()),
                verb: None,
                english: Some(Vec::new()),
            },
        };
        let texts = result_texts(&lookup, None, None, None);
        assert_eq!(texts.senses.len(), 2);
        assert_eq!(
            sections(&texts),
            [
                Some(NO_TRANSLATION.to_owned()),
                // A sense has synonyms: nothing is said of the word.
                None,
                Some(NO_GLOSS.to_owned()),
            ]
        );
        // Russian alone has the word: the definitions are said missing, by
        // kind and language, and there is no sense to choose.
        let russian = Lookup::Grouped {
            language: Language::Es,
            grouped: Grouped {
                definitions: Some(Vec::new()),
                russian: Some(vec!["стол".to_owned()]),
                ..Grouped::default()
            },
        };
        let texts = result_texts(&russian, None, None, None);
        assert_eq!(
            texts.hint,
            Some("The dictionary has no Spanish definitions of this word.")
        );
        assert!(texts.senses.is_empty());
        let unimported = Lookup::Grouped {
            language: Language::Es,
            grouped: Grouped {
                russian: Some(vec!["стол".to_owned()]),
                ..Grouped::default()
            },
        };
        assert_eq!(
            result_texts(&unimported, None, None, None).hint,
            Some("No dictionary of Spanish definitions is imported.")
        );
        let mut shown = Shown::default();
        shown.show(Some(unimported));
        assert!(!shown.choose(0));
    }

    #[test]
    fn synonym_groups_have_source_headwords_classes_and_descriptions_without_invented_definitions()
    {
        use addict_dictionary::{format::SenseMetadata, index::SynonymGroup};
        let mut group = SynonymGroup {
            headword: "fer".to_owned(),
            definition: String::new(),
            metadata: SenseMetadata::from_label("(v; beure)"),
            synonyms: vec!["prendre".to_owned(); 9],
        };
        assert_eq!(
            super::synonym_heading(&group, 3),
            "fer · Verb · Meaning 3 — beure"
        );
        group.metadata = SenseMetadata::from_label("(v)");
        assert_eq!(super::synonym_heading(&group, 2), "fer · Verb · Meaning 2");
        group.metadata = None;
        assert_eq!(super::synonym_heading(&group, 1), "fer · Meaning 1");
        group.metadata = SenseMetadata::from_label("(unknown)");
        assert_eq!(super::synonym_heading(&group, 4), "fer · Meaning 4");
        let groups = vec![group.clone(), group];
        let lookup = Lookup::Grouped {
            language: Language::Ca,
            grouped: Grouped {
                synonyms: Some(groups.clone()),
                ..Grouped::default()
            },
        };
        let texts = result_texts(&lookup, None, None, None);
        let Some(SynonymTexts::Groups { groups: shown, .. }) = texts.word_synonyms else {
            panic!("groups");
        };
        assert_eq!(shown, groups);
        assert_eq!(shown[0].synonyms.len(), 9);
    }

    #[test]
    fn a_word_with_no_synonyms_anywhere_says_so_for_the_word() {
        let lookup = Lookup::Grouped {
            language: Language::Es,
            grouped: Grouped {
                definitions: Some(vec![Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: "Lo que existe.".to_owned(),
                    synonyms: Vec::new(),
                }]),
                synonyms: Some(Vec::new()),
                ..Grouped::default()
            },
        };
        let texts = result_texts(&lookup, None, None, None);
        assert_eq!(
            texts.word_synonyms,
            Some(SynonymTexts::Missing(NO_WORD_SYNONYMS))
        );
        assert_eq!(
            NO_WORD_SYNONYMS,
            "The dictionaries give no synonyms for this word."
        );
    }

    #[test]
    fn a_spanish_word_in_no_index_or_without_one_has_the_lines_it_had() {
        let no_entry = Lookup::NoEntry {
            word: "zzz".to_owned(),
        };
        let texts = result_texts(&no_entry, None, None, None);
        assert_eq!(
            texts.line.as_deref(),
            Some("The dictionary has no entry for “zzz”.")
        );
        assert_eq!(sections(&texts), [None, None, None]);
        let none = result_texts(&Lookup::NoDictionary(Language::Ca), None, None, None);
        assert_eq!(
            none.line.as_deref(),
            Some("There is no offline dictionary for Catalan.")
        );
        assert_eq!(sections(&none), [None, None, None]);
    }

    #[test]
    fn an_english_article_has_no_section_of_another_language() {
        let lookup = Lookup::Article(article());
        let texts = result_texts(&lookup, Some(0), None, None);
        assert_eq!(texts.word_synonyms, None);
        assert_eq!(texts.glosses, None);
        assert_eq!(texts.senses_label, "English definitions");
        assert_eq!(
            texts.translations.map(listed_line),
            Some(format!("{RUSSIAN_LABEL} раз, два"))
        );
        for lookup in every_lookup() {
            let texts = result_texts(&lookup, None, None, None);
            assert_eq!(texts.word_synonyms, None, "{lookup:?}");
            assert_eq!(texts.glosses, None, "{lookup:?}");
        }
    }

    #[test]
    fn a_spanish_article_is_labelled_by_its_kind_and_language() {
        let lookup = spanish();
        let texts = result_texts(&lookup, None, None, None);
        assert_eq!(texts.senses_label, "Spanish definitions");
        assert_eq!(texts.line, None);
        // The kinds not imported are named, but for the synonyms: a sense
        // has its own, so nothing is said of a synonyms index.
        assert_eq!(
            texts.translations,
            Some(Listed::Missing(
                "No Spanish-Russian dictionary is imported."
            ))
        );
        assert_eq!(texts.word_synonyms, None);
        assert_eq!(
            texts.glosses,
            Some(Listed::Missing(
                "No Spanish-English dictionary is imported."
            ))
        );
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
        // Without senses, the line says so, about Spanish.
        let bare = Lookup::Grouped {
            language: Language::Es,
            grouped: Grouped {
                definitions: Some(Vec::new()),
                ..Grouped::default()
            },
        };
        assert_eq!(
            result_texts(&bare, None, None, None).hint,
            Some("The dictionary has no Spanish definitions of this word.")
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
    fn a_chosen_spanish_or_catalan_definition_is_sent_from_its_language() {
        for language in [Language::Es, Language::Ca] {
            let mut shown = Shown::default();
            shown.show(Some(Lookup::Grouped {
                language,
                grouped: Grouped {
                    definitions: Some(spanish_senses()),
                    ..Grouped::default()
                },
            }));
            // Nothing chosen: no action.
            assert_eq!(shown.definition_action(), (false, false));
            assert_eq!(shown.request_definition(), None);
            assert!(shown.choose(1));
            assert!(!shown.choose(2));
            assert_eq!(shown.sense(), Some(1));
            assert_eq!(shown.definition_action(), (true, true));
            let (number, request) = shown.request_definition().expect("request");
            assert_eq!(
                request,
                Request {
                    text: "Mueble alargado para sentarse.".to_owned(),
                    context: None,
                    source: language,
                }
            );
            assert_eq!(shown.definition_action(), (true, false));
            // A refusal stands under it and the action can be pressed again.
            assert!(shown.answer_definition(number, Online::Refused(Refusal::Network)));
            assert_eq!(shown.definition_action(), (true, true));
            let (retry, request) = shown.request_definition().expect("retry");
            assert_eq!(request.source, language);
            assert!(shown.answer_definition(retry, translated("скамья")));
        }
        // The English article still sends from English.
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Article(article())));
        assert!(shown.choose(0));
        assert_eq!(shown.definition_action(), (true, true));
        let (_, request) = shown.request_definition().expect("request");
        assert_eq!(request.source, Language::En);
    }

    #[test]
    fn a_blank_spanish_definition_or_gloss_offers_no_action() {
        let mut shown = Shown::default();
        shown.show(Some(Lookup::Grouped {
            language: Language::Es,
            grouped: Grouped {
                definitions: Some(vec![Sense {
                    part_of_speech: None,
                    entries: Vec::new(),
                    definition: " \n ".to_owned(),
                    synonyms: Vec::new(),
                }]),
                english: Some(vec!["\t".to_owned()]),
                ..Grouped::default()
            },
        }));
        assert!(shown.choose(0));
        assert_eq!(shown.definition_action(), (false, false));
        assert_eq!(shown.request_definition(), None);
        assert!(shown.choose_gloss(0));
        assert_eq!(shown.definition_action(), (false, false));
        assert_eq!(shown.request_definition(), None);
        // Even with an answer at hand, a blank text has no block.
        let lookup = shown.lookup().expect("article");
        let texts = super::choice_texts(lookup, shown.choice(), Some(&Online::Pending), None, None);
        assert_eq!(texts.gloss.expect("chosen").definition, None);
        let texts = result_texts(lookup, Some(0), Some(&Online::Pending), None);
        assert_eq!(
            texts.senses[0]
                .explained
                .as_ref()
                .expect("explained")
                .definition,
            None
        );
    }

    #[test]
    fn an_english_gloss_is_chosen_alone_and_sent_from_english() {
        for language in [Language::Es, Language::Ca] {
            let mut shown = Shown::default();
            let mut lookup = spanish_all();
            if let Lookup::Grouped {
                language: shown_language,
                ..
            } = &mut lookup
            {
                *shown_language = language;
            }
            shown.show(Some(lookup));
            assert!(!shown.choose_gloss(2));
            assert_eq!(shown.choice(), None);
            assert!(shown.choose(0));
            let (old, _) = shown.request_definition().expect("definition");
            // The gloss takes the place of the sense, and the pending
            // definition is forgotten.
            assert!(shown.choose_gloss(1));
            assert_eq!(shown.sense(), None);
            assert_eq!(shown.gloss(), Some(1));
            assert_eq!(shown.definition(), None);
            assert!(!shown.answer_definition(old, translated("stale")));
            assert_eq!(shown.definition_action(), (true, true));
            let (number, request) = shown.request_definition().expect("gloss");
            assert_eq!(
                request,
                Request {
                    text: "bench".to_owned(),
                    context: None,
                    source: Language::En,
                }
            );
            assert!(shown.answer_definition(number, translated("скамья")));
            let texts = super::choice_texts(
                shown.lookup().expect("article"),
                shown.choice(),
                shown.definition(),
                None,
                None,
            );
            // Under the gloss, not under a sense.
            assert!(texts.senses.iter().all(|sense| sense.explained.is_none()));
            assert_eq!(texts.hint, None);
            assert_eq!(
                texts.gloss,
                Some(super::ChosenGloss {
                    index: 1,
                    definition: Some(super::Block {
                        label: super::DEFINITION_LABEL,
                        line: "скамья".into(),
                    }),
                })
            );
            // The same gloss again keeps its answer; a sense takes its place.
            assert!(shown.choose_gloss(1));
            assert_eq!(shown.definition(), Some(&translated("скамья")));
            assert!(shown.choose(1));
            assert_eq!(shown.gloss(), None);
            assert_eq!(shown.definition(), None);
            // A new result forgets the gloss.
            assert!(shown.choose_gloss(0));
            shown.show(Some(spanish_all()));
            assert_eq!(shown.choice(), None);
        }
    }

    #[test]
    fn a_gloss_is_chosen_only_in_an_article_that_has_it() {
        let mut shown = Shown::default();
        assert!(!shown.choose_gloss(0));
        // No English glosses index, or no gloss of the word.
        shown.show(Some(spanish()));
        assert!(!shown.choose_gloss(0));
        let mut lookup = spanish_all();
        if let Lookup::Grouped { grouped, .. } = &mut lookup {
            grouped.english = Some(Vec::new());
        }
        shown.show(Some(lookup));
        assert!(!shown.choose_gloss(0));
        for lookup in every_lookup() {
            shown.show(Some(lookup.clone()));
            assert!(!shown.choose_gloss(0), "{lookup:?}");
            assert_eq!(shown.choice(), None, "{lookup:?}");
        }
    }

    #[test]
    fn the_online_block_stands_under_a_chosen_spanish_definition() {
        let lookup = spanish_all();
        let online = translated("скамья");
        let texts = result_texts(&lookup, Some(1), Some(&online), None);
        let explained = texts.senses[1].explained.as_ref().expect("explained");
        assert_eq!(
            explained.definition,
            Some(super::Block {
                label: super::DEFINITION_LABEL,
                line: "скамья".into(),
            })
        );
        assert!(texts.senses[0].explained.is_none());
        assert_eq!(texts.gloss, None);
        assert_eq!(
            super::DEFINITION_LABEL,
            "Online, this definition in Russian:"
        );
    }

    #[test]
    fn the_line_of_the_indexes_does_not_change_with_the_spanish_index() {
        for definitions in STATES {
            for translations in STATES {
                let english = Indexes {
                    definitions,
                    translations,
                    spanish: State::Missing,
                    catalan: State::Missing,
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
    fn refusal_lines_are_distinct_and_name_the_key_file() {
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
            let line_shown = super::online_rows(
                &OnlineTexts {
                    lone: Some(&online),
                    ..OnlineTexts::default()
                },
                Some(std::path::Path::new("/config/<key>&/deepl.key")),
            )
            .remove(0)
            .line;
            if matches!(
                refusal,
                Refusal::Key | Refusal::UnreadableKey | Refusal::UnsafeKey | Refusal::RejectedKey
            ) {
                assert!(
                    line_shown.contains("/config/<key>&/deepl.key"),
                    "{line_shown}"
                );
                assert!(
                    line.contains("XDG_CONFIG_HOME") && line.contains("HOME"),
                    "{line}"
                );
            } else {
                assert!(!line_shown.contains("deepl.key"), "{line_shown}");
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

    mod metadata {
        use addict_core::{
            dictionary::{
                Article, Audio, EntryIdentity, LexicalEntry, PartOfSpeech, Pronunciation, Sense,
            },
            translation::Source as Language,
        };
        use addict_dictionary::lookup::Grouped;

        use super::super::{Lookup, PronunciationText, pos_label, result_texts};

        fn identity(headword: &str, id: &str) -> EntryIdentity {
            EntryIdentity {
                source: "test".to_owned(),
                headword: headword.to_owned(),
                id: id.to_owned(),
            }
        }

        fn ipa(text: &str, labels: &[&str]) -> Pronunciation {
            Pronunciation {
                text: Some(text.to_owned()),
                notation: Some("IPA".to_owned()),
                labels: labels.iter().map(|label| (*label).to_owned()).collect(),
                ..Pronunciation::default()
            }
        }

        fn audio(url: &str, format: &str) -> Audio {
            Audio {
                url: url.to_owned(),
                format: Some(format.to_owned()),
            }
        }

        fn sense(pos: Option<&str>, entries: &[EntryIdentity], definition: &str) -> Sense {
            Sense {
                part_of_speech: pos.and_then(PartOfSpeech::from_label),
                entries: entries.to_vec(),
                definition: definition.to_owned(),
                synonyms: Vec::new(),
            }
        }

        fn entry(
            identity: EntryIdentity,
            pos: Option<&str>,
            pronunciations: Vec<Pronunciation>,
        ) -> LexicalEntry {
            LexicalEntry {
                identity,
                part_of_speech: pos.and_then(PartOfSpeech::from_label),
                pronunciations,
            }
        }

        /// `lead` as OEWN gives it: a noun and a verb entry, each with US
        /// and GB IPA, the GB one shared, and no recording.
        fn lead() -> Lookup {
            let (noun, verb) = (identity("lead", "n"), identity("lead", "v"));
            Lookup::Article(Article {
                entries: vec![
                    entry(
                        noun.clone(),
                        Some("n"),
                        vec![ipa("lɛd", &["US"]), ipa("lɛd", &["GB"])],
                    ),
                    entry(
                        verb.clone(),
                        Some("v"),
                        vec![ipa("liːd", &["US"]), ipa("lɛd", &["GB"])],
                    ),
                ],
                sense_metadata: Vec::new(),
                senses: vec![
                    sense(Some("n"), &[noun], "a soft heavy metal"),
                    sense(None, &[verb], "take somebody somewhere"),
                ],
                translations: Vec::new(),
            })
        }

        fn grouped(entries: Vec<LexicalEntry>, senses: Vec<Sense>) -> Lookup {
            Lookup::Grouped {
                language: Language::Es,
                grouped: Grouped {
                    entries,
                    definitions: Some(senses),
                    ..Grouped::default()
                },
            }
        }

        #[test]
        fn every_sense_of_an_english_word_has_its_part_of_speech() {
            let lookup = lead();
            let texts = result_texts(&lookup, None, None, None);
            let pos: Vec<_> = texts.senses.iter().map(|sense| sense.pos).collect();
            assert_eq!(pos, [Some("noun"), Some("verb")]);
            assert_eq!(
                texts.pronunciations,
                [
                    PronunciationText {
                        headword: "lead",
                        transcription: Some("lɛd"),
                        labels: vec!["US"],
                        audio: None,
                    },
                    PronunciationText {
                        headword: "lead",
                        transcription: Some("lɛd"),
                        labels: vec!["GB"],
                        audio: None,
                    },
                    PronunciationText {
                        headword: "lead",
                        transcription: Some("liːd"),
                        labels: vec!["US"],
                        audio: None,
                    },
                ]
            );
            assert_eq!(texts.pronunciations[0].prefix("Lead"), None);
        }

        #[test]
        fn a_sense_takes_the_part_of_speech_its_entries_agree_on() {
            let (a, b) = (identity("x", "a"), identity("x", "b"));
            let lookup = grouped(
                vec![
                    entry(a.clone(), Some("noun"), Vec::new()),
                    entry(b.clone(), Some("verb"), Vec::new()),
                    entry(identity("x", "c"), Some("n"), Vec::new()),
                ],
                vec![
                    sense(None, std::slice::from_ref(&a), "one"),
                    sense(None, &[a.clone(), b], "two"),
                    sense(None, &[a, identity("x", "c")], "three"),
                    sense(None, &[], "four"),
                ],
            );
            let texts = result_texts(&lookup, None, None, None);
            let pos: Vec<_> = texts.senses.iter().map(|sense| sense.pos).collect();
            assert_eq!(pos, [Some("noun"), None, Some("noun"), None]);
        }

        #[test]
        fn a_recording_is_played_ogg_first_and_a_record_may_have_no_text() {
            let gracias = identity("gracias", "1");
            let lookup = grouped(
                vec![entry(
                    gracias.clone(),
                    Some("intj"),
                    vec![
                        Pronunciation {
                            audio: vec![
                                audio("https://example.org/Gracias.mp3", "mp3"),
                                audio("https://example.org/Gracias.ogg", "ogg"),
                            ],
                            ..ipa("/ˈɡɾa.θjas/", &["Spain"])
                        },
                        Pronunciation {
                            labels: vec!["Mexico".to_owned()],
                            audio: vec![audio("https://example.org/Gracias-mx.mp3", "mp3")],
                            ..Pronunciation::default()
                        },
                    ],
                )],
                vec![sense(None, &[gracias], "agradecimiento")],
            );
            let texts = result_texts(&lookup, None, None, None);
            assert_eq!(texts.senses[0].pos, Some("interjection"));
            assert_eq!(
                texts.pronunciations,
                [
                    PronunciationText {
                        headword: "gracias",
                        transcription: Some("/ˈɡɾa.θjas/"),
                        labels: vec!["Spain"],
                        audio: Some("https://example.org/Gracias.ogg"),
                    },
                    PronunciationText {
                        headword: "gracias",
                        transcription: None,
                        labels: vec!["Mexico"],
                        audio: Some("https://example.org/Gracias-mx.mp3"),
                    },
                ]
            );
        }

        #[test]
        fn the_pronunciation_of_a_verb_form_names_its_lemma() {
            let hablar = identity("hablar", "1");
            let lookup = grouped(
                vec![entry(
                    hablar.clone(),
                    Some("verb"),
                    vec![ipa("/aˈblaɾ/", &[])],
                )],
                vec![sense(None, &[hablar], "expresarse con palabras")],
            );
            let texts = result_texts(&lookup, None, None, None);
            assert_eq!(texts.pronunciations[0].prefix("hablo"), Some("hablar"));
            assert_eq!(texts.pronunciations[0].prefix(" Hablar "), None);
            assert!(texts.pronunciations[0].labels.is_empty());
        }

        #[test]
        fn an_article_without_metadata_is_as_before() {
            let lookup = Lookup::Article(super::article());
            let texts = result_texts(&lookup, None, None, None);
            assert!(texts.pronunciations.is_empty());
            assert!(texts.senses.iter().all(|sense| sense.pos.is_none()));
            assert!(
                result_texts(&super::spanish(), None, None, None)
                    .pronunciations
                    .is_empty()
            );
        }

        #[test]
        fn every_variant_is_kept_in_source_order_and_dropped_once_repeated() {
            let parlament = identity("parlament", "1");
            let variants: Vec<_> = (1..=10).map(|n| ipa(&format!("v{n}"), &[])).collect();
            let mut repeated = variants.clone();
            repeated.push(ipa("v3", &[]));
            repeated.push(Pronunciation {
                notation: Some("enPR".to_owned()),
                ..ipa("v3", &[])
            });
            let lookup = grouped(
                vec![entry(parlament.clone(), Some("nom"), repeated)],
                vec![sense(None, &[parlament], "assemblea")],
            );
            let texts = result_texts(&lookup, None, None, None);
            let shown: Vec<_> = texts
                .pronunciations
                .iter()
                .map(|text| (text.transcription.unwrap(), text.labels.clone()))
                .collect();
            let mut expected: Vec<_> = (1..=10)
                .map(|n| (format!("v{n}"), Vec::<&str>::new()))
                .collect();
            expected.push(("v3".to_owned(), vec!["enPR"]));
            assert_eq!(
                shown,
                expected
                    .iter()
                    .map(|(text, labels)| (text.as_str(), labels.clone()))
                    .collect::<Vec<_>>()
            );
        }

        #[test]
        fn a_transcription_written_with_brackets_or_syllable_dots_is_one_row() {
            let (crushing, crush) = (identity("crushing", "1"), identity("crush", "2"));
            let bare = |text: &str| Pronunciation {
                text: Some(text.to_owned()),
                ..Pronunciation::default()
            };
            let lookup = grouped(
                vec![
                    entry(crushing.clone(), None, vec![bare("ˈkʌʃɪŋ")]),
                    entry(crush.clone(), None, vec![bare("kʌʃ")]),
                    entry(
                        crushing.clone(),
                        None,
                        vec![Pronunciation {
                            audio: vec![audio("https://example.org/crushing.ogg", "ogg")],
                            ..ipa("/ˈkʌ.ʃɪŋ/", &[])
                        }],
                    ),
                    entry(
                        crush.clone(),
                        None,
                        vec![ipa("/kʌʃ/", &[]), ipa("[kʌʃ]", &[]), ipa("[kʰʌʃ]", &[])],
                    ),
                ],
                vec![sense(None, &[crushing, crush], "pressing hard")],
            );
            let texts = result_texts(&lookup, None, None, None);
            let rows: Vec<_> = texts
                .pronunciations
                .iter()
                .map(|text| (text.headword, text.transcription, text.audio))
                .collect();
            assert_eq!(
                rows,
                [
                    (
                        "crushing",
                        Some("ˈkʌʃɪŋ"),
                        Some("https://example.org/crushing.ogg")
                    ),
                    ("crush", Some("kʌʃ"), None),
                    ("crush", Some("[kʰʌʃ]"), None),
                ]
            );
        }

        #[test]
        fn the_rows_of_a_headword_stand_together_in_source_order() {
            let (paga, pagar, pago) = (
                identity("paga", "1"),
                identity("pagar", "2"),
                identity("pago", "3"),
            );
            let lookup = grouped(
                vec![
                    entry(paga.clone(), None, vec![ipa("[ˈpaɣa]", &[])]),
                    entry(pagar.clone(), None, vec![ipa("[paˈɣaɾ]", &[])]),
                    entry(pago.clone(), None, vec![ipa("[ˈpaɣo]", &[])]),
                    entry(paga.clone(), None, vec![ipa("/ˈpaga/", &[])]),
                    entry(pagar.clone(), None, vec![ipa("/paˈgar/", &[])]),
                    entry(pago.clone(), None, vec![ipa("/ˈpago/", &[])]),
                ],
                vec![sense(None, &[paga, pagar, pago], "pagar")],
            );
            let texts = result_texts(&lookup, None, None, None);
            let rows: Vec<_> = texts
                .pronunciations
                .iter()
                .map(|text| (text.headword, text.transcription.unwrap()))
                .collect();
            assert_eq!(
                rows,
                [
                    ("paga", "[ˈpaɣa]"),
                    ("paga", "/ˈpaga/"),
                    ("pagar", "[paˈɣaɾ]"),
                    ("pagar", "/paˈgar/"),
                    ("pago", "[ˈpaɣo]"),
                    ("pago", "/ˈpago/"),
                ]
            );
        }

        #[test]
        fn a_transcription_without_labels_gives_way_to_the_same_one_with_labels() {
            let (tancat, tancar) = (identity("tancat", "1"), identity("tancar", "2"));
            let lookup = grouped(
                vec![
                    entry(
                        tancat.clone(),
                        None,
                        vec![
                            Pronunciation {
                                audio: vec![audio("https://example.org/tancat.ogg", "ogg")],
                                ..ipa("/taŋˈkat/", &[])
                            },
                            ipa("/təŋˈkat/", &[]),
                        ],
                    ),
                    entry(
                        tancar.clone(),
                        None,
                        vec![ipa("/taŋˈkar/", &[]), ipa("/taŋˈka/", &[])],
                    ),
                    entry(
                        tancat.clone(),
                        None,
                        vec![
                            ipa("[təŋˈkat]", &["Balearic", "Central"]),
                            ipa("[taŋˈkat]", &["Valencia"]),
                        ],
                    ),
                    entry(
                        tancar.clone(),
                        None,
                        vec![
                            ipa("[taŋˈkaɾ]", &["Valencia"]),
                            ipa("[taŋˈka]", &["Northwestern"]),
                        ],
                    ),
                    // Another headword's labels are not this one's.
                    entry(
                        identity("tanca", "3"),
                        None,
                        vec![ipa("[taŋˈkat]", &["Valencia"])],
                    ),
                ],
                vec![sense(None, &[tancat, tancar], "tancar")],
            );
            let texts = result_texts(&lookup, None, None, None);
            let rows: Vec<_> = texts
                .pronunciations
                .iter()
                .map(|text| {
                    (
                        text.headword,
                        text.transcription.unwrap(),
                        text.labels.clone(),
                        text.audio,
                    )
                })
                .collect();
            assert_eq!(
                rows,
                [
                    ("tancat", "[təŋˈkat]", vec!["Balearic", "Central"], None),
                    (
                        "tancat",
                        "[taŋˈkat]",
                        vec!["Valencia"],
                        Some("https://example.org/tancat.ogg")
                    ),
                    ("tancar", "/taŋˈkar/", vec![], None),
                    ("tancar", "[taŋˈkaɾ]", vec!["Valencia"], None),
                    ("tancar", "[taŋˈka]", vec!["Northwestern"], None),
                    ("tanca", "[taŋˈkat]", vec!["Valencia"], None),
                ]
            );
        }

        #[test]
        fn distinct_recordings_and_headwords_keep_their_rows() {
            let (parlar, parla) = (identity("parlar", "1"), identity("parla", "2"));
            let recording = |url: &str| Pronunciation {
                audio: vec![audio(url, "ogg")],
                ..Pronunciation::default()
            };
            let lookup = grouped(
                vec![
                    entry(
                        parlar.clone(),
                        None,
                        vec![
                            recording("https://example.org/a.ogg"),
                            recording("https://example.org/b.ogg"),
                            recording("https://example.org/a.ogg"),
                            ipa("/parˈla/", &[]),
                        ],
                    ),
                    entry(parla.clone(), None, vec![ipa("/parˈla/", &[])]),
                ],
                vec![sense(None, &[parlar, parla], "dir")],
            );
            let texts = result_texts(&lookup, None, None, None);
            let rows: Vec<_> = texts
                .pronunciations
                .iter()
                .map(|text| (text.prefix("parlar"), text.transcription, text.audio))
                .collect();
            assert_eq!(
                rows,
                [
                    (None, None, Some("https://example.org/a.ogg")),
                    (None, None, Some("https://example.org/b.ogg")),
                    (None, Some("/parˈla/"), None),
                    (Some("parla"), Some("/parˈla/"), None),
                ]
            );
        }

        #[test]
        fn an_unknown_part_of_speech_is_shown_as_written() {
            let pos = PartOfSpeech::from_label(" sustantivo femenino ").unwrap();
            assert_eq!(pos_label(&pos), "sustantivo femenino");
            let known = PartOfSpeech::from_label("adjective_satellite").unwrap();
            assert_eq!(pos_label(&known), "adjective");
            let lookup = grouped(
                Vec::new(),
                vec![sense(Some("sustantivo femenino"), &[], "casa")],
            );
            let texts = result_texts(&lookup, None, None, None);
            assert_eq!(texts.senses[0].pos, Some("sustantivo femenino"));
        }
    }
}
