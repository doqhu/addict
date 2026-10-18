//! Which application was in front, as the accessibility bus told it.
//!
//! [`watch`] starts one worker thread that holds one connection to the
//! accessibility bus and a few strings: the bus name of the application that
//! was activated last and whether it is Android Studio. It never reads a
//! tree and never polls: it waits for `Activate` and `Deactivate` of
//! `org.a11y.atspi.Event.Window` and for `StateChanged` with the state
//! `active` of `org.a11y.atspi.Event.Object`. Both kinds are listened to
//! because which of them an application emits depends on its toolkit; an
//! activation seen twice is the same activation.
//!
//! With accessibility off the worker holds no connection to the
//! accessibility bus at all: it waits for `IsEnabled` of `org.a11y.Status`
//! on the session bus to change, and subscribes when it turns true. It
//! changes no setting.
//!
//! [`Foreground::source`] is what the press of the shortcut snapshots, and
//! [`Foreground::accessibility`] whether accessibility is on, as the worker
//! last read `IsEnabled`. The product's own window needs no identification
//! here: it is another application, [`Source::Other`].
//!
//! Android Studio on Wayland sends neither of these signals, but the
//! `ACTIVE` state of its frame follows the focus. [`Foreground::probe`] asks
//! the worker, once, whether a frame of Android Studio is `ACTIVE` now: on
//! the connection it holds, with the names of Android Studio it has kept,
//! by the registry's `GetChildren`, `Name`, the `GetChildren` of Android
//! Studio's root and `GetState`, never a text, within `CALL_LIMIT`. It is
//! asked at a press only, never on a timer. Without a connection, while it
//! still subscribes, with accessibility off or not known, it answers false
//! at once; so it does when the probe fails or Android Studio is silent,
//! which is logged.

use std::{
    cell::Cell,
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    thread::JoinHandle,
    time::Duration,
};

use futures_util::{
    StreamExt,
    future::join_all,
    stream::{self, SelectAll},
};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};
use zbus::{Connection, MatchRule, MessageStream, zvariant::OwnedValue};

use crate::atspi::{self, A11Y_BUS_NAME, A11Y_BUS_PATH, CALL_LIMIT, Locate, Names, REGISTRY_NAME};

const STATUS: &str = "org.a11y.Status";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const REGISTRY_PATH: &str = "/org/a11y/atspi/registry";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const EVENT_WINDOW: &str = "org.a11y.atspi.Event.Window";
const EVENT_OBJECT: &str = "org.a11y.atspi.Event.Object";

/// What the registry is told to forward to the accessibility bus.
const REGISTERED: [&str; 3] = [
    "window:activate",
    "window:deactivate",
    "object:state-changed:active",
];

/// The match rules of the signals the worker waits for.
const RULES: [&str; 3] = [
    "type='signal',interface='org.a11y.atspi.Event.Window',member='Activate'",
    "type='signal',interface='org.a11y.atspi.Event.Window',member='Deactivate'",
    "type='signal',interface='org.a11y.atspi.Event.Object',member='StateChanged',arg0='active'",
];

/// How long the first answer of `org.a11y.Bus` may take: the service may
/// have to be started for it.
const STATUS_LIMIT: Duration = Duration::from_secs(10);

/// What application was in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// The application activated last is Android Studio.
    AndroidStudio,
    /// It is another application.
    Other,
    /// Nothing is known: no activation was seen, the active application was
    /// deactivated, its name was not given or accessibility is off.
    #[default]
    Unknown,
}

/// Whether accessibility is on, as `IsEnabled` of `org.a11y.Status` was last
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Accessibility {
    /// `IsEnabled` was read as true.
    On,
    /// `IsEnabled` was read as false.
    Off,
    /// The status was not read yet, or cannot be had: the listener is not
    /// running, or the session bus or `org.a11y.Bus` did not answer.
    #[default]
    Unknown,
}

/// What the worker shares with the handle: the tracker and the status of
/// accessibility.
#[derive(Debug, Default)]
struct Shared {
    tracker: Tracker,
    accessibility: Accessibility,
}

