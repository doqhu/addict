//! The first-run offer to download the English dictionary.
//!
//! The offer is due when no index is there, of any language, and the user
//! has not answered it before: the choice file, `first-run-download` in the
//! product's configuration directory ([`addict_deepl::key::config_dir`]),
//! holds `declined` once Not now was pressed. Download writes no choice: an
//! install that fails is offered again at the next start, and one that
//! succeeds leaves an index, so nothing is offered again.
//!
//! The dialog states the download size from the manifest, which is fetched
//! before it shows; Download turns it into a line that says the install
//! runs, then that it ended or why it failed. The file is read and written on
//! the runner, never on the UI thread, as the language file is. The texts are
//! constants and plain functions, tested without a display.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

use addict_dictionary::{import::Source, release::Manifest, sources};
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

/// The index the offer downloads.
pub const OFFERED: Source = Source::Oewn;

/// The title of the dialog.
pub const TITLE: &str = "Download a dictionary";

/// The heading of the dialog.
pub const HEADING: &str = "No dictionary is installed";

/// What the dialog offers.
pub const BODY: &str = "Addict can download the English dictionary, Open English WordNet, \
                        so that English words can be looked up offline. It comes from the \
                        releases of Addict on GitHub.";

/// The button that downloads.
pub const DOWNLOAD: &str = "Download";

/// The button that declines, for good.
pub const NOT_NOW: &str = "Not now";

/// The button that closes the dialog once the install ended.
pub const CLOSE: &str = "Close";

/// The line while the install runs.
pub const INSTALLING: &str = "Installing…";

/// The line once the index is installed.
pub const INSTALLED: &str = "The English dictionary is installed.";

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
/// `downloader`, and an error when the manifest cannot be had, which shows
/// no dialog and keeps no choice, so the next start asks again.
pub async fn offered_manifest(
    indexes: Indexes,
    chosen: bool,
    downloader: &Downloader,
) -> Result<Option<Manifest>, DownloadError> {
    if !should_offer(&indexes, chosen) {
        return Ok(None);
    }
    downloader.manifest().await.map(Some)
}

/// The size line of the dialog for an asset of `bytes`, in whole megabytes,
/// at least one.
pub fn size_text(bytes: u64) -> String {
    let megabytes = bytes.div_ceil(1_000_000).max(1);
    format!("Download size: {megabytes} MB")
}

/// The credit of the offered dictionary, from the table of sources.
pub fn credit_text() -> String {
    let row = sources::row(OFFERED);
    format!("{}. License: {}.", row.credit, row.license)
}

/// The line of an install that failed, `error` being why.
pub fn failed_text(error: &str) -> String {
    format!(
        "The download failed: {}. It is offered again at the next start.",
        error.trim_end_matches('.')
    )
}

/// The dialog of the offer.
pub struct Offer {
    window: gtk::Window,
    /// The line of the install: empty before Download.
    status: gtk::Label,
    download: gtk::Button,
    not_now: gtk::Button,
    close: gtk::Button,
}

impl Offer {
    /// Builds the dialog, hidden, above `parent`, for an asset of `size`
    /// bytes. `on_download` and `on_not_now` run at a press of their
    /// buttons; Not now also closes the dialog. Esc and the title bar close
    /// it and choose nothing.
    pub fn new(
        parent: &impl IsA<gtk::Window>,
        size: u64,
        on_download: impl Fn() + 'static,
        on_not_now: impl Fn() + 'static,
    ) -> Self {
        let heading = label(HEADING, &["about-name"]);
        let body = label(BODY, &[]);
        let size = label(&size_text(size), &["hint"]);
        let credit = label(&credit_text(), &["hint"]);
        let status = label("", &[]);
        status.set_visible(false);

        let not_now = gtk::Button::builder()
            .label(NOT_NOW)
            .css_classes(["settings-button"])
            .build();
        let download = gtk::Button::builder()
            .label(DOWNLOAD)
            .css_classes(["save"])
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
        buttons.append(&not_now);
        buttons.append(&download);
        buttons.append(&close);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(20)
            .margin_bottom(20)
            .margin_start(20)
            .margin_end(20)
            .build();
        for widget in [&heading, &body, &size, &credit, &status] {
            content.append(widget);
        }
        content.append(&buttons);

        let window = gtk::Window::builder()
            .title(TITLE)
            .transient_for(parent)
            .default_width(460)
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
        let closing = window.downgrade();
        close.connect_clicked(move |_| {
            if let Some(window) = closing.upgrade() {
                window.close();
            }
        });
        Self {
            window,
            status,
            download,
            not_now,
            close,
        }
    }

    /// Shows the dialog, with Download focused.
    pub fn present(&self) {
        self.window.present();
        self.download.grab_focus();
    }

    /// The install runs: the line says so and no button but the title
    /// bar's can be pressed.
    pub fn installing(&self) {
        self.say(INSTALLING);
        self.download.set_sensitive(false);
        self.not_now.set_sensitive(false);
    }

    /// The install ended, with `error` when it failed: the line says how,
    /// and only Close is left. The dialog is shown again, as it may have
    /// been closed while the install ran.
    pub fn ended(&self, error: Option<&str>) {
        match error {
            None => self.say(INSTALLED),
            Some(error) => self.say(&failed_text(error)),
        }
        self.download.set_visible(false);
        self.not_now.set_visible(false);
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

        const MANIFEST: &str = "/indexes-v2/manifest.json";
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
            MANIFEST.to_owned(),
            Route::File(b"<html>Not Found</html>".to_vec()),
        )]))
        .expect("the stub");
        let downloader = Downloader::with_base_url(garbled.base_url());
        let error = offered_manifest(MISSING, false, &downloader)
            .await
            .expect_err("garbled");
        assert!(matches!(error, DownloadError::Manifest { .. }), "{error:?}");

        let published = Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: Vec::new(),
        };
        let good = Stub::start(HashMap::from([(
            MANIFEST.to_owned(),
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

    #[test]
    fn the_texts_are_english_and_name_the_dictionary() {
        assert_eq!(DOWNLOAD, "Download");
        assert_eq!(NOT_NOW, "Not now");
        assert_eq!(INSTALLING, "Installing…");
        assert!(BODY.contains("Open English WordNet"));
        let credit = credit_text();
        assert!(credit.contains("Open English WordNet"), "{credit}");
        assert!(credit.contains("CC BY 4.0"), "{credit}");
        let failed = failed_text("cannot download oewn: the server answered HTTP 404");
        assert!(
            failed.contains("HTTP 404") && !failed.contains('\n'),
            "{failed}"
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
            INSTALLING,
            INSTALLED,
            NO_PLACE,
            STOPPED,
            &late_text(Duration::from_secs(60)),
        ] {
            assert!(text.is_ascii() || text == INSTALLING, "{text}");
            assert!(!text.trim().is_empty());
        }
    }

    /// Draws the dialog into the PNG file `ADDICT_SHOT` names: as offered,
    /// or, with `ADDICT_SHOT_STATE=installing` or `failed`, in that state.
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
        let offer = Offer::new(&parent, 5_935_000, || {}, || {});
        offer.present();
        match std::env::var("ADDICT_SHOT_STATE").as_deref() {
            Ok("installing") => offer.installing(),
            Ok("failed") => {
                offer.installing();
                offer.ended(Some("cannot download oewn: the server answered HTTP 404"));
            }
            _ => {}
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
