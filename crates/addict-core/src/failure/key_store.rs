//! Why storing the DeepL key did not happen.

/// Why the shell did not store the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStoreFailure {
    /// What was entered is not a key: no file was written.
    Invalid,
    /// There is no configuration directory in which to put the key file.
    NoPlace,
    /// The runner or the place to store the key is not available.
    Unavailable,
    /// The user dismissed the work or the shell honoured `Cancel`.
    Cancelled,
    /// Everything else; the text names what the shell said.
    Failed(String),
}