impl Shared {
    /// Locks `shared`; a worker or a caller that panicked holding it leaves
    /// what it wrote, which is still a state of its own.
    fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
        shared.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The application activated last, by its bus name, and what it is. A pure
/// state: the worker feeds it what it hears and the press reads it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tracker {
    active: Option<(String, Source)>,
}

impl Tracker {
    /// `bus_name` was activated; what it is is not known yet.
    pub fn activate(&mut self, bus_name: &str) {
        self.active = Some((bus_name.to_owned(), Source::Unknown));
    }

    /// What `bus_name` turned out to be. Ignored when another application
    /// has been activated or the active one deactivated in the meantime.
    pub fn name(&mut self, bus_name: &str, source: Source) {
        if let Some((active, kept)) = &mut self.active
            && active == bus_name
        {
            *kept = source;
        }
    }

    /// `bus_name` was deactivated. Nothing is active then, unless another
    /// application was activated already.
    pub fn deactivate(&mut self, bus_name: &str) {
        if self
            .active
            .as_ref()
            .is_some_and(|(active, _)| active == bus_name)
        {
            self.active = None;
        }
    }

    /// Forgets everything: the bus was lost or accessibility is off.
    pub fn clear(&mut self) {
        self.active = None;
    }

    /// What the active application is, if `bus_name` is the active one.
    pub fn source_of(&self, bus_name: &str) -> Option<Source> {
        self.active
            .as_ref()
            .filter(|(active, _)| active == bus_name)
            .map(|(_, source)| *source)
    }

    /// What the application activated last is; [`Source::Unknown`] when
    /// nothing is active.
    pub fn source(&self) -> Source {
        self.active
            .as_ref()
            .map_or(Source::Unknown, |(_, source)| *source)
    }
}

/// What one signal says about an application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Active,
    Inactive,
}

/// What a signal means, from its interface and member and, for
/// `StateChanged`, the state it is about and the value that was set. Other
/// signals and other states mean nothing.
fn change_of(interface: &str, member: &str, state: Option<(&str, i32)>) -> Option<Change> {
    match (interface, member) {
        (EVENT_WINDOW, "Activate") => Some(Change::Active),
        (EVENT_WINDOW, "Deactivate") => Some(Change::Inactive),
        (EVENT_OBJECT, "StateChanged") => match state? {
            ("active", 0) => Some(Change::Inactive),
            ("active", _) => Some(Change::Active),
            _ => None,
        },
        _ => None,
    }
}

type StateChangedBody = (String, i32, i32, OwnedValue, HashMap<String, OwnedValue>);

/// The tracker with the callback for its changes. Lives on the worker.
struct Observer {
    shared: Arc<Mutex<Shared>>,
    last: Cell<Source>,
    on_change: Box<dyn Fn(Source) + Send>,
}

impl Observer {
    /// Runs `update` on the tracker and reports the source if it changed.
    fn with<R>(&self, update: impl FnOnce(&mut Tracker) -> R) -> R {
        let mut shared = Shared::lock(&self.shared);
        let result = update(&mut shared.tracker);
        let now = shared.tracker.source();
        drop(shared);
        if self.last.replace(now) != now {
            (self.on_change)(now);
        }
        result
    }

    /// What `IsEnabled` was read as, or [`Accessibility::Unknown`] when the
    /// status can no longer be had.
    fn set_accessibility(&self, accessibility: Accessibility) {
        Shared::lock(&self.shared).accessibility = accessibility;
    }
}

/// The answer of one [`Foreground::probe`]: whether a frame of Android
/// Studio was `ACTIVE`. An error is a probe that was dropped unanswered: the
/// worker is gone.
pub type Probe = oneshot::Receiver<bool>;

/// Where the worker takes the probes: each is the sender of its answer.
type Probes = mpsc::UnboundedReceiver<oneshot::Sender<bool>>;

/// Keeps the worker alive. Dropping it stops the worker and waits for the
/// thread; nothing in the worker blocks, so that is at once.
#[derive(Debug)]
pub struct Foreground {
    shared: Arc<Mutex<Shared>>,
    stop: Option<oneshot::Sender<()>>,
    probes: mpsc::UnboundedSender<oneshot::Sender<bool>>,
    thread: Option<JoinHandle<()>>,
}

