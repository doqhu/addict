//! EXP-5: the check set of DeepL for EN→RU.
//!
//! A set of words, each in a sentence and with the Russian forms accepted for
//! it, is read from a file and checked before anything is asked. [`run`] then
//! asks DeepL through the client of the product: the languages, the usage,
//! each word alone and then with its sentence as context, and the usage
//! again. Every answer goes to a results file at once, one JSON object per
//! line, as the client gave it and beside its verdict.
//!
//! The requests go one at a time, each under the limit the caller gives.
//! Nothing is asked twice and nothing is kept between requests. The key is
//! never read here: the client reads it, and no record, message or error of
//! this crate can hold it.

use std::{
    fmt,
    fs::{File, OpenOptions},
    future::Future,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use addict_core::translation::{Refusal, Request};
use addict_deepl::{DeepL, Language, Usage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The limit of each request of the experiment.
pub const LIMIT: Duration = Duration::from_secs(10);

/// The version of the lines of the results file.
const SCHEMA: u32 = 1;

/// The codes of the two languages, as `/v3/languages` writes them.
const SOURCE: &str = "en";
const TARGET: &str = "ru";

/// One item of the set: a word, the sentence it is read in and the Russian
/// forms that are its marked meaning there.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: u32,
    pub word: String,
    /// Absent in the file is empty here, so that the refusal names the item.
    #[serde(default)]
    pub sentence: String,
    #[serde(default)]
    pub accepted: Vec<String>,
}

/// The check set as its file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Set {
    pub items: Vec<Item>,
}

/// A valid set and the SHA-256 of the bytes it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub set: Set,
    pub sha256: String,
}

/// Why a file gave no set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetError {
    /// The file cannot be read.
    Unreadable(io::ErrorKind),
    /// The file is not the JSON of a set. It says where and of what kind
    /// the fault is, and holds nothing of the file's content.
    Malformed(String),
    /// The set has no item.
    Empty,
    /// Two items have this number.
    Repeated(u32),
    /// The item with this number is not usable.
    Item { id: u32, fault: Fault },
}

/// What is wrong with an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    NoWord,
    NoSentence,
    NoAccepted,
    EmptyAccepted,
    WordNotInSentence,
}

impl fmt::Display for SetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(kind) => write!(f, "the set cannot be read: {kind}"),
            Self::Malformed(why) => write!(f, "the set is not valid JSON of a set: {why}"),
            Self::Empty => f.write_str("the set has no item"),
            Self::Repeated(id) => write!(f, "the set has two items numbered {id}"),
            Self::Item { id, fault } => {
                let fault = match fault {
                    Fault::NoWord => "has no word",
                    Fault::NoSentence => "has no sentence",
                    Fault::NoAccepted => "has no accepted form",
                    Fault::EmptyAccepted => "has an empty accepted form",
                    Fault::WordNotInSentence => "has a word that is not in its sentence",
                };
                write!(f, "item {id} of the set {fault}")
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

/// The set of `bytes`, refused when any item cannot be asked or judged.
pub fn parse_set(bytes: &[u8]) -> Result<Loaded, SetError> {
    let set: Set = serde_json::from_slice(bytes).map_err(|error| malformed(&error))?;
    if set.items.is_empty() {
        return Err(SetError::Empty);
    }
    for (at, item) in set.items.iter().enumerate() {
        if set.items[..at].iter().any(|earlier| earlier.id == item.id) {
            return Err(SetError::Repeated(item.id));
        }
        if let Some(fault) = fault(item) {
            return Err(SetError::Item { id: item.id, fault });
        }
    }
    Ok(Loaded {
        set,
        sha256: sha256_hex(bytes),
    })
}

/// What is said of a file that is not a set: the kind of the fault and its
/// place. The message of `serde_json` is not used, since it quotes the value
/// it could not read, and the file may be anything, a key file included.
fn malformed(error: &serde_json::Error) -> SetError {
    use serde_json::error::Category;
    let kind = match error.classify() {
        Category::Io => "unreadable",
        Category::Syntax => "a syntax error",
        Category::Data => "an unexpected value or field",
        Category::Eof => "an early end",
    };
    SetError::Malformed(format!(
        "{kind} at line {}, column {}",
        error.line(),
        error.column()
    ))
}

/// The set of the file at `path`.
pub fn load_set(path: &Path) -> Result<Loaded, SetError> {
    let bytes = std::fs::read(path).map_err(|error| SetError::Unreadable(error.kind()))?;
    parse_set(&bytes)
}

/// The first thing wrong with `item`, if anything is.
fn fault(item: &Item) -> Option<Fault> {
    if item.word.trim().is_empty() {
        return Some(Fault::NoWord);
    }
    if item.sentence.trim().is_empty() {
        return Some(Fault::NoSentence);
    }
    if item.accepted.is_empty() {
        return Some(Fault::NoAccepted);
    }
    if item.accepted.iter().any(|form| normalized(form).is_empty()) {
        return Some(Fault::EmptyAccepted);
    }
    // A whole word of the sentence, whatever its case.
    let word = item.word.to_lowercase();
    let in_sentence = item
        .sentence
        .split(|c: char| !c.is_alphanumeric())
        .any(|part| part.to_lowercase() == word);
    (!in_sentence).then_some(Fault::WordNotInSentence)
}

/// `text` as the verdict compares it: trimmed, without a final full stop,
/// lowercased and with `ё` read as `е`.
pub fn normalized(text: &str) -> String {
    let text = text.trim();
    let text = text.strip_suffix('.').unwrap_or(text).trim_end();
    text.to_lowercase().replace('ё', "е")
}

/// Whether an answer is the marked meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Match,
    Mismatch,
}

