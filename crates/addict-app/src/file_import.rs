//! Settings' file chooser and manual import. GTK only chooses and renders;
//! the existing importer and the subsequent index check run off its thread.

use std::{cell::RefCell, future::Future, path::PathBuf, rc::Rc};

use addict_dictionary::import::{self, Imported};
use gtk::{gio, prelude::*};

use crate::{download, settings::label};

pub const LABEL: &str = "Import file…";

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Idle,
    Choosing,
    Importing,
    Imported(Imported),
    Failed(String),
}

impl State {
    fn busy(&self) -> bool {
        matches!(self, Self::Choosing | Self::Importing)
    }

    fn text(&self) -> String {
        match self {
            Self::Idle => String::new(),
            Self::Choosing => "Choose a downloaded dictionary file…".to_owned(),
            Self::Importing => "Importing dictionary…".to_owned(),
            Self::Imported(done) => format!(
                "Imported {}: {} headwords ({}). If this dictionary was already in use, restart Addict to use the new edition.",
                done.path.file_name().unwrap_or_default().to_string_lossy(),
                done.headwords,
                done.edition,
            ),
            Self::Failed(message) => message.clone(),
        }
    }
}

/// Shared by the real button and display-free tests. Choosing, importing
/// and delivering refreshed states all keep a second activation disabled.
#[derive(Clone)]
struct Action {
    state: Rc<RefCell<State>>,
    show: Rc<dyn Fn(&State)>,
}

impl Action {
    fn new(show: impl Fn(&State) + 'static) -> Self {
        Self {
            state: Rc::new(RefCell::new(State::Idle)),
            show: Rc::new(show),
        }
    }

    fn set(&self, state: State) {
        *self.state.borrow_mut() = state.clone();
        (self.show)(&state);
    }

    async fn run<C, F>(&self, choose: C, data_dir: Option<PathBuf>, on_imported: impl FnOnce() -> F)
    where
        C: Future<Output = Result<Option<PathBuf>, String>>,
        F: Future<Output = Result<(), String>>,
    {
        if self.state.borrow().busy() {
            return;
        }
        self.set(State::Choosing);
        let path = match choose.await {
            Ok(Some(path)) => path,
            Ok(None) => {
                self.set(State::Idle);
                return;
            }
            Err(message) => {
                self.set(State::Failed(message));
                return;
            }
        };
        let Some(dir) = data_dir.filter(|dir| dir.is_absolute()) else {
            self.set(State::Failed(
                "Cannot import a dictionary: no absolute data directory is available.".to_owned(),
            ));
            return;
        };
        self.set(State::Importing);
        match download::import_file(path, dir).await {
            Ok(done) => match on_imported().await {
                Ok(()) => self.set(State::Imported(done)),
                Err(message) => self.set(State::Failed(format!(
                    "The file was imported, but dictionary readiness could not be refreshed: {message}"
                ))),
            },
            Err(message) => self.set(State::Failed(message)),
        }
    }
}

fn selected_path(file: gio::File) -> Result<Option<PathBuf>, String> {
    file.path().map(Some).ok_or_else(|| {
        "Cannot import this file: the file chooser did not provide a local or document-portal path. Choose a downloaded file on this computer.".to_owned()
    })
}

async fn choose(parent: &gtk::Window) -> Result<Option<PathBuf>, String> {
    let dialog = gtk::FileDialog::builder()
        .title("Import a dictionary file")
        .accept_label(LABEL)
        .modal(true)
        .build();
    match dialog.open_future(Some(parent)).await {
        Ok(file) => selected_path(file),
        Err(error)
            if error.matches(gtk::DialogError::Cancelled)
                || error.matches(gtk::DialogError::Dismissed) =>
        {
            Ok(None)
        }
        Err(error) => Err(format!("Cannot choose a dictionary file: {error}")),
    }
}

pub struct Row {
    pub root: gtk::Box,
    button: gtk::Button,
    status: gtk::Label,
}

impl Row {
    pub fn new() -> Self {
        let name = label("Import a downloaded dictionary", &[]);
        name.set_hexpand(true);
        name.set_valign(gtk::Align::Center);
        let button = gtk::Button::builder()
            .label(LABEL)
            .valign(gtk::Align::Center)
            .css_classes(["settings-button"])
            .build();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.append(&name);
        line.append(&button);
        let status = label("", &["hint"]);
        status.set_selectable(true);
        status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        status.set_visible(false);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.append(&line);
        root.append(&status);
        Self {
            root,
            button,
            status,
        }
    }

