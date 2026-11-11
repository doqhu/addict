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
    shortcut::{self, Action, Bindings, Limits, ShortcutEvent},
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

/// How the person answers the dialog of GNOME's backend, opened by a bind.
#[derive(Clone)]
enum Dialog {
    /// Accepted, with these rows edited (0 is the clipboard, 1 the screenshot);
    /// the other rows keep what they were filled in with.
    Accept(Vec<(usize, &'static str)>),
    /// Closed or Esc: nothing is stored, and the response has code 2.
    Cancel,
    /// Stays open until [`Mock::answer`].
    Hold,
}

/// A dialog that is open, to be answered.
struct OpenDialog {
    request: OwnedObjectPath,
    rows: Vec<Definition>,
}

/// One portal process: what it was asked, and how it answers.
#[derive(Default)]
struct State {
    /// With a value, the backend works like GNOME's: a bind is answered from
    /// this map, a dialog opens only for an ID not in it, and
    /// `ConfigureShortcuts`, if the frontend lets it through, is not implemented.
    stored: Mutex<Option<HashMap<String, String>>>,
    /// The answers to the next dialogs.
    answers: Mutex<VecDeque<Dialog>>,
    /// The rows of each dialog that opened.
    dialogs: Mutex<Vec<Vec<Definition>>>,
    open_dialog: Mutex<Option<OpenDialog>>,
    /// The IDs of each bind.
    binds: Mutex<Vec<Vec<String>>>,
    /// How many of the next binds the portal refuses.
    refused_binds: AtomicUsize,
    created: AtomicUsize,
    /// Binds made while an older session of this process was open: it would
    /// hold the keys.
    binds_over_a_session: AtomicUsize,
    /// The calls that matter to the order: `Register` must come first.
    log: Mutex<Vec<&'static str>>,
    registered: Mutex<Option<String>>,
    registered_senders: Mutex<HashSet<String>>,
    refuse_register: bool,
    /// The portal has no host Registry, like xdg-desktop-portal 1.18: the
    /// interface is not served and an unregistered caller is let through.
    no_registry: bool,
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
        if self.no_registry || self.registered_senders.lock().unwrap().contains(&sender) {
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
        self.state.created.fetch_add(1, Ordering::SeqCst);
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
        self.state
            .binds
            .lock()
            .unwrap()
            .push(shortcuts.iter().map(|(id, _)| id.clone()).collect());
        if self.state.created.load(Ordering::SeqCst) != self.state.closed.load(Ordering::SeqCst) + 1
        {
            self.state
                .binds_over_a_session
                .fetch_add(1, Ordering::SeqCst);
        }
        let refusals = &self.state.refused_binds;
        if refusals.load(Ordering::SeqCst) > 0 {
            refusals.fetch_sub(1, Ordering::SeqCst);
            return Err(zbus::fdo::Error::Failed("bind refused".to_owned()));
        }
        if self.state.stored.lock().unwrap().is_some() {
            bind_like_gnome(&self.state, connection, request.clone()).await?;
            return Ok(request);
        }
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
    ) -> zbus::fdo::Result<()> {
        self.state.configured.lock().unwrap().push(session.clone());
        if self.state.stored.lock().unwrap().is_some() {
            // GNOME's backend has no such method; a version 2 frontend before
            // xdg-desktop-portal 1.22 passes on gdbus-codegen's answer.
            return Err(zbus::fdo::Error::UnknownMethod(
                "Method ConfigureShortcuts is not implemented on interface \
                 org.freedesktop.impl.portal.GlobalShortcuts"
                    .to_owned(),
            ));
        }
        changed(connection, &session, &[("screenshot", "Super+S")]).await;
        Ok(())
    }
}

/// What the version 1 portal does with a bind: a dialog only for an unknown ID,
/// otherwise an answer at once with what is stored.
async fn bind_like_gnome(
    state: &State,
    connection: &Connection,
    request: OwnedObjectPath,
) -> zbus::Result<()> {
    let rows = state.definitions.lock().unwrap().clone();
    let known = {
        let stored = state.stored.lock().unwrap();
        let stored = stored.as_ref().unwrap();
        rows.iter().all(|row| stored.contains_key(&row.id))
    };
    if known {
        return answer_from_store(state, connection, &request, &rows).await;
    }
    state.dialogs.lock().unwrap().push(rows.clone());
    let answer = state.answers.lock().unwrap().pop_front();
    match answer.unwrap_or(Dialog::Hold) {
        Dialog::Hold => {
            *state.open_dialog.lock().unwrap() = Some(OpenDialog { request, rows });
            Ok(())
        }
        answer => answer_dialog(state, connection, &request, &rows, answer).await,
    }
}

async fn answer_dialog(
    state: &State,
    connection: &Connection,
    request: &OwnedObjectPath,
    rows: &[Definition],
    answer: Dialog,
) -> zbus::Result<()> {
    let Dialog::Accept(edits) = answer else {
        return coded_response(connection, request, 2, Options::new()).await;
    };
    // Accepting replaces the whole list, with every ID: one with no key is
    // stored empty, so it opens no dialog, and the answer omits it.
    let accepted = rows
        .iter()
        .enumerate()
        .map(|(row, definition)| {
            let edited = edits.iter().find(|(index, _)| *index == row);
            let key = edited.map_or_else(
                || {
                    definition
                        .preferred
                        .as_deref()
                        .map_or_else(String::new, described)
                },
                |(_, key)| (*key).to_owned(),
            );
            (definition.id.clone(), key)
        })
        .collect();
    *state.stored.lock().unwrap() = Some(accepted);
    answer_from_store(state, connection, request, rows).await
}

async fn answer_from_store(
    state: &State,
    connection: &Connection,
    request: &OwnedObjectPath,
    rows: &[Definition],
) -> zbus::Result<()> {
    let answered: Shortcuts = {
        let stored = state.stored.lock().unwrap();
        let stored = stored.as_ref().unwrap();
        rows.iter()
            .filter_map(|row| {
                let key = stored.get(&row.id).filter(|key| !key.is_empty())?;
                let (id, options) = descriptors(&[(row.id.as_str(), key.as_str())]).remove(0);
                Some((id, options))
            })
            .collect()
    };
    let shortcuts = OwnedValue::try_from(Value::new(answered)).unwrap();
    response(
        connection,
        request,
        HashMap::from([("shortcuts".to_owned(), shortcuts)]),
    )
    .await
}

/// A preferred trigger as GNOME's dialog shows it once accepted.
fn described(preferred: &str) -> String {
    let mut parts: Vec<&str> = preferred.split('+').collect();
    let key = parts.pop().unwrap();
    let modifiers: String = parts
        .into_iter()
        .map(|modifier| match modifier {
            "CTRL" => "<Control>",
            "LOGO" => "<Super>",
            "SHIFT" => "<Shift>",
            "ALT" => "<Alt>",
            other => panic!("not a modifier of the syntax: {other}"),
        })
        .collect();
    format!("Press {modifiers}{key}")
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
    let mut builder = zbus::connection::Builder::session()
        .unwrap()
        .allow_name_replacements(true)
        .replace_existing_names(replacing)
        .name(PORTAL)
        .unwrap();
    if !state.no_registry {
        builder = builder.serve_at(DESKTOP, Registry(state.clone())).unwrap();
    }
    builder
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
    // First, so that it goes first: a failing test must not leave a worker to
    // see this portal go and activate the system's on the private bus.
    worker: Option<shortcut::ShortcutHandle>,
    runtime: tokio::runtime::Runtime,
    connection: Connection,
    state: Arc<State>,
    events: mpsc::Receiver<ShortcutEvent>,
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
        mock.start_worker(limit, None);
        mock
    }

    /// A version 1 portal that holds `stored`, like GNOME's, with no worker.
    fn gnome(stored: &[(&str, &str)]) -> Self {
        Self::gnome_behind(1, stored)
    }

    /// GNOME's backend behind a frontend of `version`: of version 2, it lets
    /// `ConfigureShortcuts` through, and the backend lacks it.
    fn gnome_behind(version: u32, stored: &[(&str, &str)]) -> Self {
        let mock = Self::without_worker(vec![], version, false);
        *mock.state.stored.lock().unwrap() = Some(
            stored
                .iter()
                .map(|(id, key)| ((*id).to_owned(), (*key).to_owned()))
                .collect(),
        );
        mock
    }

    /// A worker on this portal, as a start of addict is, with `limit` for both
    /// steps of an attempt.
    fn start_worker(&mut self, limit: Duration, file: Option<PathBuf>) {
        self.start_worker_with(
            Limits {
                start: limit,
                answer: limit,
            },
            file,
        );
    }

    fn start_worker_with(&mut self, limits: Limits, file: Option<PathBuf>) {
        let (sender, events) = mpsc::channel();
        self.events = events;
        self.worker = Some(shortcut::start(
            APP_ID,
            "LOGO+q",
            limits,
            file,
            move |event| {
                sender.send(event).unwrap();
            },
        ));
    }

    /// Stops the worker: every session it created is closed.
    fn stop_worker(&mut self) {
        drop(self.worker.take());
        assert_eq!(
            self.state.closed.load(Ordering::SeqCst),
            self.state.created.load(Ordering::SeqCst)
        );
    }

    fn press_configure(&self) {
        self.worker.as_ref().unwrap().configure();
    }

    fn script(&self, answer: Dialog) {
        self.state.answers.lock().unwrap().push_back(answer);
    }

    fn dialogs(&self) -> Vec<Vec<Definition>> {
        self.state.dialogs.lock().unwrap().clone()
    }

    fn binds(&self) -> Vec<Vec<String>> {
        self.state.binds.lock().unwrap().clone()
    }

    fn stored(&self) -> HashMap<String, String> {
        self.state.stored.lock().unwrap().clone().unwrap()
    }

    /// Waits for a [`Dialog::Hold`] to be open.
    fn wait_for_dialog(&self) {
        let deadline = Instant::now() + WAIT;
        while self.state.open_dialog.lock().unwrap().is_none() {
            assert!(Instant::now() < deadline, "no dialog opened");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The person answers the dialog that is open.
    fn answer(&self, answer: Dialog) {
        let open = self.state.open_dialog.lock().unwrap().take().unwrap();
        self.runtime
            .block_on(answer_dialog(
                &self.state,
                &self.connection,
                &open.request,
                &open.rows,
                answer,
            ))
            .unwrap();
    }

    /// No session stayed open under a later bind.
    fn assert_no_bind_over_a_session(&self) {
        assert_eq!(self.state.binds_over_a_session.load(Ordering::SeqCst), 0);
    }

    /// Nothing more arrives from the worker for a moment.
    fn assert_quiet(&self) {
        assert_eq!(
            self.events.recv_timeout(Duration::from_millis(300)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
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

/// Where a test keeps the generation file: a new directory of its own, in the
/// child that has the private bus.
struct GenerationDir(PathBuf);

impl GenerationDir {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "addict-test-generation-{}-{test}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("shortcut-generation")
    }

    fn saved(&self) -> Option<String> {
        std::fs::read_to_string(self.file()).ok()
    }
}

impl Drop for GenerationDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SUPER_Q: &str = "Press <Super>q";
const CTRL_SUPER_E: &str = "Press <Control><Super>e";

fn stored_keys() -> [(&'static str, &'static str); 2] {
    [("invoke", SUPER_Q), ("screenshot", CTRL_SUPER_E)]
}

fn bindings(clipboard: Option<&str>, screenshot: Option<&str>) -> Bindings {
    Bindings {
        clipboard: clipboard.map(str::to_owned),
        screenshot: screenshot.map(str::to_owned),
    }
}

fn ids(generation: &[&str]) -> Vec<String> {
    generation.iter().map(|id| (*id).to_owned()).collect()
}

/// A person changes both keys, then changes one again, on GNOME's backend
/// behind a frontend of `version`.
fn change_keys_through_the_dialog(version: u32) {
    let dir = GenerationDir::new("change");
    let mut portal = Mock::gnome_behind(version, &[("invoke", SUPER_Q), ("screenshot", "")]);
    portal.start_worker(WAIT, Some(dir.file()));
    let old = bindings(Some(SUPER_Q), None);
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: old.clone()
        }
    );
    assert!(portal.dialogs().is_empty(), "stored keys open no dialog");

    portal.script(Dialog::Accept(vec![
        (0, "Press <Super>x"),
        (1, "Press <Super>F8"),
    ]));
    portal.press_configure();
    assert_eq!(portal.event(), ShortcutEvent::Rebinding { bindings: old });
    let new = bindings(Some("Press <Super>x"), Some("Press <Super>F8"));
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: new.clone()
        }
    );
    assert_eq!(portal.event(), ShortcutEvent::RebindEnded { message: None });
    // A version 1 frontend refuses the dialog itself; version 2 asks the backend.
    let asked = |presses| if version == 1 { 0 } else { presses };
    assert_eq!(portal.state.configured.lock().unwrap().len(), asked(1));
    assert_eq!(
        portal.binds(),
        [
            ids(&["invoke", "screenshot"]),
            ids(&["invoke-1", "screenshot-1"])
        ]
    );
    assert_eq!(portal.state.closed.load(Ordering::SeqCst), 1);
    portal.assert_no_bind_over_a_session();
    assert_eq!(dir.saved().as_deref(), Some("1\n"));

    // The new keys work, and an old ID is no action of this session.
    let session = portal.session();
    portal.activated(&session, "invoke", "old-id", 1);
    portal.activated(&session, "screenshot-1", "new-id", 90);
    assert_eq!(
        portal.event(),
        ShortcutEvent::Activated {
            action: Action::Screenshot,
            activation_token: Some("new-id".into()),
            timestamp_ms: 90
        }
    );

    // Assignments that change later in this session still follow the new IDs.
    portal.changed(&session, &[("screenshot-1", "Press <Super>F9")]);
    assert_eq!(
        portal.event(),
        ShortcutEvent::Changed {
            bindings: bindings(Some("Press <Super>x"), Some("Press <Super>F9"))
        }
    );

    // The next change uses the next generation.
    portal.script(Dialog::Accept(vec![]));
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    assert_eq!(portal.event(), ShortcutEvent::RebindEnded { message: None });
    assert_eq!(portal.binds()[2], ids(&["invoke-2", "screenshot-2"]));
    assert_eq!(dir.saved().as_deref(), Some("2\n"));
    assert_eq!(portal.state.configured.lock().unwrap().len(), asked(2));
    portal.stop_worker();
}

#[test]
fn a_version_1_portal_changes_keys_through_its_dialog_under_new_ids() {
    if !private("a_version_1_portal_changes_keys_through_its_dialog_under_new_ids") {
        return;
    }
    change_keys_through_the_dialog(1);
}

#[test]
fn a_version_2_frontend_without_the_backend_method_changes_keys_under_new_ids() {
    if !private("a_version_2_frontend_without_the_backend_method_changes_keys_under_new_ids") {
        return;
    }
    change_keys_through_the_dialog(2);
}

/// A stand-in `dconf` first on the `PATH` leaves a mark if anything runs it.
#[test]
fn the_rebind_never_runs_dconf() {
    if std::env::var_os(CHILD).is_some() {
        change_keys_through_the_dialog(1);
        return;
    }
    let dir = std::env::temp_dir().join(format!("addict-test-dconf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mark = dir.join("ran");
    let stand_in = dir.join("dconf");
    std::fs::write(&stand_in, format!("#!/bin/sh\n: > '{}'\n", mark.display())).unwrap();
    std::fs::set_permissions(
        &stand_in,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let mut path = std::ffi::OsString::from(&dir);
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap());
    common::run_private_bus(
        "the_rebind_never_runs_dconf",
        Duration::from_secs(20),
        &[(CHILD, OsStr::new("1")), ("PATH", path.as_os_str())],
    );
    assert!(!mark.exists(), "dconf was run");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn changing_one_key_keeps_the_other_through_its_filled_in_row() {
    if !private("changing_one_key_keeps_the_other_through_its_filled_in_row") {
        return;
    }
    let dir = GenerationDir::new("keep");
    let mut portal = Mock::gnome(&stored_keys());
    portal.start_worker(WAIT, Some(dir.file()));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.script(Dialog::Accept(vec![(0, "Press F7")]));
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: bindings(Some("Press F7"), Some(CTRL_SUPER_E))
        }
    );
    let dialogs = portal.dialogs();
    assert_eq!(dialogs.len(), 1);
    assert_eq!(dialogs[0][0].preferred.as_deref(), Some("LOGO+q"));
    assert_eq!(dialogs[0][1].preferred.as_deref(), Some("CTRL+LOGO+e"));
    assert_eq!(
        portal.stored(),
        HashMap::from([
            ("invoke-1".to_owned(), "Press F7".to_owned()),
            ("screenshot-1".to_owned(), CTRL_SUPER_E.to_owned()),
        ])
    );
    portal.stop_worker();
}

/// A description that can't be read gives no preference: that row is empty.
#[test]
fn a_key_that_cannot_be_read_leaves_its_row_empty() {
    if !private("a_key_that_cannot_be_read_leaves_its_row_empty") {
        return;
    }
    let dir = GenerationDir::new("unreadable");
    let mut portal = Mock::gnome(&[("invoke", SUPER_Q), ("screenshot", "Some other sentence")]);
    portal.start_worker(WAIT, Some(dir.file()));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.script(Dialog::Accept(vec![]));
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: bindings(Some(SUPER_Q), None)
        }
    );
    assert_eq!(portal.dialogs()[0][1].preferred, None);
    portal.stop_worker();
}

#[test]
fn a_cancelled_dialog_returns_to_the_old_keys_and_says_so() {
    if !private("a_cancelled_dialog_returns_to_the_old_keys_and_says_so") {
        return;
    }
    let dir = GenerationDir::new("cancel");
    let mut portal = Mock::gnome(&stored_keys());
    portal.start_worker(WAIT, Some(dir.file()));
    let old = bindings(Some(SUPER_Q), Some(CTRL_SUPER_E));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.script(Dialog::Cancel);
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: old.clone()
        }
    );
    let ShortcutEvent::RebindEnded {
        message: Some(message),
    } = portal.event()
    else {
        panic!("the end of the rebind must say what happened");
    };
    assert_eq!(message, "No new keys were chosen. The keys are unchanged.");
    assert_eq!(portal.dialogs().len(), 1, "going back opens no dialog");
    assert_eq!(
        portal.binds(),
        [
            ids(&["invoke", "screenshot"]),
            ids(&["invoke-1", "screenshot-1"]),
            ids(&["invoke", "screenshot"]),
        ]
    );
    portal.assert_no_bind_over_a_session();
    assert_eq!(dir.saved(), None, "the generation is unchanged");
    assert_eq!(portal.stored().len(), 2, "cancelling stores nothing");
    portal.activated(&portal.session(), "invoke", "still-works", 91);
    assert!(matches!(
        portal.event(),
        ShortcutEvent::Activated {
            action: Action::Clipboard,
            timestamp_ms: 91,
            ..
        }
    ));
    portal.stop_worker();
}