impl Foreground {
    /// What the application activated last is, now.
    pub fn source(&self) -> Source {
        Shared::lock(&self.shared).tracker.source()
    }

    /// Whether accessibility is on, as the worker last read it.
    pub fn accessibility(&self) -> Accessibility {
        Shared::lock(&self.shared).accessibility
    }

    /// Asks the worker once whether a frame of Android Studio is `ACTIVE`
    /// now, and returns at once; the answer comes within 500 ms, the call
    /// limit of the adapter, of the worker taking the probe. False when it
    /// cannot be asked: no worker, no connection, accessibility off or not
    /// known.
    pub fn probe(&self) -> Probe {
        let (reply, answer) = oneshot::channel();
        if let Err(mpsc::error::SendError(reply)) = self.probes.send(reply) {
            let _ = reply.send(false);
        }
        answer
    }
}

impl Drop for Foreground {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            // The worker may already be gone.
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts the listener on the session's buses. `on_change` runs on the
/// worker thread, with the new source, each time what [`Foreground::source`]
/// gives changes; it must not block.
pub fn watch(on_change: impl Fn(Source) + Send + 'static) -> Foreground {
    watch_at(None, Locate::Session, on_change)
}

/// [`watch`] with the accessibility bus at `locate` and `org.a11y.Status`
/// on the bus at `status_bus`, or on the session bus when that is `None`.
pub fn watch_at(
    status_bus: Option<&str>,
    locate: Locate,
    on_change: impl Fn(Source) + Send + 'static,
) -> Foreground {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let (stop, stopped) = oneshot::channel();
    let (probes, mut asked) = mpsc::unbounded_channel();
    let observer = Observer {
        shared: shared.clone(),
        last: Cell::new(Source::Unknown),
        on_change: Box::new(on_change),
    };
    let status_bus = status_bus.map(str::to_owned);
    let spawned = std::thread::Builder::new()
        .name("addict-foreground".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    eprintln!("addict: foreground listener runtime: {e}");
                    return;
                }
            };
            runtime.block_on(async {
                tokio::select! {
                    // Resolves on the signal and when the handle is gone.
                    _ = stopped => {}
                    () = serve(status_bus.as_deref(), &locate, &observer, &mut asked) => {
                        // The status can no longer be had.
                        observer.set_accessibility(Accessibility::Unknown);
                    }
                }
            });
        });
    match spawned {
        Ok(thread) => Foreground {
            shared,
            stop: Some(stop),
            probes,
            thread: Some(thread),
        },
        Err(e) => {
            eprintln!("addict: foreground listener thread: {e}");
            Foreground {
                shared,
                stop: None,
                probes,
                thread: None,
            }
        }
    }
}

/// The session bus, or the bus given for the status.
async fn status_connection(status_bus: Option<&str>) -> Result<Connection, String> {
    match status_bus {
        None => Connection::session().await.map_err(|e| e.to_string()),
        Some(address) => zbus::connection::Builder::address(address)
            .map_err(|e| e.to_string())?
            .build()
            .await
            .map_err(|e| e.to_string()),
    }
}

/// Runs `future` and answers every probe false meanwhile: no accessibility
/// connection is held.
async fn refusing<F: Future>(probes: &mut Probes, future: F) -> F::Output {
    tokio::pin!(future);
    loop {
        tokio::select! {
            output = &mut future => return output,
            Some(reply) = probes.recv() => {
                let _ = reply.send(false);
            }
        }
    }
}

