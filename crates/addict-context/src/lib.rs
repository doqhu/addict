//! EXP-1/2 probe: asks the accessibility bus what a named application
//! exposes and, when a word is selected, reads the word and its sentence.
//! Read-only: no setting is changed and nothing is written outside `--out`.

pub mod bus;
pub mod context;
pub mod record;
pub mod text;
pub mod tree;
