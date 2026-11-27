//! Source messages for the read of the selection and the clipboard.

use addict_core::failure::{Ended, selection::SelectionFailure};

use super::NOTHING_SELECTED_TEXT;

/// The source line when the read of the selection and the clipboard gave no
/// text. Today's Linux never tells its failures apart, and no 0.5.0 text
/// exists for `NoPermission` or `WrongApp`, so every one of them, and both
/// outcomes the controller originates, says that nothing is selected or
/// copied, with the manual path (AD-7).
pub fn selection_failure_text(failure: &Ended<SelectionFailure>) -> String {
    match failure {
        Ended::Shell(SelectionFailure::NoPermission)
        | Ended::Shell(SelectionFailure::WrongApp)
        | Ended::Shell(SelectionFailure::Unavailable)
        | Ended::Shell(SelectionFailure::Cancelled)
        | Ended::DeadlinePassed
        | Ended::Unfinished => NOTHING_SELECTED_TEXT.to_owned(),
    }
}
