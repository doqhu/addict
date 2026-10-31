//! The global shortcuts of the resident application.
//!
//! [`start`] spawns one worker thread that owns a session-bus connection,
//! registers the host app id and holds a GlobalShortcuts session with separate
//! clipboard and screenshot actions. Everything it learns goes to a plain
//! `Send` callback, so the caller decides how to wake its own thread; nothing here polls.
//!
//! The attempt, from the bus connection to the bound shortcuts, has a limit
//! given by the caller: a portal that never answers ends it as a failure.
//!
//! A registration and a session die with the portal process, so when the owner
//! of the portal's bus name goes, the worker starts a new attempt on a fresh
//! connection with the same IDs and preferences, under a new limit.

use std::{cell::Cell, collections::HashMap, thread::JoinHandle, time::Duration};

use ashpd::{
    AppID,
    desktop::{
        CreateSessionOptions, Session,
        global_shortcuts::{
            BindShortcutsOptions, ConfigureShortcutsOptions, GlobalShortcuts, NewShortcut,
        },
    },
    zbus,
    zvariant::{self, OwnedObjectPath, OwnedValue},
};
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};

/// The existing clipboard action's portal identity; keep stored assignments.
pub const SHORTCUT_ID: &str = "invoke";
/// The screenshot action has its own stable identity and no preferred key.
pub const SCREENSHOT_ID: &str = "screenshot";

/// The action requested by an activation from our session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Clipboard,
    Screenshot,
}

impl Action {
    fn from_id(id: &str) -> Option<Self> {
        match id {
            SHORTCUT_ID => Some(Self::Clipboard),
            SCREENSHOT_ID => Some(Self::Screenshot),
            _ => None,
        }
    }
}

/// The portal owns both assignments. Missing and empty triggers are unassigned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bindings {
    pub clipboard: Option<String>,
    pub screenshot: Option<String>,
}

impl Bindings {
    fn changed<'a>(
        &mut self,
        expected_session: &str,
        actual_session: &str,
        entries: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> bool {
        if expected_session != actual_session {
            return false;
        }
        self.update(entries);
        true
    }

    /// Changed signals may contain only a subset: preserve every other ID.
    fn update<'a>(&mut self, entries: impl IntoIterator<Item = (&'a str, &'a str)>) {
        for (id, trigger) in entries {
            let target = match Action::from_id(id) {
                Some(Action::Clipboard) => &mut self.clipboard,
                Some(Action::Screenshot) => &mut self.screenshot,
                None => continue,
            };
            *target = (!trigger.trim().is_empty()).then(|| trigger.to_owned());
        }
    }
}

fn shortcuts(trigger: &str) -> [NewShortcut; 2] {
    [
        NewShortcut::new(SHORTCUT_ID, "Translate selection or clipboard")
            .preferred_trigger(Some(trigger)),
        NewShortcut::new(SCREENSHOT_ID, "Capture a screen region (OCR)"),
    ]
}

fn session_action(expected: &str, actual: &str, id: &str) -> Option<Action> {
    (expected == actual).then(|| Action::from_id(id)).flatten()
}

/// The bus name of the portal, whose owner holds our registration and session.
const PORTAL: &str = "org.freedesktop.portal.Desktop";

/// How long closing the session may take when the worker stops.
const CLOSE_LIMIT: Duration = Duration::from_secs(1);

/// How long the portal may take to take the request for its dialog; presses
/// wait for it.
const CONFIGURE_LIMIT: Duration = Duration::from_secs(5);

/// An initialized session can have either, both or neither action assigned.
/// Assignment updates and activations follow it until the final `Failed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutEvent {
    Bound {
        bindings: Bindings,
    },
    Changed {
        bindings: Bindings,
    },
    /// A fresh activation token belongs to this press and this action.
    Activated {
        action: Action,
        activation_token: Option<String>,
        timestamp_ms: u64,
    },
    /// The portal answered a [`ShortcutHandle::configure`]: `message` is one
    /// line saying why the dialog for the shortcuts could not be asked for,
    /// and `None` when it was.
    ConfigureEnded {
        message: Option<String>,
    },
    /// The portal is too old for its dialog: `ConfigureShortcuts` is from
    /// version 2 of the interface. Keep the session and every assignment.
    ConfigureUnsupported,
    /// The shortcuts are not available. `message` is one line: either the step
    /// that failed with the portal's own error text, or, when the attempt ran
    /// out of time, the limit alone with no portal text. It does not say why.
    Failed {
        message: String,
    },
}