/// The verdict of `answer`: a match only when it equals one of the
/// `accepted` forms once both are [`normalized`]. An inflected form or a
/// synonym outside the list is a mismatch.
pub fn verdict(answer: &str, accepted: &[String]) -> Verdict {
    let answer = normalized(answer);
    if accepted.iter().any(|form| normalized(form) == answer) {
        Verdict::Match
    } else {
        Verdict::Mismatch
    }
}

/// The historical EXP-5 name of a refusal in the results file.
/// More specific product refusals retain the existing evidence vocabulary.
pub fn refusal_name(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::Key | Refusal::UnreadableKey | Refusal::UnsafeKey => "key",
        Refusal::RejectedKey => "rejected_key",
        Refusal::Quota => "quota",
        Refusal::TimedOut => "timed_out",
        Refusal::Other | Refusal::Network | Refusal::UnsupportedLanguage => "other",
    }
}

/// One language of the answer of `/v3/languages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LanguageRecord {
    pub lang: String,
    pub usable_as_source: bool,
    pub usable_as_target: bool,
}

/// Which reading of the usage a line holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum When {
    Before,
    After,
}

/// One line of the results file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// The first line: what was run.
    Run {
        schema: u32,
        /// When the run began, UTC.
        date: String,
        set_sha256: String,
        items: usize,
        limit_ms: u64,
        runner: &'static str,
    },
    /// The answer of `/v3/languages`, or its refusal.
    Languages {
        delay_ms: u64,
        refusal: Option<&'static str>,
        /// Whether `en` is listed as usable as a source; `None` on a refusal.
        en_as_source: Option<bool>,
        /// Whether `ru` is listed as usable as a target; `None` on a refusal.
        ru_as_target: Option<bool>,
        languages: Vec<LanguageRecord>,
    },
    /// One reading of `/v2/usage`, or its refusal.
    Usage {
        when: When,
        delay_ms: u64,
        refusal: Option<&'static str>,
        character_count: Option<u64>,
        character_limit: Option<u64>,
    },
    /// One request of one item.
    Item {
        item: u32,
        word: String,
        sentence: String,
        /// Whether the sentence was sent as the context.
        context: bool,
        accepted: Vec<String>,
        delay_ms: u64,
        /// The answer as the client gave it, unchanged.
        answer: Option<String>,
        refusal: Option<&'static str>,
        verdict: Option<Verdict>,
    },
    /// The last line of a run that did not reach its end.
    Incomplete { reason: String },
}

/// What a run that reached its end found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub items: usize,
    /// Requests of items that were refused or got no answer in time.
    pub refused: usize,
    pub matched_without_context: usize,
    pub matched_with_context: usize,
    pub before: Usage,
    pub after: Usage,
}