/// Follows `IsEnabled` and listens while it is true; each read of it is
/// what [`Foreground::accessibility`] gives. Returns only when the status
/// cannot be had at all; the source stays unknown then, and the caller sets
/// the accessibility to unknown. A probe is answered by [`listen`] once it
/// follows the events on the accessibility connection, and false at once
/// otherwise.
async fn serve(
    status_bus: Option<&str>,
    locate: &Locate,
    observer: &Observer,
    probes: &mut Probes,
) {
    let connection = match refusing(probes, status_connection(status_bus)).await {
        Ok(connection) => connection,
        Err(e) => return eprintln!("addict: foreground listener, session bus: {e}"),
    };
    let rule = format!(
        "type='signal',path='{A11Y_BUS_PATH}',interface='{PROPERTIES}',member='PropertiesChanged'"
    );
    let rule = match MatchRule::try_from(rule.as_str()) {
        Ok(rule) => rule,
        Err(e) => return eprintln!("addict: foreground listener, status rule: {e}"),
    };
    let mut changes = match refusing(
        probes,
        MessageStream::for_match_rule(rule, &connection, None),
    )
    .await
    {
        Ok(changes) => changes,
        Err(e) => return eprintln!("addict: foreground listener, status subscription: {e}"),
    };
    let status = atspi::Object::new(&connection, A11Y_BUS_NAME, A11Y_BUS_PATH);
    loop {
        // Read after the subscription: a change in between is not lost.
        let read = timeout(STATUS_LIMIT, status.property::<bool>(STATUS, "IsEnabled"));
        let enabled = match refusing(probes, read).await {
            Ok(Ok(enabled)) => enabled,
            Ok(Err(e)) => return eprintln!("addict: foreground listener, IsEnabled: {e}"),
            Err(_) => return eprintln!("addict: foreground listener, IsEnabled: no answer"),
        };
        observer.set_accessibility(if enabled {
            Accessibility::On
        } else {
            Accessibility::Off
        });
        let mut ended = false;
        if enabled {
            tokio::select! {
                () = listen(locate, observer, probes) => ended = true,
                changed = status_changed(&mut changes) => {
                    if !changed {
                        return;
                    }
                }
            }
        }
        // Whatever was seen belongs to a connection that is gone or never
        // was.
        observer.with(Tracker::clear);
        if (ended || !enabled) && !refusing(probes, status_changed(&mut changes)).await {
            return;
        }
    }
}

/// Whether a `PropertiesChanged` of `interface` is about `IsEnabled` of
/// `org.a11y.Status`, changed or invalidated.
fn is_enabled_changed(
    interface: &str,
    changed: &HashMap<String, OwnedValue>,
    invalidated: &[String],
) -> bool {
    interface == STATUS
        && (changed.contains_key("IsEnabled") || invalidated.iter().any(|n| n == "IsEnabled"))
}

/// Waits for the next change of `IsEnabled` of `org.a11y.Status`; another
/// property of it, such as `ScreenReaderEnabled`, is not one. False when the
/// stream ended.
async fn status_changed(changes: &mut MessageStream) -> bool {
    while let Some(message) = changes.next().await {
        let Ok(message) = message else { continue };
        let body = message.body();
        let Ok((interface, changed, invalidated)) =
            body.deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
        else {
            continue;
        };
        if is_enabled_changed(&interface, &changed, &invalidated) {
            return true;
        }
    }
    false
}

/// Holds the accessibility connection and tracks what it hears, until the
/// connection ends or fails; meanwhile it answers the probes on it, each on
/// its own, beside the events.
async fn listen(locate: &Locate, observer: &Observer, probes: &mut Probes) {
    let connection = match refusing(probes, atspi::open(locate)).await {
        Ok(connection) => connection,
        Err(e) => return eprintln!("addict: foreground listener: {e:?}"),
    };
    // Until the events are followed, a probe is answered false at once.
    let Some(mut streams) = refusing(probes, subscribe(&connection)).await else {
        return;
    };
    // Of this connection only: a new one may be another bus.
    let names = Names::default();
    let answering = stream::poll_fn(|context| probes.poll_recv(context))
        .for_each_concurrent(None, |reply| answer(&connection, &names, reply));
    tokio::select! {
        () = follow(&connection, &names, observer, &mut streams) => {}
        () = answering => {
            // The handle is gone: the worker stops with it.
            std::future::pending::<()>().await;
        }
    }
}

/// Subscribes to the signals on `connection` and asks the registry for them;
/// none when a subscription fails.
async fn subscribe(connection: &Connection) -> Option<SelectAll<MessageStream>> {
    let opened = join_all(RULES.iter().map(|rule| async {
        let rule = MatchRule::try_from(*rule)?;
        MessageStream::for_match_rule(rule, connection, None).await
    }))
    .await;
    let mut streams: SelectAll<MessageStream> = stream::select_all(Vec::new());
    for opened in opened {
        match opened {
            Ok(stream) => streams.push(stream),
            Err(e) => {
                eprintln!("addict: foreground listener, subscription: {e}");
                return None;
            }
        }
    }
    // After the rules: the first event an application sends is heard.
    for event in REGISTERED {
        register(connection, event).await;
    }
    Some(streams)
}

