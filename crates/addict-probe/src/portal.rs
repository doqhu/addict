//! Shared portal plumbing: the bus connection, optional host registration
//! and the mapping from portal errors to probe outcomes.

use ashpd::{
    AppID, PortalError,
    desktop::ResponseError,
    zbus::{self, zvariant::OwnedValue},
};

use crate::record::{Finish, Outcome, Progress};

pub const DESKTOP_DESTINATION: &str = "org.freedesktop.portal.Desktop";
pub const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";

/// What a portal failure means, independent of which probe met it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The request got a `Response` with code 1: the user cancelled.
    Cancelled,
    /// The request got a `Response` with code 2: ended without a reason.
    Other,
    /// The bus, the portal or the interface is not there.
    Unavailable,
    /// The portal answered with an error.
    Failed,
}

/// D-Bus error names that mean "nobody implements this here".
const ABSENT: &[&str] = &[
    "org.freedesktop.DBus.Error.ServiceUnknown",
    "org.freedesktop.DBus.Error.NameHasNoOwner",
    "org.freedesktop.DBus.Error.UnknownMethod",
    "org.freedesktop.DBus.Error.UnknownInterface",
    "org.freedesktop.DBus.Error.UnknownObject",
    "org.freedesktop.DBus.Error.UnknownProperty",
    "org.freedesktop.DBus.Error.Spawn.ServiceNotFound",
];

pub fn classify(error: &ashpd::Error) -> Failure {
    match error {
        ashpd::Error::Response(ResponseError::Cancelled) => Failure::Cancelled,
        ashpd::Error::Response(ResponseError::Other) => Failure::Other,
        ashpd::Error::PortalNotFound(_) | ashpd::Error::RequiresVersion(..) => Failure::Unavailable,
        ashpd::Error::Zbus(e) | ashpd::Error::Portal(PortalError::ZBus(e)) => classify_zbus(e),
        _ => Failure::Failed,
    }
}

pub fn classify_zbus(error: &zbus::Error) -> Failure {
    match error {
        zbus::Error::MethodError(name, ..) => classify_error_name(name.as_str()),
        zbus::Error::FDO(e) => match **e {
            zbus::fdo::Error::ServiceUnknown(_)
            | zbus::fdo::Error::NameHasNoOwner(_)
            | zbus::fdo::Error::UnknownMethod(_)
            | zbus::fdo::Error::UnknownInterface(_)
            | zbus::fdo::Error::UnknownObject(_)
            | zbus::fdo::Error::UnknownProperty(_) => Failure::Unavailable,
            _ => Failure::Failed,
        },
        zbus::Error::Address(_)
        | zbus::Error::InputOutput(_)
        | zbus::Error::Handshake(_)
        | zbus::Error::InterfaceNotFound
        | zbus::Error::Unsupported => Failure::Unavailable,
        _ => Failure::Failed,
    }
}

pub fn classify_error_name(name: &str) -> Failure {
    if ABSENT.contains(&name) {
        Failure::Unavailable
    } else {
        Failure::Failed
    }
}

/// The outcome for a failure that the calling probe gives no special meaning.
pub fn finish_for(error: &ashpd::Error) -> Finish {
    let outcome = match classify(error) {
        Failure::Cancelled => Outcome::Cancelled,
        Failure::Unavailable => Outcome::Unavailable,
        Failure::Other | Failure::Failed => Outcome::Error,
    };
    Finish::with_error(outcome, error.to_string())
}

/// Connects to the session bus. A bus that cannot be reached is the
/// `unavailable` outcome, with the bus error as text.
pub async fn connect(progress: &Progress) -> Result<zbus::Connection, Finish> {
    progress.phase("connect-session-bus");
    zbus::Connection::session()
        .await
        .map_err(|e| Finish::with_error(Outcome::Unavailable, format!("session bus: {e}")))
}

/// Registers the host app id on this connection when one was given. Must run
/// before any other portal call on the connection. Without an app id nothing
/// is sent and the portal derives the identity itself.
pub async fn register(
    connection: &zbus::Connection,
    app_id: Option<&str>,
    progress: &Progress,
) -> Result<(), Finish> {
    let Some(app_id) = app_id else {
        progress.put("registration", "not-requested");
        return Ok(());
    };
    progress.phase("register-host-app");
    if ashpd::is_sandboxed() {
        // The wrapper sends no `Register` from inside a sandbox; say so
        // instead of claiming a registration that never happened.
        progress.put("registration", "not-sent-sandboxed");
        return Ok(());
    }
    let parsed: AppID = app_id.parse().map_err(|e: ashpd::Error| {
        progress.put("registration", "invalid-app-id");
        Finish::with_error(Outcome::Error, format!("app id {app_id:?}: {e}"))
    })?;
    match ashpd::register_host_app_with_connection(connection.clone(), parsed).await {
        Ok(()) => {
            progress.put("registration", "registered");
            Ok(())
        }
        Err(e) => {
            progress.put("registration", "refused");
            Err(Finish::with_error(
                Outcome::Unavailable,
                format!("host registration of {app_id:?}: {e}"),
            ))
        }
    }
}

/// Records the interface `version` the portal itself gives. A failed read is
/// recorded as missing with its error, never as a wrapper's fallback value.
pub async fn record_version(connection: &zbus::Connection, interface: &str, progress: &Progress) {
    match property_u32(connection, interface, "version").await {
        Ok(version) => progress.put("interface_version", version),
        Err(e) => {
            progress.put("interface_version", None::<u32>);
            progress.put("interface_version_error", e.to_string());
        }
    }
}

/// Reads one property straight from the portal object, with no version gate
/// from a wrapper in between.
pub async fn property(
    connection: &zbus::Connection,
    interface: &str,
    name: &str,
) -> zbus::Result<OwnedValue> {
    let reply = connection
        .call_method(
            Some(DESKTOP_DESTINATION),
            DESKTOP_PATH,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(interface, name),
        )
        .await?;
    reply.body().deserialize::<OwnedValue>()
}

pub async fn property_u32(
    connection: &zbus::Connection,
    interface: &str,
    name: &str,
) -> zbus::Result<u32> {
    let value = property(connection, interface, name).await?;
    u32::try_from(value).map_err(zbus::Error::Variant)
}

/// Names of the Screenshot `AvailableTargets` bits that are set. Unknown bits
/// are reported by number so nothing the backend said is dropped.
pub fn target_names(bits: u32) -> Vec<String> {
    const KNOWN: &[(u32, &str)] = &[
        (1, "screen"),
        (2, "window"),
        (4, "area"),
        (8, "active-window"),
    ];
    let mut names: Vec<String> = KNOWN
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| (*name).to_owned())
        .collect();
    let unknown = bits & !KNOWN.iter().fold(0, |all, (bit, _)| all | bit);
    if unknown != 0 {
        names.push(format!("unknown-bits-{unknown:#x}"));
    }
    names
}
