//! Download offer texts and the display state of the install queue.

use std::time::Duration;

use addict_core::translation::Source as Language;
use addict_dictionary::{
    release::{self, Published},
    sources,
};

/// The title of the dialog.
pub const TITLE: &str = "Download dictionaries";

/// The heading of the dialog while it offers the download.
pub const HEADING: &str = "Look words up offline";

/// The small line above the heading of the first-run offer: the state that
/// the offer answers.
pub const EYEBROW: &str = "No dictionary is installed yet";

/// The heading while the installs run.
pub const INSTALLING_HEADING: &str = "Downloading dictionaries";

/// The heading once every source is installed.
pub const READY_HEADING: &str = "Dictionaries are ready";

/// The heading once some sources, not all, are installed.
pub const PARTIAL_HEADING: &str = "Download finished";

/// The heading once nothing was installed, or the install could not start.
pub const FAILED_HEADING: &str = "The download failed";

/// What the dialog offers.
pub const BODY: &str = "Addict looks words up in dictionaries kept on this computer. They come \
                        from the releases of Addict on GitHub and install one after another; \
                        one that fails does not stop the others.";

/// The name of the my-language choice of the first-run offer.
pub const MY_LANGUAGE: &str = "My language";

/// What the my-language choice of the first-run offer decides.
pub const MY_LANGUAGE_HINT: &str = "Only the dictionaries for this language are downloaded. \
                                    Settings can change it later.";

/// What the dialog says while the installs run.
pub const INSTALLING_BODY: &str = "They install one after another. You can close this window; \
                                   the download goes on, and the window opens again when it ends.";

/// What follows the summary once every source of the first-run offer is
/// installed.
pub const READY_BODY: &str = "Words can now be looked up offline.";

/// The note beside Cancel all of the first-run offer.
pub const AFTER_FAILURE: &str = "One that fails is offered again at the next start.";

/// The button that closes the dialog once every source is installed.
pub const START: &str = "Start looking up";

/// The status of a row whose source failed; the reason is under its name.
pub const FAILED: &str = "Failed";

/// Where the credits of the dictionaries are.
pub const CREDIT: &str = "Each dictionary keeps the license of its source, which is written \
                          inside its file. About → Credits names every source.";

/// The button that downloads.
pub const DOWNLOAD: &str = "Download";

/// The button that declines, for good.
pub const NOT_NOW: &str = "Not now";

/// The button that closes the dialog once the install ended.
pub const CLOSE: &str = "Close";

/// The button of a row, and of the dialog, that stops installing.
pub const CANCEL: &str = "Cancel";

/// The button of the dialog that stops every install.
pub const CANCEL_ALL: &str = "Cancel all";

/// The row of a source whose download has come whole and is checked and
/// unpacked.
pub const INSTALLING: &str = "Installing…";

/// The row of a source that waits for its turn.
pub const WAITING: &str = "Waiting";

/// The row of a source that is installed.
pub const INSTALLED: &str = "Installed";

/// The row of a source that was cancelled.
pub const CANCELLED: &str = "Cancelled";

/// Why Download could not start: no data directory, or no runner.
pub const NO_PLACE: &str = "cannot find the data directory, or the runner is not there";

/// Why an install that was given up or panicked failed.
pub const STOPPED: &str = "it stopped";

/// Why an install failed that did not end within `limit`.
pub fn late_text(limit: Duration) -> String {
    format!("it did not finish within {} minutes", limit.as_secs() / 60)
}

/// A recovery offer must not claim that every dictionary is absent.
pub const RECOVERY_HEADING: &str = "Restore missing or unreadable dictionaries";
pub const FETCHING: &str = "Fetching the dictionary list and checking installed files…";
pub const DIALOG_OPEN: &str = "The dictionary download dialog is open.";
pub const RECOVERY_TIMEOUT: &str = "the dictionary check did not finish in time";

/// Settings recovery found every dictionary of my language readable.
pub fn nothing_text(my: Language) -> String {
    format!(
        "All {} dictionaries are readable. Nothing to download.",
        super::settings::my_language_name(my)
    )
}
pub const RETRY: &str = "You can try again through Settings → Download dictionaries.";

pub fn recovery_failed_text(error: &str) -> String {
    format!(
        "The download failed: {}. {RETRY}",
        error.trim_end_matches('.')
    )
}

