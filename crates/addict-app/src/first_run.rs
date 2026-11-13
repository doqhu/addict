//! The first-run offer to download the published dictionaries.
//!
//! The offer is due when no index is there, of any language, and the user
//! has not answered it before: the choice file, `first-run-download` in the
//! product's configuration directory ([`addict_deepl::key::config_dir`]),
//! holds `declined` once Not now was pressed. Download writes no choice: an
//! install that fails is offered again at the next start, and one that
//! succeeds leaves an index, so nothing is offered again.
//!
//! The dialog lists every index of the manifest, which is fetched before it
//! shows, with its size. Download installs them one after another, as a
//! [`Queue`] orders them: each row shows its progress and has its own
//! Cancel, and a source that fails says why on its row while the others go
//! on. The dialog's Cancel stops them all. The file is read and written on
//! the runner, never on the UI thread, as the language file is. The texts
//! and the queue are constants, plain functions and plain data, tested
//! without a display.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

use addict_dictionary::{
    release::{self, Manifest, Published},
    sources,
};
use gtk::{glib, prelude::*};

use crate::{
    dictionaries::{Indexes, State},
    download::{DownloadError, Downloader},
    settings::label,
};

/// The name of the choice file inside the configuration directory.
const CHOICE_FILE: &str = "first-run-download";

/// What the choice file holds once Not now was pressed.
pub const DECLINED: &str = "declined";

/// How many bytes of the file are read.
const CHOICE_LIMIT: u64 = 64;

/// How long a read or a write of the file may take on the runner.
pub const FILE_LIMIT: Duration = Duration::from_secs(2);

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

/// The choice file of the product, or `None` when the environment names no
/// configuration directory. Nothing is read here.
pub fn path() -> Option<PathBuf> {
    Some(addict_deepl::key::config_dir()?.join(CHOICE_FILE))
}

/// Whether the file at `path` says that the offer was declined. No path, a
/// missing or unreadable file and anything but `declined`, white space
/// around it aside, is no choice. This reads the file, so it runs on the
/// runner.
pub fn read(path: Option<&Path>) -> bool {
    path.and_then(|path| read_choice(path).ok())
        .is_some_and(|content| content.trim() == DECLINED)
}

/// The first bytes of the regular file at `path`, as text.
fn read_choice(path: &Path) -> io::Result<String> {
    // Before opening: opening a pipe would wait for its writer.
    if !fs::metadata(path)?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut content = String::new();
    File::open(path)?
        .take(CHOICE_LIMIT)
        .read_to_string(&mut content)?;
    Ok(content)
}

/// Writes that the offer was declined to the file at `path`, making its
/// directory when there is none. This writes a file, so it runs on the
/// runner.
pub fn write(path: Option<&Path>) -> io::Result<()> {
    let path = path.ok_or(io::ErrorKind::NotFound)?;
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    fs::write(path, format!("{DECLINED}\n"))
}

/// Whether the offer is due: every index is missing, none merely
/// unreadable, and the user has not `chosen` before.
pub fn should_offer(indexes: &Indexes, chosen: bool) -> bool {
    let Indexes {
        definitions,
        translations,
        spanish,
        catalan,
    } = *indexes;
    !chosen
        && [definitions, translations, spanish, catalan]
            .iter()
            .all(|state| *state == State::Missing)
}

/// The manifest to offer from, when the offer is due for `indexes` and the
/// `chosen` answer: `Ok(None)` when it is not due, which asks nothing of
/// `downloader`, and an error when the manifest cannot be had or lists no
/// index, which shows no dialog and keeps no choice, so the next start asks
/// again.
pub async fn offered_manifest(
    indexes: Indexes,
    chosen: bool,
    downloader: &Downloader,
) -> Result<Option<Manifest>, DownloadError> {
    if !should_offer(&indexes, chosen) {
        return Ok(None);
    }
    let manifest = downloader.manifest().await?;
    if manifest.indexes.is_empty() {
        return Err(DownloadError::Manifest {
            cause: "it lists no index".to_owned(),
        });
    }
    Ok(Some(manifest))
}

/// A recovery offer must not claim that every dictionary is absent.
pub const RECOVERY_HEADING: &str = "Restore missing or unreadable dictionaries";
pub const FETCHING: &str = "Fetching the dictionary list and checking installed files…";
pub const NOTHING: &str = "All published dictionaries are readable. Nothing to download.";
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
        Source::WikdictRuEn => ("Russian", "WikDict, Russian → English"),
        Source::WikdictRuEs => ("Russian", "WikDict, Russian → Spanish"),
        Source::WikdictRuCa => ("Russian", "WikDict, Russian → Catalan"),
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