/// Why a run did not reach its end.
#[derive(Debug)]
pub enum Failure {
    /// The results file is there already; nothing was asked.
    Exists(PathBuf),
    /// The results file cannot be created or begun; nothing was asked.
    Results(io::Error),
    /// A line of the results file could not be written after the requests
    /// began.
    Interrupted(io::Error),
    /// The languages were refused; no translation was asked.
    Languages(Refusal),
    /// The account does not offer English as source or Russian as target.
    NotOffered {
        en_as_source: bool,
        ru_as_target: bool,
    },
    /// A reading of the usage was refused.
    Usage(When, Refusal),
    /// The request of this item was refused in a way that ends the run: the
    /// quota is used up or the key is not accepted.
    Item { id: u32, refusal: Refusal },
}

impl Failure {
    /// The refusal behind this failure, if it is one.
    fn refusal(&self) -> Option<Refusal> {
        match self {
            Self::Languages(refusal) | Self::Usage(_, refusal) => Some(*refusal),
            Self::Item { refusal, .. } => Some(*refusal),
            Self::Exists(_) | Self::Results(_) | Self::Interrupted(_) | Self::NotOffered { .. } => {
                None
            }
        }
    }

    /// The reason as the last line of the results file holds it.
    fn reason(&self) -> String {
        match self {
            Self::Exists(_) => "results_exist".to_owned(),
            Self::Results(_) | Self::Interrupted(_) => "results_not_written".to_owned(),
            Self::Languages(refusal) => format!("languages_{}", refusal_name(*refusal)),
            Self::NotOffered { .. } => "en_ru_not_offered".to_owned(),
            Self::Usage(When::Before, refusal) => {
                format!("usage_before_{}", refusal_name(*refusal))
            }
            Self::Usage(When::After, refusal) => format!("usage_after_{}", refusal_name(*refusal)),
            Self::Item { id, refusal } => format!("item_{id}_{}", refusal_name(*refusal)),
        }
    }
}

/// The message of `failure` for the user. It names the key file, at
/// `key_file`, when the key is the cause; nothing of the file's content can
/// be in it.
pub fn explain(failure: &Failure, key_file: Option<&Path>) -> String {
    let what = match failure {
        Failure::Exists(path) => {
            return format!(
                "the results file {} exists already and is not overwritten; nothing was asked",
                path.display()
            );
        }
        Failure::Results(error) => {
            return format!(
                "the results file cannot be created or begun: {}; nothing was asked",
                error.kind()
            );
        }
        Failure::Interrupted(error) => {
            return format!(
                "the results file could not be written during the run: {}; the run stopped",
                error.kind()
            );
        }
        Failure::NotOffered {
            en_as_source,
            ru_as_target,
        } => {
            return format!(
                "DeepL does not offer EN→RU to this key (en as source: {en_as_source}, \
                 ru as target: {ru_as_target}); no translation was asked"
            );
        }
        Failure::Languages(_) => "the languages".to_owned(),
        Failure::Usage(When::Before, _) => "the usage before the run".to_owned(),
        Failure::Usage(When::After, _) => "the usage after the run".to_owned(),
        Failure::Item { id, .. } => format!("item {id}"),
    };
    let key_file = key_file.map_or_else(
        || "the key file (no configuration directory is known)".to_owned(),
        |path| format!("the key file {}", path.display()),
    );
    let why = match failure.refusal() {
        Some(Refusal::Key | Refusal::UnreadableKey | Refusal::UnsafeKey) => format!(
            "{key_file} gives no usable key: it must exist, hold the key alone and have mode 0600"
        ),
        Some(Refusal::RejectedKey) => format!("DeepL rejected the key of {key_file}"),
        Some(Refusal::Quota) => "the quota of the key is used up".to_owned(),
        Some(Refusal::TimedOut) => "no answer came in time".to_owned(),
        Some(Refusal::Other | Refusal::Network | Refusal::UnsupportedLanguage) | None => {
            "the request failed (no network, another status or an unreadable answer)".to_owned()
        }
    };
    format!("{what}: {why}; the run stopped")
}

