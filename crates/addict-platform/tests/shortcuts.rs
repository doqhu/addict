//! Exercise the real shortcut worker against a portal on an isolated session bus.
//! No test talks to, changes or asks for a dialog from the desktop's portal.

mod common;

use std::{
    collections::HashMap,
    ffi::OsStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use addict_platform::shortcut::{self, Action, Bindings, ShortcutEvent};
use zbus::{
    Connection,
    message::Header,
    zvariant::{OwnedObjectPath, OwnedValue, Str, Value},
};

const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";
const CHILD: &str = "ADDICT_TEST_SHORTCUT_PORTAL_CHILD";
const WAIT: Duration = Duration::from_secs(5);
type Options = HashMap<String, OwnedValue>;
type Shortcuts = Vec<(String, Options)>;

#[derive(Debug, PartialEq, Eq)]
struct Definition {
    id: String,
    description: String,
    preferred: Option<String>,
}

#[derive(Default)]
struct State {
    registered: Mutex<Option<String>>,
    definitions: Mutex<Vec<Definition>>,
    session: Mutex<Option<OwnedObjectPath>>,
    configured: Mutex<Vec<OwnedObjectPath>>,
    closed: AtomicUsize,
}

struct Registry(Arc<State>);

#[zbus::interface(name = "org.freedesktop.host.portal.Registry")]
impl Registry {
    fn register(&self, app_id: String, _options: Options) {
        *self.0.registered.lock().unwrap() = Some(app_id);
    }
}

struct Portal {
    state: Arc<State>,
    initial: Vec<(&'static str, &'static str)>,
    version: u32,
    activate_during_bind: bool,
}

#[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts")]
impl Portal {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.version
    }

    async fn create_session(
        &self,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let session = path("session", "session_handle_token", &options, &header);
        connection
            .object_server()
            .at(session.clone(), Session(self.state.clone()))
            .await?;
        *self.state.session.lock().unwrap() = Some(session.clone());
        let request = path("request", "handle_token", &options, &header);
        response(
            connection,
            &request,
            HashMap::from([(
                "session_handle".to_owned(),
                OwnedValue::from(Str::from(session.as_str())),
            )]),
        )
        .await?;
        Ok(request)
    }

    async fn bind_shortcuts(
        &self,
        session: OwnedObjectPath,
        shortcuts: Shortcuts,
        _parent: String,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        assert_eq!(self.state.session.lock().unwrap().as_ref(), Some(&session));
        *self.state.definitions.lock().unwrap() = shortcuts
            .iter()
            .map(|(id, options)| Definition {
                id: id.clone(),
                description: option_text(options, "description").to_owned(),
                preferred: options
                    .get("preferred_trigger")
                    .map(|value| <&str>::try_from(value).unwrap().to_owned()),
            })
            .collect();
        if self.activate_during_bind {
            activated(connection, &session, "screenshot", "during-bind", 41).await;
        }
        let request = path("request", "handle_token", &options, &header);
        let shortcuts = OwnedValue::try_from(Value::new(descriptors(&self.initial))).unwrap();
        response(
            connection,
            &request,
            HashMap::from([("shortcuts".to_owned(), shortcuts)]),
        )
        .await?;
        Ok(request)
    }

    async fn configure_shortcuts(
        &self,
        session: OwnedObjectPath,
        _parent: String,
        _options: Options,
        #[zbus(connection)] connection: &Connection,
    ) {
        self.state.configured.lock().unwrap().push(session.clone());
        changed(connection, &session, &[("screenshot", "Super+S")]).await;
    }
}

struct Session(Arc<State>);

#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl Session {
    fn close(&self) {
        self.0.closed.fetch_add(1, Ordering::SeqCst);
    }
}

fn option_text<'a>(options: &'a Options, key: &str) -> &'a str {
    <&str>::try_from(&options[key]).unwrap()
}

fn path(kind: &str, token: &str, options: &Options, header: &Header<'_>) -> OwnedObjectPath {
    let sender = header
        .sender()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    format!("{DESKTOP}/{kind}/{sender}/{}", option_text(options, token))
        .try_into()
        .unwrap()
}

async fn response(
    connection: &Connection,
    path: &OwnedObjectPath,
    results: Options,
) -> zbus::Result<()> {
    connection
        .emit_signal(
            None::<&str>,
            path,
            "org.freedesktop.portal.Request",
            "Response",
            &(0u32, results),
        )
        .await
}

