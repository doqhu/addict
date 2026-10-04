//! Non-interactive capability record: portal interface versions, Screenshot
//! `AvailableTargets`, host Registry presence, session type and the Wayland
//! globals that decide how a clipboard can be read.

use serde_json::{Value, json};

use crate::{
    portal::{self, Failure},
    record::{Finish, Outcome, Progress},
};

pub const EXPECTED: &str = "outcome reported: version of GlobalShortcuts, Screenshot, Clipboard, \
     RemoteDesktop and host Registry as the running portal answers, Screenshot AvailableTargets, \
     session type and clipboard-related Wayland globals; a missing interface is recorded as \
     missing";

/// Portal interfaces whose `version` is read. Clipboard needs a
/// RemoteDesktop session, so that one is read too.
pub const INTERFACES: &[(&str, &str)] = &[
    ("GlobalShortcuts", "org.freedesktop.portal.GlobalShortcuts"),
    ("Screenshot", "org.freedesktop.portal.Screenshot"),
    ("Clipboard", "org.freedesktop.portal.Clipboard"),
    ("RemoteDesktop", "org.freedesktop.portal.RemoteDesktop"),
    ("Registry", "org.freedesktop.host.portal.Registry"),
];

/// Wayland globals that bear on reading a selection or activating a window.
pub const WATCHED_GLOBALS: &[&str] = &[
    "wl_data_device_manager",
    "zwp_primary_selection_device_manager_v1",
    "ext_data_control_manager_v1",
    "zwlr_data_control_manager_v1",
    "xdg_activation_v1",
];

pub async fn run(progress: &Progress) -> Finish {
    progress.put("session_type", std::env::var("XDG_SESSION_TYPE").ok());

    // The Wayland side does not depend on the bus; read it first so a record
    // of a missing portal still says what the compositor offers.
    progress.phase("wayland-globals");
    let globals = wayland_globals().await;
    progress.put("wayland", watched(&globals));

    let connection = match portal::connect(progress).await {
        Ok(connection) => connection,
        Err(finish) => return finish,
    };

    progress.phase("portal-versions");
    let mut interfaces = serde_json::Map::new();
    let mut answered = 0usize;
    let mut first_error = None;
    for (short, interface) in INTERFACES {
        match portal::property_u32(&connection, interface, "version").await {
            Ok(version) => {
                answered += 1;
                interfaces.insert(
                    (*short).to_owned(),
                    json!({ "interface": interface, "present": true, "version": version }),
                );
            }
            Err(e) => {
                let text = e.to_string();
                first_error.get_or_insert_with(|| (portal::classify_zbus(&e), text.clone()));
                interfaces.insert(
                    (*short).to_owned(),
                    json!({ "interface": interface, "present": false, "error": text }),
                );
            }
        }
    }
    progress.put("interfaces", Value::Object(interfaces));

    progress.phase("screenshot-available-targets");
    let targets = match portal::property_u32(
        &connection,
        "org.freedesktop.portal.Screenshot",
        "AvailableTargets",
    )
    .await
    {
        Ok(bits) => json!({ "present": true, "bits": bits, "names": portal::target_names(bits) }),
        Err(e) => json!({ "present": false, "error": e.to_string() }),
    };
    progress.put("screenshot_available_targets", targets);
    progress.phase("done");

    if answered == 0 {
        let (failure, text) = first_error.unwrap_or((Failure::Unavailable, String::new()));
        let outcome = match failure {
            Failure::Unavailable => Outcome::Unavailable,
            _ => Outcome::Error,
        };
        return Finish::with_error(outcome, format!("no portal interface answered: {text}"));
    }
    Finish::ok(Outcome::Reported)
}

/// One advertised Wayland global.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Global {
    pub interface: String,
    pub version: u32,
}

/// The watched globals as a record value: each is either present with its
/// version or recorded as missing.
pub fn watched(globals: &Result<Vec<Global>, String>) -> Value {
    match globals {
        Err(error) => json!({ "reachable": false, "error": error }),
        Ok(all) => {
            let mut map = serde_json::Map::new();
            for name in WATCHED_GLOBALS {
                let entry = match all.iter().find(|g| g.interface == *name) {
                    Some(g) => json!({ "present": true, "version": g.version }),
                    None => json!({ "present": false }),
                };
                map.insert((*name).to_owned(), entry);
            }
            json!({ "reachable": true, "global_count": all.len(), "globals": map })
        }
    }
}

/// Lists the compositor's globals on a helper thread; the caller's overall
/// bound covers the wait.
async fn wayland_globals() -> Result<Vec<Global>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(list_globals());
    });
    rx.await
        .unwrap_or_else(|_| Err("the Wayland helper thread ended without an answer".to_owned()))
}

fn list_globals() -> Result<Vec<Global>, String> {
    use wayland_client::{
        Connection, Dispatch, QueueHandle,
        globals::{GlobalListContents, registry_queue_init},
        protocol::wl_registry::{self, WlRegistry},
    };

    struct State;
    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    let connection = Connection::connect_to_env().map_err(|e| format!("connect: {e}"))?;
    let (globals, _queue) =
        registry_queue_init::<State>(&connection).map_err(|e| format!("registry: {e}"))?;
    Ok(globals
        .contents()
        .clone_list()
        .into_iter()
        .map(|g| Global {
            interface: g.interface,
            version: g.version,
        })
        .collect())
}
