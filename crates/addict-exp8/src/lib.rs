//! EXP-8: OCR quality on screenshots of real materials.
//!
//! A manifest lists English screenshots, each with its SHA-256, its source,
//! the display scale it was taken at, the word the user wants and the
//! sentence it is read in. It is checked whole, images decoded included,
//! before anything is recognized. [`run`] then upscales every screenshot
//! itself to each of [`SCALES`] (Catmull-Rom) and recognizes every variant
//! once per [`PAGE_SEGMENTATIONS`] value through the recognition of the
//! product, `addict_platform::ocr::recognize`, unchanged, one at a time.
//! Every text goes raw to a results file at once, one JSON object per line,
//! beside its failure, its time, the peak memory of the child and an aid to
//! the user's count: whether the word is a token of the text and the
//! word-level edits from the text to the sentence.
//!
//! Nothing is retried, nothing is preprocessed beyond the upscale and no
//! line is joined: the user's hand count of corrections is the verdict.

use std::{
    collections::HashSet,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Write},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use addict_platform::ocr::{self, OcrFailure, Params};
use image::{DynamicImage, ImageFormat, ImageReader, imageops::FilterType};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The limit of each recognition: the app's `OCR_LIMIT`.
pub const LIMIT: Duration = Duration::from_secs(15);

/// The upscale factors of the grid; 1 is the screenshot as it is.
pub const SCALES: [u32; 3] = [1, 2, 3];

/// The `--psm` values of the grid.
pub const PAGE_SEGMENTATIONS: [u8; 3] = [3, 4, 6];

/// The language of every recognition, as the app gives it.
pub const LANGUAGE: &str = "eng";

/// The directory beside the manifest that holds the screenshots.
pub const SCREENSHOTS: &str = "screenshots";

/// The prefix of the temporary directory of the scaled images.
pub const TEMP_PREFIX: &str = "addict-exp8-";

/// The version of the lines of the results file.
const SCHEMA: u32 = 1;

/// Where a screenshot came from.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// The application it was shown in, with its version if known.
    pub application: String,
    /// The material: book, document or page.
    pub material: String,
}

/// One screenshot as the manifest holds it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The file name inside `screenshots/` beside the manifest.
    pub file: String,
    /// The SHA-256 of the file, hexadecimal.
    pub sha256: String,
    pub source: Source,
    /// The display scale the screenshot was taken at, e.g. 1 or 1.25.
    pub display_scale: f64,
    /// The target word. Absent is empty here, so that the refusal names
    /// the entry.
    #[serde(default)]
    pub word: String,
    /// The correct sentence around the word.
    #[serde(default)]
    pub sentence: String,
}

/// A checked screenshot.
#[derive(Debug, Clone, PartialEq)]
pub struct Screenshot {
    pub entry: Entry,
    /// Where the file is.
    pub path: PathBuf,
    /// Its size as decoded.
    pub width: u32,
    pub height: u32,
}

/// A valid manifest, every screenshot checked, and the SHA-256 of the bytes
/// it was read from.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub screenshots: Vec<Screenshot>,
    pub sha256: String,
}

/// Why a manifest is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// The manifest cannot be read.
    Unreadable(io::ErrorKind),
    /// The manifest is not the JSON of a list of entries: where and of what
    /// kind the fault is.
    Malformed(String),
    /// The manifest has no entry.
    Empty,
    /// The entry at `index` (from 1), for `file`, is not usable.
    Entry {
        index: usize,
        file: String,
        fault: Fault,
    },
}

