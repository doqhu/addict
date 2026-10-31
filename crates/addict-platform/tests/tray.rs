//! The real tray worker against a mock StatusNotifierWatcher on a private
//! session bus: never the desktop's watcher.

mod common;

use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use addict_platform::tray::{self, TrayEvent};
use zbus::{Connection, fdo::DBusProxy, message::Header};

const CHILD: &str = "ADDICT_TEST_TRAY_WATCHER_CHILD";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WAIT: Duration = Duration::from_secs(5);

/// Each registration: the name it gave and the sender of the call.
type Registered = Arc<Mutex<Vec<(String, String)>>>;

struct Watcher(Registered);

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    fn register_status_notifier_item(&self, service: String, #[zbus(header)] header: Header<'_>) {
        let sender = header.sender().expect("a sender").to_string();
        self.0.lock().expect("registered").push((service, sender));
    }
}

/// Runs the test in a child with a private bus; true inside that child.
fn private(test: &str) -> bool {
    // No service directory: nothing can activate a watcher on this bus.
    common::on_private_bus(CHILD, "tray", test)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime")
}

async fn serve_watcher(registered: Registered) -> Connection {
    zbus::connection::Builder::session()
        .expect("the session bus")
        .name(WATCHER)
        .expect("the watcher name")
        .serve_at("/StatusNotifierWatcher", Watcher(registered))
        .expect("the watcher object")
        .build()
        .await
        .expect("the watcher connection")
}

/// Waits for the first registration, then checks its name and what the bus
/// holds: the item's unique name, and no StatusNotifierItem name.
async fn assert_registered_under_unique_name(connection: &Connection, registered: &Registered) {
    let deadline = Instant::now() + WAIT;
    let (service, sender) = loop {
        if let Some(first) = registered.lock().expect("registered").first().cloned() {
            break first;
        }
        assert!(Instant::now() < deadline, "no registration within {WAIT:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        service, sender,
        "registers the unique name of its connection"
    );
    assert!(service.starts_with(':'), "{service} is not a unique name");
    let names = DBusProxy::new(connection)
        .await
        .expect("the bus")
        .list_names()
        .await
        .expect("the names");
    assert!(
        !names
            .iter()
            .any(|name| name.starts_with("org.kde.StatusNotifierItem")),
        "owns a well-known name: {names:?}"
    );
}

#[test]
fn the_item_registers_under_its_unique_name_and_owns_no_well_known_name() {
    if !private("the_item_registers_under_its_unique_name_and_owns_no_well_known_name") {
        return;
    }
    let runtime = runtime();
    let registered = Registered::default();
    let connection = runtime.block_on(serve_watcher(registered.clone()));
    let (told, heard) = mpsc::channel();
    let told = Mutex::new(told);
    let handle = tray::start("io.github.doqhu.Test", move |event| {
        let _ = told.lock().expect("told").send(event);
    });
    runtime.block_on(assert_registered_under_unique_name(
        &connection,
        &registered,
    ));
    drop(handle);
    assert_eq!(
        heard.try_recv().ok(),
        None,
        "a registered item says nothing"
    );
}

#[test]
fn a_missing_watcher_is_said_once_and_a_later_one_gets_the_item() {
    if !private("a_missing_watcher_is_said_once_and_a_later_one_gets_the_item") {
        return;
    }
    let (told, heard) = mpsc::channel();
    let told = Mutex::new(told);
    let handle = tray::start("io.github.doqhu.Test", move |event| {
        let _ = told.lock().expect("told").send(event);
    });
    let Ok(TrayEvent::Unavailable { message }) = heard.recv_timeout(WAIT) else {
        panic!("the missing watcher was not said");
    };
    assert!(message.starts_with("no tray host"), "{message}");
    // ksni says this before it subscribes to the watcher's arrival, and the
    // subscription cannot be observed: a watcher in that gap would be missed.
    std::thread::sleep(Duration::from_millis(300));

    let runtime = runtime();
    let registered = Registered::default();
    let connection = runtime.block_on(serve_watcher(registered.clone()));
    runtime.block_on(assert_registered_under_unique_name(
        &connection,
        &registered,
    ));
    assert!(heard.try_recv().is_err(), "said only once");
    drop(handle);
}
