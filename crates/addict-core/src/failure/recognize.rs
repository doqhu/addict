//! Why a text recognition gave no text.

/// Why a recognition gave no text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrFailure {
    /// No file is at the image path.
    NoImage,
    /// `tesseract` is not installed: spawning it found no program.
    TesseractMissing,
    /// The data of the language is not installed.
    LanguageMissing,
    /// Any other failure: the exit status with the first stderr line, an
    /// output over the recognizer's output limit, or the step that failed
    /// with its error.
    Failed(String),
    /// Tesseract ended well but found no text.
    Empty,
    /// The child was still running at the limit; it was killed and reaped.
    TimedOut,
}