/// What is wrong with an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// The file name is empty or not a plain name inside `screenshots/`.
    BadName,
    /// An earlier entry has this file.
    Repeated,
    NoWord,
    NoSentence,
    WordNotInSentence,
    /// The display scale is not a positive number.
    BadDisplayScale,
    /// The file cannot be read.
    Missing(io::ErrorKind),
    /// The file's SHA-256 is not the manifest's.
    Mismatch {
        actual: String,
    },
    /// The file is not PNG or JPEG.
    NotAnImage,
    /// The file cannot be decoded.
    Undecodable(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(kind) => write!(f, "the manifest cannot be read: {kind}"),
            Self::Malformed(why) => write!(f, "the manifest is not valid JSON of entries: {why}"),
            Self::Empty => f.write_str("the manifest has no entry"),
            Self::Entry { index, file, fault } => {
                write!(f, "entry {index} ({file:?}) of the manifest ")?;
                match fault {
                    Fault::BadName => {
                        write!(f, "has no plain file name inside {SCREENSHOTS}/")
                    }
                    Fault::Repeated => f.write_str("repeats the file of an earlier entry"),
                    Fault::NoWord => f.write_str("has no word"),
                    Fault::NoSentence => f.write_str("has no sentence"),
                    Fault::WordNotInSentence => {
                        f.write_str("has a word that is not a word of its sentence")
                    }
                    Fault::BadDisplayScale => f.write_str("has no positive display scale"),
                    Fault::Missing(kind) => write!(f, "names a file that cannot be read: {kind}"),
                    Fault::Mismatch { actual } => {
                        write!(f, "does not match its file, whose SHA-256 is {actual}")
                    }
                    Fault::NotAnImage => f.write_str("names a file that is not PNG or JPEG"),
                    Fault::Undecodable(why) => {
                        write!(f, "names an image that cannot be decoded: {why}")
                    }
                }
            }
        }
    }
}

/// The SHA-256 of `bytes` in lowercase hexadecimal.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The manifest at `path`; its screenshots are in `screenshots/` beside it.
pub fn load_manifest(path: &Path) -> Result<Manifest, ManifestError> {
    let bytes = fs::read(path).map_err(|error| ManifestError::Unreadable(error.kind()))?;
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join(SCREENSHOTS);
    parse_manifest(&bytes, &directory)
}

/// The manifest of `bytes`, its files in `directory`. Refused when any entry
/// cannot be recognized or judged.
pub fn parse_manifest(bytes: &[u8], directory: &Path) -> Result<Manifest, ManifestError> {
    let entries: Vec<Entry> = serde_json::from_slice(bytes).map_err(|error| malformed(&error))?;
    if entries.is_empty() {
        return Err(ManifestError::Empty);
    }
    let mut seen = HashSet::new();
    let mut screenshots = Vec::with_capacity(entries.len());
    for (at, entry) in entries.into_iter().enumerate() {
        let refused = |fault| ManifestError::Entry {
            index: at + 1,
            file: entry.file.clone(),
            fault,
        };
        if let Some(fault) = fault(&entry) {
            return Err(refused(fault));
        }
        if !seen.insert(entry.file.clone()) {
            return Err(refused(Fault::Repeated));
        }
        let path = directory.join(&entry.file);
        let (width, height) = checked_image(&path, &entry.sha256).map_err(refused)?;
        screenshots.push(Screenshot {
            entry,
            path,
            width,
            height,
        });
    }
    Ok(Manifest {
        screenshots,
        sha256: sha256_hex(bytes),
    })
}

/// The kind and place of a JSON fault; nothing of the content is quoted.
fn malformed(error: &serde_json::Error) -> ManifestError {
    use serde_json::error::Category;
    let kind = match error.classify() {
        Category::Io => "unreadable",
        Category::Syntax => "a syntax error",
        Category::Data => "an unexpected value or field",
        Category::Eof => "an early end",
    };
    ManifestError::Malformed(format!(
        "{kind} at line {}, column {}",
        error.line(),
        error.column()
    ))
}

/// The first thing wrong with `entry` that its file is not needed for.
fn fault(entry: &Entry) -> Option<Fault> {
    let plain = !entry.file.is_empty()
        && entry.file != "."
        && entry.file != ".."
        && !entry.file.contains('/')
        && !entry.file.contains('\0');
    if !plain {
        return Some(Fault::BadName);
    }
    if bare(&entry.word).is_empty() {
        return Some(Fault::NoWord);
    }
    if entry.sentence.trim().is_empty() {
        return Some(Fault::NoSentence);
    }
    if !word_found(&entry.sentence, &entry.word) {
        return Some(Fault::WordNotInSentence);
    }
    if !(entry.display_scale.is_finite() && entry.display_scale > 0.0) {
        return Some(Fault::BadDisplayScale);
    }
    None
}