/// Why an attempt or the session it bound ended.
enum Ended {
    /// The owner of the portal's name went: bind again.
    PortalLeft,
    Failed(ShortcutEvent),
}

impl From<ShortcutEvent> for Ended {
    fn from(event: ShortcutEvent) -> Self {
        Self::Failed(event)
    }
}

/// Keeps the worker alive. Dropping it stops the worker, which closes the
/// session, and waits for the thread; that takes at most about one second.
#[derive(Debug)]
pub struct ShortcutHandle {
    commands: mpsc::UnboundedSender<Command>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

/// What the owner of the handle asks of the worker.
enum Command {
    Configure,
}

impl ShortcutHandle {
    /// Asks the portal for its dialog to change the shortcuts of both actions.
    /// The answer is a [`ShortcutEvent::ConfigureEnded`], or a
    /// [`ShortcutEvent::ConfigureUnsupported`] from a portal too old for the
    /// dialog. A request made before the
    /// shortcuts are bound waits for them, and one made after the worker has
    /// failed is not answered.
    pub fn configure(&self) {
        let _ = self.commands.send(Command::Configure);
    }
}

impl Drop for ShortcutHandle {
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

/// Starts the worker for `app_id` with the preferred `trigger` in the XDG
/// shortcuts syntax (for example `LOGO+q`). `limit` is how long each attempt
/// may take up to `Bound`, the one after a portal restart included; when it
/// runs out the worker reports `Failed` like for any other failure. `on_event`
/// runs on the worker thread and must not block.
pub fn start(
    app_id: &str,
    trigger: &str,
    limit: Duration,
    on_event: impl Fn(ShortcutEvent) + Send + 'static,
) -> ShortcutHandle {
    let (stop, stopped) = oneshot::channel();
    let (commands, requests) = mpsc::unbounded_channel();
    // The callback travels over a channel so that it is still here to report
    // a thread that could not be spawned.
    let (hand_over, handed_over) = std::sync::mpsc::channel::<Callback>();
    let (app_id, trigger) = (app_id.to_owned(), trigger.to_owned());
    let spawned = std::thread::Builder::new()
        .name("addict-shortcut".to_owned())
        .spawn(move || {
            let Ok(on_event) = handed_over.recv() else {
                return;
            };
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => {
                    runtime.block_on(run(&app_id, &trigger, limit, &*on_event, stopped, requests))
                }
                Err(e) => on_event(failed("shortcut worker runtime", &e)),
            }
        });
    let on_event: Callback = Box::new(on_event);
    match spawned {
        Ok(thread) => {
            // The worker is waiting for exactly this message.
            let _ = hand_over.send(on_event);
            ShortcutHandle {
                commands,
                stop: Some(stop),
                thread: Some(thread),
            }
        }
        Err(e) => {
            on_event(failed("shortcut worker thread", &e));
            ShortcutHandle {
                commands,
                stop: None,
                thread: None,
            }
        }
    }
}

type Callback = Box<dyn Fn(ShortcutEvent) + Send>;

type ShortcutSession = Session<GlobalShortcuts>;

/// Serves until a failure, the limit of an attempt or the stop signal, then
/// closes the session if one was created. `Failed` is sent after the close,
/// as the last act of the worker; the stop signal sends nothing. A portal that
/// left only starts the next attempt: its session went with it, so that one
/// is dropped unclosed.
async fn run(
    app_id: &str,
    trigger: &str,
    limit: Duration,
    on_event: &(dyn Fn(ShortcutEvent) + Send),
    mut stopped: oneshot::Receiver<()>,
    mut requests: mpsc::UnboundedReceiver<Command>,
) {
    loop {
        let mut session = None;
        // The limit covers the attempt only: once bound, presses are awaited
        // without one.
        let bound = Cell::new(false);
        let report = |event: ShortcutEvent| {
            if matches!(event, ShortcutEvent::Bound { .. }) {
                bound.set(true);
            }
            on_event(event);
        };
        let failure = tokio::select! {
            // Resolves on the signal and when the handle is gone without one.
            _ = &mut stopped => None,
            result = serve(app_id, trigger, &report, &mut session, &mut requests) => {
                let Err(ended) = result;
                match ended {
                    Ended::PortalLeft => continue,
                    Ended::Failed(event) => Some(event),
                }
            }
            () = unbound_after(limit, &bound) => Some(limit_reached(limit)),
        };
        if let Some(session) = session {
            // Best effort: the portal also drops the session with the connection.
            let _ = tokio::time::timeout(CLOSE_LIMIT, session.close()).await;
        }
        if let Some(event) = failure {
            on_event(event);
        }
        return;
    }
}

/// Resolves after `limit` if the shortcuts are still not bound, and never
/// otherwise.
async fn unbound_after(limit: Duration, bound: &Cell<bool>) {
    tokio::time::sleep(limit).await;
    if bound.get() {
        std::future::pending::<()>().await;
    }
}

/// Watch the portal's name, register, subscribe, create the session, bind,
/// then forward presses. Only returns with why it ended.
async fn serve(
    app_id: &str,
    trigger: &str,
    on_event: &dyn Fn(ShortcutEvent),
    session_slot: &mut Option<ShortcutSession>,
    requests: &mut mpsc::UnboundedReceiver<Command>,
) -> Result<std::convert::Infallible, Ended> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|e| failed("session bus", &e))?;

    // Before registering, so that the owner holding the registration cannot
    // leave unseen. The call to the bus is not a call to the portal. An owner
    // that left before the registration arrived costs one needless attempt; a
    // new owner appearing is no reason to act, it is the portal being started.
    let bus = zbus::fdo::DBusProxy::new(&connection)
        .await
        .map_err(|e| failed("watching the portal", &e))?;
    let owners = bus
        .receive_name_owner_changed_with_args(&[(0, PORTAL)])
        .await
        .map_err(|e| failed("watching the portal", &e))?;
    let mut owners = std::pin::pin!(owners);

    // Must precede any other portal call on the connection.
    let parsed: AppID = app_id
        .parse()
        .map_err(|e: ashpd::Error| failed(&format!("app id {app_id:?}"), &e))?;
    ashpd::register_host_app_with_connection(connection.clone(), parsed)
        .await
        .map_err(|e| failed(&format!("host registration of {app_id:?}"), &e))?;

    let portal = GlobalShortcuts::with_connection(connection)
        .await
        .map_err(|e| failed("GlobalShortcuts portal", &e))?;

    // Subscribe before binding so neither presses nor assignment changes slip past.
    let activations = portal
        .receive_activated()
        .await
        .map_err(|e| failed("subscribing to Activated", &e))?;
    let mut activations = std::pin::pin!(activations);

    let changes = portal
        .receive_shortcuts_changed()
        .await
        .map_err(|e| failed("subscribing to ShortcutsChanged", &e))?;
    let mut changes = std::pin::pin!(changes);

    let session: &ShortcutSession = session_slot.insert(
        portal
            .create_session(CreateSessionOptions::default())
            .await
            .map_err(|e| failed("creating the shortcuts session", &e))?,
    );
    let session_path = session_path(session).map_err(|e| failed("session handle", &e))?;

    let bound = portal
        .bind_shortcuts(
            session,
            &shortcuts(trigger),
            None,
            BindShortcutsOptions::default(),
        )
        .await
        .and_then(|request| request.response())
        .map_err(|e| failed("binding the shortcuts", &e))?;
    let mut bindings = Bindings::default();
    bindings.update(
        bound
            .shortcuts()
            .iter()
            .map(|s| (s.id(), s.trigger_description())),
    );
    on_event(ShortcutEvent::Bound {
        bindings: bindings.clone(),
    });

    loop {
        tokio::select! {
            signal = owners.next() => {
                let Some(signal) = signal else { break };
                let args = signal.args().map_err(|e| failed("watching the portal", &e))?;
                if args.old_owner().is_some() {
                    return Err(Ended::PortalLeft);
                }
            }
            signal = activations.next() => {
                let Some(signal) = signal else { break };
                if let Some(action) = session_action(
                    session_path.as_str(), signal.session_handle().as_str(), signal.shortcut_id(),
                ) {
                    on_event(ShortcutEvent::Activated {
                        action,
                        activation_token: activation_token(signal.options()),
                        timestamp_ms: u64::try_from(signal.timestamp().as_millis())
                            .unwrap_or(u64::MAX),
                    });
                }
            }
            signal = changes.next() => {
                let Some(signal) = signal else { break };
                if bindings.changed(
                    session_path.as_str(), signal.session_handle().as_str(),
                    signal.shortcuts().iter().map(|s| (s.id(), s.trigger_description())),
                ) {
                    on_event(ShortcutEvent::Changed { bindings: bindings.clone() });
                }
            }
            // Disabled once the handle is gone; the stop signal ends the
            // worker then.
            Some(Command::Configure) = requests.recv() => {
                let asked = tokio::time::timeout(
                    CONFIGURE_LIMIT,
                    portal.configure_shortcuts(
                        session,
                        None,
                        ConfigureShortcutsOptions::default(),
                    ),
                )
                .await;
                on_event(configure_event(asked.ok()));
            }
        }
    }
    Err(Ended::Failed(ShortcutEvent::Failed {
        message: "a shortcuts signal stream ended".to_owned(),
    }))
}