pub fn recovery_summary_text(queue: &Queue) -> String {
    if queue
        .rows()
        .iter()
        .any(|(_, state)| *state != RowState::Installed)
    {
        let installed = queue
            .rows()
            .iter()
            .filter(|(_, state)| *state == RowState::Installed)
            .count();
        format!(
            "{installed} of {} missing or unreadable dictionaries are restored. {RETRY}",
            queue.rows().len()
        )
    } else {
        "Every missing or unreadable dictionary is restored.".to_owned()
    }
}

/// `bytes` in whole megabytes, rounded up, at least one.
fn megabytes(bytes: u64) -> u64 {
    bytes.div_ceil(1_000_000).max(1)
}

/// The row of `published` before Download: its size.
pub fn offered_text(published: &Published) -> String {
    format!("{} MB", megabytes(published.size))
}

/// The tooltip of the row of the index `name`: its credit and license, from
/// the table of sources; `None` for a name this build does not know.
pub fn row_credit_text(name: &str) -> Option<String> {
    let row = sources::row(release::source_of(name)?);
    Some(format!("{}. License: {}.", row.credit, row.license))
}

/// The message of the source `name` that failed for `error`, on one line
/// and naming the source.
pub fn failure_text(name: &str, error: &str) -> String {
    let error = error.split_whitespace().collect::<Vec<_>>().join(" ");
    if error.contains(name) {
        error
    } else {
        format!("cannot install {name}: {error}")
    }
}

/// Why the source `name` failed, under its name in its row: `message`
/// without the part that names it again.
pub fn reason_text(name: &str, message: &str) -> String {
    let reason = ["cannot download", "cannot install"]
        .iter()
        .find_map(|verb| message.strip_prefix(&format!("{verb} {name}: ")))
        .unwrap_or(message);
    let mut letters = reason.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

/// The line of an install that could not start, `error` being why.
pub fn failed_text(error: &str) -> String {
    format!(
        "The download failed: {}. It is offered again at the next start.",
        error.trim_end_matches('.')
    )
}

/// Whether every source of `queue` is installed.
pub fn is_complete(queue: &Queue) -> bool {
    !queue.rows().is_empty()
        && queue
            .rows()
            .iter()
            .all(|(_, state)| *state == RowState::Installed)
}

/// The heading once every source of `queue` has ended.
pub fn ended_heading(queue: &Queue) -> &'static str {
    let installed = queue
        .rows()
        .iter()
        .filter(|(_, state)| *state == RowState::Installed)
        .count();
    if is_complete(queue) {
        READY_HEADING
    } else if installed > 0 {
        PARTIAL_HEADING
    } else {
        FAILED_HEADING
    }
}

/// The languages the dialog groups the sources by, in their order: the
/// language of the words each one looks up.
pub const LANGUAGES: [&str; 4] = ["English", "Spanish", "Catalan", "Russian"];

/// The group of the sources this build does not know.
pub const OTHER: &str = "Other";

/// The language of the words of the index `name`, as the dialog groups it,
/// and a short title of its source; `None` for a name this build does not
/// know. The match has no wildcard arm, so a new source needs both.
pub fn describe(name: &str) -> Option<(&'static str, &'static str)> {
    use addict_dictionary::import::Source;
    Some(match release::source_of(name)? {
        Source::Oewn => ("English", "Open English WordNet"),
        Source::Freedict => ("English", "FreeDict, English → Russian"),
        Source::KaikkiEs => ("Spanish", "Spanish Wiktionary, via Kaikki"),
        Source::WikdictEsRu => ("Spanish", "WikDict, Spanish → Russian"),
        Source::OmwEs => ("Spanish", "Spanish wordnet, Open Multilingual Wordnet"),
        Source::KaikkiEnSpanish => ("Spanish", "English Wiktionary, via Kaikki"),
        Source::WikdictCaRu => ("Catalan", "WikDict, Catalan → Russian"),
        Source::DbnaryCa => ("Catalan", "Catalan Wiktionary, via DBnary"),
        Source::SinonimsCat => ("Catalan", "Softcatalà thesaurus"),
        Source::KaikkiEnCatalan => ("Catalan", "English Wiktionary, via Kaikki"),
        Source::KaikkiRu => ("Russian", "Russian Wiktionary, via Kaikki"),
        Source::WikdictRuEn => ("Russian", "WikDict, Russian → English"),
        Source::WikdictRuEs => ("Russian", "WikDict, Russian → Spanish"),
        Source::WikdictRuCa => ("Russian", "WikDict, Russian → Catalan"),
        Source::WikdictEsCa => ("Spanish", "WikDict, Spanish → Catalan"),
        Source::WikdictEnEs => ("English", "WikDict, English → Spanish"),
        Source::WikdictEnCa => ("English", "WikDict, English → Catalan"),
        Source::WikdictEsEn => ("Spanish", "WikDict, Spanish → English"),
        Source::WikdictCaEn => ("Catalan", "WikDict, Catalan → English"),
        Source::WikdictCaEs => ("Catalan", "WikDict, Catalan → Spanish"),
    })
}

