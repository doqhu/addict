/// The shell or audio fetch could not complete playback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackFailure {
    /// The recording could not be reached over the network.
    Network,
    /// The user dismissed the work or the shell honoured Cancel.
    Cancelled,
    /// Everything else, with its diagnostic detail.
    Failed(String),
}
