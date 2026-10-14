//! The screen capture: one interactive Screenshot request as the given host
//! app id, and the local file behind the URI it returns.
//!
//! [`capture`] opens a bus connection of its own, registers the app id on it
//! before any other portal call, and sends `interactive=true` with no target:
//! the user chooses the area in the desktop's own interface. It ends with the
//! path of the snapshot or one [`CaptureFailure`]. It has no limit of its
//! own; the caller bounds the wait.

use std::path::PathBuf;

use ashpd::{
    AppID, PortalError,
    desktop::{
        ResponseError,
        screenshot::{ScreenshotOptions, ScreenshotProxy},
    },
    zbus,
};

/// Why a capture gave no snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFailure {
    /// The portal answered with code 1: the user cancelled.
    Cancelled,
    /// The portal answered with code 2: the request ended otherwise. On
    /// GNOME this is also what Esc gives; no cause is claimed.
    Ended,
    /// The bus, the portal or its Screenshot interface is not there.
    Unavailable,
    /// A step failed; the text names it with the portal's own error.
    Failed(String),
    /// The URI is not a local file, or no file is behind it.
    NoImage,
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

/// Asks the portal, as `app_id`, for one interactive screenshot and gives
/// the local path of the image it made.
pub async fn capture(app_id: &str) -> Result<PathBuf, CaptureFailure> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|e| from_zbus("session bus", &e))?;

    // Must precede any other portal call on the connection.
    let parsed: AppID = app_id
        .parse()
        .map_err(|e: ashpd::Error| CaptureFailure::Failed(format!("app id {app_id:?}: {e}")))?;
    ashpd::register_host_app_with_connection(connection.clone(), parsed)
        .await
        .map_err(|e| classify(&format!("host registration of {app_id:?}"), &e))?;

    let proxy = ScreenshotProxy::with_connection(connection)
        .await
        .map_err(|e| classify("Screenshot portal", &e))?;
    let options = ScreenshotOptions::default().set_interactive(true);
    let shot = proxy
        .screenshot(None, options)
        .await
        .and_then(|request| request.response())
        .map_err(|e| classify("screenshot", &e))?;

    let path = file_path_from_uri(shot.uri().as_str()).ok_or(CaptureFailure::NoImage)?;
    if path.is_file() {
        Ok(path)
    } else {
        Err(CaptureFailure::NoImage)
    }
}

/// The failure for a portal `error` met at `step`.
fn classify(step: &str, error: &ashpd::Error) -> CaptureFailure {
    match error {
        ashpd::Error::Response(ResponseError::Cancelled) => CaptureFailure::Cancelled,
        ashpd::Error::Response(ResponseError::Other) => CaptureFailure::Ended,
        ashpd::Error::PortalNotFound(_) | ashpd::Error::RequiresVersion(..) => {
            CaptureFailure::Unavailable
        }
        ashpd::Error::Zbus(e) | ashpd::Error::Portal(PortalError::ZBus(e)) => from_zbus(step, e),
        _ => CaptureFailure::Failed(format!("{step}: {error}")),
    }
}

/// The failure for a bus `error` met at `step`.
fn from_zbus(step: &str, error: &zbus::Error) -> CaptureFailure {
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
        CaptureFailure::Unavailable
    } else {
        CaptureFailure::Failed(format!("{step}: {error}"))
    }
}

/// The local path of a `file://` URI, percent-decoded. Any other scheme, a
/// non-local host or a malformed escape gives `None`.
pub fn file_path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path = if rest.starts_with('/') {
        rest
    } else {
        rest.strip_prefix("localhost")
            .filter(|p| p.starts_with('/'))?
    };
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ashpd::desktop::ResponseError;

    use super::{CaptureFailure, classify, file_path_from_uri, from_zbus};

    #[test]
    fn a_plain_file_uri_is_its_path() {
        assert_eq!(
            file_path_from_uri("file:///home/u/Pictures/Screenshot.png"),
            Some(PathBuf::from("/home/u/Pictures/Screenshot.png"))
        );
    }

    #[test]
    fn localhost_is_a_local_host() {
        assert_eq!(
            file_path_from_uri("file://localhost/tmp/a.png"),
            Some(PathBuf::from("/tmp/a.png"))
        );
    }

    #[test]
    fn percent_escapes_are_decoded() {
        assert_eq!(
            file_path_from_uri("file:///tmp/Screenshot%20from%202026.png"),
            Some(PathBuf::from("/tmp/Screenshot from 2026.png"))
        );
        assert_eq!(
            file_path_from_uri("file:///tmp/%D0%B0.png"),
            Some(PathBuf::from("/tmp/\u{430}.png"))
        );
    }

    #[test]
    fn another_scheme_or_host_is_not_local() {
        for uri in [
            "https://example.org/a.png",
            "file://example.org/a.png",
            "/tmp/a.png",
            "file://",
        ] {
            assert_eq!(file_path_from_uri(uri), None, "{uri}");
        }
    }

    #[test]
    fn a_bad_escape_is_refused() {
        for uri in ["file:///tmp/a%2", "file:///tmp/a%zz.png", "file:///tmp/%"] {
            assert_eq!(file_path_from_uri(uri), None, "{uri}");
        }
    }

    #[test]
    fn response_codes_keep_their_meaning() {
        let cancelled = ashpd::Error::Response(ResponseError::Cancelled);
        let other = ashpd::Error::Response(ResponseError::Other);
        assert_eq!(classify("x", &cancelled), CaptureFailure::Cancelled);
        assert_eq!(classify("x", &other), CaptureFailure::Ended);
    }

    #[test]
    fn a_missing_bus_or_portal_is_unavailable() {
        let missing = ashpd::Error::PortalNotFound(
            "org.freedesktop.portal.Screenshot"
                .try_into()
                .expect("name"),
        );
        assert_eq!(classify("x", &missing), CaptureFailure::Unavailable);
        let unknown = ashpd::zbus::Error::FDO(Box::new(ashpd::zbus::fdo::Error::ServiceUnknown(
            "no portal".to_owned(),
        )));
        assert_eq!(from_zbus("x", &unknown), CaptureFailure::Unavailable);
        let refused = ashpd::zbus::Error::FDO(Box::new(ashpd::zbus::fdo::Error::AccessDenied(
            "no".to_owned(),
        )));
        assert!(
            matches!(from_zbus("step", &refused), CaptureFailure::Failed(t) if t.starts_with("step: "))
        );
    }
}
