//! The failure sets of the effects, one module per family (AD-7). The
//! platform adapters return them and `addict-engine::view` words them.

pub mod capture;
pub mod file_choice;
pub mod key_store;
pub mod login_item;
pub mod recognize;
pub mod selection;

/// A result that ended in the shell or was finished by the controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended<F> {
    /// The shell reported a family-specific failure.
    Shell(F),
    /// No result arrived by the controller's deadline.
    DeadlinePassed,
    /// The shell could not start or complete the effect.
    Unfinished,
}
