//! The tray icon of the resident application, a StatusNotifierItem.
//!
//! [`start`] spawns one worker thread that owns a session-bus connection and
//! the item: the app icon and a menu of three entries, Show, Settings and
//! Exit. A choice is handed to a plain `Send` callback, so the caller decides
//! how to wake its own thread; nothing here polls. A click on the icon opens
//! the menu.
//!
//! A session without a StatusNotifierHost, or a bus that cannot be reached, is
//! reported once as [`TrayEvent::Unavailable`]; the application goes on
//! without a tray.

use std::{sync::Arc, thread::JoinHandle, time::Duration};

use ksni::{
    Tray, TrayMethods,
    menu::{MenuItem, StandardItem},
};
use tokio::sync::oneshot;

/// What the tray tells the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEvent {
    /// Show was chosen.
    Show,
    /// Settings was chosen.
    Settings,
    /// Exit was chosen.
    Exit,
    /// There is no tray: `message` is one line saying why. Always the last
    /// event of the worker.
    Unavailable { message: String },
}

/// The entries of the menu, in order: what the user reads and what each one
/// tells.
pub const ENTRIES: [(&str, TrayEvent); 3] = [
    ("Show", TrayEvent::Show),
    ("Settings", TrayEvent::Settings),
    ("Exit", TrayEvent::Exit),
];

/// Shared by the item, which reports the choices, and the worker, which
/// reports the failure.
/// How long registering the item and removing it may each take. A host that
/// does not answer ends the tray, not the application's shutdown.
const BUS_LIMIT: Duration = Duration::from_secs(1);

type Callback = Arc<dyn Fn(TrayEvent) + Send + Sync>;

/// Keeps the worker alive. Dropping it removes the item and waits for the
/// thread.
#[derive(Debug)]
pub struct TrayHandle {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            // The worker may already be gone after a failure.
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The item the bus sees.
struct Item {
    app_id: String,
    on_event: Callback,
}

impl Tray for Item {
    /// A click on the icon opens the menu: the GNOME shell's host needs this
    /// to show it on a primary click.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        self.app_id.clone()
    }

    fn title(&self) -> String {
        "Addict".to_owned()
    }

    /// The icon `addict install` writes into the user theme under the app id.
    fn icon_name(&self) -> String {
        self.app_id.clone()
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        ENTRIES
            .iter()
            .map(|(label, event)| {
                let event = event.clone();
                StandardItem {
                    label: (*label).to_owned(),
                    activate: Box::new(move |item: &mut Self| (item.on_event)(event.clone())),
                    ..Default::default()
                }
                .into()
            })
            .collect()
    }
}

/// Starts the worker for `app_id`. `on_event` runs on the worker thread and
/// must not block.
pub fn start(app_id: &str, on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> TrayHandle {
    let (stop, stopped) = oneshot::channel();
    // The callback travels over a channel so that it is still here to report
    // a thread that could not be spawned.
    let (hand_over, handed_over) = std::sync::mpsc::channel::<Callback>();
    let app_id = app_id.to_owned();
    let spawned = std::thread::Builder::new()
        .name("addict-tray".to_owned())
        .spawn(move || {
            let Ok(on_event) = handed_over.recv() else {
                return;
            };
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime.block_on(run(app_id, on_event, stopped)),
                Err(e) => on_event(unavailable("tray worker runtime", &e)),
            }
        });
    let on_event: Callback = Arc::new(on_event);
    match spawned {
        Ok(thread) => {
            // The worker is waiting for exactly this message.
            let _ = hand_over.send(on_event);
            TrayHandle {
                stop: Some(stop),
                thread: Some(thread),
            }
        }
        Err(e) => {
            on_event(unavailable("tray worker thread", &e));
            TrayHandle {
                stop: None,
                thread: None,
            }
        }
    }
}

/// Registers the item, then serves until the stop signal or the handle is
/// gone. Registration and removal each have [`BUS_LIMIT`]; the stop signal
/// also ends a registration that is still waiting.
async fn run(app_id: String, on_event: Callback, mut stopped: oneshot::Receiver<()>) {
    let item = Item {
        app_id,
        on_event: on_event.clone(),
    };
    let registered = tokio::select! {
        _ = &mut stopped => return,
        registered = tokio::time::timeout(BUS_LIMIT, item.spawn()) => registered,
    };
    match registered {
        Ok(Ok(handle)) => {
            // Resolves on the signal and when the handle is gone without one.
            let _ = stopped.await;
            let _ = tokio::time::timeout(BUS_LIMIT, handle.shutdown()).await;
        }
        Ok(Err(e)) => on_event(unavailable("registering the tray item", &e)),
        Err(_) => on_event(TrayEvent::Unavailable {
            message: format!("registering the tray item: no result within {BUS_LIMIT:?}"),
        }),
    }
}

/// One line: the step that failed and the error text as given.
fn unavailable(step: &str, error: &dyn std::fmt::Display) -> TrayEvent {
    let text = format!("{step}: {error}");
    TrayEvent::Unavailable {
        message: text.split_whitespace().collect::<Vec<_>>().join(" "),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn item() -> (Item, Arc<Mutex<Vec<TrayEvent>>>) {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let sink = heard.clone();
        let item = Item {
            app_id: "dev.addict.Addict".to_owned(),
            on_event: Arc::new(move |event| sink.lock().expect("sink").push(event)),
        };
        (item, heard)
    }

    #[test]
    fn the_menu_is_show_settings_exit_in_that_order() {
        let (item, _) = item();
        let labels: Vec<String> = item
            .menu()
            .into_iter()
            .map(|entry| match entry {
                MenuItem::Standard(entry) => entry.label,
                _ => panic!("only standard items"),
            })
            .collect();
        assert_eq!(labels, ["Show", "Settings", "Exit"]);
    }

    #[test]
    fn each_entry_tells_its_own_event_and_no_other() {
        let (mut item, heard) = item();
        for entry in item.menu() {
            let MenuItem::Standard(entry) = entry else {
                panic!("only standard items");
            };
            (entry.activate)(&mut item);
        }
        assert_eq!(
            *heard.lock().expect("heard"),
            [TrayEvent::Show, TrayEvent::Settings, TrayEvent::Exit]
        );
    }

    #[test]
    fn a_click_on_the_icon_opens_the_menu_and_is_not_show() {
        const { assert!(Item::MENU_ON_ACTIVATE) };
        let (mut item, heard) = item();
        item.activate(0, 0);
        assert!(heard.lock().expect("heard").is_empty());
    }

    #[test]
    fn the_icon_and_the_id_are_the_app_id() {
        let (item, _) = item();
        assert_eq!(item.id(), "dev.addict.Addict");
        assert_eq!(item.icon_name(), "dev.addict.Addict");
    }

    #[test]
    fn unavailable_is_one_line() {
        let TrayEvent::Unavailable { message } = unavailable(
            "registering the tray item",
            &"no StatusNotifierHost\nexists",
        ) else {
            panic!("a failure");
        };
        assert_eq!(
            message,
            "registering the tray item: no StatusNotifierHost exists"
        );
    }
}