fn descriptors(entries: &[(&str, &str)]) -> Shortcuts {
    entries
        .iter()
        .map(|(id, trigger)| {
            (
                (*id).to_owned(),
                HashMap::from([
                    ("description".to_owned(), OwnedValue::from(Str::from(*id))),
                    (
                        "trigger_description".to_owned(),
                        OwnedValue::from(Str::from(*trigger)),
                    ),
                ]),
            )
        })
        .collect()
}

async fn activated(
    connection: &Connection,
    session: &OwnedObjectPath,
    id: &str,
    token: &str,
    time: u64,
) {
    let options = HashMap::from([("activation_token", Value::from(token))]);
    connection
        .emit_signal(
            None::<&str>,
            DESKTOP,
            SHORTCUTS,
            "Activated",
            &(session, id, time, options),
        )
        .await
        .unwrap();
}

async fn changed(connection: &Connection, session: &OwnedObjectPath, entries: &[(&str, &str)]) {
    connection
        .emit_signal(
            None::<&str>,
            DESKTOP,
            SHORTCUTS,
            "ShortcutsChanged",
            &(session, descriptors(entries)),
        )
        .await
        .unwrap();
}

struct Mock {
    runtime: tokio::runtime::Runtime,
    connection: Connection,
    state: Arc<State>,
    events: mpsc::Receiver<ShortcutEvent>,
    worker: Option<shortcut::ShortcutHandle>,
}

impl Mock {
    fn start(
        initial: Vec<(&'static str, &'static str)>,
        version: u32,
        activate_during_bind: bool,
    ) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(State::default());
        let connection = runtime.block_on(async {
            zbus::connection::Builder::session()
                .unwrap()
                .name("org.freedesktop.portal.Desktop")
                .unwrap()
                .serve_at(DESKTOP, Registry(state.clone()))
                .unwrap()
                .serve_at(
                    DESKTOP,
                    Portal {
                        state: state.clone(),
                        initial,
                        version,
                        activate_during_bind,
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap()
        });
        let (sender, events) = mpsc::channel();
        let worker = Some(shortcut::start(
            "io.github.doqhu.Test",
            "LOGO+q",
            WAIT,
            move |event| {
                sender.send(event).unwrap();
            },
        ));
        Self {
            runtime,
            connection,
            state,
            events,
            worker,
        }
    }

    fn event(&self) -> ShortcutEvent {
        self.events.recv_timeout(WAIT).expect("worker callback")
    }

    fn session(&self) -> OwnedObjectPath {
        self.state.session.lock().unwrap().clone().unwrap()
    }

    fn changed(&self, session: &OwnedObjectPath, entries: &[(&str, &str)]) {
        self.runtime
            .block_on(changed(&self.connection, session, entries));
    }

    fn activated(&self, session: &OwnedObjectPath, id: &str, token: &str, time: u64) {
        self.runtime
            .block_on(activated(&self.connection, session, id, token, time));
    }

    fn close(&mut self) {
        drop(self.worker.take());
        assert_eq!(self.state.closed.load(Ordering::SeqCst), 1);
        assert_eq!(
            self.events.recv_timeout(WAIT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }
}

fn private(test: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        true
    } else {
        common::run_private_bus(test, Duration::from_secs(20), &[(CHILD, OsStr::new("1"))]);
        false
    }
}

#[test]
fn both_actions_bind_and_deliver_distinct_activations_with_tokens() {
    if !private("both_actions_bind_and_deliver_distinct_activations_with_tokens") {
        return;
    }
    let mut portal = Mock::start(vec![("invoke", "F8"), ("screenshot", "F9")], 2, true);
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: Bindings {
                clipboard: Some("F8".into()),
                screenshot: Some("F9".into())
            }
        }
    );
    assert_eq!(
        *portal.state.registered.lock().unwrap(),
        Some("io.github.doqhu.Test".to_owned())
    );
    let definitions = portal.state.definitions.lock().unwrap();
    assert_eq!(definitions.len(), 2);
    assert_eq!(definitions[0].id, "invoke");
    assert_eq!(definitions[0].preferred.as_deref(), Some("LOGO+q"));
    assert_eq!(definitions[1].id, "screenshot");
    assert_eq!(definitions[1].preferred, None);
    assert_ne!(definitions[0].description, definitions[1].description);
    drop(definitions);
    // Emitted by BindShortcuts itself, before the request's response: subscription must precede bind.
    assert_eq!(
        portal.event(),
        ShortcutEvent::Activated {
            action: Action::Screenshot,
            activation_token: Some("during-bind".into()),
            timestamp_ms: 41
        }
    );
    let session = portal.session();
    let other: OwnedObjectPath = "/other/session".try_into().unwrap();
    portal.activated(&other, "invoke", "wrong-session", 1);
    portal.activated(&session, "unknown", "wrong-action", 2);
    portal.activated(&session, "invoke", "clipboard-token", 42);
    portal.activated(&session, "screenshot", "screenshot-token", 43);
    for (action, token, timestamp_ms) in [
        (Action::Clipboard, "clipboard-token", 42),
        (Action::Screenshot, "screenshot-token", 43),
    ] {
        assert_eq!(
            portal.event(),
            ShortcutEvent::Activated {
                action,
                activation_token: Some(token.into()),
                timestamp_ms
            }
        );
    }
    portal.close();
}

