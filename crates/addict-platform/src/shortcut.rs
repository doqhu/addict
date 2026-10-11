//! The global shortcut of the resident application.
//!
//! [`start`] spawns one worker thread that owns a session-bus connection,
//! registers the host app id, holds a GlobalShortcuts session and binds one
//! shortcut. Everything it learns is handed to a plain `Send` callback, so the
//! caller decides how to wake its own thread; nothing here polls.
//!
//! The attempt, from the bus connection to the bound shortcut, has a limit
//! given by the caller: a portal that never answers ends it as a failure.

use std::{cell::Cell, collections::HashMap, thread::JoinHandle, time::Duration};

use ashpd::{
    AppID,
    desktop::{
        CreateSessionOptions, Session,
        global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut},
    },
    zbus,
    zvariant::{self, OwnedObjectPath, OwnedValue},
};
use futures_util::StreamExt;
use tokio::sync::oneshot;

/// The id of the single shortcut bound inside the session.
pub const SHORTCUT_ID: &str = "invoke";

const SHORTCUT_DESCRIPTION: &str = "Show Addict";

/// How long closing the session may take when the worker stops.
const CLOSE_LIMIT: Duration = Duration::from_secs(1);

/// What the worker reports, in order: at most one `Bound` followed by any
/// number of `Activated`; `Failed` is always the last event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutEvent {
    /// The portal bound the shortcut; `trigger` is its own description of the
    /// keys, which the user may have changed from the preferred ones.
    Bound { trigger: String },
    /// The shortcut was pressed. `activation_token` is fresh for this press
    /// and absent when the portal sent none.
    Activated {
        activation_token: Option<String>,
        timestamp_ms: u64,
    },
    /// The shortcut is not available. `message` is one line: either the step
    /// that failed with the portal's own error text, or, when the attempt ran
    /// out of time, the limit alone with no portal text. It does not say why.
    Failed { message: String },
}

/// Keeps the worker alive. Dropping it stops the worker, which closes the
/// session, and waits for the thread; that takes at most about one second.
#[derive(Debug)]
pub struct ShortcutHandle {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
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
/// shortcuts syntax (for example `LOGO+q`). `limit` is how long the attempt
/// may take up to `Bound`; when it runs out the worker reports `Failed` like
/// for any other failure. `on_event` runs on the worker thread and must not
/// block.
pub fn start(
    app_id: &str,
    trigger: &str,
    limit: Duration,
    on_event: impl Fn(ShortcutEvent) + Send + 'static,
) -> ShortcutHandle {
    let (stop, stopped) = oneshot::channel();
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
                Ok(runtime) => runtime.block_on(run(&app_id, &trigger, limit, &*on_event, stopped)),
                Err(e) => on_event(failed("shortcut worker runtime", &e)),
            }
        });
    let on_event: Callback = Box::new(on_event);
    match spawned {
        Ok(thread) => {
            // The worker is waiting for exactly this message.
            let _ = hand_over.send(on_event);
            ShortcutHandle {
                stop: Some(stop),
                thread: Some(thread),
            }
        }
        Err(e) => {
            on_event(failed("shortcut worker thread", &e));
            ShortcutHandle {
                stop: None,
                thread: None,
            }
        }
    }
}

type Callback = Box<dyn Fn(ShortcutEvent) + Send>;

type ShortcutSession = Session<GlobalShortcuts>;

/// Serves until a failure, the limit of the attempt or the stop signal, then
/// closes the session if one was created. `Failed` is sent after the close,
/// as the last act of the worker; the stop signal sends nothing.
async fn run(
    app_id: &str,
    trigger: &str,
    limit: Duration,
    on_event: &(dyn Fn(ShortcutEvent) + Send),
    stopped: oneshot::Receiver<()>,
) {
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
        _ = stopped => None,
        result = serve(app_id, trigger, &report, &mut session) => {
            let Err(event) = result;
            Some(event)
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
}

/// Resolves after `limit` if the shortcut is still not bound, and never
/// otherwise.
async fn unbound_after(limit: Duration, bound: &Cell<bool>) {
    tokio::time::sleep(limit).await;
    if bound.get() {
        std::future::pending::<()>().await;
    }
}

/// Register, subscribe, create the session, bind, then forward presses. Only
/// returns with the `Failed` event to report.
async fn serve(
    app_id: &str,
    trigger: &str,
    on_event: &dyn Fn(ShortcutEvent),
    session_slot: &mut Option<ShortcutSession>,
) -> Result<std::convert::Infallible, ShortcutEvent> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|e| failed("session bus", &e))?;

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

    // Subscribe before binding so no activation can slip between the two.
    let activations = portal
        .receive_activated()
        .await
        .map_err(|e| failed("subscribing to Activated", &e))?;
    let mut activations = std::pin::pin!(activations);

    let session: &ShortcutSession = session_slot.insert(
        portal
            .create_session(CreateSessionOptions::default())
            .await
            .map_err(|e| failed("creating the shortcuts session", &e))?,
    );
    let session_path = session_path(session).map_err(|e| failed("session handle", &e))?;

    let shortcut =
        NewShortcut::new(SHORTCUT_ID, SHORTCUT_DESCRIPTION).preferred_trigger(Some(trigger));
    let bound = portal
        .bind_shortcuts(session, &[shortcut], None, BindShortcutsOptions::default())
        .await
        .and_then(|request| request.response())
        .map_err(|e| failed("binding the shortcut", &e))?;
    let Some(own) = bound.shortcuts().iter().find(|s| s.id() == SHORTCUT_ID) else {
        return Err(ShortcutEvent::Failed {
            message: format!(
                "binding the shortcut: the portal bound {} shortcuts, none with id {SHORTCUT_ID:?}",
                bound.shortcuts().len()
            ),
        });
    };
    on_event(ShortcutEvent::Bound {
        trigger: own.trigger_description().to_owned(),
    });

    while let Some(signal) = activations.next().await {
        if signal.session_handle().as_str() == session_path.as_str()
            && signal.shortcut_id() == SHORTCUT_ID
        {
            on_event(ShortcutEvent::Activated {
                activation_token: activation_token(signal.options()),
                timestamp_ms: u64::try_from(signal.timestamp().as_millis()).unwrap_or(u64::MAX),
            });
        }
    }
    Err(ShortcutEvent::Failed {
        message: "the Activated signal stream ended".to_owned(),
    })
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
    let text = format!("{step}: {error}");
    ShortcutEvent::Failed {
        message: text.split_whitespace().collect::<Vec<_>>().join(" "),
    }
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
}