/// Answers one probe on `connection`: whether a frame of Android Studio is
/// `ACTIVE`, false when that cannot be had within [`CALL_LIMIT`].
async fn answer(connection: &Connection, names: &Names, reply: oneshot::Sender<bool>) {
    let active = match timeout(CALL_LIMIT, atspi::studio_active(connection, names)).await {
        Ok(Ok(active)) => active,
        Ok(Err(e)) => {
            eprintln!("addict: foreground probe: {e}");
            false
        }
        Err(_) => {
            eprintln!("addict: foreground probe: no answer within {CALL_LIMIT:?}");
            false
        }
    };
    let _ = reply.send(active);
}

/// Tracks what `streams` hear on `connection`, until they end.
async fn follow(
    connection: &Connection,
    names: &Names,
    observer: &Observer,
    streams: &mut SelectAll<MessageStream>,
) {
    while let Some(message) = streams.next().await {
        let Ok(message) = message else { continue };
        let header = message.header();
        let (Some(sender), Some(interface), Some(member)) =
            (header.sender(), header.interface(), header.member())
        else {
            continue;
        };
        let body = message.body();
        let state = body.deserialize::<StateChangedBody>().ok();
        let kind = state
            .as_ref()
            .map(|(kind, detail, ..)| (kind.as_str(), *detail));
        let Some(change) = change_of(interface.as_str(), member.as_str(), kind) else {
            continue;
        };
        let bus_name = sender.as_str();
        match change {
            Change::Inactive => observer.with(|tracker| tracker.deactivate(bus_name)),
            Change::Active => {
                // The same activation seen through the other signal.
                let known = observer.with(|tracker| tracker.source_of(bus_name));
                if known.is_some_and(|source| source != Source::Unknown) {
                    continue;
                }
                observer.with(|tracker| tracker.activate(bus_name));
                let source = source_of(connection, names, bus_name).await;
                observer.with(|tracker| tracker.name(bus_name, source));
            }
        }
    }
}

/// Tells the registry that this client wants `event`, with the signature of
/// current registries and then the one of old ones. A refusal is only
/// logged: the match rules are in place and an application may send anyway.
async fn register(connection: &Connection, event: &str) {
    let registry = atspi::Object::new(connection, REGISTRY_NAME, REGISTRY_PATH);
    let own = connection
        .unique_name()
        .map(|name| name.as_str().to_owned())
        .unwrap_or_default();
    let current = timeout(
        CALL_LIMIT,
        registry.call::<_, ()>(
            REGISTRY,
            "RegisterEvent",
            &(event, Vec::<String>::new(), own),
        ),
    )
    .await;
    if matches!(current, Ok(Ok(()))) {
        return;
    }
    let old = timeout(
        CALL_LIMIT,
        registry.call::<_, ()>(REGISTRY, "RegisterEvent", &(event,)),
    )
    .await;
    if !matches!(old, Ok(Ok(()))) {
        eprintln!("addict: foreground listener: the registry did not take {event:?}");
    }
}

