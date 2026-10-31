//! Exercise the real shortcut worker against a portal on an isolated session bus.
//! No test talks to, changes or asks for a dialog from the desktop's portal.

mod common;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsStr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use addict_platform::{
    screenshot::{self, CaptureFailure},
    shortcut::{self, Action, Bindings, ShortcutEvent},
};
use zbus::{
    Connection,
    message::Header,
    zvariant::{OwnedObjectPath, OwnedValue, Str, Value},
};

const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const APP_ID: &str = "io.github.doqhu.Test";
const CHILD: &str = "ADDICT_TEST_SHORTCUT_PORTAL_CHILD";
/// Set for the process the bus activates in place of a portal that left.
const ACTIVATED: &str = "ADDICT_TEST_SHORTCUT_PORTAL_ACTIVATED";
const WAIT: Duration = Duration::from_secs(5);
type Options = HashMap<String, OwnedValue>;
type Shortcuts = Vec<(String, Options)>;

#[derive(Debug, PartialEq, Eq)]
struct Definition {
    id: String,
    description: String,
    preferred: Option<String>,
}

/// How the mock Screenshot portal answers one request.
enum Shot {
    /// The portal's response has this code and no results.
    Code(u32),
    /// A success with this URI.
    Uri(String),
    /// The call itself fails.
    Refused,
}

/// One portal process: what it was asked, and how it answers.
#[derive(Default)]
struct State {
    /// The calls that matter to the order: `Register` must come first.
    log: Mutex<Vec<&'static str>>,
    registered: Mutex<Option<String>>,
    registered_senders: Mutex<HashSet<String>>,
    refuse_register: bool,
    silent_bind: bool,
    /// The answers of the next Screenshot calls; after them, success with
    /// [`shot_path`].
    shots: Mutex<VecDeque<Shot>>,
    /// The `interactive` option of each Screenshot call.
    interactive: Mutex<Vec<Option<bool>>>,
    definitions: Mutex<Vec<Definition>>,
    session: Mutex<Option<OwnedObjectPath>>,
    configured: Mutex<Vec<OwnedObjectPath>>,
    closed: AtomicUsize,
}

struct Registry(Arc<State>);

#[zbus::interface(name = "org.freedesktop.host.portal.Registry")]
impl Registry {
    /// Like the real portal, refuses a second registration of a connection.
    fn register(
        &self,
        app_id: String,
        _options: Options,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        if self.0.refuse_register {
            return Err(zbus::fdo::Error::Failed("registration refused".to_owned()));
        }
        let sender = header.sender().unwrap().to_string();
        if !self.0.registered_senders.lock().unwrap().insert(sender) {
            return Err(zbus::fdo::Error::Failed("already registered".to_owned()));
        }
        self.0.log.lock().unwrap().push("Register");
        *self.0.registered.lock().unwrap() = Some(app_id);
        Ok(())
    }
}

impl State {
    /// A portal call from a host app must follow its registration.
    fn check_registered(&self, header: &Header<'_>) -> zbus::fdo::Result<()> {
        let sender = header.sender().unwrap().to_string();
        if self.registered_senders.lock().unwrap().contains(&sender) {
            Ok(())
        } else {
            Err(zbus::fdo::Error::AccessDenied("not registered".to_owned()))
        }
    }
}

struct Screenshot(Arc<State>);

#[zbus::interface(name = "org.freedesktop.portal.Screenshot")]
impl Screenshot {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }

    async fn screenshot(
        &self,
        _parent: String,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0.check_registered(&header)?;
        self.0.log.lock().unwrap().push("Screenshot");
        self.0.interactive.lock().unwrap().push(
            options
                .get("interactive")
                .map(|value| bool::try_from(value).unwrap()),
        );
        let request = path("request", "handle_token", &options, &header);
        let shot = self.0.shots.lock().unwrap().pop_front();
        let uri = match shot {
            Some(Shot::Code(code)) => {
                coded_response(connection, &request, code, Options::new()).await?;
                return Ok(request);
            }
            Some(Shot::Refused) => {
                return Err(zbus::fdo::Error::Failed("screenshot refused".to_owned()));
            }
            Some(Shot::Uri(uri)) => uri,
            None => format!("file://{}", shot_path().display()),
        };
        response(
            connection,
            &request,
            HashMap::from([("uri".to_owned(), OwnedValue::from(Str::from(uri)))]),
        )
        .await?;
        Ok(request)
    }
}

