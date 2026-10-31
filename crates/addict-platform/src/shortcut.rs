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
//!
//! A version 1 portal has no dialog to ask for, and GNOME opens one only for an
//! ID it has not stored. So on such a portal, a request for the dialog closes
//! the session and binds under IDs of the next generation, offering each
//! current key as the preferred one. If the person accepts, that generation is
//! saved in a small file, a cache of what the desktop stores: without it the
//! dialog opens once more and nothing breaks. If not, the worker binds the IDs
//! it had, and the keys are as they were.

use std::{
    cell::Cell,
    collections::HashMap,
    path::{Path, PathBuf},
    pin::Pin,
    thread::JoinHandle,
    time::Duration,
};

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
use futures_util::{Stream, StreamExt};
use tokio::sync::{mpsc, oneshot};

/// The clipboard action's portal identity in generation 0; keep stored
/// assignments.
pub const SHORTCUT_ID: &str = "invoke";
/// The screenshot action's identity in generation 0; by default it has no
/// preferred key.
pub const SCREENSHOT_ID: &str = "screenshot";

/// The action requested by an activation from our session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Clipboard,
    Screenshot,
}

/// The portal identities of both actions in one generation: 0 is the plain
/// `invoke` and `screenshot`, N is `invoke-N` and `screenshot-N`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ids {
    clipboard: String,
    screenshot: String,
}

impl Ids {
    fn of(generation: u64) -> Self {
        match generation {
            0 => Self {
                clipboard: SHORTCUT_ID.to_owned(),
                screenshot: SCREENSHOT_ID.to_owned(),
            },
            n => Self {
                clipboard: format!("{SHORTCUT_ID}-{n}"),
                screenshot: format!("{SCREENSHOT_ID}-{n}"),
            },
        }
    }

    fn action(&self, id: &str) -> Option<Action> {
        if id == self.clipboard {
            Some(Action::Clipboard)
        } else if id == self.screenshot {
            Some(Action::Screenshot)
        } else {
            None
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
        ids: &Ids,
        expected_session: &str,
        actual_session: &str,
        entries: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> bool {
        if expected_session != actual_session {
            return false;
        }
        self.update(ids, entries);
        true
    }

    /// Changed signals may contain only a subset: preserve every other ID.
    fn update<'a>(&mut self, ids: &Ids, entries: impl IntoIterator<Item = (&'a str, &'a str)>) {
        for (id, trigger) in entries {
            let target = match ids.action(id) {
                Some(Action::Clipboard) => &mut self.clipboard,
                Some(Action::Screenshot) => &mut self.screenshot,
                None => continue,
            };
            *target = (!trigger.trim().is_empty()).then(|| trigger.to_owned());
        }
    }
}

/// The key each action asks for in the XDG shortcuts syntax, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Preferences {
    clipboard: Option<String>,
    screenshot: Option<String>,
}

impl Preferences {
    /// The current keys as preferences, so that changing one keeps the other.
    fn offering(bindings: &Bindings) -> Self {
        Self {
            clipboard: bindings.clipboard.as_deref().and_then(preferred_trigger),
            screenshot: bindings.screenshot.as_deref().and_then(preferred_trigger),
        }
    }
}

/// A key the portal described, in the syntax for a preferred trigger. The
/// sentence in GNOME's `Press <Super>q` is translated, with the key anywhere in
/// it and perhaps in quotes, so only the `<...>` part counts: `<Control><Super>q`
/// is `CTRL+LOGO+q`. With no such part, a function key in any word is accepted.
/// Anything else gives no preference.
fn preferred_trigger(description: &str) -> Option<String> {
    let Some(start) = description.find('<') else {
        return description
            .split_whitespace()
            .map(without_punctuation)
            .find(|word| is_function_key(word))
            .map(str::to_owned);
    };
    let mut rest = &description[start..];
    let mut parts = Vec::new();
    while let Some(after) = rest.strip_prefix('<') {
        let (modifier, tail) = after.split_once('>')?;
        parts.push(trigger_modifier(modifier)?);
        rest = tail;
    }
    let key = without_punctuation(rest.split_whitespace().next()?);
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    parts.push(key);
    Some(parts.join("+"))
}

