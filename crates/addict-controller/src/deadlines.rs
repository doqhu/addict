//! Product deadlines supplied to jobs and shell adapters.

use std::time::Duration;

pub const LOOKUP_LIMIT: Duration = Duration::from_secs(2);
pub const TRANSLATE_LIMIT: Duration = Duration::from_secs(10);

/// Includes the wait for the window to become active.
pub const CLIPBOARD_LIMIT: Duration = Duration::from_secs(1);
/// Time before the shortcut bind is requested, and time for the user's answer.
pub const BIND_START_LIMIT: Duration = Duration::from_secs(60);
pub const BIND_ANSWER_LIMIT: Duration = Duration::from_secs(600);

pub const CAPTURE_LIMIT: Duration = Duration::from_secs(300);
pub const OCR_LIMIT: Duration = Duration::from_secs(15);
pub const CAPTURE_JOB_LIMIT: Duration = CAPTURE_LIMIT
    .saturating_add(OCR_LIMIT)
    .saturating_add(Duration::from_secs(5));

pub const MANIFEST_LIMIT: Duration = Duration::from_secs(60);
pub const DOWNLOAD_LIMIT: Duration = Duration::from_secs(30 * 60);

pub const CREDIT_LIMIT: Duration = Duration::from_secs(10);
pub const AUDIO_LIMIT: Duration = Duration::from_secs(20);

pub const LANGUAGE_FILE_LIMIT: Duration = Duration::from_secs(2);
pub const FIRST_RUN_FILE_LIMIT: Duration = Duration::from_secs(2);
pub const KEY_SAVE_LIMIT: Duration = Duration::from_secs(2);
pub const CACHE_LIMIT: Duration = Duration::from_secs(5);
pub const STARTUP_READ_LIMIT: Duration = Duration::from_secs(2);
/// The portal may wait for the user to answer its dialog.
pub const STARTUP_LIMIT: Duration = Duration::from_secs(60);