/// The file the mock Screenshot portal says it made.
fn shot_path() -> PathBuf {
    std::env::temp_dir().join(format!("addict-test-shot-{}.png", std::process::id()))
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
        self.state.check_registered(&header)?;
        self.state.log.lock().unwrap().push("CreateSession");
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
        if self.state.silent_bind {
            return Ok(request);
        }
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
    coded_response(connection, path, 0, results).await
}

async fn coded_response(
    connection: &Connection,
    path: &OwnedObjectPath,
    code: u32,
    results: Options,
) -> zbus::Result<()> {
    connection
        .emit_signal(
            None::<&str>,
            path,
            "org.freedesktop.portal.Request",
            "Response",
            &(code, results),
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

/// One portal process on the bus. `replacing` takes the name from the owner in
/// one step, as a restarted portal seen by a bus with no gap in between.
async fn portal_process(
    state: Arc<State>,
    initial: Vec<(&'static str, &'static str)>,
    version: u32,
    activate_during_bind: bool,
    replacing: bool,
) -> Connection {
    zbus::connection::Builder::session()
        .unwrap()
        .allow_name_replacements(true)
        .replace_existing_names(replacing)
        .name(PORTAL)
        .unwrap()
        .serve_at(DESKTOP, Registry(state.clone()))
        .unwrap()
        .serve_at(DESKTOP, Screenshot(state.clone()))
        .unwrap()
        .serve_at(
            DESKTOP,
            Portal {
                state,
                initial,
                version,
                activate_during_bind,
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap()
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
        Self::limited(initial, version, activate_during_bind, WAIT)
    }

    fn limited(
        initial: Vec<(&'static str, &'static str)>,
        version: u32,
        activate_during_bind: bool,
        limit: Duration,
    ) -> Self {
        let mut mock = Self::without_worker(initial, version, activate_during_bind);
        let (sender, events) = mpsc::channel();
        mock.events = events;
        mock.worker = Some(shortcut::start(APP_ID, "LOGO+q", limit, move |event| {
            sender.send(event).unwrap();
        }));
        mock
    }

    fn without_worker(
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
        let connection = runtime.block_on(portal_process(
            state.clone(),
            initial,
            version,
            activate_during_bind,
            false,
        ));
        let (_, events) = mpsc::channel();
        Self {
            runtime,
            connection,
            state,
            events,
            worker: None,
        }
    }

    /// The portal process ends and a new one takes the name. Gives the state
    /// of the old one.
    fn restart(&mut self, new: State, initial: Vec<(&'static str, &'static str)>) -> Arc<State> {
        let new = Arc::new(new);
        self.connection =
            self.runtime
                .block_on(portal_process(new.clone(), initial, 2, false, true));
        std::mem::replace(&mut self.state, new)
    }

    /// The portal process ends and nothing takes the name.
    fn vanish(&self) {
        assert!(
            self.runtime
                .block_on(self.connection.release_name(PORTAL))
                .unwrap()
        );
    }

    fn log(&self) -> Vec<&'static str> {
        self.state.log.lock().unwrap().clone()
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

#[test]
fn a_restarted_portal_is_registered_with_and_bound_again_with_the_same_shortcuts() {
    if !private("a_restarted_portal_is_registered_with_and_bound_again_with_the_same_shortcuts") {
        return;
    }
    let mut portal = Mock::start(vec![("invoke", "F8"), ("screenshot", "F9")], 2, false);
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    let old_session = portal.session();

    let old = portal.restart(State::default(), vec![("invoke", "F6")]);
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: Bindings {
                clipboard: Some("F6".into()),
                screenshot: None
            }
        }
    );
    assert_eq!(portal.log(), ["Register", "CreateSession"]);
    assert_eq!(
        *portal.state.registered.lock().unwrap(),
        Some(APP_ID.to_owned())
    );
    assert_eq!(
        *portal.state.definitions.lock().unwrap(),
        *old.definitions.lock().unwrap()
    );
    assert_eq!(old.closed.load(Ordering::SeqCst), 0);

    // A press of the new session is the same action; the old session is gone.
    let session = portal.session();
    assert_ne!(session, old_session);
    portal.activated(&old_session, "invoke", "old-session", 1);
    portal.activated(&session, "invoke", "new-session", 80);
    assert_eq!(
        portal.event(),
        ShortcutEvent::Activated {
            action: Action::Clipboard,
            activation_token: Some("new-session".into()),
            timestamp_ms: 80
        }
    );

    // The dialog goes to the new session too.
    portal.worker.as_ref().unwrap().configure();
    assert_eq!(
        portal.event(),
        ShortcutEvent::ConfigureEnded { message: None }
    );
    assert!(matches!(portal.event(), ShortcutEvent::Changed { .. }));
    assert_eq!(*portal.state.configured.lock().unwrap(), vec![session]);

    // And it survives another restart.
    portal.restart(State::default(), vec![("invoke", "F5")]);
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    assert_eq!(portal.log(), ["Register", "CreateSession"]);
    portal.close();
}

#[test]
fn a_refused_registration_of_the_new_portal_reports_one_failure() {
    if !private("a_refused_registration_of_the_new_portal_reports_one_failure") {
        return;
    }
    let mut portal = Mock::start(vec![("invoke", "F8")], 2, false);
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.restart(
        State {
            refuse_register: true,
            ..State::default()
        },
        vec![],
    );
    let ShortcutEvent::Failed { message } = portal.event() else {
        panic!("the rebind must fail");
    };
    assert!(message.contains("host registration"), "{message}");
    assert!(message.contains("registration refused"), "{message}");
    // That was the last act of the worker.
    assert_eq!(
        portal.events.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
    assert!(portal.log().is_empty());
}

#[test]
fn a_portal_that_vanishes_and_cannot_be_activated_reports_one_failure() {
    if std::env::var_os(CHILD).is_none() {
        // Only an empty service directory: the default configuration would
        // read the system's, where the host's real portal may be activatable.
        let dir =
            std::env::temp_dir().join(format!("addict-test-noservice-{}", std::process::id()));
        let services = dir.join("services");
        std::fs::create_dir_all(&services).unwrap();
        let config = dir.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                "<busconfig><type>session</type>\
                 <listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth>\
                 <servicedir>{}</servicedir>\
                 <policy context=\"default\"><allow send_destination=\"*\"/>\
                 <allow receive_sender=\"*\"/>\
                 <allow own=\"*\"/></policy></busconfig>\n",
                services.display()
            ),
        )
        .unwrap();
        common::run_private_bus_with_config(
            "a_portal_that_vanishes_and_cannot_be_activated_reports_one_failure",
            &config,
            Duration::from_secs(20),
            &[(CHILD, OsStr::new("1"))],
        );
        std::fs::remove_dir_all(&dir).unwrap();
        return;
    }
    let portal = Mock::start(vec![("invoke", "F8")], 2, false);
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.vanish();
    let ShortcutEvent::Failed { message } = portal.event() else {
        panic!("the rebind must fail");
    };
    assert!(message.starts_with("host registration of"), "{message}");
    assert!(message.contains("ServiceUnknown"), "{message}");
    assert_eq!(
        portal.events.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}

/// The first attempt's limit is long past when the portal restarts, so a
/// failure a whole limit later is the limit of the new attempt.
#[test]
fn the_limit_starts_again_for_each_rebind() {
    if !private("the_limit_starts_again_for_each_rebind") {
        return;
    }
    let limit = Duration::from_secs(1);
    let mut portal = Mock::limited(vec![("invoke", "F8")], 2, false, limit);
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    std::thread::sleep(limit + limit / 2);

    let restarted = Instant::now();
    portal.restart(
        State {
            silent_bind: true,
            ..State::default()
        },
        vec![],
    );
    assert_eq!(
        portal.event(),
        ShortcutEvent::Failed {
            message: "binding the shortcut: no result within 1s".to_owned()
        }
    );
    assert!(restarted.elapsed() >= limit - Duration::from_millis(100));
    // The session of the new attempt is closed before the failure is sent.
    assert_eq!(portal.state.closed.load(Ordering::SeqCst), 1);
    assert_eq!(
        portal.events.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}

/// The bus starts a portal when the rebind calls it. Binding at all shows that
/// the new process had the registration first: it refuses a session without.
#[test]
fn a_portal_that_vanishes_is_activated_by_the_rebind() {
    if std::env::var_os(CHILD).is_none() {
        let data = std::env::temp_dir().join(format!("addict-test-bus-{}", std::process::id()));
        let services = data.join("dbus-1/services");
        std::fs::create_dir_all(&services).unwrap();
        std::fs::write(
            services.join(format!("{PORTAL}.service")),
            format!(
                "[D-BUS Service]\nName={PORTAL}\nExec={} --exact \
                 activated_portal_process --nocapture --test-threads=1\n",
                std::env::current_exe().unwrap().display()
            ),
        )
        .unwrap();
        common::run_private_bus(
            "a_portal_that_vanishes_is_activated_by_the_rebind",
            Duration::from_secs(20),
            &[
                (CHILD, OsStr::new("1")),
                (ACTIVATED, OsStr::new("1")),
                ("XDG_DATA_HOME", data.as_os_str()),
            ],
        );
        std::fs::remove_dir_all(&data).unwrap();
        return;
    }
    let portal = Mock::start(vec![("invoke", "F8")], 2, false);
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.vanish();
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: Bindings {
                clipboard: Some("F7".into()),
                screenshot: None
            }
        }
    );
    // Stopping the worker closes the session, which ends that process.
    drop(portal);
}

/// Not a test of its own: the process the service file of
/// `a_portal_that_vanishes_is_activated_by_the_rebind` starts.
#[test]
fn activated_portal_process() {
    if std::env::var_os(ACTIVATED).is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let state = Arc::new(State::default());
    let _connection = runtime.block_on(portal_process(
        state.clone(),
        vec![("invoke", "F7")],
        2,
        false,
        false,
    ));
    let deadline = Instant::now() + WAIT + WAIT;
    while state.closed.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_capture_registers_before_its_screenshot_also_after_a_restart() {
    if !private("a_capture_registers_before_its_screenshot_also_after_a_restart") {
        return;
    }
    let shot = shot_path();
    std::fs::write(&shot, b"image").unwrap();
    let mut portal = Mock::without_worker(vec![], 2, false);
    for _ in 0..2 {
        assert_eq!(
            portal.runtime.block_on(screenshot::capture(APP_ID)),
            Ok(shot.clone())
        );
    }
    // A fresh connection each time: each registers, and none is refused.
    assert_eq!(
        portal.log(),
        ["Register", "Screenshot", "Register", "Screenshot"]
    );

    portal.restart(State::default(), vec![]);
    assert_eq!(
        portal.runtime.block_on(screenshot::capture(APP_ID)),
        Ok(shot.clone())
    );
    assert_eq!(portal.log(), ["Register", "Screenshot"]);
    std::fs::remove_file(&shot).unwrap();
}

/// Each way a capture ends without a snapshot, on the wire, and the capture
/// after it: every request is a new interactive one, and the next one works.
#[test]
fn every_failed_capture_can_be_retried_with_a_new_interactive_request() {
    if !private("every_failed_capture_can_be_retried_with_a_new_interactive_request") {
        return;
    }
    let shot = shot_path();
    std::fs::write(&shot, b"image").unwrap();
    let portal = Mock::without_worker(vec![], 2, false);
    let capture = || portal.runtime.block_on(screenshot::capture(APP_ID));
    let missing = format!("file://{}.absent", shot.display());
    let answers = [
        (Shot::Code(1), CaptureFailure::Cancelled),
        (Shot::Code(2), CaptureFailure::Ended),
        (Shot::Uri(missing), CaptureFailure::NoImage),
        (
            Shot::Uri("https://example.org/a.png".to_owned()),
            CaptureFailure::NoImage,
        ),
    ];
    let mut requests = 0;
    for (answer, failure) in answers {
        portal.state.shots.lock().unwrap().push_back(answer);
        assert_eq!(capture(), Err(failure));
        requests += 1;
        assert_eq!(capture(), Ok(shot.clone()));
        requests += 1;
    }

    portal.state.shots.lock().unwrap().push_back(Shot::Refused);
    let Err(CaptureFailure::Failed(detail)) = capture() else {
        panic!("a refused call must fail");
    };
    assert!(detail.starts_with("screenshot: "), "{detail}");
    assert!(detail.contains("screenshot refused"), "{detail}");
    requests += 1;
    assert_eq!(capture(), Ok(shot.clone()));
    requests += 1;

    // The interface goes: the request never reaches the portal.
    let server = portal.connection.object_server();
    assert!(
        !portal
            .runtime
            .block_on(server.remove::<Screenshot, _>(DESKTOP))
            .unwrap()
    );
    assert_eq!(capture(), Err(CaptureFailure::Unavailable));
    assert!(
        portal
            .runtime
            .block_on(server.at(DESKTOP, Screenshot(portal.state.clone())))
            .unwrap()
    );
    assert_eq!(capture(), Ok(shot.clone()));
    requests += 1;

    assert_eq!(
        *portal.state.interactive.lock().unwrap(),
        vec![Some(true); requests]
    );
    std::fs::remove_file(&shot).unwrap();
}