/// The row of one source in the dialog.
struct Row {
    name: String,
    /// The whole line, tinted while its source runs.
    line: gtk::Box,
    /// The title of its source, or why it failed.
    sub: gtk::Label,
    /// The check of an installed source.
    icon: gtk::Image,
    /// The progress of a running source.
    bar: gtk::ProgressBar,
    status: gtk::Label,
    cancel: gtk::Button,
}

/// A vertical box with `classes`.
fn column(spacing: i32, classes: &[&str]) -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(spacing)
        .css_classes(classes.to_vec())
        .build()
}

/// A horizontal box with `classes`.
fn line(spacing: i32, classes: &[&str]) -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(spacing)
        .css_classes(classes.to_vec())
        .build()
}

/// A label on one line.
fn single(text: &str, classes: &[&str]) -> gtk::Label {
    let single = label(text, classes);
    single.set_wrap(false);
    single
}

/// A round mark holding a check, `classes` giving its size and colours.
fn check(classes: &[&str], pixels: i32) -> gtk::Box {
    let mark = line(0, classes);
    mark.set_halign(gtk::Align::Center);
    mark.set_valign(gtk::Align::Center);
    let image = gtk::Image::from_icon_name("object-select-symbolic");
    image.set_pixel_size(pixels);
    image.set_hexpand(true);
    image.set_halign(gtk::Align::Center);
    mark.append(&image);
    mark
}

/// The card of the three figures of the offer.
fn figures_card(indexes: &[Published]) -> gtk::Box {
    let card = line(0, &["first-run-card", "first-run-figures"]);
    card.set_homogeneous(true);
    for (i, (value, caption)) in figures(indexes).into_iter().enumerate() {
        let cell = column(0, &["figure"]);
        if i > 0 {
            cell.add_css_class("divided");
        }
        cell.append(&single(&value, &["figure-value"]));
        cell.append(&single(caption, &["hint"]));
        card.append(&cell);
    }
    card
}

/// The card of a group once every source is installed.
fn group_card(group: &Group) -> gtk::Box {
    let card = line(12, &["first-run-card", "group-card"]);
    let text = column(1, &[]);
    text.set_hexpand(true);
    text.append(&single(group.name, &["group-name"]));
    text.append(&single(&group_text(&group.indexes), &["hint"]));
    card.append(&text);
    let mark = check(&["group-check"], 14);
    mark.update_property(&[gtk::accessible::Property::Label(INSTALLED)]);
    card.append(&mark);
    card
}

/// The row of `published`, whose Cancel calls `on_cancel` with its name.
fn source_row(published: &Published, on_cancel: &std::rc::Rc<dyn Fn(Option<String>)>) -> Row {
    let line_box = line(12, &["first-run-row"]);
    if let Some(credit) = row_credit_text(&published.name) {
        line_box.set_tooltip_text(Some(&credit));
    }
    let text = column(1, &[]);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&single(&published.name, &["row-name"]));
    let title = describe(&published.name).map_or("", |(_, title)| title);
    let sub = single(title, &["row-sub"]);
    sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
    sub.set_visible(!title.is_empty());
    text.append(&sub);
    line_box.append(&text);

    let icon = gtk::Image::from_icon_name("object-select-symbolic");
    icon.add_css_class("row-check");
    icon.set_visible(false);
    let bar = gtk::ProgressBar::builder()
        .css_classes(["row-bar"])
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    let status = single(&offered_text(published), &["row-status"]);
    status.set_valign(gtk::Align::Center);
    let cancel = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .css_classes(["row-cancel"])
        .tooltip_text(format!("{CANCEL} {}", published.name))
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    cancel.update_property(&[gtk::accessible::Property::Label(&format!(
        "{CANCEL} {}",
        published.name
    ))]);
    let pressed = on_cancel.clone();
    let which = published.name.clone();
    cancel.connect_clicked(move |_| pressed(Some(which.clone())));
    // Cancel keeps its place on every row, so that the states line up.
    let slot = line(0, &["cancel-slot"]);
    slot.set_valign(gtk::Align::Center);
    slot.append(&cancel);
    for widget in [
        icon.upcast_ref::<gtk::Widget>(),
        bar.upcast_ref(),
        status.upcast_ref(),
        slot.upcast_ref(),
    ] {
        line_box.append(widget);
    }
    Row {
        name: published.name.clone(),
        line: line_box,
        sub,
        icon,
        bar,
        status,
        cancel,
    }
}

/// The dialog of the offer.
pub struct Offer {
    window: gtk::Window,
    recovery: bool,
    hero: gtk::Box,
    done_mark: gtk::Box,
    eyebrow: gtk::Label,
    heading: gtk::Label,
    lead: gtk::Label,
    figures: gtk::Box,
    progress: gtk::Box,
    current: gtk::Label,
    bytes: gtk::Label,
    bar: gtk::ProgressBar,
    counts: gtk::Label,
    list: gtk::ScrolledWindow,
    cards: gtk::Grid,
    rows: Vec<Row>,
    note: gtk::Label,
    credit: gtk::Label,
    footer: gtk::Box,
    download: gtk::Button,
    not_now: gtk::Button,
    cancel: gtk::Button,
    close: gtk::Button,
}