/// The wrapper keeps the session path private; it serializes as the path.
fn session_path(session: &ShortcutSession) -> zvariant::Result<OwnedObjectPath> {
    let context = zvariant::serialized::Context::new_dbus(zvariant::LE, 0);
    let bytes = zvariant::to_bytes(context, session)?;
    let (path, _) = bytes.deserialize::<OwnedObjectPath>()?;
    Ok(path)
}

/// The `activation_token` of an `Activated` signal: a string option, absent
/// when missing, of another type, or empty.
fn activation_token(options: &HashMap<String, OwnedValue>) -> Option<String> {
    options
        .get("activation_token")
        .and_then(|value| <&str>::try_from(value).ok())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}

/// One line: the step that failed and the error text as given.
fn failed(step: &str, error: &dyn std::fmt::Display) -> ShortcutEvent {
    ShortcutEvent::Failed {
        message: one_line(step, error),
    }
}

fn one_line(step: &str, error: &dyn std::fmt::Display) -> String {
    let text = format!("{step}: {error}");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The event for the answer to a request for the portal's dialog: the
/// refusal for the version of the interface is [`ShortcutEvent::ConfigureUnsupported`],
/// anything else is [`configure_ended`].
fn configure_event(answer: Option<Result<(), ashpd::Error>>) -> ShortcutEvent {
    match answer {
        Some(Err(ashpd::Error::RequiresVersion(..))) => ShortcutEvent::ConfigureUnsupported,
        other => configure_ended(other),
    }
}

/// The answer to a request for the portal's dialog: its result, or `None`
/// when [`CONFIGURE_LIMIT`] ran out first.
fn configure_ended(answer: Option<Result<(), impl std::fmt::Display>>) -> ShortcutEvent {
    let message = match answer {
        Some(Ok(())) => None,
        Some(Err(e)) => Some(one_line("changing the shortcut", &e)),
        None => Some(format!(
            "changing the shortcut: no result within {CONFIGURE_LIMIT:?}"
        )),
    };
    ShortcutEvent::ConfigureEnded { message }
}

/// The failure of an attempt that ran out of time: it names the limit and
/// nothing else, because nothing is known about the reason.
fn limit_reached(limit: Duration) -> ShortcutEvent {
    ShortcutEvent::Failed {
        message: format!("binding the shortcut: no result within {limit:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(entries: &[(&str, OwnedValue)]) -> HashMap<String, OwnedValue> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.try_clone().expect("clone value")))
            .collect()
    }

    fn text(value: &str) -> OwnedValue {
        OwnedValue::from(zvariant::Str::from(value))
    }

    #[test]
    fn token_is_taken_from_the_string_option() {
        let options = options(&[("activation_token", text("gnome-shell/Addict/1-2-3"))]);
        assert_eq!(
            activation_token(&options).as_deref(),
            Some("gnome-shell/Addict/1-2-3")
        );
    }

    #[test]
    fn token_is_absent_without_the_option() {
        assert_eq!(activation_token(&HashMap::new()), None);
        assert_eq!(activation_token(&options(&[("other", text("x"))])), None);
    }

    #[test]
    fn empty_token_is_absent() {
        let options = options(&[("activation_token", text(""))]);
        assert_eq!(activation_token(&options), None);
    }

    #[test]
    fn token_of_another_type_is_absent() {
        let options = options(&[("activation_token", OwnedValue::from(7u32))]);
        assert_eq!(activation_token(&options), None);
    }

    #[test]
    fn limit_message_names_the_limit_and_no_cause() {
        let ShortcutEvent::Failed { message } = limit_reached(Duration::from_secs(60)) else {
            panic!("the limit must be a failure");
        };
        assert_eq!(message, "binding the shortcut: no result within 60s");
        let lowered = message.to_lowercase();
        for cause in ["cancel", "denied", "refus", "dismiss", "portal", "error"] {
            assert!(!lowered.contains(cause), "{message}");
        }
    }

    #[test]
    fn limit_fires_only_while_unbound() {
        let limit = Duration::from_millis(20);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let bound = Cell::new(false);
            let far = Duration::from_secs(10);
            assert!(
                tokio::time::timeout(far, unbound_after(limit, &bound))
                    .await
                    .is_ok()
            );
            bound.set(true);
            assert!(
                tokio::time::timeout(limit * 10, unbound_after(limit, &bound))
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    fn failure_message_is_one_line_with_step_and_error_text() {
        let event = failed("binding the shortcut", &"first line\n  second line");
        assert_eq!(
            event,
            ShortcutEvent::Failed {
                message: "binding the shortcut: first line second line".to_owned()
            }
        );
    }

    #[test]
    fn a_configure_answer_says_nothing_only_when_the_portal_took_it() {
        assert_eq!(
            configure_ended(Some(Ok::<(), &str>(()))),
            ShortcutEvent::ConfigureEnded { message: None }
        );
        assert_eq!(
            configure_ended(Some(Err("Requested version 2\nbut 1"))),
            ShortcutEvent::ConfigureEnded {
                message: Some("changing the shortcut: Requested version 2 but 1".to_owned())
            }
        );
        assert_eq!(
            configure_ended(None::<Result<(), &str>>),
            ShortcutEvent::ConfigureEnded {
                message: Some("changing the shortcut: no result within 5s".to_owned())
            }
        );
    }

    #[test]
    fn the_version_refusal_is_unsupported_and_no_other_error_is() {
        assert_eq!(
            configure_event(Some(Err(ashpd::Error::RequiresVersion(2, 1)))),
            ShortcutEvent::ConfigureUnsupported
        );
        assert_eq!(
            configure_event(Some(Err(ashpd::Error::InvalidAppID))),
            ShortcutEvent::ConfigureEnded {
                message: Some(format!(
                    "changing the shortcut: {}",
                    ashpd::Error::InvalidAppID
                ))
            }
        );
        assert_eq!(
            configure_event(Some(Ok(()))),
            ShortcutEvent::ConfigureEnded { message: None }
        );
        assert_eq!(
            configure_event(None),
            ShortcutEvent::ConfigureEnded {
                message: Some("changing the shortcut: no result within 5s".to_owned())
            }
        );
    }

    #[test]
    fn action_identity_and_preference_preserve_existing_clipboard_assignment() {
        let definitions = shortcuts("LOGO+q");
        let context = zvariant::serialized::Context::new_dbus(zvariant::LE, 0);
        let bytes =
            zvariant::to_bytes(context, &definitions.as_slice()).expect("serialize shortcuts");
        let (definitions, _) = bytes
            .deserialize::<Vec<(String, HashMap<String, OwnedValue>)>>()
            .expect("shortcut properties");
        assert_eq!(definitions[0].0, "invoke");
        assert_eq!(
            <&str>::try_from(&definitions[0].1["preferred_trigger"]).unwrap(),
            "LOGO+q"
        );
        assert_eq!(definitions[1].0, "screenshot");
        assert!(!definitions[1].1.contains_key("preferred_trigger"));
    }

    #[test]
    fn partial_bindings_and_changed_ids_keep_independent_assignments() {
        let mut bindings = Bindings::default();
        bindings.update([("invoke", "Super+Q")]);
        assert_eq!(bindings.clipboard.as_deref(), Some("Super+Q"));
        assert_eq!(bindings.screenshot, None);
        assert!(!bindings.changed("/ours", "/other", [("invoke", "F1"), ("screenshot", "F2")]));
        assert_eq!(bindings.clipboard.as_deref(), Some("Super+Q"));
        assert_eq!(bindings.screenshot, None);
        assert!(bindings.changed(
            "/ours",
            "/ours",
            [("screenshot", "Super+S"), ("other", "F1")]
        ));
        assert_eq!(bindings.clipboard.as_deref(), Some("Super+Q"));
        assert_eq!(bindings.screenshot.as_deref(), Some("Super+S"));
        bindings.update([("screenshot", " ")]);
        assert_eq!(bindings.screenshot, None);
        assert_eq!(bindings.clipboard.as_deref(), Some("Super+Q"));
        bindings.update([("invoke", "")]);
        assert_eq!(bindings, Bindings::default());
    }

    #[test]
    fn only_known_actions_in_our_session_activate() {
        assert_eq!(
            session_action("/ours", "/ours", "invoke"),
            Some(Action::Clipboard)
        );
        assert_eq!(
            session_action("/ours", "/ours", "screenshot"),
            Some(Action::Screenshot)
        );
        assert_eq!(session_action("/ours", "/other", "invoke"), None);
        assert_eq!(session_action("/ours", "/other", "screenshot"), None);
        assert_eq!(session_action("/ours", "/ours", "unknown"), None);
    }
}