/// The size of the image at `path` once its SHA-256 is `expected` and it
/// decodes as PNG or JPEG.
fn checked_image(path: &Path, expected: &str) -> Result<(u32, u32), Fault> {
    let bytes = fs::read(path).map_err(|error| Fault::Missing(error.kind()))?;
    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(expected.trim()) {
        return Err(Fault::Mismatch { actual });
    }
    let image = decode(&bytes)?;
    Ok((image.width(), image.height()))
}

/// The image of `bytes`, PNG or JPEG only.
fn decode(bytes: &[u8]) -> Result<DynamicImage, Fault> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| Fault::Undecodable(error.to_string()))?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err(Fault::NotAnImage);
    }
    reader
        .decode()
        .map_err(|error| Fault::Undecodable(error.to_string()))
}

/// The size of a `width`×`height` image upscaled `scale` times, if it fits.
pub fn scaled_size(width: u32, height: u32, scale: u32) -> Option<(u32, u32)> {
    Some((width.checked_mul(scale)?, height.checked_mul(scale)?))
}

/// The tokens of `text`: split on white space, line breaks included.
fn tokens(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

/// Whether `word` is a token of `text` once the leading and trailing
/// punctuation of both is stripped; case matters.
pub fn word_found(text: &str, word: &str) -> bool {
    let word = bare(word);
    !word.is_empty() && text.split_whitespace().any(|token| bare(token) == word)
}

/// `token` without its leading and trailing punctuation and white space.
fn bare(token: &str) -> &str {
    token.trim_matches(|c: char| !c.is_alphanumeric())
}

/// The word-level edits from `text` to `sentence`: a semi-global Levenshtein
/// distance in which the sentence may align with any span of the text, the
/// tokens before and after that span being free. Tokens compare exactly.
pub fn sentence_edits(text: &str, sentence: &str) -> usize {
    let text = tokens(text);
    let sentence = tokens(sentence);
    // `row[j]`: the edits of the sentence so far against a span of the text
    // ending before token `j`. The first row is free: the span may begin
    // anywhere.
    let mut row = vec![0_usize; text.len() + 1];
    for (i, wanted) in sentence.iter().enumerate() {
        let mut next = vec![i + 1; text.len() + 1];
        for (j, got) in text.iter().enumerate() {
            let substitution = row[j] + usize::from(wanted != got);
            next[j + 1] = substitution.min(row[j + 1] + 1).min(next[j] + 1);
        }
        row = next;
    }
    // The span may end anywhere.
    row.into_iter().min().unwrap_or(0)
}

/// The name of a failure in the results file.
pub fn failure_name(failure: &OcrFailure) -> &'static str {
    match failure {
        OcrFailure::NoImage => "no_image",
        OcrFailure::TesseractMissing => "tesseract_missing",
        OcrFailure::LanguageMissing => "language_missing",
        OcrFailure::Failed(_) => "failed",
        OcrFailure::Empty => "empty",
        OcrFailure::TimedOut => "timed_out",
    }
}

/// One screenshot of the run line.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScreenshotRecord {
    pub file: String,
    pub sha256: String,
    pub source: Source,
    pub display_scale: f64,
    pub width: u32,
    pub height: u32,
    pub word: String,
    pub sentence: String,
}

/// One line of the results file.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// The first line: what was run.
    Run {
        schema: u32,
        /// When the run began, UTC.
        date: String,
        runner: &'static str,
        /// The first line of `tesseract --version`, if it ran.
        tesseract: Option<String>,
        /// `TESSDATA_PREFIX` of the run, if set.
        tessdata_prefix: Option<String>,
        manifest_sha256: String,
        language: &'static str,
        scales: Vec<u32>,
        psms: Vec<u8>,
        limit_ms: u64,
        screenshots: Vec<ScreenshotRecord>,
    },
    /// One recognition of one variant.
    Recognition {
        file: String,
        scale: u32,
        /// The size of the image recognized, after the upscale.
        width: u32,
        height: u32,
        psm: u8,
        /// The text as the recognition gave it, unchanged.
        text: Option<String>,
        failure: Option<&'static str>,
        /// The message of a `failed` recognition.
        failure_detail: Option<String>,
        elapsed_ms: u64,
        peak_rss_bytes: Option<u64>,
        word_found: bool,
        sentence_edits: Option<usize>,
    },
}