#[test]
fn a_restart_binds_the_saved_generation_without_a_dialog() {
    if !private("a_restart_binds_the_saved_generation_without_a_dialog") {
        return;
    }
    let dir = GenerationDir::new("restart");
    let mut portal = Mock::gnome(&stored_keys());
    portal.start_worker(WAIT, Some(dir.file()));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.script(Dialog::Accept(vec![(1, "Press F6")]));
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    assert!(matches!(portal.event(), ShortcutEvent::RebindEnded { .. }));
    portal.stop_worker();

    portal.start_worker(WAIT, Some(dir.file()));
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: bindings(Some(SUPER_Q), Some("Press F6"))
        }
    );
    assert_eq!(portal.dialogs().len(), 1, "no second dialog");
    assert_eq!(
        portal.binds().last().unwrap(),
        &ids(&["invoke-1", "screenshot-1"])
    );
    portal.activated(&portal.session(), "invoke-1", "after-restart", 92);
    assert!(matches!(
        portal.event(),
        ShortcutEvent::Activated {
            action: Action::Clipboard,
            timestamp_ms: 92,
            ..
        }
    ));
    portal.stop_worker();

    // With the file lost, the worker starts over at generation 0, whose IDs
    // are no longer stored: the dialog opens, and that is all that happens.
    std::fs::remove_file(dir.file()).unwrap();
    portal.script(Dialog::Accept(vec![(0, "Press F5"), (1, "Press F4")]));
    portal.start_worker(WAIT, Some(dir.file()));
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: bindings(Some("Press F5"), Some("Press F4"))
        }
    );
    assert_eq!(portal.dialogs().len(), 2);
    assert_eq!(
        portal.binds().last().unwrap(),
        &ids(&["invoke", "screenshot"])
    );
    portal.stop_worker();

    // So with a file that holds no number.
    std::fs::write(dir.file(), "not a number\n").unwrap();
    portal.start_worker(WAIT, Some(dir.file()));
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: bindings(Some("Press F5"), Some("Press F4"))
        }
    );
    assert_eq!(portal.dialogs().len(), 2);
    portal.stop_worker();
}