#[test]
fn partial_bindings_configure_live_without_losing_the_existing_clipboard() {
    if !private("partial_bindings_configure_live_without_losing_the_existing_clipboard") {
        return;
    }
    let mut portal = Mock::start(vec![("invoke", "F8")], 2, false);
    let initial = Bindings {
        clipboard: Some("F8".into()),
        screenshot: None,
    };
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: initial.clone()
        }
    );
    let session = portal.session();
    portal.changed(
        &"/other/session".try_into().unwrap(),
        &[("invoke", "wrong")],
    );
    portal.worker.as_ref().unwrap().configure();
    assert_eq!(
        portal.event(),
        ShortcutEvent::ConfigureEnded { message: None }
    );
    let configured = Bindings {
        clipboard: Some("F8".into()),
        screenshot: Some("Super+S".into()),
    };
    assert_eq!(
        portal.event(),
        ShortcutEvent::Changed {
            bindings: configured
        }
    );
    assert_eq!(
        *portal.state.configured.lock().unwrap(),
        vec![session.clone()]
    );
    portal.changed(&session, &[("screenshot", ""), ("unknown", "F1")]);
    assert_eq!(portal.event(), ShortcutEvent::Changed { bindings: initial });
    portal.activated(&session, "invoke", "still-usable", 55);
    assert!(matches!(
        portal.event(),
        ShortcutEvent::Activated {
            action: Action::Clipboard,
            timestamp_ms: 55,
            ..
        }
    ));
    portal.close();
}

#[test]
fn an_empty_session_remains_configurable_and_screenshot_only_is_usable() {
    if !private("an_empty_session_remains_configurable_and_screenshot_only_is_usable") {
        return;
    }
    let mut portal = Mock::start(vec![], 2, false);
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: Bindings::default()
        }
    );
    portal.worker.as_ref().unwrap().configure();
    assert_eq!(
        portal.event(),
        ShortcutEvent::ConfigureEnded { message: None }
    );
    assert_eq!(
        portal.event(),
        ShortcutEvent::Changed {
            bindings: Bindings {
                clipboard: None,
                screenshot: Some("Super+S".into())
            }
        }
    );
    portal.activated(&portal.session(), "screenshot", "screen-only", 60);
    assert!(matches!(
        portal.event(),
        ShortcutEvent::Activated {
            action: Action::Screenshot,
            timestamp_ms: 60,
            ..
        }
    ));
    portal.close();
}

#[test]
fn an_older_portal_keeps_bindings_and_activations_after_configuration_is_unsupported() {
    if !private("an_older_portal_keeps_bindings_and_activations_after_configuration_is_unsupported")
    {
        return;
    }
    let mut portal = Mock::start(vec![("invoke", "F8"), ("screenshot", "")], 1, false);
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: Bindings {
                clipboard: Some("F8".into()),
                screenshot: None
            }
        }
    );
    portal.worker.as_ref().unwrap().configure();
    assert_eq!(portal.event(), ShortcutEvent::ConfigureUnsupported);
    assert!(portal.state.configured.lock().unwrap().is_empty());
    assert_eq!(portal.state.closed.load(Ordering::SeqCst), 0);
    portal.activated(&portal.session(), "invoke", "unchanged", 70);
    assert!(matches!(
        portal.event(),
        ShortcutEvent::Activated {
            action: Action::Clipboard,
            timestamp_ms: 70,
            ..
        }
    ));
    portal.close();
}
