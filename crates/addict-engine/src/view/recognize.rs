//! Source messages for text recognition.

use std::borrow::Cow;

use addict_core::{
    failure::{Ended, recognize::OcrFailure},
    translation::Source as Language,
};

use super::capture::{NO_SNAPSHOT_FILE, with_retry};

/// The name of `language` before "OCR data" in the line of missing data.
fn ocr_language(language: Language) -> &'static str {
    match language {
        Language::En => "English",
        Language::Es => "Spanish",
        Language::Ca => "Catalan",
        Language::Ru => "Russian",
    }
}

/// The source line when the recognizer gave no text. The platform supplies
/// the matching install hint for an engine or language-data failure.
pub fn recognize_failure_text(
    failure: &Ended<OcrFailure>,
    language: Language,
    install_hint: Option<&str>,
) -> Cow<'static, str> {
    match failure {
        Ended::Shell(OcrFailure::NoImage) => with_retry(NO_SNAPSHOT_FILE),
        Ended::Shell(OcrFailure::EngineMissing) => {
            with_install_hint("Tesseract is not installed.".into(), install_hint)
        }
        Ended::Shell(OcrFailure::LanguageMissing) => with_install_hint(
            format!("{} OCR data is not installed.", ocr_language(language)).into(),
            install_hint,
        ),
        Ended::Shell(OcrFailure::Failed(detail)) => {
            format!("Text recognition failed: {detail}").into()
        }
        Ended::Shell(OcrFailure::Empty) => "No text was recognized.".into(),
        Ended::Shell(OcrFailure::Unavailable) => "Text recognition is not available here.".into(),
        Ended::Shell(OcrFailure::Cancelled) => "Text recognition was cancelled.".into(),
        Ended::DeadlinePassed => "Text recognition took too long.".into(),
        Ended::Unfinished => with_retry("The capture did not finish."),
    }
}

fn with_install_hint(line: Cow<'static, str>, hint: Option<&str>) -> Cow<'static, str> {
    match hint {
        Some(hint) => format!("{line} Install it with: {hint}").into(),
        None => line,
    }
}