#[test]
fn keys_that_cannot_be_remembered_work_now_and_settings_is_told() {
    if !private("keys_that_cannot_be_remembered_work_now_and_settings_is_told") {
        return;
    }
    let dir = GenerationDir::new("save");
    // A regular file stands where the directory of the file would be.
    let blocker = dir.0.join("blocker");
    std::fs::write(&blocker, "").unwrap();
    let file = blocker.join("shortcut-generation");
    let mut portal = Mock::gnome(&stored_keys());
    portal.start_worker(WAIT, Some(file.clone()));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.script(Dialog::Accept(vec![(0, "Press F7")]));
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    let ShortcutEvent::RebindEnded {
        message: Some(message),
    } = portal.event()
    else {
        panic!("the end of the rebind must say the keys were not saved");
    };
    assert!(message.contains("next start"), "{message}");
    assert!(!message.contains('\n'), "{message}");
    portal.activated(&portal.session(), "invoke-1", "this-run", 93);
    assert!(matches!(
        portal.event(),
        ShortcutEvent::Activated {
            action: Action::Clipboard,
            timestamp_ms: 93,
            ..
        }
    ));
    portal.stop_worker();

    // As it said: at the next start the dialog returns.
    portal.script(Dialog::Accept(vec![]));
    portal.start_worker(WAIT, Some(file));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    assert_eq!(portal.dialogs().len(), 2);
    portal.stop_worker();
}

