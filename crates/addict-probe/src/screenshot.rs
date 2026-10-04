//! Capture contract: one interactive Screenshot request, the returned URI,
//! the image behind it, and cancellation as its own outcome.

use std::path::{Path, PathBuf};

use ashpd::desktop::screenshot::{ScreenshotOptions, ScreenshotProxy};
use serde_json::json;

use crate::{
    portal,
    record::{Finish, Outcome, Progress},
};

pub const EXPECTED: &str = "outcome captured: the interactive request returns a URI to an \
     existing image whose pixel size can be read; cancelled when the user presses Esc, with no \
     file referenced";

/// The only options the probe sends besides the request handle token. The
/// installed backend advertises no targets, so none is ever passed.
pub const SENT_OPTIONS: &[&str] = &["interactive"];

#[derive(Debug, Clone)]
pub struct Args {
    pub app_id: Option<String>,
}

pub async fn run(args: &Args, progress: &Progress) -> Finish {
    let connection = match portal::connect(progress).await {
        Ok(connection) => connection,
        Err(finish) => return finish,
    };
    if let Err(finish) = portal::register(&connection, args.app_id.as_deref(), progress).await {
        return finish;
    }

    progress.phase("open-screenshot");
    let proxy = match ScreenshotProxy::with_connection(connection.clone()).await {
        Ok(proxy) => proxy,
        Err(e) => return portal::finish_for(&e),
    };
    portal::record_version(&connection, "org.freedesktop.portal.Screenshot", progress).await;
    match portal::property_u32(
        &connection,
        "org.freedesktop.portal.Screenshot",
        "AvailableTargets",
    )
    .await
    {
        Ok(bits) => progress.put(
            "available_targets",
            json!({ "present": true, "bits": bits, "names": portal::target_names(bits) }),
        ),
        Err(e) => progress.put(
            "available_targets",
            json!({ "present": false, "error": e.to_string() }),
        ),
    }

    progress.phase("capture-interaction");
    eprintln!("Interactive capture: take the screenshot, or press Esc to cancel.");
    let options = ScreenshotOptions::default().set_interactive(true);
    let shot = match proxy
        .screenshot(None, options)
        .await
        .and_then(|request| request.response())
    {
        Ok(shot) => shot,
        Err(e) => return portal::finish_for(&e),
    };

    progress.phase("inspect-image");
    let uri = shot.uri().as_str().to_owned();
    progress.put("uri", &uri);
    match file_path_from_uri(&uri) {
        None => progress.put("file", json!({ "local": false })),
        Some(path) => progress.put("file", inspect(&path)),
    }
    progress.phase("done");
    Finish::ok(Outcome::Captured)
}

/// What is on disk behind the URI: existence, byte size and pixel size.
pub fn inspect(path: &Path) -> serde_json::Value {
    let Ok(metadata) = std::fs::metadata(path) else {
        return json!({ "local": true, "path": path, "exists": false });
    };
    let mut file = json!({
        "local": true,
        "path": path,
        "exists": true,
        "bytes": metadata.len(),
    });
    match imagesize::size(path) {
        Ok(size) => {
            file["width"] = json!(size.width);
            file["height"] = json!(size.height);
        }
        Err(e) => file["size_error"] = json!(e.to_string()),
    }
    file
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
