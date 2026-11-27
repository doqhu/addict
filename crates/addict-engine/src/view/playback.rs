//! The text for a pronunciation that could not be played.

use addict_core::failure::playback::PlaybackFailure;

use super::{PLAYBACK_FAILED, RECORDING_NEEDS_NETWORK};

pub fn playback_text(failure: &PlaybackFailure) -> Option<&'static str> {
    match failure {
        PlaybackFailure::Network => Some(RECORDING_NEEDS_NETWORK),
        PlaybackFailure::Failed(_) => Some(PLAYBACK_FAILED),
        PlaybackFailure::Cancelled => None,
    }
}