impl Offer {
    /// Builds the dialog, hidden, above `parent`, with a row for each of
    /// `indexes`, grouped by language. `on_download` and `on_not_now` run at
    /// a press of their buttons; Not now also closes the dialog. `on_cancel`
    /// runs at a press of a row's Cancel, with its name, and of the
    /// dialog's, with `None`. Esc and the title bar close it and choose
    /// nothing.
    pub fn new(
        parent: &impl IsA<gtk::Window>,
        indexes: &[Published],
        recovery: bool,
        on_download: impl Fn() + 'static,
        on_not_now: impl Fn() + 'static,
        on_cancel: impl Fn(Option<String>) + 'static,
    ) -> Self {
        let on_cancel: std::rc::Rc<dyn Fn(Option<String>)> = std::rc::Rc::new(on_cancel);

        let done_mark = check(&["done-mark"], 26);
        done_mark.set_visible(false);
        let eyebrow = single(
            &eyebrow_text(recovery, indexes.len()),
            &["section-label", "eyebrow", "warn"],
        );
        let heading = label(
            if recovery { RECOVERY_HEADING } else { HEADING },
            &["first-run-heading"],
        );
        let lead = label(BODY, &["first-run-lead"]);
        let hero = column(8, &[]);
        for widget in [
            done_mark.upcast_ref::<gtk::Widget>(),
            eyebrow.upcast_ref(),
            heading.upcast_ref(),
            lead.upcast_ref(),
        ] {
            hero.append(widget);
        }

        let figures = figures_card(indexes);

        let current = single(INSTALLING, &["strong"]);
        current.set_hexpand(true);
        current.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let bytes = single("", &["hint", "figures-tabular"]);
        let top = line(12, &[]);
        top.append(&current);
        top.append(&bytes);
        let bar = gtk::ProgressBar::builder()
            .css_classes(["first-run-bar"])
            .build();
        let counts = single("", &["hint", "figures-tabular"]);
        let progress = column(10, &["first-run-card", "first-run-progress"]);
        progress.append(&top);
        progress.append(&bar);
        progress.append(&counts);
        progress.set_visible(false);

        let groups = groups(indexes);
        let list_box = column(1, &["first-run-groups"]);
        let mut rows = Vec::new();
        for group in &groups {
            let head = line(12, &["group-head"]);
            let name = single(group.name, &["section-label"]);
            name.set_hexpand(true);
            head.append(&name);
            head.append(&single(
                &group_text(&group.indexes),
                &["hint", "group-size"],
            ));
            list_box.append(&head);
            for published in &group.indexes {
                let row = source_row(published, &on_cancel);
                list_box.append(&row.line);
                rows.push(row);
            }
        }
        let list = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(340)
            .css_classes(["first-run-list"])
            .overflow(gtk::Overflow::Hidden)
            .child(&list_box)
            .build();

        let cards = gtk::Grid::builder()
            .column_spacing(10)
            .row_spacing(10)
            .column_homogeneous(true)
            .visible(false)
            .build();
        for (i, group) in groups.iter().enumerate() {
            let i = i32::try_from(i).unwrap_or(i32::MAX);
            cards.attach(&group_card(group), i % 2, i / 2, 1, 1);
        }

        let note = label(CREDIT, &["hint"]);
        note.set_hexpand(true);
        let credit = label(CREDIT, &["hint"]);
        credit.set_justify(gtk::Justification::Center);
        credit.set_halign(gtk::Align::Center);
        credit.set_xalign(0.5);
        credit.set_visible(false);

        let not_now = gtk::Button::builder()
            .label(if recovery { CLOSE } else { NOT_NOW })
            .css_classes(["first-run-button"])
            .build();
        let download_content = line(8, &[]);
        download_content.append(&gtk::Image::from_icon_name("folder-download-symbolic"));
        download_content.append(&gtk::Label::new(Some(DOWNLOAD)));
        let download = gtk::Button::builder()
            .child(&download_content)
            .css_classes(["save"])
            .build();
        download.update_property(&[gtk::accessible::Property::Label(DOWNLOAD)]);
        let cancel = gtk::Button::builder()
            .label(CANCEL_ALL)
            .css_classes(["first-run-button"])
            .visible(false)
            .build();
        let close = gtk::Button::builder()
            .label(CLOSE)
            .css_classes(["save"])
            .visible(false)
            .build();
        let buttons = line(10, &[]);
        buttons.set_valign(gtk::Align::Center);
        for button in [&not_now, &download, &cancel, &close] {
            buttons.append(button);
        }
        let footer = line(16, &[]);
        footer.append(&note);
        footer.append(&buttons);

        let content = column(18, &[]);
        content.set_margin_top(26);
        content.set_margin_bottom(24);
        content.set_margin_start(28);
        content.set_margin_end(28);
        for widget in [
            hero.upcast_ref::<gtk::Widget>(),
            figures.upcast_ref(),
            progress.upcast_ref(),
            list.upcast_ref(),
            cards.upcast_ref(),
            credit.upcast_ref(),
            footer.upcast_ref(),
        ] {
            content.append(widget);
        }

        let window = gtk::Window::builder()
            .title(TITLE)
            .transient_for(parent)
            .default_width(560)
            .resizable(false)
            .hide_on_close(true)
            .titlebar(&crate::resident::header_bar(TITLE))
            .child(&content)
            .css_classes(["addict", "settings", "first-run"])
            .build();
        let escape = gtk::EventControllerKey::new();
        let esc = window.downgrade();
        escape.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                if let Some(window) = esc.upgrade() {
                    window.close();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(escape);

        download.connect_clicked(move |_| on_download());
        let closing = window.downgrade();
        not_now.connect_clicked(move |_| {
            on_not_now();
            if let Some(window) = closing.upgrade() {
                window.close();
            }
        });
        cancel.connect_clicked(move |_| on_cancel(None));
        let closing = window.downgrade();
        close.connect_clicked(move |_| {
            if let Some(window) = closing.upgrade() {
                window.close();
            }
        });
        Self {
            window,
            recovery,
            hero,
            done_mark,
            eyebrow,
            heading,
            lead,
            figures,
            progress,
            current,
            bytes,
            bar,
            counts,
            list,
            cards,
            rows,
            note,
            credit,
            footer,
            download,
            not_now,
            cancel,
            close,
        }
    }

    #[cfg(test)]
    pub(crate) fn window(&self) -> &gtk::Window {
        &self.window
    }

    /// Shows the dialog, with Download focused.
    pub fn present(&self) {
        self.window.present();
        self.download.grab_focus();
    }

    /// The installs run: the figures give way to the progress card, and
    /// Download and Not now to Cancel all.
    pub fn installing(&self) {
        self.eyebrow.remove_css_class("warn");
        self.eyebrow.add_css_class("accent");
        self.heading.set_label(INSTALLING_HEADING);
        self.lead.set_label(INSTALLING_BODY);
        self.figures.set_visible(false);
        self.progress.set_visible(true);
        self.note
            .set_label(if self.recovery { RETRY } else { AFTER_FAILURE });
        self.download.set_visible(false);
        self.not_now.set_visible(false);
        self.cancel.set_visible(true);
        self.cancel.grab_focus();
    }

    /// Every row says where its source stands in `queue`, and the card
    /// above them how the whole stands.
    pub fn show(&self, queue: &Queue) {
        for (published, state) in queue.rows() {
            self.show_row(&published.name, state, published.size);
        }
        let progress = progress(queue);
        self.current.set_label(&progress.current);
        self.bytes.set_label(&progress.bytes);
        self.bar.set_fraction(progress.fraction);
        self.counts.set_label(&progress.counts);
        if let Some(step) = &progress.step {
            self.eyebrow.set_label(step);
        }
    }

    /// The row of `name` says `state`, for an asset of `size` bytes. Its
    /// Cancel is there while the source has not ended.
    pub fn show_row(&self, name: &str, state: &RowState, size: u64) {
        let Some(row) = self.rows.iter().find(|row| row.name == name) else {
            return;
        };
        row.line.remove_css_class("running");
        row.status.set_css_classes(&["row-status"]);
        row.icon.set_visible(false);
        row.bar.set_visible(false);
        row.status.set_label(&state.text(size));
        match state {
            RowState::Running { received } => {
                row.line.add_css_class("running");
                row.status.add_css_class("running");
                row.bar.set_visible(true);
                row.bar
                    .set_fraction(percent(*received, size) as f64 / 100.0);
            }
            RowState::Installed => {
                row.icon.set_visible(true);
                row.status.add_css_class("good");
            }
            RowState::Failed(message) => {
                row.status.add_css_class("failed");
                row.sub.set_label(&reason_text(name, message));
                row.sub.add_css_class("failed");
                row.sub.set_tooltip_text(Some(message));
                row.sub.set_visible(true);
            }
            RowState::Queued | RowState::Cancelled => {}
        }
        row.cancel.set_visible(state.is_open());
    }

    /// The installs ended: the heading says how, `line` under it says what
    /// came, and only Close is left. `complete`, every source installed,
    /// trades the list for a card per language. The dialog is shown
    /// again, as it may have been closed while they ran.
    pub fn ended(&self, heading: &str, line: &str, complete: bool) {
        self.heading.set_label(heading);
        if complete && !self.recovery {
            self.lead.set_label(&format!("{line} {READY_BODY}"));
        } else {
            self.lead.set_label(line);
        }
        self.eyebrow.set_visible(false);
        self.figures.set_visible(false);
        self.progress.set_visible(false);
        for row in &self.rows {
            row.cancel.set_visible(false);
        }
        if complete {
            self.done_mark.set_visible(true);
            self.hero.set_halign(gtk::Align::Center);
            for text in [&self.heading, &self.lead] {
                text.set_halign(gtk::Align::Center);
                text.set_justify(gtk::Justification::Center);
                text.set_xalign(0.5);
            }
            self.list.set_visible(false);
            self.cards.set_visible(true);
            self.note.set_visible(false);
            self.credit.set_visible(true);
            self.footer.set_halign(gtk::Align::Center);
            if !self.recovery {
                self.close.set_label(START);
            }
        } else {
            self.note.set_label(CREDIT);
        }
        self.download.set_visible(false);
        self.not_now.set_visible(false);
        self.cancel.set_visible(false);
        self.close.set_visible(true);
        self.window.present();
        self.close.grab_focus();
    }
}

impl Drop for Offer {
    fn drop(&mut self) {
        self.window.destroy();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use addict_dictionary::Index;

    use super::*;
    use crate::download::recovery_manifest;

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

        fn file(&self) -> PathBuf {
            self.0.join("addict").join(CHOICE_FILE)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const MISSING: Indexes = Indexes {
        definitions: State::Missing,
        translations: State::Missing,
        spanish: State::Missing,
        catalan: State::Missing,
    };

    /// A missing or garbled manifest gives an error, so no dialog shows and
    /// nothing is kept; with an index or a choice, the release is not asked.
    #[tokio::test]
    async fn the_offer_needs_the_manifest_and_asks_it_only_when_due() {
        use std::collections::HashMap;

        use crate::stub::{Route, Stub};

        let manifest_path = format!("/{}/manifest.json", addict_dictionary::release::tag());
        let empty = Stub::start(HashMap::new()).expect("the stub");
        let downloader = Downloader::with_base_url(empty.base_url());
        let error = offered_manifest(MISSING, false, &downloader)
            .await
            .expect_err("no manifest");
        assert!(
            matches!(error, DownloadError::Status { status: 404, .. }),
            "{error:?}"
        );

        let garbled = Stub::start(HashMap::from([(
            manifest_path.clone(),
            Route::File(b"<html>Not Found</html>".to_vec()),
        )]))
        .expect("the stub");
        let downloader = Downloader::with_base_url(garbled.base_url());
        let error = offered_manifest(MISSING, false, &downloader)
            .await
            .expect_err("garbled");
        assert!(matches!(error, DownloadError::Manifest { .. }), "{error:?}");

        // A manifest that lists no index offers nothing either.
        let nothing = Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: Vec::new(),
        };
        let empty_list = Stub::start(HashMap::from([(
            manifest_path.clone(),
            Route::File(nothing.to_json().into_bytes()),
        )]))
        .expect("the stub");
        let downloader = Downloader::with_base_url(empty_list.base_url());
        let error = offered_manifest(MISSING, false, &downloader)
            .await
            .expect_err("no index");
        assert!(matches!(error, DownloadError::Manifest { .. }), "{error:?}");
        assert!(error.to_string().contains("no index"), "{error}");

        let published = Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: vec![addict_dictionary::release::Published::of("oewn", b"xz")],
        };
        let good = Stub::start(HashMap::from([(
            manifest_path.clone(),
            Route::File(published.to_json().into_bytes()),
        )]))
        .expect("the stub");
        let downloader = Downloader::with_base_url(good.base_url());
        let due = offered_manifest(MISSING, false, &downloader).await;
        assert_eq!(due.expect("due"), Some(published));
        assert_eq!(good.seen().len(), 1);
        let installed = Indexes {
            definitions: State::Open,
            ..MISSING
        };
        for (indexes, chosen) in [(MISSING, true), (installed, false)] {
            let due = offered_manifest(indexes, chosen, &downloader).await;
            assert_eq!(due.expect("not due"), None);
        }
        assert_eq!(good.seen().len(), 1, "the release is asked only when due");
    }

