//! Source messages for text recognition.

use std::borrow::Cow;

use addict_core::{
    failure::{Ended, recognize::OcrFailure},
    translation::Source as Language,
};

use super::capture::{NO_SNAPSHOT_FILE, with_retry};

/// The package of the Tesseract data of `language`. No wildcard arm: a new
/// language does not compile until it is given its package.
pub(crate) fn ocr_package(language: Language) -> &'static str {
    match language {
        Language::En => "tesseract-langpack-eng",
        Language::Es => "tesseract-langpack-spa",
        Language::Ca => "tesseract-langpack-cat",
        Language::Ru => "tesseract-langpack-rus",
    }
}

/// The name of `language` before "OCR data" in the line of missing data.
fn ocr_language(language: Language) -> &'static str {
    match language {
        Language::En => "English",
        Language::Es => "Spanish",
        Language::Ca => "Catalan",
        Language::Ru => "Russian",
    }
}

/// The command that installs Tesseract with the data of `language`.
pub fn install_tesseract(language: Language) -> String {
    format!("sudo dnf install tesseract {}", ocr_package(language))
}

/// The command that installs the Tesseract data of `language`.
pub fn install_language(language: Language) -> String {
    format!("sudo dnf install {}", ocr_package(language))
}

/// The source line when the recognizer gave no text.
pub fn recognize_failure_text(
    failure: &Ended<OcrFailure>,
    language: Language,
) -> Cow<'static, str> {
    match failure {
        Ended::Shell(OcrFailure::NoImage) => with_retry(NO_SNAPSHOT_FILE),
        Ended::Shell(OcrFailure::EngineMissing) => format!(
            "Tesseract is not installed. Install it with: {}",
            install_tesseract(language)
        )
        .into(),
        Ended::Shell(OcrFailure::LanguageMissing) => format!(
            "{} OCR data is not installed. Install it with: {}",
            ocr_language(language),
            install_language(language)
        )
        .into(),
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
