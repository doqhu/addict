//! Settings' manual import state and file chooser messages.

use addict_core::failure::file_choice::FileChoiceFailure;
use addict_dictionary::import::Imported;

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum ImportState {
    #[default]
    Idle,
    Choosing,
    Importing,
    Imported(Imported),
    Failed(String),
}

impl ImportState {
    pub fn busy(&self) -> bool {
        matches!(self, Self::Choosing | Self::Importing)
    }

    pub fn text(&self) -> String {
        match self {
            Self::Idle => String::new(),
            Self::Choosing => "Choose a downloaded dictionary file…".to_owned(),
            Self::Importing => "Importing dictionary…".to_owned(),
            Self::Imported(done) => format!(
                "Imported {}: {} headwords ({}). If this dictionary was already in use, restart Addict to use the new edition.",
                done.path.file_name().unwrap_or_default().to_string_lossy(),
                done.headwords,
                done.edition,
            ),
            Self::Failed(message) => message.clone(),
        }
    }
}

pub const NO_DATA_DIR: &str =
    "Cannot import a dictionary: no absolute data directory is available.";

pub fn refresh_failed_text(message: &str) -> String {
    format!("The file was imported, but dictionary readiness could not be refreshed: {message}")
}

pub fn file_choice_text(failure: &FileChoiceFailure) -> Option<String> {
    Some(match failure {
        FileChoiceFailure::Cancelled => return None,
        FileChoiceFailure::NotLocal => "Cannot import this file: the file chooser did not provide a local or document-portal path. Choose a downloaded file on this computer.".to_owned(),
        FileChoiceFailure::Unavailable => "Cannot choose a dictionary file: the file chooser is unavailable.".to_owned(),
        FileChoiceFailure::Failed(error) => format!("Cannot choose a dictionary file: {error}"),
    })
}