    fn recovery_index(edition: &str) -> Vec<u8> {
        use addict_dictionary::format::{self, AnalysedForm, Contents, Headword, SenseRecord};
        format::encode(&Contents {
            source: edition.to_owned(),
            senses: vec![SenseRecord {
                definition: "a bank".to_owned(),
                members: vec!["bank".to_owned()],
                ..Default::default()
            }],
            headwords: vec![Headword {
                word: "bank".to_owned(),
                senses: vec![0],
                ..Default::default()
            }],
            analyses: vec![AnalysedForm {
                form: "banking".to_owned(),
                lemmas: vec![addict_dictionary::analysis::Analyses {
                    lemma: "bank".to_owned(),
                    shapes: vec![addict_dictionary::analysis::Shape::Gerund],
                }],
            }],
            ..Default::default()
        })
        .unwrap()
    }

    fn recovery_stub(indexes: Vec<Published>, asset: &[u8]) -> crate::stub::Stub {
        use crate::stub::{Route, Stub};
        let tag = release::tag();
        let manifest = Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: indexes.clone(),
        };
        let mut routes = std::collections::HashMap::from([(
            format!("/{tag}/manifest.json"),
            Route::File(manifest.to_json().into_bytes()),
        )]);
        for published in indexes {
            routes.insert(
                format!("/{tag}/{}", published.asset()),
                Route::File(asset.to_vec()),
            );
        }
        Stub::start(routes).unwrap()
    }