/// Runs the experiment: asks `client` for the languages, the usage, every
/// item of `set` without and with its context, and the usage again, writing
/// each answer to a new file at `results` as it comes.
///
/// Each request has `limit` to answer. A refusal of an item is written to its
/// line; the run goes on unless the quota is used up or the key is refused.
pub async fn run(
    client: &DeepL,
    set: &Loaded,
    results: &Path,
    limit: Duration,
) -> Result<Summary, Failure> {
    let mut file = create(results)?;
    let run = Record::Run {
        schema: SCHEMA,
        date: utc(SystemTime::now()),
        set_sha256: set.sha256.clone(),
        items: set.set.items.len(),
        limit_ms: millis(limit),
        runner: concat!("addict-exp5/", env!("CARGO_PKG_VERSION")),
    };
    write(&mut file, &run)?;
    // A line that cannot be written from here on is lost after a request.
    let outcome = ask(client, &set.set, &mut file, limit)
        .await
        .map_err(|failure| match failure {
            Failure::Results(error) => Failure::Interrupted(error),
            other => other,
        });
    if let Err(failure) = &outcome
        && !matches!(failure, Failure::Interrupted(_))
    {
        let last = Record::Incomplete {
            reason: failure.reason(),
        };
        write(&mut file, &last)?;
    }
    outcome
}

/// Every request of the run after its first line.
async fn ask(
    client: &DeepL,
    set: &Set,
    file: &mut File,
    limit: Duration,
) -> Result<Summary, Failure> {
    let (languages, delay_ms) = timed(limit, client.languages()).await;
    let offered = languages.as_deref().ok().map(offered);
    let record = Record::Languages {
        delay_ms,
        refusal: languages.as_ref().err().copied().map(refusal_name),
        en_as_source: offered.map(|(en, _)| en),
        ru_as_target: offered.map(|(_, ru)| ru),
        languages: languages
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|language| LanguageRecord {
                lang: language.lang.clone(),
                usable_as_source: language.usable_as_source,
                usable_as_target: language.usable_as_target,
            })
            .collect(),
    };
    write(file, &record)?;
    languages.map_err(Failure::Languages)?;

    let before = usage(client, When::Before, file, limit).await?;
    if let Some((en_as_source, ru_as_target)) = offered
        && !(en_as_source && ru_as_target)
    {
        return Err(Failure::NotOffered {
            en_as_source,
            ru_as_target,
        });
    }

    let mut summary = Summary {
        items: set.items.len(),
        refused: 0,
        matched_without_context: 0,
        matched_with_context: 0,
        before,
        after: before,
    };
    for item in &set.items {
        for context in [false, true] {
            let request = Request {
                text: item.word.clone(),
                context: context.then(|| item.sentence.clone()),
            };
            let (answer, delay_ms) = timed(limit, client.translate(&request)).await;
            let verdict = answer
                .as_deref()
                .ok()
                .map(|answer| verdict(answer, &item.accepted));
            let record = Record::Item {
                item: item.id,
                word: item.word.clone(),
                sentence: item.sentence.clone(),
                context,
                accepted: item.accepted.clone(),
                delay_ms,
                refusal: answer.as_ref().err().copied().map(refusal_name),
                answer: answer.clone().ok(),
                verdict,
            };
            write(file, &record)?;
            match (answer, verdict) {
                (
                    Err(
                        refusal @ (Refusal::Quota
                        | Refusal::Key
                        | Refusal::UnreadableKey
                        | Refusal::UnsafeKey
                        | Refusal::RejectedKey),
                    ),
                    _,
                ) => {
                    return Err(Failure::Item {
                        id: item.id,
                        refusal,
                    });
                }
                (
                    Err(
                        Refusal::TimedOut
                        | Refusal::Other
                        | Refusal::Network
                        | Refusal::UnsupportedLanguage,
                    ),
                    _,
                ) => summary.refused += 1,
                (Ok(_), Some(Verdict::Match)) if context => summary.matched_with_context += 1,
                (Ok(_), Some(Verdict::Match)) => summary.matched_without_context += 1,
                (Ok(_), _) => {}
            }
        }
    }

    summary.after = usage(client, When::After, file, limit).await?;
    Ok(summary)
}

