//! Invocation contract: optional host registration, a GlobalShortcuts
//! session, one binding, the list the portal reports back, and a bounded
//! wait for `Activated`.

use std::time::Instant;

use ashpd::desktop::{
    CreateSessionOptions,
    global_shortcuts::{
        BindShortcutsOptions, GlobalShortcuts, ListShortcutsOptions, NewShortcut, Shortcut,
    },
};
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::{
    portal::{self, Failure},
    record::{Bounded, Finish, Outcome, Progress, bounded},
};

pub const EXPECTED: &str = "outcome activated: the binding is accepted with a non-empty trigger \
     and pressing it while another application is focused delivers Activated; not-bound when the \
     dialog is dismissed or zero shortcuts are bound; timeout when nothing is pressed in time";

/// The id of the single shortcut the probe binds inside its session.
pub const SHORTCUT_ID: &str = "probe-invoke";

#[derive(Debug, Clone)]
pub struct Args {
    pub app_id: Option<String>,
    /// Preferred trigger in the XDG shortcuts syntax; the user may change it.
    pub trigger: Option<String>,
}

/// What a `BindShortcuts` (or `ListShortcuts`) answer of `count` shortcuts
/// means for the run.
pub fn bind_outcome(count: usize) -> Option<Outcome> {
    (count == 0).then_some(Outcome::NotBound)
}

/// A dismissed binding dialog is `not-bound`, not a cancellation of the run.
pub fn bind_failure(error: &ashpd::Error) -> Finish {
    match portal::classify(error) {
        Failure::Cancelled => Finish::with_error(Outcome::NotBound, error.to_string()),
        _ => portal::finish_for(error),
    }
}

fn describe(shortcuts: &[Shortcut]) -> Value {
    shortcuts
        .iter()
        .map(|s| {
            json!({
                "id": s.id(),
                "description": s.description(),
                "trigger_description": s.trigger_description(),
            })
        })
        .collect()
}

pub async fn run(args: &Args, progress: &Progress) -> Finish {
    let connection = match portal::connect(progress).await {
        Ok(connection) => connection,
        Err(finish) => return finish,
    };
    if let Err(finish) = portal::register(&connection, args.app_id.as_deref(), progress).await {
        return finish;
    }

    progress.phase("open-global-shortcuts");
    let portal = match GlobalShortcuts::with_connection(connection.clone()).await {
        Ok(portal) => portal,
        Err(e) => return portal::finish_for(&e),
    };
    portal::record_version(
        &connection,
        "org.freedesktop.portal.GlobalShortcuts",
        progress,
    )
    .await;

    // Subscribe before binding so no activation can slip between the two.
    let activations = match portal.receive_activated().await {
        Ok(stream) => stream,
        Err(e) => return portal::finish_for(&e),
    };
    let mut activations = std::pin::pin!(activations);

    progress.phase("create-session");
    let session = match portal.create_session(CreateSessionOptions::default()).await {
        Ok(session) => session,
        Err(e) => return portal::finish_for(&e),
    };

    progress.phase("bind-dialog");
    eprintln!("Binding one shortcut: answer the system dialog.");
    let shortcut = NewShortcut::new(SHORTCUT_ID, "Addict probe: invoke")
        .preferred_trigger(args.trigger.as_deref());
    let bind_started = Instant::now();
    let bound = match portal
        .bind_shortcuts(&session, &[shortcut], None, BindShortcutsOptions::default())
        .await
        .and_then(|request| request.response())
    {
        Ok(bound) => bound,
        Err(e) => {
            progress.put("bind_ms", millis(bind_started));
            return bind_failure(&e);
        }
    };
    let bound_at = Instant::now();
    progress.put("bind_ms", millis(bind_started));
    progress.put("bound", describe(bound.shortcuts()));
    if let Some(outcome) = bind_outcome(bound.shortcuts().len()) {
        return Finish::with_error(outcome, "the portal bound zero shortcuts");
    }

    progress.phase("list-shortcuts");
    match portal
        .list_shortcuts(&session, ListShortcutsOptions::default())
        .await
        .and_then(|request| request.response())
    {
        Ok(listed) => progress.put("listed", describe(listed.shortcuts())),
        Err(e) => progress.put("list_error", e.to_string()),
    }

    progress.phase("wait-for-press");
    let trigger = bound.shortcuts()[0].trigger_description().to_owned();
    eprintln!("Bound to {trigger:?}. Focus another application and press it.");
    // The wrapper keeps the session path private; it serializes as the path.
    let Some(session_path) = serde_json::to_value(&session)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
    else {
        return Finish::with_error(Outcome::Error, "the session handle has no readable path");
    };
    let activated = loop {
        match activations.next().await {
            Some(signal) if signal.session_handle().as_str() == session_path => break signal,
            Some(_) => continue,
            None => {
                return Finish::with_error(
                    Outcome::Error,
                    "the Activated signal stream ended before any press",
                );
            }
        }
    };

    let mut option_keys: Vec<&String> = activated.options().keys().collect();
    option_keys.sort();
    progress.put("bind_to_press_ms", millis(bound_at));
    progress.put("activated_shortcut_id", activated.shortcut_id());
    progress.put(
        "activated_timestamp_raw",
        activated.timestamp().as_millis() as u64,
    );
    progress.put("activated_option_keys", &option_keys);
    progress.put(
        "activation_token_present",
        activated.options().contains_key("activation_token"),
    );

    progress.phase("close-session");
    match bounded(std::time::Duration::from_secs(1), session.close()).await {
        Bounded::Done(Ok(())) => progress.put("session_closed", true),
        Bounded::Done(Err(e)) => progress.put("session_close_error", e.to_string()),
        Bounded::TimedOut => progress.put("session_close_error", "no answer within 1 s"),
    }
    progress.phase("done");
    Finish::ok(Outcome::Activated)
}

fn millis(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}
