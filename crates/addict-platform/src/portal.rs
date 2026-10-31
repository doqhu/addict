//! What the portal adapters share: a connection registered as the host app,
//! and what a portal error means.

use ashpd::{AppID, PortalError, desktop::ResponseError, zbus};

/// Why a portal request gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PortalFailure {
    /// The portal answered with code 1.
    Cancelled,
    /// The portal answered with code 2.
    Ended,
    /// The bus, the portal or the interface is not there.
    Unavailable,
    /// A step failed; the text names it with the portal's own error.
    Failed(String),
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

/// A new connection to the session bus, with `app_id` registered on it before
/// any other portal call. In a sandbox the portal knows the app already and
/// `ashpd` registers nothing.
pub(crate) async fn registered_connection(app_id: &str) -> Result<zbus::Connection, PortalFailure> {
    // No connection is no bus, whichever error zbus gives for it.
    let connection = zbus::Connection::session()
        .await
        .map_err(|_| PortalFailure::Unavailable)?;
    let parsed: AppID = app_id
        .parse()
        .map_err(|e: ashpd::Error| PortalFailure::Failed(format!("app id {app_id:?}: {e}")))?;
    ashpd::register_host_app_with_connection(connection.clone(), parsed)
        .await
        .map_err(|e| classify(&format!("host registration of {app_id:?}"), &e))?;
    Ok(connection)
}

/// The failure for a portal `error` met at `step`.
pub(crate) fn classify(step: &str, error: &ashpd::Error) -> PortalFailure {
    match error {
        ashpd::Error::Response(ResponseError::Cancelled) => PortalFailure::Cancelled,
        ashpd::Error::Response(ResponseError::Other) => PortalFailure::Ended,
        ashpd::Error::PortalNotFound(_) | ashpd::Error::RequiresVersion(..) => {
            PortalFailure::Unavailable
        }
        ashpd::Error::Zbus(e) | ashpd::Error::Portal(PortalError::ZBus(e)) => from_zbus(step, e),
        _ => PortalFailure::Failed(format!("{step}: {error}")),
    }
}

/// The failure for a bus `error` met at `step`.
pub(crate) fn from_zbus(step: &str, error: &zbus::Error) -> PortalFailure {
    let absent = match error {
        zbus::Error::MethodError(name, ..) => ABSENT.contains(&name.as_str()),
        zbus::Error::FDO(e) => matches!(
            **e,
            zbus::fdo::Error::ServiceUnknown(_)
                | zbus::fdo::Error::NameHasNoOwner(_)
                | zbus::fdo::Error::UnknownMethod(_)
                | zbus::fdo::Error::UnknownInterface(_)
                | zbus::fdo::Error::UnknownObject(_)
                | zbus::fdo::Error::UnknownProperty(_)
        ),
        zbus::Error::Address(_)
        | zbus::Error::InputOutput(_)
        | zbus::Error::Handshake(_)
        | zbus::Error::InterfaceNotFound
        | zbus::Error::Unsupported => true,
        _ => false,
    };
    if absent {
        PortalFailure::Unavailable
    } else {
        PortalFailure::Failed(format!("{step}: {error}"))
    }
}