/// Asks the usage once and writes its line.
async fn usage(
    client: &DeepL,
    when: When,
    file: &mut File,
    limit: Duration,
) -> Result<Usage, Failure> {
    let (usage, delay_ms) = timed(limit, client.usage()).await;
    let record = Record::Usage {
        when,
        delay_ms,
        refusal: usage.err().map(refusal_name),
        character_count: usage.ok().map(|usage| usage.character_count),
        character_limit: usage.ok().map(|usage| usage.character_limit),
    };
    write(file, &record)?;
    usage.map_err(|refusal| Failure::Usage(when, refusal))
}

/// Whether `languages` lists `en` as usable as a source and `ru` as usable
/// as a target.
fn offered(languages: &[Language]) -> (bool, bool) {
    let any = |code: &str, usable: fn(&Language) -> bool| {
        languages
            .iter()
            .any(|language| language.lang.eq_ignore_ascii_case(code) && usable(language))
    };
    (
        any(SOURCE, |language| language.usable_as_source),
        any(TARGET, |language| language.usable_as_target),
    )
}

/// The answer of `asking` and how long it took; [`Refusal::TimedOut`] when
/// `limit` passes first, which gives the request up.
async fn timed<T>(
    limit: Duration,
    asking: impl Future<Output = Result<T, Refusal>>,
) -> (Result<T, Refusal>, u64) {
    let began = Instant::now();
    let answer = tokio::time::timeout(limit, asking)
        .await
        .unwrap_or(Err(Refusal::TimedOut));
    (answer, millis(began.elapsed()))
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
        std::fs::create_dir_all(parent).map_err(Failure::Results)?;
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
fn write(file: &mut File, record: &Record) -> Result<(), Failure> {
    let mut line =
        serde_json::to_string(record).map_err(|error| Failure::Results(io::Error::other(error)))?;
    line.push('\n');
    file.write_all(line.as_bytes())
        .and_then(|()| file.flush())
        .map_err(Failure::Results)
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
    use std::time::{Duration, UNIX_EPOCH};

    use super::{Fault, SetError, Verdict, normalized, parse_set, sha256_hex, utc, verdict};

    fn forms(forms: &[&str]) -> Vec<String> {
        forms.iter().map(|&form| form.to_owned()).collect()
    }

    #[test]
    fn an_answer_matches_whatever_its_case_its_final_stop_and_its_yo() {
        let accepted = forms(&["берег", "Среднее значение", "самолёт"]);
        for answer in [
            "берег",
            "Берег",
            " берег \n",
            "берег.",
            "Берег. ",
            "БЕРЕГ",
            "среднее значение",
            "самолет",
            "Самолёт.",
        ] {
            assert_eq!(verdict(answer, &accepted), Verdict::Match, "{answer:?}");
        }
    }

    #[test]
    fn an_inflected_form_a_synonym_and_anything_more_are_mismatches() {
        let accepted = forms(&["берег", "среднее значение"]);
        for answer in [
            "берега",
            "берегу",
            "побережье",
            "берег реки",
            "берег!",
            "берег..",
            "«берег»",
            "среднее",
            "среднее  значение",
            "bank",
            "",
        ] {
            assert_eq!(verdict(answer, &accepted), Verdict::Mismatch, "{answer:?}");
        }
    }

    #[test]
    fn the_comparison_form_is_trimmed_lowercased_without_a_stop_and_with_ye() {
        assert_eq!(normalized("  Ёлка. "), "елка");
        assert_eq!(normalized("ёж ."), "еж");
        assert_eq!(normalized("."), "");
    }

    #[test]
    fn the_hash_is_the_sha_256_of_the_bytes() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let bytes =
            br#"{"items":[{"id":1,"word":"bank","sentence":"The bank.","accepted":["x"]}]}"#;
        assert_eq!(parse_set(bytes).expect("a set").sha256, sha256_hex(bytes));
    }

    #[test]
    fn an_item_that_cannot_be_asked_or_judged_is_named() {
        let set = |item: &str| {
            format!(
                r#"{{"items":[{{"id":1,"word":"a","sentence":"a b","accepted":["x"]}},{item}]}}"#
            )
        };
        for (item, fault) in [
            (
                r#"{"id":7,"word":"bank","accepted":["x"]}"#,
                Fault::NoSentence,
            ),
            (
                r#"{"id":7,"word":"bank","sentence":"  ","accepted":["x"]}"#,
                Fault::NoSentence,
            ),
            (
                r#"{"id":7,"word":"bank","sentence":"The bank."}"#,
                Fault::NoAccepted,
            ),
            (
                r#"{"id":7,"word":"bank","sentence":"The bank.","accepted":[]}"#,
                Fault::NoAccepted,
            ),
            (
                r#"{"id":7,"word":"bank","sentence":"The bank.","accepted":["x"," . "]}"#,
                Fault::EmptyAccepted,
            ),
            (
                r#"{"id":7,"word":" ","sentence":"The bank.","accepted":["x"]}"#,
                Fault::NoWord,
            ),
            (
                r#"{"id":7,"word":"bank","sentence":"The banks of the river.","accepted":["x"]}"#,
                Fault::WordNotInSentence,
            ),
            (
                r#"{"id":7,"word":"bank","sentence":"A riverbank.","accepted":["x"]}"#,
                Fault::WordNotInSentence,
            ),
        ] {
            let error = parse_set(set(item).as_bytes()).expect_err(item);
            assert_eq!(error, SetError::Item { id: 7, fault }, "{item}");
            assert!(error.to_string().contains("item 7"), "{error}");
        }
    }

    #[test]
    fn a_word_is_found_in_its_sentence_whatever_its_case_and_its_neighbours() {
        for sentence in ["Bank on it.", "the (bank)", "bank", "It's the bank's."] {
            let set = format!(
                r#"{{"items":[{{"id":1,"word":"bank","sentence":"{sentence}","accepted":["x"]}}]}}"#
            );
            assert!(parse_set(set.as_bytes()).is_ok(), "{sentence}");
        }
    }

    #[test]
    fn what_is_not_a_set_is_refused() {
        assert_eq!(parse_set(br#"{"items":[]}"#), Err(SetError::Empty));
        let twice = br#"{"items":[
            {"id":3,"word":"a","sentence":"a","accepted":["x"]},
            {"id":3,"word":"a","sentence":"a","accepted":["x"]}]}"#;
        assert_eq!(parse_set(twice), Err(SetError::Repeated(3)));
        for bytes in [
            &b""[..],
            b"[]",
            b"{}",
            br#"{"items":[{"word":"a","sentence":"a","accepted":["x"]}]}"#,
            br#"{"items":[{"id":1,"sentence":"a","accepted":["x"]}]}"#,
            br#"{"items":[{"id":1,"word":"a","sentence":"a","accepted":["x"],"note":1}]}"#,
            br#"{"items":[],"more":1}"#,
        ] {
            let error = parse_set(bytes).expect_err("not a set");
            assert!(matches!(error, SetError::Malformed(_)), "{error:?}");
        }
    }

    #[test]
    fn the_message_of_a_file_that_is_not_a_set_holds_nothing_of_the_file() {
        // A key file given as the set by mistake, and what else may quote it.
        const KEY: &str = "0a1b2c3d-key-like-secret:fx";
        for content in [
            KEY.to_owned(),
            format!("{KEY}\n"),
            format!("\"{KEY}\""),
            format!("[\"{KEY}\"]"),
            format!("{{\"{KEY}\":1}}"),
            format!("{{\"items\":\"{KEY}\"}}"),
            format!("{{\"items\":[{{\"id\":\"{KEY}\"}}]}}"),
            format!("{{\"items\":[{{\"id\":1,\"word\":\"a\",\"{KEY}\":1}}]}}"),
        ] {
            let error = parse_set(content.as_bytes()).expect_err("not a set");
            assert!(matches!(error, SetError::Malformed(_)), "{error:?}");
            for shown in [error.to_string(), format!("{error:?}")] {
                assert!(!shown.contains(KEY), "{shown}");
                assert!(!shown.contains("secret"), "{shown}");
                assert!(!shown.contains("fx"), "{shown}");
                assert!(shown.contains("line 1"), "{shown}");
            }
        }
    }

    #[test]
    fn a_time_is_written_as_its_utc_date_and_time() {
        let at = |seconds| utc(UNIX_EPOCH + Duration::from_secs(seconds));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_399), "2000-02-28T23:59:59Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_791_915_725), "2026-10-13T18:22:05Z");
    }
}