impl Record {
    /// The line of `recognition` of `screenshot` at `scale` and `psm`, on an
    /// image of `width`×`height`.
    pub fn recognition(
        entry: &Entry,
        (scale, width, height, psm): (u32, u32, u32, u8),
        recognition: &ocr::Recognition,
    ) -> Self {
        let text = recognition.result.as_ref().ok();
        let failure = recognition.result.as_ref().err();
        Self::Recognition {
            file: entry.file.clone(),
            scale,
            width,
            height,
            psm,
            text: text.cloned(),
            failure: failure.map(failure_name),
            failure_detail: match failure {
                Some(OcrFailure::Failed(detail)) => Some(detail.clone()),
                _ => None,
            },
            elapsed_ms: millis(recognition.elapsed),
            peak_rss_bytes: recognition.peak_rss_bytes,
            word_found: text.is_some_and(|text| word_found(text, &entry.word)),
            sentence_edits: text.map(|text| sentence_edits(text, &entry.sentence)),
        }
    }
}

/// What a run that reached its end did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub recognitions: usize,
    /// Recognitions that gave no text.
    pub failures: usize,
}

/// Why a run did not reach its end.
#[derive(Debug)]
pub enum Failure {
    /// The results file is there already; nothing was recognized.
    Exists(PathBuf),
    /// The results file cannot be created or begun; nothing was recognized.
    Results(io::Error),
    /// The directory of the scaled images cannot be made; nothing was
    /// recognized.
    Scratch(io::Error),
    /// A line could not be written after the recognitions began.
    Interrupted(io::Error),
    /// A scaled image of `file` could not be made or written.
    Scale { file: String, why: String },
    /// A recognition found no Tesseract or no language data; its line is
    /// written.
    Stopped { file: String, failure: OcrFailure },
}

impl Failure {
    /// Whether nothing was recognized.
    pub fn before_any_recognition(&self) -> bool {
        matches!(self, Self::Exists(_) | Self::Results(_) | Self::Scratch(_))
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exists(path) => write!(
                f,
                "the results file {} exists already and is not overwritten; nothing was recognized",
                path.display()
            ),
            Self::Results(error) => write!(
                f,
                "the results file cannot be created or begun: {error}; nothing was recognized"
            ),
            Self::Scratch(error) => write!(
                f,
                "the directory of the scaled images cannot be made: {error}; nothing was recognized"
            ),
            Self::Interrupted(error) => write!(
                f,
                "the results file could not be written during the run: {error}; the run stopped"
            ),
            Self::Scale { file, why } => {
                write!(f, "{file} could not be upscaled: {why}; the run stopped")
            }
            Self::Stopped { file, failure } => {
                let what = match failure {
                    OcrFailure::TesseractMissing => "tesseract is not installed",
                    OcrFailure::LanguageMissing => "the eng language data is not installed",
                    _ => "the recognition cannot go on",
                };
                write!(
                    f,
                    "{what} (at {file}, {}); its line is written; the run stopped",
                    failure_name(failure)
                )
            }
        }
    }
}

/// Runs the grid on every screenshot of `manifest`, writing each line to a
/// new file at `results` as it comes. The scaled images are made in the
/// system temporary directory.
pub async fn run(manifest: &Manifest, results: &Path, limit: Duration) -> Result<Summary, Failure> {
    run_in(manifest, results, limit, &std::env::temp_dir()).await
}