    fn recovery_xz(bytes: &[u8]) -> Vec<u8> {
        let mut xz = Vec::new();
        lzma_rs::xz_compress(&mut &bytes[..], &mut xz).unwrap();
        xz
    }

    #[tokio::test]
    async fn settings_restores_only_a_deleted_index_and_preserves_healthy_editions_and_choice() {
        let dir = TempDir::new("recovery-deleted");
        let data = dir.0.join("data");
        fs::create_dir(&data).unwrap();
        let manual = recovery_index("manual-edition");
        let published = recovery_index("published-edition");
        let xz = recovery_xz(&published);
        for name in ["oewn", "freedict", "wikdict-ru-es"] {
            fs::write(data.join(format!("{name}.index")), &manual).unwrap();
        }
        fs::remove_file(data.join("freedict.index")).unwrap();
        write(Some(&dir.file())).unwrap();
        let choice = fs::read(dir.file()).unwrap();
        let entries = ["oewn", "freedict", "wikdict-ru-es"]
            .into_iter()
            .map(|name| Published::of(name, &xz))
            .collect();
        let stub = recovery_stub(entries, &xz);
        let downloader = Downloader::with_base_url(stub.base_url());
        let manifest = recovery_manifest(&downloader, data.clone()).await.unwrap();
        assert_eq!(
            manifest
                .indexes
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["freedict"]
        );
        let mut queue = Queue::new(manifest.indexes);
        while let Some(published) = queue.next() {
            queue.finished(
                &published.name,
                downloader
                    .install(&published, &data, |_| {})
                    .await
                    .map(drop)
                    .map_err(|error| error.to_string()),
            );
        }
        assert_eq!(fs::read(data.join("oewn.index")).unwrap(), manual);
        assert_eq!(fs::read(data.join("wikdict-ru-es.index")).unwrap(), manual);
        assert_eq!(fs::read(data.join("freedict.index")).unwrap(), published);
        assert_eq!(fs::read(dir.file()).unwrap(), choice);
        let complete = recovery_manifest(&downloader, data).await.unwrap();
        assert!(complete.indexes.is_empty());
        assert_eq!(
            stub.seen()
                .iter()
                .filter(|line| line.contains(".index.xz"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn settings_selects_each_unreadable_source_and_retains_unknown_names_in_order() {
        use addict_dictionary::format::{self, Header, SectionId};
        let dir = TempDir::new("recovery-unreadable");
        let good = recovery_index("manual");
        let xz = recovery_xz(&good);
        let mut names = Vec::new();
        for (name, section) in [
            ("oewn", SectionId::WordRecords),
            ("freedict", SectionId::Senses),
            ("wikdict-ru-es", SectionId::AnalysisRecords),
        ] {
            let mut bad = good.clone();
            let header = Header::parse(&good, good.len() as u64).unwrap();
            let offset = header.section(section).offset as usize
                + if section == SectionId::Senses { 4 } else { 0 };
            bad[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            let path = dir.0.join(format!("{name}.index"));
            fs::write(&path, &bad).unwrap();
            let index = Index::open(&path).expect("header-valid corruption");
            let result = if section == SectionId::AnalysisRecords {
                index.analyses("banking").map(drop)
            } else {
                index.find("bank").map(drop)
            };
            assert!(result.is_err(), "a real lookup must expose the damage");
            names.push(name);
        }
        let mut unsupported = good.clone();
        unsupported[8..12].copy_from_slice(&(format::VERSION + 1).to_le_bytes());
        fs::write(dir.0.join("omw-es.index"), &unsupported).unwrap();
        fs::write(dir.0.join("sinonims-cat.index"), &good).unwrap();
        names.extend(["omw-es", "unknown", "sinonims-cat"]);
        let stub = recovery_stub(
            names
                .into_iter()
                .map(|name| Published::of(name, &xz))
                .collect(),
            &xz,
        );
        let downloader = Downloader::with_base_url(stub.base_url());
        let manifest = recovery_manifest(&downloader, dir.0.clone()).await.unwrap();
        assert_eq!(
            manifest
                .indexes
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["oewn", "freedict", "wikdict-ru-es", "omw-es", "unknown"]
        );
        // A failed replacement keeps the original damaged bytes.
        let original = fs::read(dir.0.join("oewn.index")).unwrap();
        let mut wrong = manifest.indexes[0].clone();
        wrong.size = 0;
        assert!(downloader.install(&wrong, &dir.0, |_| {}).await.is_err());
        assert_eq!(fs::read(dir.0.join("oewn.index")).unwrap(), original);

        let healthy = fs::read(dir.0.join("sinonims-cat.index")).unwrap();
        let recovering = Downloader::with_base_url(stub.base_url()).recovering();
        let mut queue = Queue::new(manifest.indexes);
        while let Some(published) = queue.next() {
            let result = recovering.install(&published, &dir.0, |_| {}).await;
            queue.finished(
                &published.name,
                result.map(drop).map_err(|error| error.to_string()),
            );
        }
        for name in ["oewn", "freedict", "wikdict-ru-es", "omw-es"] {
            assert_eq!(queue.state(name), Some(&RowState::Installed), "{name}");
            let path = dir.0.join(format!("{name}.index"));
            let restored = Index::open(&path).expect("repaired index opens");
            restored.validate().expect("all repaired records decode");
            assert!(restored.find("bank").unwrap().is_some(), "{name}");
            assert!(!restored.analyses("banking").unwrap().is_empty(), "{name}");
            assert_eq!(
                fs::read(path).unwrap(),
                good,
                "replacement installed the checked asset"
            );
        }
        let Some(RowState::Failed(error)) = queue.state("unknown") else {
            panic!("unknown source must fail on its own row");
        };
        assert!(
            error.contains("unknown") && error.contains("does not know"),
            "{error}"
        );
        assert_eq!(fs::read(dir.0.join("sinonims-cat.index")).unwrap(), healthy);
        assert!(!stub.seen().iter().any(
            |line| line.contains("sinonims-cat.index.xz") || line.contains("unknown.index.xz")
        ));
    }

    #[tokio::test]
    async fn settings_manifest_failures_leave_installed_files_unchanged_and_can_retry() {
        use crate::stub::{Route, Stub};
        let dir = TempDir::new("recovery-manifests");
        let good = recovery_index("manual");
        fs::write(dir.0.join("oewn.index"), &good).unwrap();
        for body in [
            None,
            Some(b"garbled".to_vec()),
            Some(
                Manifest {
                    format: addict_dictionary::format::VERSION,
                    indexes: Vec::new(),
                }
                .to_json()
                .into_bytes(),
            ),
        ] {
            let routes = body
                .map(|body| {
                    (
                        format!("/{}/manifest.json", release::tag()),
                        Route::File(body),
                    )
                })
                .into_iter()
                .collect();
            let stub = Stub::start(routes).unwrap();
            let downloader = Downloader::with_base_url(stub.base_url());
            for _ in 0..2 {
                assert!(recovery_manifest(&downloader, dir.0.clone()).await.is_err());
                assert_eq!(fs::read(dir.0.join("oewn.index")).unwrap(), good);
            }
            assert_eq!(stub.seen().len(), 2, "retry fetches afresh");
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

    #[test]
    fn the_offer_is_due_only_with_every_index_missing_and_no_choice() {
        assert!(should_offer(&MISSING, false));
        assert!(!should_offer(&MISSING, true));
        for state in [State::Open, State::Unreadable] {
            for indexes in [
                Indexes {
                    definitions: state,
                    ..MISSING
                },
                Indexes {
                    translations: state,
                    ..MISSING
                },
                Indexes {
                    spanish: state,
                    ..MISSING
                },
                Indexes {
                    catalan: state,
                    ..MISSING
                },
            ] {
                assert!(!should_offer(&indexes, false), "{indexes:?}");
            }
        }
        assert!(!should_offer(&Indexes::OPEN, false));
    }

    #[test]
    fn a_declined_offer_is_read_back() {
        let dir = TempDir::new("round");
        let file = dir.file();
        assert!(!read(Some(&file)));
        write(Some(&file)).expect("written");
        assert_eq!(fs::read_to_string(&file).expect("the file"), "declined\n");
        assert!(read(Some(&file)));
    }

    #[test]
    fn a_missing_unreadable_or_other_file_is_no_choice() {
        let dir = TempDir::new("bad");
        let file = dir.file();
        assert!(!read(None));
        assert!(!read(Some(&file)));
        fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        for content in ["", "accepted\n", "DECLINED", "declined later", "\u{0}"] {
            fs::write(&file, content).expect("write");
            assert!(!read(Some(&file)), "{content:?}");
        }
        fs::write(&file, "  declined \n").expect("write");
        assert!(read(Some(&file)));
        fs::remove_file(&file).expect("remove");
        fs::create_dir(&file).expect("a directory");
        assert!(!read(Some(&file)));
    }

    #[test]
    fn a_write_without_a_place_fails() {
        assert!(write(None).is_err());
        let dir = TempDir::new("blocked");
        fs::write(dir.0.join("addict"), "").expect("a file");
        assert!(write(Some(&dir.file())).is_err());
        assert!(!read(Some(&dir.file())));
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

        use crate::stub::{Route, Stub};

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
                format!("/{tag}/manifest.json"),
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
            .filter(|line| line.contains("manifest.json"))
            .count();
        assert_eq!(manifests, 1, "{:?}", stub.seen());
    }

    /// Draws the dialog into the PNG file `ADDICT_SHOT` names: as offered,
    /// or, with `ADDICT_SHOT_STATE=installing` or `ended`, in that state.
    /// Run by hand, and alone, as `draws_the_window_into_a_png` is;
    /// `ADDICT_SHOT_SCHEME=dark` or `light` draws it in that scheme.
    #[test]
    #[ignore = "opens a window; run alone with ADDICT_SHOT=<file.png>"]
    fn draws_the_offer_into_a_png() {
        let path = std::env::var("ADDICT_SHOT").expect("ADDICT_SHOT");
        gtk::init().expect("a display");
        crate::style::install();
        let scheme = match std::env::var("ADDICT_SHOT_SCHEME").as_deref() {
            Ok("dark") => Some(gtk::InterfaceColorScheme::Dark),
            Ok("light") => Some(gtk::InterfaceColorScheme::Light),
            _ => None,
        };
        if let Some(scheme) = scheme {
            gtk::Settings::default()
                .expect("settings")
                .set_gtk_interface_color_scheme(scheme);
        }
        let parent = gtk::Window::new();
        let indexes: Vec<Published> = [
            ("oewn", 5_936_412),
            ("freedict", 4_477_832),
            ("wikdict-es-ru", 455_188),
            ("kaikki-es-Espanol", 31_450_328),
            ("omw-es", 2_990_976),
            ("kaikki-en-Spanish", 24_882_036),
            ("wikdict-ca-ru", 283_076),
            ("dbnary-ca", 12_662_468),
            ("sinonims-cat", 1_466_508),
            ("kaikki-en-Catalan", 6_575_224),
            ("wikdict-ru-en", 1_474_032),
            ("wikdict-ru-es", 511_408),
            ("wikdict-ru-ca", 361_780),
        ]
        .into_iter()
        .map(|(name, size)| entry(name, size))
        .collect();
        let offer = Offer::new(&parent, &indexes, false, || {}, || {}, |_| {});
        offer.present();
        let mut queue = Queue::new(indexes);
        let state = std::env::var("ADDICT_SHOT_STATE");
        if state.as_deref() == Ok("complete") {
            offer.installing();
            while let Some(published) = queue.next() {
                queue.finished(&published.name, Ok(()));
            }
            offer.show(&queue);
            offer.ended(ended_heading(&queue), &summary_text(&queue), true);
        } else if matches!(state.as_deref(), Ok("installing" | "ended")) {
            offer.installing();
            queue.next();
            queue.finished("oewn", Ok(()));
            queue.next();
            queue.finished("freedict", Ok(()));
            queue.next();
            queue.finished(
                "wikdict-es-ru",
                Err("cannot download wikdict-es-ru: the server answered HTTP 503".to_owned()),
            );
            queue.next();
            queue.progress("kaikki-es-Espanol", 20_128_210);
            if state.as_deref() == Ok("ended") {
                queue.cancel_all();
            }
            offer.show(&queue);
            if state.as_deref() == Ok("ended") {
                offer.ended(ended_heading(&queue), &summary_text(&queue), false);
            }
        }
        let context = glib::MainContext::default();
        let until = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        while std::time::Instant::now() < until {
            context.iteration(false);
        }
        let window = &offer.window;
        let (width, height) = (window.width(), window.height());
        let paintable = gtk::WidgetPaintable::new(Some(window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
        let node = snapshot.to_node().expect("something drawn");
        let renderer = window
            .native()
            .expect("a native")
            .renderer()
            .expect("a renderer");
        let texture = renderer.render_texture(&node, None);
        texture.save_to_png(&path).expect("the PNG");
        window.destroy();
        parent.destroy();
    }
}