#[test]
fn a_refused_rebind_returns_to_the_old_keys() {
    if !private("a_refused_rebind_returns_to_the_old_keys") {
        return;
    }
    let dir = GenerationDir::new("refused");
    let mut portal = Mock::gnome(&stored_keys());
    portal.start_worker(WAIT, Some(dir.file()));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.state.refused_binds.store(1, Ordering::SeqCst);
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: bindings(Some(SUPER_Q), Some(CTRL_SUPER_E))
        }
    );
    let ShortcutEvent::RebindEnded {
        message: Some(message),
    } = portal.event()
    else {
        panic!("the end of the rebind must say what happened");
    };
    assert!(message.contains("unchanged"), "{message}");
    assert!(message.contains("bind refused"), "{message}");
    assert_eq!(dir.saved(), None);
    assert_eq!(
        portal.binds().last().unwrap(),
        &ids(&["invoke", "screenshot"])
    );
    portal.assert_no_bind_over_a_session();
    portal.stop_worker();
}

#[test]
fn a_rebind_that_fails_twice_is_a_failure_with_retry_rereading_the_file() {
    if !private("a_rebind_that_fails_twice_is_a_failure_with_retry_rereading_the_file") {
        return;
    }
    let dir = GenerationDir::new("twice");
    let mut portal = Mock::gnome(&stored_keys());
    portal.start_worker(WAIT, Some(dir.file()));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.state.refused_binds.store(2, Ordering::SeqCst);
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    let ShortcutEvent::Failed { message } = portal.event() else {
        panic!("the second failure must be the end");
    };
    assert!(message.contains("bind refused"), "{message}");
    assert_eq!(
        portal.events.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
    assert_eq!(dir.saved(), None);
    portal.stop_worker();

    // What Retry does: a new worker.
    portal.start_worker(WAIT, Some(dir.file()));
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: bindings(Some(SUPER_Q), Some(CTRL_SUPER_E))
        }
    );
    portal.stop_worker();
}