/// [`run`] with the scaled images in a fresh directory under `temp_root`,
/// removed whatever happens.
pub async fn run_in(
    manifest: &Manifest,
    results: &Path,
    limit: Duration,
    temp_root: &Path,
) -> Result<Summary, Failure> {
    let scratch = TempDir::create(temp_root).map_err(Failure::Scratch)?;
    let mut file = create(results)?;
    let run = Record::Run {
        schema: SCHEMA,
        date: utc(SystemTime::now()),
        runner: concat!("addict-exp8/", env!("CARGO_PKG_VERSION")),
        tesseract: tesseract_version(),
        tessdata_prefix: std::env::var_os("TESSDATA_PREFIX")
            .map(|prefix| prefix.to_string_lossy().into_owned()),
        manifest_sha256: manifest.sha256.clone(),
        language: LANGUAGE,
        scales: SCALES.to_vec(),
        psms: PAGE_SEGMENTATIONS.to_vec(),
        limit_ms: millis(limit),
        screenshots: manifest
            .screenshots
            .iter()
            .map(|shot| ScreenshotRecord {
                file: shot.entry.file.clone(),
                sha256: shot.entry.sha256.trim().to_ascii_lowercase(),
                source: shot.entry.source.clone(),
                display_scale: shot.entry.display_scale,
                width: shot.width,
                height: shot.height,
                word: shot.entry.word.clone(),
                sentence: shot.entry.sentence.clone(),
            })
            .collect(),
    };
    write(&mut file, &run).map_err(Failure::Results)?;

    let mut summary = Summary {
        recognitions: 0,
        failures: 0,
    };
    for (index, shot) in manifest.screenshots.iter().enumerate() {
        let source = fs::read(&shot.path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| decode(&bytes).map_err(|fault| format!("{fault:?}")))
            .map_err(|why| Failure::Scale {
                file: shot.entry.file.clone(),
                why,
            })?;
        for scale in SCALES {
            let (image, width, height) = if scale == 1 {
                (shot.path.clone(), source.width(), source.height())
            } else {
                scaled(&source, scale, &scratch.0, index).map_err(|why| Failure::Scale {
                    file: shot.entry.file.clone(),
                    why,
                })?
            };
            for psm in PAGE_SEGMENTATIONS {
                let params = Params {
                    language: LANGUAGE,
                    page_segmentation: psm,
                };
                let recognition = ocr::recognize(&image, &params, limit).await;
                let record =
                    Record::recognition(&shot.entry, (scale, width, height, psm), &recognition);
                write(&mut file, &record).map_err(Failure::Interrupted)?;
                summary.recognitions += 1;
                match recognition.result {
                    Ok(_) => {}
                    Err(failure @ (OcrFailure::TesseractMissing | OcrFailure::LanguageMissing)) => {
                        return Err(Failure::Stopped {
                            file: shot.entry.file.clone(),
                            failure,
                        });
                    }
                    Err(_) => summary.failures += 1,
                }
            }
            if scale != 1 {
                // Not needed any more; the guard removes the rest.
                let _ = fs::remove_file(&image);
            }
        }
    }
    Ok(summary)
}

/// Upscales `source` `scale` times with Catmull-Rom and writes it as PNG in
/// `directory`; gives its path and size.
fn scaled(
    source: &DynamicImage,
    scale: u32,
    directory: &Path,
    index: usize,
) -> Result<(PathBuf, u32, u32), String> {
    let (width, height) = scaled_size(source.width(), source.height(), scale)
        .ok_or_else(|| format!("{scale}× is too large"))?;
    let image = source.resize_exact(width, height, FilterType::CatmullRom);
    let path = directory.join(format!("{index}-x{scale}.png"));
    image
        .save_with_format(&path, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok((path, width, height))
}

/// The first non-empty line of `tesseract --version`, if it runs.
fn tesseract_version() -> Option<String> {
    let output = Command::new("tesseract").arg("--version").output().ok()?;
    let first = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
    };
    first(&output.stdout).or_else(|| first(&output.stderr))
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// A new file at `path`, in a directory made for it if there is none. A file
/// that is there already is left as it is.
fn create(path: &Path) -> Result<File, Failure> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(Failure::Results)?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => Failure::Exists(path.to_owned()),
            _ => Failure::Results(error),
        })
}

