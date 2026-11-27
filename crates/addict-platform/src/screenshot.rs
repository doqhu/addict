//! The screen capture: one interactive Screenshot request as the given host
//! app id, and the local file behind the URI it returns.
//!
//! [`capture`] opens a bus connection of its own, registers the app id on it
//! before any other portal call, and sends `interactive=true` with no target:
//! the user chooses the area in the desktop's own interface. It ends with the
//! path of the snapshot or one [`CaptureFailure`]. It has no limit of its
//! own; the caller bounds the wait.

use std::{
    collections::HashMap,
    future::{self, Future},
    path::PathBuf,
    time::Duration,
};

use addict_core::failure::capture::CaptureFailure;
use futures_util::{FutureExt, StreamExt};
use zbus::{
    Connection, MatchRule, MessageStream,
    message::Type,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

use tokio::time::{Instant, timeout_at};

use crate::portal::{self, PortalFailure};

impl From<PortalFailure> for CaptureFailure {
    fn from(failure: PortalFailure) -> Self {
        match failure {
            PortalFailure::Cancelled => Self::Cancelled,
            PortalFailure::Ended => Self::Ended,
            PortalFailure::Unavailable => Self::Unavailable,
            PortalFailure::Failed(text) => Self::Failed(text),
        }
    }
}

/// Asks the portal, as `app_id`, for one interactive screenshot and gives
/// the local path of the image it made. Where the portal has no host Registry
/// the request is made unregistered, which the interactive picker allows.
pub async fn capture(app_id: &str) -> Result<PathBuf, CaptureFailure> {
    capture_until(app_id, future::pending()).await
}

/// Captures on a dedicated bus connection. Cancellation closes the portal
/// request on that same connection before returning to the caller.
pub async fn capture_until(
    app_id: &str,
    cancelled: impl Future<Output = ()>,
) -> Result<PathBuf, CaptureFailure> {
    tokio::pin!(cancelled);
    let connection = tokio::select! {
        biased;
        () = &mut cancelled => return Err(CaptureFailure::Cancelled),
        connection = portal::connection_degrading_without_registry(app_id) => connection?,
    };

    ask(&connection, cancelled).await
}

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const REQUEST: &str = "org.freedesktop.portal.Request";
/// A bound on cleanup after cancellation, independent of the picker deadline.
const CLOSE_LIMIT: Duration = Duration::from_secs(1);

async fn ask(
    connection: &Connection,
    cancelled: impl Future<Output = ()>,
) -> Result<PathBuf, CaptureFailure> {
    tokio::pin!(cancelled);
    // Subscribe before Screenshot so even a response on a different handle
    // can be queued while the method reply is still in flight.
    let prepare = async {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(PORTAL)?
            .interface(REQUEST)?
            .member("Response")?
            .build();
        MessageStream::for_match_rule(rule, connection, None).await
    };
    let responses = tokio::select! {
        biased;
        () = &mut cancelled => return Err(CaptureFailure::Cancelled),
        prepared = prepare => prepared.map_err(|e| portal::from_zbus("Screenshot portal", &e))?,
    };
    let options = HashMap::from([
        ("handle_token", Value::from("capture")),
        ("interactive", Value::from(true)),
    ]);
    let body = ("", options);
    let method = connection.call_method(
        Some(PORTAL),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.Screenshot"),
        "Screenshot",
        &body,
    );
    tokio::pin!(method);
    let (reply, cleanup_deadline) = tokio::select! {
        biased;
        reply = &mut method => (reply, None),
        () = &mut cancelled => {
            let deadline = Instant::now() + CLOSE_LIMIT;
            let reply = timeout_at(deadline, &mut method).await
                .map_err(|_| CaptureFailure::Cancelled)?;
            (reply, Some(deadline))
        }
    };
    let reply = reply.map_err(|e| portal::from_zbus("screenshot", &e))?;
    let returned: OwnedObjectPath = reply
        .body()
        .deserialize()
        .map_err(|e| portal::from_zbus("screenshot", &e))?;
    let mut responses = responses.filter_map(|message| {
        future::ready(message.ok().filter(|message| {
            message
                .header()
                .path()
                .is_some_and(|path| path.as_str() == returned.as_str())
        }))
    });
    let deadline = match cleanup_deadline {
        Some(deadline) => deadline,
        None => tokio::select! {
            biased;
            response = responses.next() => return decode(response),
            () = &mut cancelled => Instant::now() + CLOSE_LIMIT,
        },
    };
    let close =
        connection.call_method(Some(PORTAL), returned.as_str(), Some(REQUEST), "Close", &());
    let _ = timeout_at(deadline, close).await;
    // If the picker completed concurrently with Close, pass the ready file
    // to the caller so its normal ownership guard can remove it.
    match responses.next().now_or_never().flatten() {
        Some(response) => decode(Some(response)),
        None => Err(CaptureFailure::Cancelled),
    }
}

fn decode(response: Option<zbus::Message>) -> Result<PathBuf, CaptureFailure> {
    let response =
        response.ok_or_else(|| CaptureFailure::Failed("screenshot: no portal response".into()))?;
    let (code, data): (u32, HashMap<String, OwnedValue>) = response
        .body()
        .deserialize()
        .map_err(|e| portal::from_zbus("screenshot", &e))?;
    match code {
        0 => {}
        1 => return Err(CaptureFailure::Cancelled),
        2 => return Err(CaptureFailure::Ended),
        _ => {
            return Err(CaptureFailure::Failed(format!(
                "screenshot: unknown response code {code}"
            )));
        }
    }
    let path = data
        .get("uri")
        .and_then(|uri| <&str>::try_from(uri).ok())
        .and_then(file_path_from_uri)
        .ok_or(CaptureFailure::NoImage)?;
    if path.is_file() {
        Ok(path)
    } else {
        Err(CaptureFailure::NoImage)
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

    use addict_core::failure::capture::CaptureFailure;

    use super::file_path_from_uri;
    use crate::portal;

    fn classify(step: &str, error: &ashpd::Error) -> CaptureFailure {
        portal::classify(step, error).into()
    }

    fn from_zbus(step: &str, error: &ashpd::zbus::Error) -> CaptureFailure {
        portal::from_zbus(step, error).into()
    }

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