/// A word without the quotes or other punctuation around it.
fn without_punctuation(word: &str) -> &str {
    word.trim_matches(|c: char| !c.is_alphanumeric())
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('F').is_some_and(|number| {
        matches!(number.len(), 1 | 2) && number.chars().all(|c| c.is_ascii_digit())
    })
}

/// The name in the shortcuts syntax of a modifier in an accelerator.
fn trigger_modifier(accelerator: &str) -> Option<&'static str> {
    match accelerator.to_ascii_lowercase().as_str() {
        "control" | "ctrl" | "primary" => Some("CTRL"),
        "shift" => Some("SHIFT"),
        "alt" | "mod1" => Some("ALT"),
        "super" | "mod4" => Some("LOGO"),
        _ => None,
    }
}

fn shortcuts(ids: &Ids, preferences: &Preferences) -> [NewShortcut; 2] {
    [
        NewShortcut::new(ids.clipboard.as_str(), "Translate selection or clipboard")
            .preferred_trigger(preferences.clipboard.as_deref()),
        NewShortcut::new(ids.screenshot.as_str(), "Capture a screen region (OCR)")
            .preferred_trigger(preferences.screenshot.as_deref()),
    ]
}

fn session_action(ids: &Ids, expected: &str, actual: &str, id: &str) -> Option<Action> {
    (expected == actual).then(|| ids.action(id)).flatten()
}

/// The generation in the file, or 0 when it is missing, is not a regular file
/// or holds anything but a number.
fn read_generation(path: &Path) -> u64 {
    // Before opening: opening a pipe would wait for its writer.
    if !std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        return 0;
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// The generation after `generation`. Its IDs are new either way, also when
/// the numbers ran out.
fn next_generation(generation: u64) -> u64 {
    generation.checked_add(1).unwrap_or(1)
}

/// What Settings says when the new keys work but their generation can't be
/// found again, or `None` when it was saved.
fn save_generation(file: Option<&Path>, generation: u64) -> Option<String> {
    let reason = match file {
        Some(path) => crate::startup::replace_file(path, &format!("{generation}\n"))
            .err()?
            .to_string(),
        None => "there is no configuration directory".to_owned(),
    };
    Some(one_line_text(&format!(
        "The new keys work, but they could not be remembered, so the system's dialog returns \
         at the next start: {reason}"
    )))
}

/// The bus name of the portal, whose owner holds our registration and session.
const PORTAL: &str = "org.freedesktop.portal.Desktop";

/// How long closing the session may take when the worker stops.
const CLOSE_LIMIT: Duration = Duration::from_secs(1);

/// How long the portal may take to take the request for its dialog; presses
/// wait for it.
const CONFIGURE_LIMIT: Duration = Duration::from_secs(5);

/// How long the attempt under new IDs may take: a person is answering a dialog.
const REBIND_LIMIT: Duration = Duration::from_secs(600);

/// What Settings says when the portal answered without new keys: GNOME's
/// response to a cancelled dialog is the same as to a failure of its backend,
/// so this can't claim that a dialog was there.
const REBIND_CANCELLED: &str = "No new keys were chosen. The keys are unchanged.";

/// What Settings says when the portal went away during the dialog.
const REBIND_PORTAL_LEFT: &str = "The desktop portal restarted. The keys are unchanged.";

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
    /// The portal has no dialog to ask for, so a new binding that opens one has
    /// begun. `bindings` are the keys as they are until it ends, which is with
    /// a [`ShortcutEvent::RebindEnded`] right after a `Bound`, or with `Failed`.
    Rebinding {
        bindings: Bindings,
    },
    /// The new binding ended and the shortcuts are bound. `message` is one line
    /// for the user: what happened when the keys are the old ones or the new
    /// ones won't be found again, and `None` when the new ones are in place.
    RebindEnded {
        message: Option<String>,
    },
    /// The portal answered a [`ShortcutHandle::configure`]: `message` is one
    /// line saying why the dialog for the shortcuts could not be asked for,
    /// and `None` when it was.
    ConfigureEnded {
        message: Option<String>,
    },
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
    /// The portal is too old for its dialog (`ConfigureShortcuts` is from
    /// version 2 of the interface): bind again under new IDs. `bindings` are the
    /// keys of the session that ended.
    Rebind {
        bindings: Bindings,
    },
    /// The portal's dialog for the binding was not accepted. The event is the
    /// failure, if that is what it comes to.
    Declined(ShortcutEvent),
    Failed(ShortcutEvent),
}

