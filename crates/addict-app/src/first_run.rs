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

/// The heading of the dialog.
pub const HEADING: &str = "No dictionary is installed";

/// What the dialog offers.
pub const BODY: &str = "Addict can download its dictionaries, so that words can be looked up \
                        offline. They come from the releases of Addict on GitHub and are \
                        installed one after another; one that fails does not stop the others.";

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

/// `bytes` in whole megabytes, rounded up, at least one.
fn megabytes(bytes: u64) -> u64 {
    bytes.div_ceil(1_000_000).max(1)
}

/// The size line of the dialog for assets of `bytes` in all.
pub fn size_text(bytes: u64) -> String {
    format!("Download size: {} MB", megabytes(bytes))
}

/// The line of the dialog that says what `indexes` hold: how many
/// dictionaries, and their size together.
pub fn contents_text(indexes: &[Published]) -> String {
    let count = indexes.len();
    let total = indexes.iter().map(|published| published.size).sum();
    let noun = if count == 1 {
        "dictionary"
    } else {
        "dictionaries"
    };
    format!("{count} {noun}. {}", size_text(total))
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

/// The row of a download of `size` bytes of which `received` came.
pub fn downloading_text(received: u64, size: u64) -> String {
    let percent = received.saturating_mul(100).checked_div(size).unwrap_or(0);
    format!("Downloading… {}%", percent.min(100))
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

/// The line of an install that could not start, `error` being why.
pub fn failed_text(error: &str) -> String {
    format!(
        "The download failed: {}. It is offered again at the next start.",
        error.trim_end_matches('.')
    )
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
    /// The status text of its row, for an asset of `size` bytes.
    pub fn text(&self, size: u64) -> String {
        match self {
            Self::Queued => WAITING.to_owned(),
            Self::Running { received } if size > 0 && *received >= size => INSTALLING.to_owned(),
            Self::Running { received } => downloading_text(*received, size),
            Self::Installed => INSTALLED.to_owned(),
            Self::Failed(message) => message.clone(),
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
    status: gtk::Label,
    cancel: gtk::Button,
}

/// The dialog of the offer.
pub struct Offer {
    window: gtk::Window,
    /// The line of the end: empty before.
    status: gtk::Label,
    rows: Vec<Row>,
    download: gtk::Button,
    not_now: gtk::Button,
    cancel: gtk::Button,
    close: gtk::Button,
}

impl Offer {
    /// Builds the dialog, hidden, above `parent`, with a row for each of
    /// `indexes`. `on_download` and `on_not_now` run at a press of their
    /// buttons; Not now also closes the dialog. `on_cancel` runs at a press
    /// of a row's Cancel, with its name, and of the dialog's, with `None`.
    /// Esc and the title bar close it and choose nothing.
    pub fn new(
        parent: &impl IsA<gtk::Window>,
        indexes: &[Published],
        on_download: impl Fn() + 'static,
        on_not_now: impl Fn() + 'static,
        on_cancel: impl Fn(Option<String>) + 'static,
    ) -> Self {
        let on_cancel = std::rc::Rc::new(on_cancel);
        let heading = label(HEADING, &["about-name"]);
        let body = label(BODY, &[]);
        let contents = label(&contents_text(indexes), &["hint"]);
        let credit = label(CREDIT, &["hint"]);
        let status = label("", &[]);
        status.set_visible(false);

        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        let rows: Vec<Row> = indexes
            .iter()
            .map(|published| {
                let name = label(&published.name, &[]);
                name.set_hexpand(true);
                name.set_wrap(false);
                if let Some(credit) = row_credit_text(&published.name) {
                    name.set_tooltip_text(Some(&credit));
                }
                let status = label(&offered_text(published), &["hint"]);
                status.set_halign(gtk::Align::End);
                status.set_xalign(1.0);
                // A failure names its checksums, which have no spaces.
                status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                status.set_width_chars(18);
                status.set_max_width_chars(36);
                let cancel = gtk::Button::builder()
                    .label(CANCEL)
                    .css_classes(["settings-button"])
                    .visible(false)
                    .build();
                let pressed = on_cancel.clone();
                let which = published.name.clone();
                cancel.connect_clicked(move |_| pressed(Some(which.clone())));
                let line = gtk::Box::builder()
                    .orientation(gtk::Orientation::Horizontal)
                    .spacing(12)
                    .build();
                line.append(&name);
                line.append(&status);
                line.append(&cancel);
                list.append(&line);
                Row {
                    name: published.name.clone(),
                    status,
                    cancel,
                }
            })
            .collect();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(320)
            .child(&list)
            .build();

        let not_now = gtk::Button::builder()
            .label(NOT_NOW)
            .css_classes(["settings-button"])
            .build();
        let download = gtk::Button::builder()
            .label(DOWNLOAD)
            .css_classes(["save"])
            .build();
        let cancel = gtk::Button::builder()
            .label(CANCEL)
            .css_classes(["settings-button"])
            .visible(false)
            .build();
        let close = gtk::Button::builder()
            .label(CLOSE)
            .css_classes(["settings-button"])
            .visible(false)
            .build();
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .halign(gtk::Align::End)
            .margin_top(8)
            .build();
        for button in [&not_now, &download, &cancel, &close] {
            buttons.append(button);
        }

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(20)
            .margin_bottom(20)
            .margin_start(20)
            .margin_end(20)
            .build();
        content.append(&heading);
        content.append(&body);
        content.append(&contents);
        content.append(&scrolled);
        content.append(&credit);
        content.append(&status);
        content.append(&buttons);

        let window = gtk::Window::builder()
            .title(TITLE)
            .transient_for(parent)
            .default_width(520)
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
            status,
            rows,
            download,
            not_now,
            cancel,
            close,
        }
    }

    /// Shows the dialog, with Download focused.
    pub fn present(&self) {
        self.window.present();
        self.download.grab_focus();
    }

    /// The installs run: Download and Not now give way to the dialog's
    /// Cancel.
    pub fn installing(&self) {
        self.download.set_visible(false);
        self.not_now.set_visible(false);
        self.cancel.set_visible(true);
        self.cancel.grab_focus();
    }

    /// Every row says where its source stands in `queue`, and has its
    /// Cancel while the source has not ended.
    pub fn show(&self, queue: &Queue) {
        for (published, state) in queue.rows() {
            self.show_row(&published.name, state, published.size);
        }
    }

    /// The row of `name` says `state`, for an asset of `size` bytes.
    pub fn show_row(&self, name: &str, state: &RowState, size: u64) {
        let Some(row) = self.rows.iter().find(|row| row.name == name) else {
            return;
        };
        let text = state.text(size);
        row.status.set_label(&text);
        row.status
            .set_tooltip_text(matches!(state, RowState::Failed(_)).then_some(text.as_str()));
        row.cancel.set_visible(state.is_open());
    }

    /// The installs ended: the line says how, and only Close is left. The
    /// dialog is shown again, as it may have been closed while they ran.
    pub fn ended(&self, line: &str) {
        self.say(line);
        for row in &self.rows {
            row.cancel.set_visible(false);
        }
        self.download.set_visible(false);
        self.not_now.set_visible(false);
        self.cancel.set_visible(false);
        self.close.set_visible(true);
        self.window.present();
        self.close.grab_focus();
    }

    fn say(&self, text: &str) {
        self.status.set_label(text);
        self.status.set_visible(!text.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

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

        use crate::download::stub::{Route, Stub};

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

    #[test]
    fn the_size_is_whole_megabytes_rounded_up() {
        assert_eq!(size_text(5_935_000), "Download size: 6 MB");
        assert_eq!(size_text(12_000_000), "Download size: 12 MB");
        assert_eq!(size_text(12_000_001), "Download size: 13 MB");
        assert_eq!(size_text(1), "Download size: 1 MB");
        assert_eq!(size_text(0), "Download size: 1 MB");
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
            contents_text(&indexes),
            "3 dictionaries. Download size: 46 MB"
        );
        assert_eq!(
            contents_text(&indexes[..1]),
            "1 dictionary. Download size: 6 MB"
        );
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
        assert_eq!(downloading_text(0, 1000), "Downloading… 0%");
        assert_eq!(downloading_text(421, 1000), "Downloading… 42%");
        assert_eq!(downloading_text(1000, 1000), "Downloading… 100%");
        assert_eq!(downloading_text(5, 0), "Downloading… 0%");
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
            late_text(Duration::from_secs(30 * 60)),
            "it did not finish within 30 minutes"
        );
        for text in [
            TITLE,
            HEADING,
            BODY,
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
        assert_eq!(
            RowState::Running { received: 500 }.text(size),
            "Downloading… 25%"
        );
        assert_eq!(RowState::Running { received: size }.text(size), INSTALLING);
        assert_eq!(RowState::Installed.text(size), INSTALLED);
        assert_eq!(RowState::Cancelled.text(size), CANCELLED);
        assert_eq!(
            RowState::Failed("cannot install oewn: x".to_owned()).text(size),
            "cannot install oewn: x"
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
        let mut one = Queue::new(vec![entry("a", 10)]);
        one.next();
        one.finished("a", Ok(()));
        assert_eq!(summary_text(&one), "The dictionary is installed.");
        let mut two = Queue::new(vec![entry("a", 10), entry("b", 10)]);
        while let Some(published) = two.next() {
            two.finished(&published.name, Ok(()));
        }
        assert_eq!(summary_text(&two), "All 2 dictionaries are installed.");
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
        let indexes = vec![
            entry("oewn", 5_935_000),
            entry("freedict", 2_100_000),
            entry("kaikki-es-Espanol", 61_000_000),
            entry("omw-es", 900_000),
        ];
        let offer = Offer::new(&parent, &indexes, || {}, || {}, |_| {});
        offer.present();
        let mut queue = Queue::new(indexes);
        let state = std::env::var("ADDICT_SHOT_STATE");
        if matches!(state.as_deref(), Ok("installing" | "ended")) {
            offer.installing();
            queue.next();
            queue.finished("oewn", Ok(()));
            queue.next();
            queue.finished(
                "freedict",
                Err("cannot download freedict: the server answered HTTP 404".to_owned()),
            );
            queue.next();
            queue.progress("kaikki-es-Espanol", 25_000_000);
            if state.as_deref() == Ok("ended") {
                queue.cancel_all();
                offer.ended(&summary_text(&queue));
            }
            offer.show(&queue);
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
