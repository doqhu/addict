//! Window lifecycle and the settings writes that must finish before exit.

use std::collections::HashMap;

use addict_engine::{view::fatal, work::Job};

use crate::{Effect, Model, RequestId, ViewState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FatalView {
    pub heading: &'static str,
    pub text: &'static str,
    pub restart: &'static str,
}

#[derive(Default)]
pub(crate) struct Lifecycle {
    panel_open: bool,
    quit: Quit,
    pub(crate) writes: HashMap<RequestId, Job>,
}

#[derive(Default, PartialEq, Eq)]
enum Quit {
    #[default]
    Running,
    Requested,
    Emitted,
}

impl Model {
    pub(crate) fn reopened(&mut self, effects: &mut Vec<Effect>) {
        self.lifecycle.panel_open = true;
        effects.push(Effect::ShowPanel {
            read_sequence: None,
        });
    }

    pub(crate) fn panel_closed(&mut self) {
        self.lifecycle.panel_open = false;
    }

    pub(crate) fn quit_requested(&mut self) {
        if self.lifecycle.quit == Quit::Running {
            self.lifecycle.quit = Quit::Requested;
        }
    }

    pub(crate) fn finish_quit(&mut self, effects: &mut Vec<Effect>) {
        if self.lifecycle.quit == Quit::Requested
            && self.lifecycle.writes.is_empty()
            && !self.settings.waiting_for_writes()
            && !self.downloads.waiting_for_choice()
        {
            self.lifecycle.quit = Quit::Emitted;
            effects.push(Effect::Quit);
        }
    }

    pub(crate) fn quit_emitted(&self) -> bool {
        self.lifecycle.quit == Quit::Emitted
    }

    pub(crate) fn fatal_view(&self) -> ViewState {
        ViewState {
            revision: self.revision + 1,
            fatal: Some(FatalView {
                heading: fatal::HEADING,
                text: fatal::TEXT,
                restart: fatal::RESTART,
            }),
            ..ViewState::default()
        }
    }
}