#[test]
fn a_portal_restarting_during_the_dialog_binds_the_old_ids_on_a_new_attempt() {
    if !private("a_portal_restarting_during_the_dialog_binds_the_old_ids_on_a_new_attempt") {
        return;
    }
    let dir = GenerationDir::new("left");
    let mut portal = Mock::gnome(&stored_keys());
    portal.start_worker(WAIT, Some(dir.file()));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    portal.wait_for_dialog();

    let stored = portal.stored();
    let old = portal.restart(
        State {
            stored: Mutex::new(Some(stored)),
            ..State::default()
        },
        vec![],
    );
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: bindings(Some(SUPER_Q), Some(CTRL_SUPER_E))
        }
    );
    let ShortcutEvent::RebindEnded {
        message: Some(message),
    } = portal.event()
    else {
        panic!("the end of the rebind must say what happened");
    };
    assert!(message.contains("unchanged"), "{message}");
    assert_eq!(portal.log(), ["Register", "CreateSession"]);
    assert_eq!(portal.binds(), [ids(&["invoke", "screenshot"])]);
    // The first session was closed for the rebind; the one of the dialog went
    // with the portal.
    assert_eq!(old.closed.load(Ordering::SeqCst), 1);
    assert_eq!(dir.saved(), None);
    portal.stop_worker();
}