/// The indexes of one language, in the order of the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: &'static str,
    pub indexes: Vec<Published>,
}

/// `indexes` grouped by language, in the order of [`LANGUAGES`], then the
/// unknown ones; a language with no index has no group.
pub fn groups(indexes: &[Published]) -> Vec<Group> {
    LANGUAGES
        .iter()
        .chain(std::iter::once(&OTHER))
        .filter_map(|&language| {
            let indexes: Vec<Published> = indexes
                .iter()
                .filter(|published| {
                    describe(&published.name).map_or(OTHER, |(group, _)| group) == language
                })
                .cloned()
                .collect();
            (!indexes.is_empty()).then_some(Group {
                name: language,
                indexes,
            })
        })
        .collect()
}

/// `count` dictionaries, in words.
fn dictionaries(count: usize) -> String {
    if count == 1 {
        "1 dictionary".to_owned()
    } else {
        format!("{count} dictionaries")
    }
}

/// The line beside the name of a group: how many dictionaries, how big.
pub fn group_text(indexes: &[Published]) -> String {
    let total = indexes.iter().map(|published| published.size).sum();
    format!("{} · {} MB", dictionaries(indexes.len()), megabytes(total))
}

/// The three figures of the offer, each a value and its caption: how many
/// dictionaries, in how many languages, and how big they are together.
pub fn figures(indexes: &[Published]) -> [(String, &'static str); 3] {
    let total = indexes.iter().map(|published| published.size).sum();
    let languages = groups(indexes)
        .iter()
        .filter(|group| group.name != OTHER)
        .count();
    [
        (
            indexes.len().to_string(),
            if indexes.len() == 1 {
                "dictionary"
            } else {
                "dictionaries"
            },
        ),
        (
            languages.to_string(),
            if languages == 1 {
                "language"
            } else {
                "languages"
            },
        ),
        (format!("{} MB", megabytes(total)), "to download"),
    ]
}

/// The small line above the heading of an offer of `count` indexes.
pub fn eyebrow_text(recovery: bool, count: usize) -> String {
    if recovery {
        format!("{} to restore", dictionaries(count))
    } else {
        EYEBROW.to_owned()
    }
}

/// The whole percent of `size` bytes that `received` are, at most 100.
pub fn percent(received: u64, size: u64) -> u64 {
    received
        .saturating_mul(100)
        .checked_div(size)
        .unwrap_or(0)
        .min(100)
}

/// How the whole of a [`Queue`] stands, for the card above the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// The part of all the bytes that came, from 0 to 1.
    pub fraction: f64,
    /// What runs: "Installing oewn…".
    pub current: String,
    /// "31 of 94 MB".
    pub bytes: String,
    /// "2 installed · 1 failed · 10 to go".
    pub counts: String,
    /// "Installing 4 of 13", while a source has not ended.
    pub step: Option<String>,
}