impl From<ShortcutEvent> for Ended {
    fn from(event: ShortcutEvent) -> Self {
        Self::Failed(event)
    }
}

/// What an attempt is for, beyond binding the shortcuts.
#[derive(Debug, PartialEq, Eq)]
enum Stage {
    /// Nothing: bind the current IDs and serve.
    Current,
    /// Bind the next generation's IDs, which opens the dialog.
    Rebinding,
    /// Bind the current IDs again, after a rebind that changed nothing and
    /// ended as `message` says.
    Returning { message: String },
}

/// What the attempts have in common, and what changes between them.
struct Plan<'a> {
    app_id: &'a str,
    file: Option<&'a Path>,
    generation: u64,
    preferences: Preferences,
    stage: Stage,
}

impl Plan<'_> {
    /// The limit of the next attempt.
    fn limit(&self, normal: Duration) -> Duration {
        match self.stage {
            Stage::Rebinding => REBIND_LIMIT,
            Stage::Current | Stage::Returning { .. } => normal,
        }
    }

    /// The generation the next attempt binds.
    fn bound_generation(&self) -> u64 {
        match self.stage {
            Stage::Rebinding => next_generation(self.generation),
            Stage::Current | Stage::Returning { .. } => self.generation,
        }
    }

    /// The attempt has its answer: a rebind ends here, with the line for
    /// [`ShortcutEvent::RebindEnded`] if there is one. Presses queued during
    /// the rebind are dropped: they would open a second dialog.
    fn answered(
        &mut self,
        requests: &mut mpsc::UnboundedReceiver<Command>,
    ) -> Option<Option<String>> {
        let generation = self.bound_generation();
        let message = match std::mem::replace(&mut self.stage, Stage::Current) {
            Stage::Current => return None,
            Stage::Rebinding => {
                self.generation = generation;
                save_generation(self.file, generation)
            }
            Stage::Returning { message } => Some(message),
        };
        while requests.try_recv().is_ok() {}
        Some(message)
    }
}

/// What the worker does after an attempt ended.
#[derive(Debug, PartialEq)]
enum Step {
    /// Another attempt, closing the old session first with `close`. A session
    /// whose portal left went with it.
    Bind { stage: Stage, close: bool },
    /// Close the session and report this, as the last act of the worker.
    Stop(ShortcutEvent),
}