#[test]
fn a_press_during_a_rebind_is_dropped_when_it_ends() {
    if !private("a_press_during_a_rebind_is_dropped_when_it_ends") {
        return;
    }
    let dir = GenerationDir::new("double");
    let mut portal = Mock::gnome(&stored_keys());
    portal.start_worker(WAIT, Some(dir.file()));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    portal.wait_for_dialog();
    portal.press_configure();
    portal.press_configure();
    portal.answer(Dialog::Accept(vec![]));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    assert_eq!(portal.event(), ShortcutEvent::RebindEnded { message: None });
    portal.assert_quiet();
    assert_eq!(portal.dialogs().len(), 1, "no second dialog");

    // Drained, not stuck: the next press works.
    portal.script(Dialog::Cancel);
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    assert!(matches!(portal.event(), ShortcutEvent::RebindEnded { .. }));
    assert_eq!(portal.dialogs().len(), 2);
    portal.stop_worker();
}

/// Stopping addict with the dialog open changes nothing.
#[test]
fn stopping_during_the_dialog_saves_nothing() {
    if !private("stopping_during_the_dialog_saves_nothing") {
        return;
    }
    let dir = GenerationDir::new("stop");
    let mut portal = Mock::gnome(&stored_keys());
    portal.start_worker(WAIT, Some(dir.file()));
    assert!(matches!(portal.event(), ShortcutEvent::Bound { .. }));
    portal.press_configure();
    assert!(matches!(portal.event(), ShortcutEvent::Rebinding { .. }));
    portal.wait_for_dialog();
    portal.stop_worker();
    assert_eq!(dir.saved(), None);
    assert_eq!(
        portal.events.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
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
        common::write_bus_config(&config, Some(&services));
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

/// On a first launch GNOME opens its dialog for the first bind, and the
/// window may cover it: an answer after the start limit still binds.
#[test]
fn a_first_bind_answered_after_the_start_limit_is_bound() {
    if !private("a_first_bind_answered_after_the_start_limit_is_bound") {
        return;
    }
    let dir = GenerationDir::new("first");
    let mut portal = Mock::gnome(&[]);
    let start = Duration::from_millis(500);
    portal.start_worker_with(
        Limits {
            start,
            answer: WAIT,
        },
        Some(dir.file()),
    );
    portal.wait_for_dialog();
    std::thread::sleep(start * 3);
    portal.answer(Dialog::Accept(vec![(0, SUPER_Q)]));
    assert_eq!(
        portal.event(),
        ShortcutEvent::Bound {
            bindings: bindings(Some(SUPER_Q), None)
        }
    );
    assert_eq!(portal.dialogs().len(), 1);
    portal.close();
}

/// A dialog nobody answers ends at the answer limit, named in the failure.
#[test]
fn an_unanswered_first_bind_fails_at_the_answer_limit() {
    if !private("an_unanswered_first_bind_fails_at_the_answer_limit") {
        return;
    }
    let mut portal = Mock::gnome(&[]);
    let limits = Limits {
        start: Duration::from_millis(300),
        answer: Duration::from_millis(1500),
    };
    let started = Instant::now();
    portal.start_worker_with(limits, None);
    assert_eq!(
        portal.event(),
        ShortcutEvent::Failed {
            message: "binding the shortcut: no result within 1.5s".to_owned()
        }
    );
    assert!(started.elapsed() >= limits.answer);
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

#[test]
fn a_capture_without_a_host_registry_goes_on_unregistered() {
    if !private("a_capture_without_a_host_registry_goes_on_unregistered") {
        return;
    }
    let shot = shot_path();
    std::fs::write(&shot, b"image").unwrap();
    let mut portal = Mock::without_worker(vec![], 2, false);
    portal.restart(
        State {
            no_registry: true,
            ..State::default()
        },
        vec![],
    );
    assert_eq!(
        portal.runtime.block_on(screenshot::capture(APP_ID)),
        Ok(shot.clone())
    );
    // No registration happened, and the screenshot was still asked interactively.
    assert_eq!(portal.log(), ["Screenshot"]);
    assert_eq!(*portal.state.interactive.lock().unwrap(), [Some(true)]);
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