/// What the application behind `bus_name` is, from the name of its root.
async fn source_of(connection: &Connection, names: &Names, bus_name: &str) -> Source {
    match atspi::application_name(connection, bus_name, names).await {
        Some(name) if atspi::is_android_studio(&name) => Source::AndroidStudio,
        Some(_) => Source::Other,
        None => Source::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_seen_is_unknown() {
        assert_eq!(Tracker::default().source(), Source::Unknown);
    }

    #[test]
    fn an_activation_is_the_source_once_it_is_named() {
        let mut tracker = Tracker::default();
        tracker.activate(":1.5");
        assert_eq!(tracker.source(), Source::Unknown);
        tracker.name(":1.5", Source::AndroidStudio);
        assert_eq!(tracker.source(), Source::AndroidStudio);
        assert_eq!(tracker.source_of(":1.5"), Some(Source::AndroidStudio));
        assert_eq!(tracker.source_of(":1.6"), None);
    }

    #[test]
    fn the_next_activation_replaces_the_source() {
        let mut tracker = Tracker::default();
        tracker.activate(":1.5");
        tracker.name(":1.5", Source::AndroidStudio);
        tracker.activate(":1.9");
        tracker.name(":1.9", Source::Other);
        assert_eq!(tracker.source(), Source::Other);
    }

    #[test]
    fn a_late_name_of_another_application_is_ignored() {
        let mut tracker = Tracker::default();
        tracker.activate(":1.5");
        tracker.activate(":1.9");
        tracker.name(":1.5", Source::AndroidStudio);
        assert_eq!(tracker.source(), Source::Unknown);
        tracker.name(":1.9", Source::Other);
        assert_eq!(tracker.source(), Source::Other);
    }

    #[test]
    fn deactivating_the_active_application_leaves_nothing_active() {
        let mut tracker = Tracker::default();
        tracker.activate(":1.5");
        tracker.name(":1.5", Source::AndroidStudio);
        tracker.deactivate(":1.5");
        assert_eq!(tracker.source(), Source::Unknown);
        // The late name does not bring it back.
        tracker.name(":1.5", Source::AndroidStudio);
        assert_eq!(tracker.source(), Source::Unknown);
    }

    #[test]
    fn deactivating_another_application_changes_nothing() {
        let mut tracker = Tracker::default();
        tracker.activate(":1.9");
        tracker.name(":1.9", Source::Other);
        tracker.deactivate(":1.5");
        assert_eq!(tracker.source(), Source::Other);
    }

    #[test]
    fn clear_forgets_the_application() {
        let mut tracker = Tracker::default();
        tracker.activate(":1.5");
        tracker.name(":1.5", Source::AndroidStudio);
        tracker.clear();
        assert_eq!(tracker, Tracker::default());
    }

    #[test]
    fn only_a_change_of_is_enabled_resets_the_listener() {
        let entry = |name: &str| HashMap::from([(name.to_owned(), OwnedValue::from(true))]);
        let none = HashMap::new();
        assert!(is_enabled_changed(STATUS, &entry("IsEnabled"), &[]));
        assert!(is_enabled_changed(STATUS, &none, &["IsEnabled".to_owned()]));
        assert!(!is_enabled_changed(
            STATUS,
            &entry("ScreenReaderEnabled"),
            &[]
        ));
        assert!(!is_enabled_changed(
            STATUS,
            &none,
            &["ScreenReaderEnabled".to_owned()]
        ));
        assert!(!is_enabled_changed("org.other", &entry("IsEnabled"), &[]));
    }

    #[test]
    fn window_signals_are_activation_and_deactivation() {
        assert_eq!(
            change_of(EVENT_WINDOW, "Activate", None),
            Some(Change::Active)
        );
        assert_eq!(
            change_of(EVENT_WINDOW, "Deactivate", None),
            Some(Change::Inactive)
        );
        assert_eq!(change_of(EVENT_WINDOW, "Create", None), None);
    }

    #[test]
    fn only_the_active_state_of_a_state_change_counts() {
        let changed = |state| change_of(EVENT_OBJECT, "StateChanged", state);
        assert_eq!(changed(Some(("active", 1))), Some(Change::Active));
        assert_eq!(changed(Some(("active", 0))), Some(Change::Inactive));
        assert_eq!(changed(Some(("focused", 1))), None);
        assert_eq!(changed(None), None);
        assert_eq!(
            change_of("org.a11y.atspi.Event.Focus", "Focus", Some(("active", 1))),
            None
        );
    }

    #[test]
    fn the_events_asked_of_the_registry_are_the_three_listened_to() {
        assert_eq!(RULES.len(), REGISTERED.len());
        assert!(RULES[0].contains("Activate") && REGISTERED[0] == "window:activate");
        assert!(RULES[1].contains("Deactivate") && REGISTERED[1] == "window:deactivate");
        assert!(RULES[2].contains("arg0='active'") && REGISTERED[2].ends_with(":active"));
    }
}