/// Writes `record` as one line, at once.
fn write(file: &mut File, record: &Record) -> io::Result<()> {
    let mut line = serde_json::to_string(record).map_err(io::Error::other)?;
    line.push('\n');
    file.write_all(line.as_bytes())?;
    file.flush()
}

/// A fresh `0700` directory, removed with all it holds on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn create(root: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let path = root.join(format!(
            "{TEMP_PREFIX}{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `time` as `2026-10-13T18:22:05Z`.
fn utc(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // The civil date of a day number, after Howard Hinnant's `civil_from_days`.
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_shifted + 2) / 5 + 1;
    let month = if month_shifted < 10 {
        month_shifted + 3
    } else {
        month_shifted - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{Duration, UNIX_EPOCH},
    };

    use addict_platform::ocr::{OcrFailure, Recognition};
    use image::{ImageFormat, RgbImage};
    use serde_json::json;

    use super::{
        Entry, Fault, ManifestError, Record, Source, parse_manifest, scaled_size, sentence_edits,
        sha256_hex, utc, word_found,
    };

    /// A fresh directory for one test, removed at its end.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("addict-exp8-unit-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).expect("scratch");
            Self(path)
        }

        /// Writes `bytes` as `name`; gives their SHA-256.
        fn put(&self, name: &str, bytes: &[u8]) -> String {
            fs::write(self.0.join(name), bytes).expect("file");
            sha256_hex(bytes)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        RgbImage::from_pixel(width, height, image::Rgb([255, 255, 255]))
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("png");
        bytes
    }

    fn entry(file: &str, sha256: &str) -> serde_json::Value {
        json!({
            "file": file,
            "sha256": sha256,
            "source": {"application": "Foliate 3.3.0", "material": "a book"},
            "display_scale": 1.0,
            "word": "bank",
            "sentence": "The bank is beside the river."
        })
    }

    fn parsed(scratch: &Scratch, entries: &serde_json::Value) -> Result<(), ManifestError> {
        let bytes = serde_json::to_vec(entries).expect("json");
        parse_manifest(&bytes, &scratch.0).map(|_| ())
    }

    fn fault_of(result: Result<(), ManifestError>) -> (usize, Fault) {
        match result {
            Err(ManifestError::Entry { index, fault, .. }) => (index, fault),
            other => panic!("not an entry refusal: {other:?}"),
        }
    }

    #[test]
    fn a_valid_manifest_gives_its_screenshots_and_hash() {
        let scratch = Scratch::new("valid");
        let sha = scratch.put("a.png", &png(7, 5));
        let bytes =
            serde_json::to_vec(&json!([entry("a.png", &sha.to_uppercase())])).expect("json");
        let manifest = parse_manifest(&bytes, &scratch.0).expect("manifest");
        assert_eq!(manifest.sha256, sha256_hex(&bytes));
        let shot = &manifest.screenshots[0];
        assert_eq!((shot.width, shot.height), (7, 5));
        assert_eq!(shot.path, scratch.0.join("a.png"));
    }

    #[test]
    fn invalid_json_and_an_empty_manifest_are_refused() {
        let scratch = Scratch::new("json");
        assert!(matches!(
            parse_manifest(b"[{", &scratch.0),
            Err(ManifestError::Malformed(_))
        ));
        assert!(matches!(
            parse_manifest(br#"{"file":"a.png"}"#, &scratch.0),
            Err(ManifestError::Malformed(_))
        ));
        let mut unknown = entry("a.png", "00");
        unknown["extra"] = json!(1);
        assert!(matches!(
            parsed(&scratch, &json!([unknown])),
            Err(ManifestError::Malformed(_))
        ));
        assert_eq!(parse_manifest(b"[]", &scratch.0), Err(ManifestError::Empty));
    }

    #[test]
    fn every_entry_fault_is_refused_and_names_the_entry() {
        let scratch = Scratch::new("faults");
        let sha = scratch.put("a.png", &png(4, 4));
        scratch.put("b.png", &png(4, 4));
        let good = entry("a.png", &sha);
        let with = |key: &str, value: serde_json::Value| {
            let mut changed = entry("b.png", &sha);
            changed[key] = value;
            json!([good.clone(), changed])
        };
        let cases = [
            (with("file", json!("")), Fault::BadName),
            (with("file", json!("../a.png")), Fault::BadName),
            (with("file", json!("..")), Fault::BadName),
            (json!([good.clone(), good.clone()]), Fault::Repeated),
            (with("word", json!(" ")), Fault::NoWord),
            (with("word", json!(" ' ")), Fault::NoWord),
            (with("sentence", json!("")), Fault::NoSentence),
            (with("word", json!("rivers")), Fault::WordNotInSentence),
            (with("word", json!("Bank")), Fault::WordNotInSentence),
            (with("word", json!("ban")), Fault::WordNotInSentence),
            (with("display_scale", json!(0)), Fault::BadDisplayScale),
            (
                with("file", json!("absent.png")),
                Fault::Missing(std::io::ErrorKind::NotFound),
            ),
            (
                with("sha256", json!("00")),
                Fault::Mismatch {
                    actual: sha.clone(),
                },
            ),
        ];
        for (manifest, expected) in cases {
            let (index, fault) = fault_of(parsed(&scratch, &manifest));
            assert_eq!((index, &fault), (2, &expected), "{manifest}");
        }
        let mut no_word = good.clone();
        no_word.as_object_mut().expect("object").remove("word");
        assert_eq!(
            fault_of(parsed(&scratch, &json!([no_word]))).1,
            Fault::NoWord
        );
        let message = parsed(&scratch, &with("word", json!(" ")))
            .expect_err("refused")
            .to_string();
        assert!(message.contains("entry 2 (\"b.png\")"), "{message}");
    }

    #[test]
    fn a_file_that_is_not_png_or_jpeg_or_does_not_decode_is_refused() {
        let scratch = Scratch::new("images");
        let text = scratch.put("a.png", b"not an image at all");
        assert_eq!(
            fault_of(parsed(&scratch, &json!([entry("a.png", &text)]))),
            (1, Fault::NotAnImage)
        );
        let mut truncated = png(20, 20);
        truncated.truncate(30);
        let truncated = scratch.put("b.png", &truncated);
        assert!(matches!(
            fault_of(parsed(&scratch, &json!([entry("b.png", &truncated)]))),
            (1, Fault::Undecodable(_))
        ));
        // A format the runner does not take, though `image` knows it.
        let bmp = scratch.put("c.bmp", b"BM\0\0\0\0\0\0\0\0\0\0\0\0");
        assert_eq!(
            fault_of(parsed(&scratch, &json!([entry("c.bmp", &bmp)]))),
            (1, Fault::NotAnImage)
        );
    }

    #[test]
    fn scaled_sizes_multiply_both_sides() {
        assert_eq!(scaled_size(640, 400, 1), Some((640, 400)));
        assert_eq!(scaled_size(640, 400, 2), Some((1280, 800)));
        assert_eq!(scaled_size(641, 401, 3), Some((1923, 1203)));
        assert_eq!(scaled_size(u32::MAX, 1, 2), None);
    }

    const SENTENCE: &str = "The bank is beside the river.";

    #[test]
    fn the_aid_on_an_exact_text() {
        assert!(word_found(SENTENCE, "bank"));
        assert_eq!(sentence_edits(SENTENCE, SENTENCE), 0);
    }

    #[test]
    fn the_aid_on_one_substitution() {
        let text = "The bank is beside\nthe rivor.";
        assert!(word_found(text, "bank"));
        assert_eq!(sentence_edits(text, SENTENCE), 1);
        assert_eq!(sentence_edits("The bonk is beside the river.", SENTENCE), 1);
        assert!(!word_found("The bonk is beside the river.", "bank"));
        // Case and punctuation count.
        assert_eq!(sentence_edits("the bank is beside the river", SENTENCE), 2);
    }

    #[test]
    fn the_aid_with_extra_lines_around() {
        let text = "Chapter 2\nIt was late.\nThe bank is beside the river. It\nwas quiet there.";
        assert!(word_found(text, "bank"));
        assert_eq!(sentence_edits(text, SENTENCE), 0);
        // A missing and an extra token inside the span.
        let text = "Header\nThe bank is the river. x\nFooter";
        assert_eq!(sentence_edits(text, SENTENCE), 1);
        let text = "Header\nThe bank is | beside the river.\nFooter";
        assert_eq!(sentence_edits(text, SENTENCE), 1);
    }

    #[test]
    fn the_aid_on_a_sentence_split_across_lines() {
        let text = "The bank is\nbeside the\n\nriver.";
        assert_eq!(sentence_edits(text, SENTENCE), 0);
        // A hyphenated break counts as edits; story 4 decides on it.
        let text = "The bank is be-\nside the river.";
        assert_eq!(sentence_edits(text, SENTENCE), 2);
        assert!(word_found("(bank),", "bank"));
        assert!(word_found("“bank”", "bank"));
        // The word's own edge punctuation is ignored too.
        assert!(word_found("The students' books.", "students'"));
        assert!(word_found("Tell 'em now.", "'em"));
        assert!(!word_found("Tell 'em now.", "'"));
    }

    #[test]
    fn the_aid_on_no_text() {
        assert!(!word_found("", "bank"));
        assert_eq!(sentence_edits("", SENTENCE), 6);
        let entry = Entry {
            file: "a.png".to_owned(),
            sha256: "00".to_owned(),
            source: Source {
                application: "app".to_owned(),
                material: "doc".to_owned(),
            },
            display_scale: 1.0,
            word: "bank".to_owned(),
            sentence: SENTENCE.to_owned(),
        };
        let recognition = Recognition {
            result: Err(OcrFailure::Empty),
            elapsed: Duration::from_millis(12),
            peak_rss_bytes: Some(4096),
        };
        let line = serde_json::to_value(Record::recognition(&entry, (2, 10, 20, 4), &recognition))
            .expect("json");
        assert_eq!(line["word_found"], json!(false));
        assert_eq!(line["sentence_edits"], json!(null));
        assert_eq!(line["text"], json!(null));
        assert_eq!(line["failure"], json!("empty"));
    }

    #[test]
    fn lines_serialize_with_every_field() {
        let entry = Entry {
            file: "a.png".to_owned(),
            sha256: "00".to_owned(),
            source: Source {
                application: "app".to_owned(),
                material: "doc".to_owned(),
            },
            display_scale: 1.25,
            word: "bank".to_owned(),
            sentence: SENTENCE.to_owned(),
        };
        let recognition = Recognition {
            result: Ok("The bank is beside\nthe rivor.".to_owned()),
            elapsed: Duration::from_micros(812_900),
            peak_rss_bytes: Some(1 << 20),
        };
        let line = serde_json::to_value(Record::recognition(&entry, (3, 30, 60, 6), &recognition))
            .expect("json");
        assert_eq!(
            line,
            json!({
                "kind": "recognition",
                "file": "a.png",
                "scale": 3,
                "width": 30,
                "height": 60,
                "psm": 6,
                "text": "The bank is beside\nthe rivor.",
                "failure": null,
                "failure_detail": null,
                "elapsed_ms": 812,
                "peak_rss_bytes": 1_048_576,
                "word_found": true,
                "sentence_edits": 1
            })
        );
        let failed = Recognition {
            result: Err(OcrFailure::Failed("tesseract exit status 1".to_owned())),
            elapsed: Duration::ZERO,
            peak_rss_bytes: None,
        };
        let line =
            serde_json::to_value(Record::recognition(&entry, (1, 1, 1, 3), &failed)).expect("json");
        assert_eq!(line["failure"], json!("failed"));
        assert_eq!(line["failure_detail"], json!("tesseract exit status 1"));
    }

    #[test]
    fn utc_writes_the_civil_date() {
        assert_eq!(utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            utc(UNIX_EPOCH + Duration::from_secs(1_791_916_925)),
            "2026-10-13T18:42:05Z"
        );
    }
}