    /// Called after Settings exists so its window, rather than the main
    /// application window, is the file dialog's parent.
    pub fn connect<F: Future<Output = Result<(), String>> + 'static>(
        &self,
        parent: &gtk::Window,
        on_imported: impl Fn() -> F + 'static,
    ) {
        let button = self.button.downgrade();
        let status = self.status.downgrade();
        let action = Action::new(move |state| {
            if let Some(button) = button.upgrade() {
                button.set_sensitive(!state.busy());
            }
            if let Some(status) = status.upgrade() {
                let text = state.text();
                status.set_label(&text);
                status.set_visible(!text.is_empty());
            }
        });
        let parent = parent.downgrade();
        let on_imported = Rc::new(on_imported);
        self.button.connect_clicked(move |_| {
            let Some(parent) = parent.upgrade() else {
                return;
            };
            let action = action.clone();
            let on_imported = on_imported.clone();
            gtk::glib::spawn_future_local(async move {
                action
                    .run(choose(&parent), import::data_dir(), || on_imported())
                    .await;
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::ready,
        path::Path,
        sync::atomic::{AtomicU32, Ordering},
    };

    use addict_dictionary::Index;
    use futures_channel::oneshot;
    use futures_util::{pin_mut, poll};

    use super::*;

    static NEXT: AtomicU32 = AtomicU32::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "addict-file-import-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir(&path).expect("private test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn failed(action: &Action) -> String {
        let State::Failed(message) = &*action.state.borrow() else {
            panic!("expected failure, got {:?}", action.state.borrow());
        };
        assert!(!action.state.borrow().busy(), "retry is available");
        message.clone()
    }

    #[tokio::test]
    async fn dismissal_is_quiet_writes_nothing_and_allows_retry() {
        let temp = Temp::new();
        let shown = Rc::new(RefCell::new(Vec::new()));
        let log = shown.clone();
        let action = Action::new(move |state| log.borrow_mut().push(state.clone()));
        for _ in 0..2 {
            action
                .run(ready(Ok(None)), Some(temp.0.join("data")), || async {
                    panic!("nothing was imported")
                })
                .await;
            assert_eq!(*action.state.borrow(), State::Idle);
        }
        assert!(!temp.0.join("data").exists());
        assert_eq!(
            *shown.borrow(),
            [State::Choosing, State::Idle, State::Choosing, State::Idle]
        );
    }

    #[tokio::test]
    async fn a_second_activation_while_choosing_is_ignored() {
        let action = Action::new(|_| {});
        let (send, receive) = oneshot::channel();
        let first = action.run(async { receive.await.expect("choice") }, None, || {
            ready(Ok(()))
        });
        pin_mut!(first);
        assert!(poll!(first.as_mut()).is_pending());
        assert_eq!(*action.state.borrow(), State::Choosing);
        action
            .run(
                async { panic!("must not open a second chooser") },
                None,
                || ready(Ok(())),
            )
            .await;
        send.send(Ok(None)).expect("deliver dismissal");
        first.await;
        assert_eq!(*action.state.borrow(), State::Idle);
    }

    #[tokio::test]
    async fn chooser_and_nonlocal_failures_are_visible_and_retryable() {
        let action = Action::new(|_| {});
        for error in [
            "Cannot choose a dictionary file: portal unavailable".to_owned(),
            selected_path(gio::File::for_uri("https://example.invalid/dictionary.gz")).unwrap_err(),
        ] {
            action
                .run(ready(Err(error.clone())), None, || ready(Ok(())))
                .await;
            assert_eq!(failed(&action), error);
        }
        let path = PathBuf::from("/run/user/1000/doc/grant/english-wordnet-2025.xml.gz");
        assert_eq!(selected_path(gio::File::for_path(&path)), Ok(Some(path)));
    }

    #[tokio::test]
    async fn missing_or_relative_data_directory_is_a_visible_error_without_a_write() {
        let temp = Temp::new();
        let action = Action::new(|_| {});
        for dir in [None, Some(PathBuf::from("relative"))] {
            action
                .run(ready(Ok(Some(temp.0.join("unread-file")))), dir, || {
                    ready(Ok(()))
                })
                .await;
            assert!(failed(&action).contains("no absolute data directory"));
        }
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn refused_and_unreadable_files_preserve_the_existing_index() {
        let temp = Temp::new();
        let data = temp.0.join("data");
        std::fs::create_dir(&data).unwrap();
        let index = data.join("oewn.index");
        std::fs::write(&index, b"previous index").unwrap();
        let corrupt = temp.0.join("english-wordnet-2025.xml.gz");
        std::fs::write(&corrupt, b"corrupt input").unwrap();
        let action = Action::new(|_| {});
        for path in [corrupt, temp.0.join("missing-file")] {
            action
                .run(
                    ready(Ok(Some(path.clone()))),
                    Some(data.clone()),
                    || async { panic!("refused import cannot announce completion") },
                )
                .await;
            assert!(failed(&action).contains(&path.display().to_string()));
            assert_eq!(std::fs::read(&index).unwrap(), b"previous index");
            assert_eq!(std::fs::read_dir(&data).unwrap().count(), 1);
        }
    }

    #[tokio::test]
    #[ignore = "imports the real pinned OEWN file; run explicitly"]
    async fn the_real_chooser_action_imports_oewn_and_preserves_it_on_refusal() {
        let fixture = std::env::var_os("ADDICT_OEWN_IMPORT_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../experiments/exp7/data/english-wordnet-2025.xml.gz")
            });
        assert!(
            fixture.is_file(),
            "set ADDICT_OEWN_IMPORT_FILE to the pinned file"
        );
        let temp = Temp::new();
        let data = temp.0.join("data");
        let action = Action::new(|_| {});
        let (send, receive) = oneshot::channel();
        let done = action.run(
            ready(Ok(Some(fixture.clone()))),
            Some(data.clone()),
            || async {
                // Success remains unavailable until the resident has applied
                // its refreshed states and acknowledges them.
                receive
                    .await
                    .map_err(|_| "no resident acknowledgement".to_owned())
            },
        );
        pin_mut!(done);
        // Drive exactly the same action as the button, with only the
        // chooser substituted. Give the import time to finish, then hold
        // it at its resident acknowledgement.
        loop {
            tokio::select! {
                () = &mut done => panic!("completed before resident acknowledgement"),
                () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                    if data.join("oewn.index").exists() { break; }
                }
            }
        }
        assert_eq!(*action.state.borrow(), State::Importing);
        action
            .run(
                async { panic!("must not choose during import") },
                None,
                || ready(Ok(())),
            )
            .await;
        let index = Index::open(&data.join("oewn.index")).expect("installed index opens");
        assert!(index.headwords() > 100_000);
        assert!(index.find("dog").expect("English lookup").is_some());
        send.send(()).unwrap();
        done.await;
        let State::Imported(imported) = action.state.borrow().clone() else {
            panic!("{:?}", action.state.borrow());
        };
        assert_eq!(imported.headwords as u64, index.headwords());
        let completion = action.state.borrow().text();
        assert!(completion.contains("Imported oewn.index"));
        assert!(completion.contains("already in use, restart Addict to use the new edition"));
        let prior = std::fs::read(&imported.path).unwrap();
        let corrupt = temp.0.join("corrupt.gz");
        std::fs::write(&corrupt, b"not an upstream dictionary").unwrap();
        action
            .run(ready(Ok(Some(corrupt))), Some(data.clone()), || {
                ready(Ok(()))
            })
            .await;
        assert!(failed(&action).contains("corrupt.gz"));
        assert_eq!(std::fs::read(&imported.path).unwrap(), prior);

        // A regular file cannot be the data directory, even as root.
        let unwritable = temp.0.join("not-a-directory");
        std::fs::write(&unwritable, b"leave this intact").unwrap();
        action
            .run(ready(Ok(Some(fixture))), Some(unwritable.clone()), || {
                ready(Ok(()))
            })
            .await;
        assert!(failed(&action).contains(&unwritable.display().to_string()));
        assert_eq!(std::fs::read(&unwritable).unwrap(), b"leave this intact");
        assert_eq!(std::fs::read(&imported.path).unwrap(), prior);
    }
}
