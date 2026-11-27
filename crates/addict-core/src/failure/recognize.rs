//! Why a text recognition gave no text.

/// Why a recognition gave no text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrFailure {
    /// No file is at the image path.
    NoImage,
    /// The recognition engine is not installed.
    EngineMissing,
    /// The data of the language is not installed.
    LanguageMissing,
    /// Any other failure: the exit status with the first stderr line, an
    /// output over the recognizer's output limit, or the step that failed
    /// with its error.
    Failed(String),
    /// Tesseract ended well but found no text.
    Empty,
    /// Recognition is not available on this platform.
    Unavailable,
    /// The user dismissed recognition or the shell honored a cancellation.
    Cancelled,
}
