//! The controller's manual import, from chooser request to refreshed indexes.

use std::path::PathBuf;

use addict_core::failure::file_choice::FileChoiceFailure;
use addict_dictionary::import::Imported;
use addict_engine::{
    download,
    view::{
        file_choice::{self, ImportState},
        first_run,
    },
    work::Outcome,
};

use crate::{Effect, Message, Model, RequestId, downloads::Refresh};

#[derive(Default)]
pub(crate) enum Import {
    #[default]
    Idle,
    Choosing(RequestId),
    Running(crate::Active),
    Refreshing(RequestId),
}

impl Model {
    pub(crate) fn import_pressed(&mut self, effects: &mut Vec<Effect>) -> bool {
        if self.downloads.view.import.busy() {
            return false;
        }
        let id = self.next_request();
        self.import = Import::Choosing(id);
        self.downloads.view.import = ImportState::Choosing;
        effects.push(Effect::ChooseFile { id });
        true
    }

    pub(crate) fn file_chosen(
        &mut self,
        id: RequestId,
        result: Result<PathBuf, FileChoiceFailure>,
    ) -> bool {
        if !matches!(self.import, Import::Choosing(current) if current == id) {
            return false;
        }
        self.import = Import::Idle;
        let path = match result {
            Ok(path) => path,
            Err(failure) => {
                self.downloads.view.import = file_choice::file_choice_text(&failure)
                    .map_or(ImportState::Idle, ImportState::Failed);
                return true;
            }
        };
        let Some(dir) = self
            .downloads
            .dirs
            .data
            .clone()
            .filter(|dir| dir.is_absolute())
        else {
            self.downloads.view.import = ImportState::Failed(file_choice::NO_DATA_DIR.to_owned());
            return true;
        };
        self.downloads.view.import = ImportState::Importing;
        let id = self.next_request();
        let send = self.send.clone();
        let job = self.work.run(
            self.downloads.limits.import,
            download::import_file(path, dir),
            move |outcome| {
                let _ = send.send(Message::Imported(id, outcome));
            },
        );
        self.import = Import::Running(crate::Active { id, _job: job });
        true
    }

    pub(crate) fn imported(
        &mut self,
        id: RequestId,
        outcome: Outcome<Result<Imported, String>>,
    ) -> bool {
        if !matches!(self.import, Import::Running(ref active) if active.id == id) {
            return false;
        }
        self.import = Import::Idle;
        let failure = match outcome {
            Outcome::Finished(Ok(imported)) => {
                self.import = Import::Refreshing(id);
                self.start_index_refresh(Refresh::Import(id, imported));
                return false;
            }
            Outcome::Finished(Err(error)) => error,
            Outcome::TimedOut => first_run::late_text(self.downloads.limits.import),
            Outcome::Cancelled | Outcome::Panicked => first_run::STOPPED.to_owned(),
        };
        self.downloads.view.import = ImportState::Failed(failure);
        true
    }

    pub(crate) fn import_refreshed(
        &mut self,
        id: RequestId,
        imported: Imported,
        result: Result<(), String>,
    ) -> bool {
        if !matches!(self.import, Import::Refreshing(current) if current == id) {
            return false;
        }
        self.import = Import::Idle;
        self.downloads.view.import = match result {
            Ok(()) => ImportState::Imported(imported),
            Err(error) => ImportState::Failed(file_choice::refresh_failed_text(&error)),
        };
        true
    }
}