fn next_step(stage: Stage, ended: Ended) -> Step {
    let returning = |message: String, close| Step::Bind {
        stage: Stage::Returning { message },
        close,
    };
    match (stage, ended) {
        (_, Ended::Rebind { .. }) => Step::Bind {
            stage: Stage::Rebinding,
            close: true,
        },
        (Stage::Rebinding, Ended::PortalLeft) => returning(REBIND_PORTAL_LEFT.to_owned(), false),
        (stage, Ended::PortalLeft) => Step::Bind {
            stage,
            close: false,
        },
        (Stage::Rebinding, Ended::Declined(_)) => returning(REBIND_CANCELLED.to_owned(), true),
        (Stage::Rebinding, Ended::Failed(event)) => {
            let ShortcutEvent::Failed { message } = event else {
                return returning("The keys are unchanged.".to_owned(), true);
            };
            returning(format!("The keys are unchanged: {message}"), true)
        }
        (_, Ended::Declined(event) | Ended::Failed(event)) => Step::Stop(event),
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
    /// The answer is a [`ShortcutEvent::ConfigureEnded`], or from a portal too
    /// old for the dialog a [`ShortcutEvent::Rebinding`], which gets the dialog
    /// another way. A request made before the shortcuts are bound waits for
    /// them, one made during a rebind is dropped, and one made after the worker
    /// has failed is not answered.
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
/// runs out the worker reports `Failed` like for any other failure.
/// `generation_file` is where the worker keeps which shortcut IDs are current;
/// with `None`, or with a file it can't read, they are the first ones. `on_event`
/// runs on the worker thread and must not block.
pub fn start(
    app_id: &str,
    trigger: &str,
    limit: Duration,
    generation_file: Option<PathBuf>,
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
                Ok(runtime) => runtime.block_on(run(
                    &app_id,
                    &trigger,
                    limit,
                    generation_file.as_deref(),
                    &*on_event,
                    stopped,
                    requests,
                )),
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
/// is dropped unclosed. A rebind closes the session first, or the desktop holds
/// on to its keys.
async fn run(
    app_id: &str,
    trigger: &str,
    limit: Duration,
    generation_file: Option<&Path>,
    on_event: &(dyn Fn(ShortcutEvent) + Send),
    mut stopped: oneshot::Receiver<()>,
    mut requests: mpsc::UnboundedReceiver<Command>,
) {
    let mut plan = Plan {
        app_id,
        file: generation_file,
        generation: generation_file.map_or(0, read_generation),
        preferences: Preferences {
            clipboard: Some(trigger.to_owned()),
            screenshot: None,
        },
        stage: Stage::Current,
    };
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
        let attempt_limit = plan.limit(limit);
        let ended = tokio::select! {
            // Resolves on the signal and when the handle is gone without one.
            _ = &mut stopped => None,
            result = serve(&mut plan, &report, &mut session, &mut requests) => {
                let Err(ended) = result;
                Some(ended)
            }
            () = unbound_after(attempt_limit, &bound) => {
                Some(Ended::Failed(limit_reached(attempt_limit)))
            }
        };
        let step = ended.map(|ended| {
            if let Ended::Rebind { bindings } = &ended {
                plan.preferences = Preferences::offering(bindings);
                on_event(ShortcutEvent::Rebinding {
                    bindings: bindings.clone(),
                });
            }
            next_step(std::mem::replace(&mut plan.stage, Stage::Current), ended)
        });
        if !matches!(step, Some(Step::Bind { close: false, .. }))
            && let Some(session) = session
        {
            // Best effort: the portal also drops the session with the connection.
            let _ = tokio::time::timeout(CLOSE_LIMIT, session.close()).await;
        }
        match step {
            Some(Step::Bind { stage, .. }) => plan.stage = stage,
            Some(Step::Stop(event)) => {
                on_event(event);
                return;
            }
            None => return,
        }
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
    plan: &mut Plan<'_>,
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
    let app_id = plan.app_id;
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

    let ids = Ids::of(plan.bound_generation());
    // The answer can be a person's, so a portal that leaves meanwhile is seen
    // now and not at the limit.
    let preferred = shortcuts(&ids, &plan.preferences);
    let answer = tokio::select! {
        answer = portal.bind_shortcuts(
            session,
            &preferred,
            None,
            BindShortcutsOptions::default(),
        ) => answer,
        ended = portal_left(owners.as_mut()) => return Err(ended),
    };
    let bound = answer.and_then(|request| request.response()).map_err(|e| {
        let event = failed("binding the shortcuts", &e);
        match e {
            ashpd::Error::Response(_) => Ended::Declined(event),
            _ => Ended::Failed(event),
        }
    })?;
    let mut bindings = Bindings::default();
    bindings.update(
        &ids,
        bound
            .shortcuts()
            .iter()
            .map(|s| (s.id(), s.trigger_description())),
    );
    let rebind_end = plan.answered(requests);
    on_event(ShortcutEvent::Bound {
        bindings: bindings.clone(),
    });
    if let Some(message) = rebind_end {
        on_event(ShortcutEvent::RebindEnded { message });
    }

    loop {
        tokio::select! {
            ended = portal_left(owners.as_mut()) => return Err(ended),
            signal = activations.next() => {
                let Some(signal) = signal else { break };
                if let Some(action) = session_action(
                    &ids, session_path.as_str(), signal.session_handle().as_str(), signal.shortcut_id(),
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
                    &ids, session_path.as_str(), signal.session_handle().as_str(),
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
                .await
                .ok();
                if refused_for_version(&asked) {
                    return Err(Ended::Rebind { bindings });
                }
                on_event(configure_ended(asked));
            }
        }
    }
    Err(Ended::Failed(stream_ended()))
}

/// Resolves when the owner of the portal's name goes, or when that can no
/// longer be known. A new owner appearing is the portal being started.
async fn portal_left(
    mut owners: Pin<&mut impl Stream<Item = zbus::fdo::NameOwnerChanged>>,
) -> Ended {
    while let Some(signal) = owners.next().await {
        match signal.args() {
            Ok(args) if args.old_owner().is_some() => return Ended::PortalLeft,
            Ok(_) => {}
            Err(e) => return Ended::Failed(failed("watching the portal", &e)),
        }
    }
    Ended::Failed(stream_ended())
}

fn stream_ended() -> ShortcutEvent {
    ShortcutEvent::Failed {
        message: "a shortcuts signal stream ended".to_owned(),
    }
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
    one_line_text(&format!("{step}: {error}"))
}

fn one_line_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether the answer to a request for the portal's dialog is the refusal for
/// the version of the interface. No other answer starts a rebind.
fn refused_for_version(answer: &Option<Result<(), ashpd::Error>>) -> bool {
    matches!(answer, Some(Err(ashpd::Error::RequiresVersion(..))))
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
    fn the_version_refusal_rebinds_and_no_other_answer_does() {
        assert!(refused_for_version(&Some(Err(
            ashpd::Error::RequiresVersion(2, 1)
        ))));
        assert!(!refused_for_version(&Some(Err(ashpd::Error::InvalidAppID))));
        assert!(!refused_for_version(&Some(Ok(()))));
        assert!(!refused_for_version(&None));
    }

    #[test]
    fn current_ids_are_plain_in_generation_zero_and_numbered_after() {
        let first = Ids::of(0);
        assert_eq!(first.clipboard, "invoke");
        assert_eq!(first.screenshot, "screenshot");
        let third = Ids::of(3);
        assert_eq!(third.clipboard, "invoke-3");
        assert_eq!(third.screenshot, "screenshot-3");
        assert_eq!(third.action("invoke-3"), Some(Action::Clipboard));
        assert_eq!(third.action("screenshot-3"), Some(Action::Screenshot));
        assert_eq!(third.action("invoke"), None);
        assert_eq!(third.action("invoke-2"), None);
        assert_eq!(next_generation(0), 1);
        assert_eq!(next_generation(u64::MAX), 1);
    }

    #[test]
    fn an_accelerator_becomes_a_preferred_trigger() {
        for (description, trigger) in [
            ("Press <Super>q", Some("LOGO+q")),
            ("Press <Control><Super>q", Some("CTRL+LOGO+q")),
            ("Press <Control><Super>F9", Some("CTRL+LOGO+F9")),
            ("Press <Shift><Alt>space", Some("SHIFT+ALT+space")),
            // The word is translated, and may be after the keys.
            ("Drücke <Super>q", Some("LOGO+q")),
            ("<Primary>KP_1 drücken", Some("CTRL+KP_1")),
            ("F8", Some("F8")),
            ("Press F12", Some("F12")),
            // The key first, in the middle, or in quotes.
            ("F8 を押す", Some("F8")),
            ("<Super>q を押す", Some("LOGO+q")),
            ("Nyomja meg az F8 billentyűt", Some("F8")),
            ("Nyomja meg a <Super>q billentyűt", Some("LOGO+q")),
            ("Натиснете „F8“", Some("F8")),
            ("Натиснете „<Super>q“", Some("LOGO+q")),
            ("Натиснете „<Primary>KP_1“", Some("CTRL+KP_1")),
            ("Press <Super>„“", None),
            ("", None),
            ("Super+Q", None),
            ("Press", None),
            ("Disabled", None),
            ("Press <Super>", None),
            ("Press <Super", None),
            ("Press <Meta>q", None),
            ("Press <Super>qを押す", None),
            ("F123", None),
            ("Fx", None),
        ] {
            assert_eq!(
                preferred_trigger(description).as_deref(),
                trigger,
                "{description}"
            );
        }
    }

    #[test]
    fn a_rebind_offers_each_key_it_can_read_and_no_other() {
        let offered = Preferences::offering(&Bindings {
            clipboard: Some("Press <Super>q".to_owned()),
            screenshot: Some("something else".to_owned()),
        });
        assert_eq!(offered.clipboard.as_deref(), Some("LOGO+q"));
        assert_eq!(offered.screenshot, None);
    }

    #[test]
    fn a_rebind_that_changed_nothing_returns_and_a_second_failure_ends() {
        let failure = || ShortcutEvent::Failed {
            message: "broke".to_owned(),
        };
        let unchanged = |message: &str| Step::Bind {
            stage: Stage::Returning {
                message: message.to_owned(),
            },
            close: true,
        };
        assert_eq!(
            next_step(Stage::Rebinding, Ended::Declined(failure())),
            unchanged(REBIND_CANCELLED)
        );
        assert_eq!(
            next_step(Stage::Rebinding, Ended::Failed(failure())),
            unchanged("The keys are unchanged: broke")
        );
        // The limit of the attempt is a failure like any other.
        assert_eq!(
            next_step(Stage::Rebinding, Ended::Failed(limit_reached(REBIND_LIMIT))),
            unchanged("The keys are unchanged: binding the shortcut: no result within 600s")
        );
        assert_eq!(
            next_step(Stage::Rebinding, Ended::PortalLeft),
            Step::Bind {
                stage: Stage::Returning {
                    message: REBIND_PORTAL_LEFT.to_owned()
                },
                close: false
            }
        );
        let returning = || Stage::Returning {
            message: "kept".to_owned(),
        };
        assert_eq!(
            next_step(returning(), Ended::PortalLeft),
            Step::Bind {
                stage: returning(),
                close: false
            }
        );
        assert_eq!(
            next_step(returning(), Ended::Failed(failure())),
            Step::Stop(failure())
        );
        assert_eq!(
            next_step(returning(), Ended::Declined(failure())),
            Step::Stop(failure())
        );
        assert_eq!(
            next_step(Stage::Current, Ended::Declined(failure())),
            Step::Stop(failure())
        );
        assert_eq!(
            next_step(
                Stage::Current,
                Ended::Rebind {
                    bindings: Bindings::default()
                }
            ),
            Step::Bind {
                stage: Stage::Rebinding,
                close: true
            }
        );
    }

    #[test]
    fn only_a_rebind_has_the_long_limit() {
        let normal = Duration::from_secs(60);
        let plan = |stage| Plan {
            app_id: "x",
            file: None,
            generation: 4,
            preferences: Preferences {
                clipboard: None,
                screenshot: None,
            },
            stage,
        };
        assert_eq!(plan(Stage::Current).limit(normal), normal);
        assert_eq!(plan(Stage::Rebinding).limit(normal), REBIND_LIMIT);
        let returning = Stage::Returning {
            message: String::new(),
        };
        assert_eq!(plan(returning).limit(normal), normal);
        assert_eq!(plan(Stage::Current).bound_generation(), 4);
        assert_eq!(plan(Stage::Rebinding).bound_generation(), 5);
    }

    #[test]
    fn a_generation_file_that_cannot_be_read_is_generation_zero() {
        let dir = std::env::temp_dir().join(format!("addict-generation-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("directory");
        let file = dir.join("generation");
        assert_eq!(read_generation(&file), 0);
        assert_eq!(save_generation(Some(&file), 7), None);
        assert_eq!(read_generation(&file), 7);
        for content in ["", "seven\n", "-1\n", "1 2\n"] {
            std::fs::write(&file, content).expect("write");
            assert_eq!(read_generation(&file), 0, "{content:?}");
        }
        std::fs::remove_file(&file).expect("remove");
        std::fs::create_dir(&file).expect("a directory in its place");
        assert_eq!(read_generation(&file), 0);
        let refused = save_generation(Some(&file), 1).expect("a directory can't be replaced");
        assert!(refused.contains("next start"), "{refused}");
        assert!(!refused.contains('\n'), "{refused}");
        let nowhere = save_generation(None, 1).expect("no file");
        assert!(nowhere.contains("next start"), "{nowhere}");
        std::fs::remove_dir_all(&dir).expect("remove");
    }

    #[test]
    fn action_identity_and_preference_preserve_existing_clipboard_assignment() {
        let definitions = shortcuts(
            &Ids::of(0),
            &Preferences {
                clipboard: Some("LOGO+q".to_owned()),
                screenshot: None,
            },
        );
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
    fn a_later_generation_asks_for_both_current_keys_under_its_own_ids() {
        let definitions = shortcuts(
            &Ids::of(2),
            &Preferences {
                clipboard: Some("CTRL+LOGO+q".to_owned()),
                screenshot: Some("F8".to_owned()),
            },
        );
        let context = zvariant::serialized::Context::new_dbus(zvariant::LE, 0);
        let bytes =
            zvariant::to_bytes(context, &definitions.as_slice()).expect("serialize shortcuts");
        let (definitions, _) = bytes
            .deserialize::<Vec<(String, HashMap<String, OwnedValue>)>>()
            .expect("shortcut properties");
        assert_eq!(definitions[0].0, "invoke-2");
        assert_eq!(
            <&str>::try_from(&definitions[0].1["preferred_trigger"]).unwrap(),
            "CTRL+LOGO+q"
        );
        assert_eq!(definitions[1].0, "screenshot-2");
        assert_eq!(
            <&str>::try_from(&definitions[1].1["preferred_trigger"]).unwrap(),
            "F8"
        );
    }

    #[test]
    fn partial_bindings_and_changed_ids_keep_independent_assignments() {
        let ids = Ids::of(0);
        let mut bindings = Bindings::default();
        bindings.update(&ids, [("invoke", "Super+Q")]);
        assert_eq!(bindings.clipboard.as_deref(), Some("Super+Q"));
        assert_eq!(bindings.screenshot, None);
        assert!(!bindings.changed(
            &ids,
            "/ours",
            "/other",
            [("invoke", "F1"), ("screenshot", "F2")]
        ));
        assert_eq!(bindings.clipboard.as_deref(), Some("Super+Q"));
        assert_eq!(bindings.screenshot, None);
        assert!(bindings.changed(
            &ids,
            "/ours",
            "/ours",
            [("screenshot", "Super+S"), ("other", "F1")]
        ));
        assert_eq!(bindings.clipboard.as_deref(), Some("Super+Q"));
        assert_eq!(bindings.screenshot.as_deref(), Some("Super+S"));
        bindings.update(&ids, [("screenshot", " ")]);
        assert_eq!(bindings.screenshot, None);
        assert_eq!(bindings.clipboard.as_deref(), Some("Super+Q"));
        bindings.update(&ids, [("invoke", "")]);
        assert_eq!(bindings, Bindings::default());
    }

    #[test]
    fn only_known_actions_in_our_session_activate() {
        let ids = Ids::of(0);
        assert_eq!(
            session_action(&ids, "/ours", "/ours", "invoke"),
            Some(Action::Clipboard)
        );
        assert_eq!(
            session_action(&ids, "/ours", "/ours", "screenshot"),
            Some(Action::Screenshot)
        );
        assert_eq!(session_action(&ids, "/ours", "/other", "invoke"), None);
        assert_eq!(session_action(&ids, "/ours", "/other", "screenshot"), None);
        assert_eq!(session_action(&ids, "/ours", "/ours", "unknown"), None);
    }
}