/// How `queue` stands. The bytes of an installed source count whole, and a
/// running one counts what came.
pub fn progress(queue: &Queue) -> Progress {
    let rows = queue.rows();
    let total: u64 = rows.iter().map(|(published, _)| published.size).sum();
    let done: u64 = rows
        .iter()
        .map(|(published, state)| match state {
            RowState::Installed => published.size,
            RowState::Running { received } => (*received).min(published.size),
            _ => 0,
        })
        .sum();
    let count = |wanted: fn(&RowState) -> bool| rows.iter().filter(|(_, s)| wanted(s)).count();
    let installed = count(|state| *state == RowState::Installed);
    let failed = count(|state| matches!(state, RowState::Failed(_)));
    let cancelled = count(|state| *state == RowState::Cancelled);
    let open = count(RowState::is_open);
    let current = rows
        .iter()
        .find(|(_, state)| matches!(state, RowState::Running { .. }))
        .map_or_else(
            || INSTALLING.to_owned(),
            |(published, _)| format!("Installing {}…", published.name),
        );
    let counts = [
        (installed, "installed"),
        (failed, "failed"),
        (cancelled, "cancelled"),
        (open, "to go"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect::<Vec<_>>()
    .join(" · ");
    Progress {
        fraction: if total == 0 {
            0.0
        } else {
            done as f64 / total as f64
        },
        current,
        bytes: format!("{} of {} MB", done / 1_000_000, megabytes(total)),
        counts,
        step: (open > 0).then(|| format!("Installing {} of {}", rows.len() - open + 1, rows.len())),
    }
}

/// The line once every source of `queue` has ended.
pub fn summary_text(queue: &Queue) -> String {
    let total = queue.rows().len();
    let installed = queue
        .rows()
        .iter()
        .filter(|(_, state)| *state == RowState::Installed)
        .count();
    if installed == 0 {
        "No dictionary was installed. The download is offered again at the next start.".to_owned()
    } else if installed == total {
        if total == 1 {
            "The dictionary is installed.".to_owned()
        } else {
            format!("All {total} dictionaries are installed.")
        }
    } else {
        format!("{installed} of {total} dictionaries are installed.")
    }
}

/// Where one source of a [`Queue`] stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    /// It waits for its turn.
    Queued,
    /// It is downloaded and installed; `received` bytes of its asset came.
    Running { received: u64 },
    /// Its index is in the data directory.
    Installed,
    /// It failed; the message names it, on one line.
    Failed(String),
    /// It was cancelled before it ended.
    Cancelled,
}

impl RowState {
    /// The status at the end of its row, for an asset of `size` bytes; a
    /// failure says why under the name of its source.
    pub fn text(&self, size: u64) -> String {
        match self {
            Self::Queued => WAITING.to_owned(),
            Self::Running { received } if size > 0 && *received >= size => INSTALLING.to_owned(),
            Self::Running { received } => format!("{}%", percent(*received, size)),
            Self::Installed => INSTALLED.to_owned(),
            Self::Failed(_) => FAILED.to_owned(),
            Self::Cancelled => CANCELLED.to_owned(),
        }
    }

    /// Whether the source has not ended yet, so it can be cancelled.
    pub fn is_open(&self) -> bool {
        matches!(self, Self::Queued | Self::Running { .. })
    }
}

/// The sources to install, in order, and where each stands. At most one
/// runs at a time: [`next`](Self::next) gives the next one only when none
/// runs. It knows nothing of GTK or of the network, so the window drives it
/// with jobs on the runner and the tests with the downloader itself:
///
/// ```ignore
/// while let Some(published) = queue.next() {
///     let result = downloader.install(&published, dir, |_| {}).await;
///     queue.finished(&published.name, result.map(drop).map_err(|e| e.to_string()));
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queue {
    rows: Vec<(Published, RowState)>,
}

impl Queue {
    /// Every one of `indexes` queued, in their order.
    pub fn new(indexes: Vec<Published>) -> Self {
        Self {
            rows: indexes
                .into_iter()
                .map(|published| (published, RowState::Queued))
                .collect(),
        }
    }

    /// The sources and where each stands, in order.
    pub fn rows(&self) -> &[(Published, RowState)] {
        &self.rows
    }

    /// Where the source `name` stands.
    #[cfg(test)]
    pub fn state(&self, name: &str) -> Option<&RowState> {
        self.row(name).map(|(_, state)| state)
    }

    /// The name of the source that runs.
    pub fn running(&self) -> Option<&str> {
        self.rows
            .iter()
            .find(|(_, state)| matches!(state, RowState::Running { .. }))
            .map(|(published, _)| published.name.as_str())
    }

    /// The first queued source, now running; `None` while one runs or when
    /// none is queued.
    #[allow(clippy::should_implement_trait)] // A running source pauses the queue.
    pub fn next(&mut self) -> Option<Published> {
        if self.running().is_some() {
            return None;
        }
        let (published, state) = self
            .rows
            .iter_mut()
            .find(|(_, state)| *state == RowState::Queued)?;
        *state = RowState::Running { received: 0 };
        Some(published.clone())
    }

    /// `received` bytes of the running source `name` came.
    pub fn progress(&mut self, name: &str, received: u64) {
        if let Some((_, state @ RowState::Running { .. })) = self.row_mut(name) {
            *state = RowState::Running { received };
        }
    }

    /// The running source `name` ended: installed, or failed for the
    /// error. A source that is not running, as one cancelled, stays as it
    /// is.
    pub fn finished(&mut self, name: &str, result: Result<(), String>) {
        if let Some((_, state @ RowState::Running { .. })) = self.row_mut(name) {
            *state = match result {
                Ok(()) => RowState::Installed,
                Err(error) => RowState::Failed(failure_text(name, &error)),
            };
        }
    }

    /// Cancels the source `name` when it has not ended; whether it was the
    /// running one, whose job the caller then drops.
    pub fn cancel(&mut self, name: &str) -> bool {
        match self.row_mut(name) {
            Some((_, state)) if state.is_open() => {
                let was_running = matches!(state, RowState::Running { .. });
                *state = RowState::Cancelled;
                was_running
            }
            _ => false,
        }
    }

    /// Cancels every source that has not ended; whether one of them was
    /// running.
    pub fn cancel_all(&mut self) -> bool {
        let mut was_running = false;
        for (_, state) in &mut self.rows {
            if state.is_open() {
                was_running |= matches!(state, RowState::Running { .. });
                *state = RowState::Cancelled;
            }
        }
        was_running
    }

    /// Whether every source has ended.
    pub fn is_done(&self) -> bool {
        self.rows.iter().all(|(_, state)| !state.is_open())
    }

    /// The entry of the source `name` and where it stands.
    #[cfg(test)]
    pub fn row(&self, name: &str) -> Option<&(Published, RowState)> {
        self.rows
            .iter()
            .find(|(published, _)| published.name == name)
    }

    fn row_mut(&mut self, name: &str) -> Option<&mut (Published, RowState)> {
        self.rows
            .iter_mut()
            .find(|(published, _)| published.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::Downloader;
    use addict_dictionary::release::Manifest;
    use std::{fs, path::PathBuf};

    /// A directory of one test, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("addict-first-run-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("the directory of the test");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn recovery_summaries_speak_of_the_restored_files_only() {
        let mut queue = Queue::new(vec![entry("oewn", 1), entry("omw-es", 1)]);
        while let Some(published) = queue.next() {
            queue.finished(&published.name, Ok(()));
        }
        assert_eq!(
            recovery_summary_text(&queue),
            "Every missing or unreadable dictionary is restored."
        );
        let mut queue = Queue::new(vec![entry("oewn", 1), entry("omw-es", 1)]);
        let first = queue.next().unwrap();
        queue.finished(&first.name, Ok(()));
        let second = queue.next().unwrap();
        queue.finished(&second.name, Err("refused".to_owned()));
        assert_eq!(
            recovery_summary_text(&queue),
            format!("1 of 2 missing or unreadable dictionaries are restored. {RETRY}")
        );
    }
    /// An entry of `name` whose asset has `size` bytes.
    fn entry(name: &str, size: u64) -> Published {
        Published {
            name: name.to_owned(),
            format: addict_dictionary::format::VERSION,
            size,
            sha256: "0".repeat(64),
        }
    }

    #[test]
    fn the_offer_counts_every_dictionary_and_sums_their_sizes() {
        let indexes = [
            entry("oewn", 5_935_000),
            entry("kaikki-es-Espanol", 40_000_000),
            entry("freedict", 1),
        ];
        assert_eq!(
            figures(&indexes),
            [
                ("3".to_owned(), "dictionaries"),
                ("2".to_owned(), "languages"),
                ("46 MB".to_owned(), "to download"),
            ]
        );
        assert_eq!(
            figures(&indexes[..1]),
            [
                ("1".to_owned(), "dictionary"),
                ("1".to_owned(), "language"),
                ("6 MB".to_owned(), "to download"),
            ]
        );
        // Grouped by the language of their words, in a fixed order, each
        // group in the order of the manifest; an unknown name goes last.
        let mut more = indexes.to_vec();
        more.push(entry("wikdict-ru-en", 1_474_032));
        more.push(entry("tessdata", 10));
        let groups = groups(&more);
        let names: Vec<_> = groups.iter().map(|group| group.name).collect();
        assert_eq!(names, ["English", "Spanish", "Russian", OTHER]);
        let english: Vec<_> = groups[0].indexes.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(english, ["oewn", "freedict"]);
        assert_eq!(group_text(&groups[0].indexes), "2 dictionaries · 6 MB");
        assert_eq!(group_text(&groups[1].indexes), "1 dictionary · 40 MB");
        assert_eq!(describe("oewn"), Some(("English", "Open English WordNet")));
        assert_eq!(
            describe("wikdict-es-ca"),
            Some(("Spanish", "WikDict, Spanish → Catalan"))
        );
        assert_eq!(
            describe("kaikki-ru-Russkiy"),
            Some(("Russian", "Russian Wiktionary, via Kaikki"))
        );
        assert_eq!(describe("tessdata"), None);
        assert_eq!(eyebrow_text(false, 13), EYEBROW);
        assert_eq!(eyebrow_text(true, 1), "1 dictionary to restore");
        assert_eq!(eyebrow_text(true, 3), "3 dictionaries to restore");
        assert_eq!(offered_text(&indexes[0]), "6 MB");
        assert_eq!(offered_text(&indexes[2]), "1 MB");
        let credit = row_credit_text("oewn").expect("a known source");
        assert!(credit.contains("Open English WordNet"), "{credit}");
        assert!(credit.contains("CC BY 4.0"), "{credit}");
        assert_eq!(row_credit_text("tessdata"), None);
    }

    #[test]
    fn the_texts_are_english_and_name_the_source() {
        assert_eq!(DOWNLOAD, "Download");
        assert_eq!(NOT_NOW, "Not now");
        assert_eq!(CANCEL, "Cancel");
        assert_eq!(INSTALLING, "Installing…");
        assert_eq!(percent(0, 1000), 0);
        assert_eq!(percent(421, 1000), 42);
        assert_eq!(percent(2000, 1000), 100);
        assert_eq!(percent(5, 0), 0);
        let failed = failed_text("cannot download oewn: the server answered HTTP 404");
        assert!(
            failed.contains("HTTP 404") && !failed.contains('\n'),
            "{failed}"
        );
        // A message that names its source is kept; one that does not, as the
        // runner's, gets the name; either is one line.
        assert_eq!(
            failure_text("oewn", "cannot download oewn: the server answered HTTP 404"),
            "cannot download oewn: the server answered HTTP 404"
        );
        assert_eq!(
            failure_text("kaikki-es-Espanol", "it did not finish\nwithin 30 minutes"),
            "cannot install kaikki-es-Espanol: it did not finish within 30 minutes"
        );
        assert_eq!(
            reason_text("oewn", "cannot download oewn: the server answered HTTP 404"),
            "The server answered HTTP 404"
        );
        assert_eq!(
            reason_text("oewn", "the index of omw-es is broken"),
            "The index of omw-es is broken"
        );
        assert_eq!(
            late_text(Duration::from_secs(30 * 60)),
            "it did not finish within 30 minutes"
        );
        for text in [
            TITLE,
            HEADING,
            EYEBROW,
            INSTALLING_HEADING,
            READY_HEADING,
            PARTIAL_HEADING,
            FAILED_HEADING,
            BODY,
            INSTALLING_BODY,
            AFTER_FAILURE,
            START,
            READY_BODY,
            FAILED,
            CANCEL_ALL,
            DOWNLOAD,
            NOT_NOW,
            CLOSE,
            CANCEL,
            WAITING,
            INSTALLED,
            CANCELLED,
            NO_PLACE,
            STOPPED,
            &late_text(Duration::from_secs(60)),
        ] {
            assert!(text.is_ascii(), "{text}");
            assert!(!text.trim().is_empty());
        }
        for text in [CREDIT, INSTALLING] {
            assert!(!text.trim().is_empty());
        }
    }

    #[test]
    fn each_row_says_where_its_source_stands() {
        let size = 2_000;
        assert_eq!(RowState::Queued.text(size), WAITING);
        assert_eq!(RowState::Running { received: 500 }.text(size), "25%");
        assert_eq!(RowState::Running { received: size }.text(size), INSTALLING);
        assert_eq!(RowState::Installed.text(size), INSTALLED);
        assert_eq!(RowState::Cancelled.text(size), CANCELLED);
        assert_eq!(
            RowState::Failed("cannot install oewn: x".to_owned()).text(size),
            FAILED
        );
        assert!(RowState::Queued.is_open());
        assert!(RowState::Running { received: 0 }.is_open());
        for ended in [
            RowState::Installed,
            RowState::Failed(String::new()),
            RowState::Cancelled,
        ] {
            assert!(!ended.is_open(), "{ended:?}");
        }
    }

    #[test]
    fn the_queue_runs_one_source_at_a_time_in_order() {
        let mut queue = Queue::new(vec![entry("a", 10), entry("b", 10), entry("c", 10)]);
        assert!(!queue.is_done());
        assert_eq!(queue.running(), None);
        assert_eq!(queue.next().map(|p| p.name), Some("a".to_owned()));
        assert_eq!(queue.running(), Some("a"));
        // Never two at once.
        assert_eq!(queue.next(), None);
        queue.progress("a", 4);
        queue.progress("b", 4);
        assert_eq!(queue.state("a"), Some(&RowState::Running { received: 4 }));
        assert_eq!(queue.state("b"), Some(&RowState::Queued));
        queue.finished("a", Ok(()));
        assert_eq!(queue.state("a"), Some(&RowState::Installed));
        // An end of a source that does not run changes nothing.
        queue.finished("a", Err("late".to_owned()));
        queue.finished("c", Err("not started".to_owned()));
        assert_eq!(queue.state("a"), Some(&RowState::Installed));
        assert_eq!(queue.state("c"), Some(&RowState::Queued));
        assert_eq!(queue.next().map(|p| p.name), Some("b".to_owned()));
        queue.finished("b", Err("the server answered HTTP 404".to_owned()));
        assert_eq!(
            queue.state("b"),
            Some(&RowState::Failed(
                "cannot install b: the server answered HTTP 404".to_owned()
            ))
        );
        assert_eq!(queue.next().map(|p| p.name), Some("c".to_owned()));
        queue.finished("c", Ok(()));
        assert_eq!(queue.next(), None);
        assert!(queue.is_done());
        assert_eq!(summary_text(&queue), "2 of 3 dictionaries are installed.");
    }

    #[test]
    fn a_cancel_ends_only_its_source_and_cancel_all_ends_the_rest() {
        let mut queue = Queue::new(vec![
            entry("a", 10),
            entry("b", 10),
            entry("c", 10),
            entry("d", 10),
        ]);
        // A queued source: cancelled, never started.
        assert!(!queue.cancel("b"));
        assert_eq!(queue.state("b"), Some(&RowState::Cancelled));
        assert_eq!(queue.next().map(|p| p.name), Some("a".to_owned()));
        // The running one: the caller drops its job.
        assert!(queue.cancel("a"));
        assert_eq!(queue.state("a"), Some(&RowState::Cancelled));
        // Its late end changes nothing.
        queue.finished("a", Ok(()));
        assert_eq!(queue.state("a"), Some(&RowState::Cancelled));
        // An ended or unknown source cannot be cancelled.
        assert!(!queue.cancel("a"));
        assert!(!queue.cancel("tessdata"));
        assert_eq!(queue.next().map(|p| p.name), Some("c".to_owned()));
        assert!(queue.cancel_all());
        for name in ["a", "b", "c", "d"] {
            assert_eq!(queue.state(name), Some(&RowState::Cancelled), "{name}");
        }
        assert!(queue.is_done());
        assert_eq!(queue.next(), None);
        assert!(!queue.cancel_all());
        assert_eq!(
            summary_text(&queue),
            "No dictionary was installed. The download is offered again at the next start."
        );
        // Nothing came: the heading says the download failed.
        assert_eq!(ended_heading(&queue), FAILED_HEADING);
        assert!(!is_complete(&queue));
        let mut one = Queue::new(vec![entry("a", 10)]);
        one.next();
        one.finished("a", Ok(()));
        assert_eq!(summary_text(&one), "The dictionary is installed.");
        assert_eq!(ended_heading(&one), READY_HEADING);
        assert!(is_complete(&one));
        let mut two = Queue::new(vec![entry("a", 10), entry("b", 10)]);
        while let Some(published) = two.next() {
            two.finished(&published.name, Ok(()));
        }
        assert_eq!(summary_text(&two), "All 2 dictionaries are installed.");
        // The card over the list: the bytes of what came, how many of each.
        let mut three = Queue::new(vec![
            entry("a", 2_000_000),
            entry("b", 4_000_000),
            entry("c", 4_000_000),
        ]);
        three.next();
        three.finished("a", Ok(()));
        three.next();
        three.progress("b", 1_000_000);
        let now = progress(&three);
        assert_eq!(now.current, "Installing b…");
        assert_eq!(now.bytes, "3 of 10 MB");
        assert_eq!(now.counts, "1 installed · 2 to go");
        assert_eq!(now.step.as_deref(), Some("Installing 2 of 3"));
        assert!((now.fraction - 0.3).abs() < 1e-9);
        three.finished("b", Err("cannot install b: x".to_owned()));
        three.next();
        three.finished("c", Ok(()));
        let end = progress(&three);
        assert_eq!(end.counts, "2 installed · 1 failed");
        assert_eq!(end.step, None);
        assert_eq!(ended_heading(&three), PARTIAL_HEADING);
        assert!(Queue::new(Vec::new()).is_done());
    }

    /// The matrix's four indexes, three of them bad: a 404, a checksum that
    /// the manifest fetched again does not fix, and an index of a newer
    /// format. The fourth is installed and each of the three says why on
    /// its row, naming itself; no temporary file is left.
    #[tokio::test]
    async fn four_indexes_three_bad_install_the_good_one_and_name_the_others() {
        use std::collections::HashMap;

        use addict_dictionary::{
            Index,
            format::{Contents, Headword, SenseRecord, VERSION, encode},
        };

        use crate::download::stub::{Route, Stub};

        fn xz_of(bytes: &[u8]) -> Vec<u8> {
            let mut packed = Vec::new();
            lzma_rs::xz_compress(&mut &bytes[..], &mut packed).expect("compress");
            packed
        }
        let index = encode(&Contents {
            source: "fixture".to_owned(),
            senses: vec![SenseRecord {
                part_of_speech: None,
                definition: "sloping land beside a body of water".to_owned(),
                members: vec!["bank".to_owned()],
                metadata: None,
            }],
            headwords: vec![Headword {
                entries: Vec::new(),
                word: "bank".to_owned(),
                senses: vec![0],
                translations: Vec::new(),
            }],
            forms: Vec::new(),
            analyses: Vec::new(),
        })
        .expect("encode");
        let good = xz_of(&index);
        let mut newer = index.clone();
        newer[8..12].copy_from_slice(&(VERSION + 1).to_le_bytes());
        let newer = xz_of(&newer);

        // A: freedict, no asset. B: omw-es, a wrong checksum. C:
        // sinonims-cat, a newer format inside. D: oewn, good.
        let missing = Published::of("freedict", &good);
        let mut wrong = Published::of("omw-es", &good);
        wrong.sha256 = "0".repeat(64);
        let indexes = vec![
            missing,
            wrong,
            Published::of("sinonims-cat", &newer),
            Published::of("oewn", &good),
        ];
        let manifest = Manifest {
            format: VERSION,
            indexes: indexes.clone(),
        };
        let tag = release::tag();
        let stub = Stub::start(HashMap::from([
            (
                format!("/{tag}/{}", release::MANIFEST_3),
                Route::File(manifest.to_json().into_bytes()),
            ),
            (format!("/{tag}/omw-es.index.xz"), Route::File(good.clone())),
            (format!("/{tag}/sinonims-cat.index.xz"), Route::File(newer)),
            (format!("/{tag}/oewn.index.xz"), Route::File(good)),
        ]))
        .expect("the stub");
        let downloader = Downloader::with_base_url(stub.base_url());
        let dir = TempDir::new("four");
        let data = dir.0.join("addict");

        let mut queue = Queue::new(indexes);
        while let Some(published) = queue.next() {
            let result = downloader.install(&published, &data, |_| {}).await;
            queue.finished(&published.name, result.map(drop).map_err(|e| e.to_string()));
        }

        assert!(queue.is_done());
        assert_eq!(queue.state("oewn"), Some(&RowState::Installed));
        let index = Index::open(&data.join("oewn.index")).expect("oewn opens");
        assert!(index.find("bank").expect("find").is_some());
        let failures: Vec<(&str, &str)> = queue
            .rows()
            .iter()
            .filter_map(|(published, state)| match state {
                RowState::Failed(message) => Some((published.name.as_str(), message.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(failures.len(), 3, "{failures:?}");
        for ((name, message), expected) in
            failures.iter().zip(["freedict", "omw-es", "sinonims-cat"])
        {
            assert_eq!(*name, expected);
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains('\n'), "{message}");
        }
        assert!(failures[0].1.contains("404"), "{}", failures[0].1);
        assert!(failures[1].1.contains("checksum"), "{}", failures[1].1);
        assert!(
            failures[2].1.contains("not a readable index"),
            "{}",
            failures[2].1
        );
        let mut entries: Vec<String> = fs::read_dir(&data)
            .expect("the data directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        entries.sort();
        assert_eq!(entries, ["oewn.index"]);
        // The manifest was asked once more, for the checksum, and only then.
        let manifests = stub
            .seen()
            .iter()
            .filter(|line| line.contains(release::MANIFEST_3))
            .count();
        assert_eq!(manifests, 1, "{:?}", stub.seen());
    }
}
