//! Dictionary availability, the first-run offer and the install queue.

mod first_run;
pub use first_run::path as first_run_choice_path;

use std::{path::PathBuf, sync::Mutex, time::Duration};

use addict_core::dirs::Dirs;
use addict_dictionary::release::{Manifest, Published};
use addict_engine::{
    dictionaries::{Articles, Indexes},
    download::{self, DownloadError, Downloader},
    view::{file_choice::ImportState, first_run as view},
    work::{Job, Outcome},
};
use futures_channel::oneshot;

use crate::{Language, Message, Model, RequestId, deadlines::*};

/// The identity of one offer, retained by the shell's dialog callbacks.
pub type OfferId = RequestId;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DownloadsView {
    pub indexes: Option<Indexes>,
    pub offer: Option<OfferView>,
    pub settings_busy: bool,
    pub settings_line: String,
    pub import: ImportState,
    pub present: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferView {
    pub id: OfferId,
    pub recovery: bool,
    pub my_language: Language,
    pub listed: Vec<Published>,
    pub phase: OfferPhase,
    finished_queue: Option<view::Queue>,
}

impl OfferView {
    /// The per-source results remain available after the aggregate result.
    pub fn queue(&self) -> Option<&view::Queue> {
        match &self.phase {
            OfferPhase::Listing => None,
            OfferPhase::Installing(queue) => Some(queue),
            OfferPhase::Ended { .. } => self.finished_queue.as_ref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferPhase {
    Listing,
    Installing(view::Queue),
    Ended {
        heading: &'static str,
        text: String,
        complete: bool,
    },
}

enum FirstRun {
    Unasked,
    Asking(crate::Active),
    Prepared(RequestId, Manifest),
    Recovering(crate::Active),
    Listing { manifest: Manifest, presented: bool },
    Installing(Option<Install>),
    Ended,
    Done,
}

struct Install {
    id: RequestId,
    name: String,
    _job: Job,
}

/// A generation changes before a committed file's readiness check. A read
/// begun before that point cannot put an older inventory back in the view.
#[derive(Default)]
pub(crate) struct IndexCache {
    pub generation: u64,
    pub known: Option<Indexes>,
    known_generation: u64,
}

impl IndexCache {
    fn current(&self) -> Option<Indexes> {
        self.known
            .filter(|_| self.known_generation == self.generation)
    }

    fn update(&mut self, generation: u64, indexes: Indexes) -> bool {
        if generation != self.generation {
            return false;
        }
        self.known = Some(indexes);
        self.known_generation = generation;
        true
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub check: Duration,
    pub manifest: Duration,
    pub download: Duration,
    pub import: Duration,
    pub choice: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            check: LOOKUP_LIMIT,
            manifest: MANIFEST_LIMIT,
            download: DOWNLOAD_LIMIT,
            import: IMPORT_LIMIT,
            choice: FIRST_RUN_FILE_LIMIT,
        }
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct ReadGates {
    pub startup: Option<std::sync::Arc<crate::settings::ReadGate>>,
    pub refresh: Option<std::sync::Arc<crate::settings::ReadGate>>,
    pub offer: Option<std::sync::Arc<crate::settings::ReadGate>>,
    pub checked: Option<std::sync::mpsc::Sender<()>>,
}

pub(crate) struct Downloads {
    pub view: DownloadsView,
    pub indexes: IndexCache,
    pub limits: Limits,
    pub dirs: Dirs,
    release: String,
    first_run: FirstRun,
    check: Option<(RequestId, Job)>,
    refresh: Option<crate::Active>,
    awaiting_refresh: Vec<Refresh>,
    #[cfg(test)]
    pub read_gates: ReadGates,
    decline: Option<(RequestId, Job)>,
    pending_language_choice: bool,
}

impl Downloads {
    pub fn new(dirs: Dirs, release: String, limits: Limits) -> Self {
        Self {
            view: DownloadsView::default(),
            indexes: IndexCache::default(),
            limits,
            dirs,
            release,
            first_run: FirstRun::Unasked,
            check: None,
            refresh: None,
            awaiting_refresh: Vec::new(),
            #[cfg(test)]
            read_gates: ReadGates::default(),
            decline: None,
            pending_language_choice: false,
        }
    }

    fn settings_line(&mut self, busy: bool, text: impl Into<String>) {
        self.view.settings_busy = busy;
        self.view.settings_line = text.into();
    }

    fn done(&mut self) {
        self.first_run = FirstRun::Done;
        self.view.offer = None;
    }

    pub fn lookup_generation(&self) -> Option<u64> {
        self.refresh.is_none().then_some(self.indexes.generation)
    }
}

/// Finalization runs independently of the cancellable install. Import
/// completion and a successful install both wait until the inventory applies.
pub(crate) enum Refresh {
    Download(oneshot::Sender<Result<(), String>>),
    Import(RequestId, addict_dictionary::import::Imported),
}

pub(crate) enum Update {
    Checked(RequestId, u64, Outcome<Result<Indexes, String>>),
    Committed(Refresh),
    Refreshed(RequestId, u64, Outcome<Result<Indexes, String>>),
    Offered(RequestId, Outcome<Result<Option<Manifest>, DownloadError>>),
    RecoveryPrepared(RequestId, Outcome<Result<Manifest, String>>),
    Progress(RequestId, OfferId, u64),
    Installed(RequestId, OfferId, Outcome<Result<PathBuf, String>>),
    Declined(RequestId, Outcome<std::io::Result<()>>),
}

impl Model {
    pub(crate) fn start_index_check(&mut self) {
        if self.downloads.refresh.is_some() {
            return;
        }
        let id = self.next_request();
        let generation = self.downloads.indexes.generation;
        let articles = self.dictionaries.clone();
        let send = self.send.clone();
        #[cfg(test)]
        let gate = self.downloads.read_gates.startup.take();
        let job = self.work.run(
            self.downloads.limits.check,
            async move {
                #[cfg(test)]
                if let Some(gate) = gate {
                    gate.wait().await;
                }
                download::blocking(move || articles.indexes()).await
            },
            move |outcome| {
                let _ = send.send(Message::Downloads(Update::Checked(id, generation, outcome)));
            },
        );
        self.downloads.check = Some((id, job));
    }

    pub(crate) fn start_index_refresh(&mut self, refresh: Refresh) {
        self.downloads.check = None;
        self.downloads.indexes.generation += 1;
        self.downloads.awaiting_refresh.push(refresh);
        if self.downloads.refresh.is_none() {
            self.refresh_indexes();
        }
    }

    fn refresh_indexes(&mut self) {
        let id = self.next_request();
        let generation = self.downloads.indexes.generation;
        let articles = self.dictionaries.clone();
        let send = self.send.clone();
        #[cfg(test)]
        let gate = self.downloads.read_gates.refresh.clone();
        let job = self.work.run(
            self.downloads.limits.download,
            async move {
                #[cfg(test)]
                if let Some(gate) = gate {
                    gate.wait().await;
                }
                download::blocking(move || articles.indexes()).await
            },
            move |outcome| {
                let _ = send.send(Message::Downloads(Update::Refreshed(
                    id, generation, outcome,
                )));
            },
        );
        self.downloads.refresh = Some(crate::Active { id, _job: job });
    }

    pub(crate) fn observe_indexes(&mut self, generation: u64, indexes: Indexes) -> bool {
        if !self.downloads.indexes.update(generation, indexes) {
            return false;
        }
        let changed = self.downloads.view.indexes != Some(indexes);
        self.downloads.view.indexes = Some(indexes);
        self.relist_offer() || changed
    }

    fn index_refreshed(
        &mut self,
        id: RequestId,
        generation: u64,
        outcome: Outcome<Result<Indexes, String>>,
    ) -> bool {
        if self
            .downloads
            .refresh
            .as_ref()
            .is_none_or(|job| job.id != id)
        {
            return false;
        }
        self.downloads.refresh = None;
        // A second commit superseded this scan. All completions wait for
        // the scan that includes both committed files.
        if generation != self.downloads.indexes.generation {
            self.refresh_indexes();
            return false;
        }
        let (result, mut changed) = match outcome {
            Outcome::Finished(Ok(indexes)) => (Ok(()), self.observe_indexes(generation, indexes)),
            Outcome::Finished(Err(error)) => (Err(error), false),
            Outcome::TimedOut => (
                Err("The dictionary check did not finish in time.".to_owned()),
                false,
            ),
            Outcome::Cancelled | Outcome::Panicked => {
                (Err("The dictionary refresh stopped.".to_owned()), false)
            }
        };
        for refresh in std::mem::take(&mut self.downloads.awaiting_refresh) {
            match refresh {
                Refresh::Download(ready) => {
                    let _ = ready.send(result.clone());
                }
                Refresh::Import(id, imported) => {
                    changed |= self.import_refreshed(id, imported, result.clone());
                }
            }
        }
        changed
    }

    pub(crate) fn download_language_chosen(&mut self) {
        self.downloads.pending_language_choice = true;
    }

    /// Evaluate startup only with a settled index check and saved language.
    pub(crate) fn finish_download_update(&mut self) -> bool {
        if !self.settings.languages_read {
            return false;
        }
        if matches!(self.downloads.first_run, FirstRun::Unasked)
            && (self.downloads.check.is_some() || self.downloads.refresh.is_some())
        {
            return false;
        }
        let chosen = std::mem::take(&mut self.downloads.pending_language_choice);
        let relisted = chosen && self.relist_offer();
        let mut changed = self.first_indexes() || relisted;
        if chosen {
            changed |= self.offer_missing_for_my_language();
        }
        changed
    }

    fn first_indexes(&mut self) -> bool {
        if self.downloads.check.is_some() {
            return false;
        }
        let Some(indexes) = self.downloads.indexes.current() else {
            return false;
        };
        let my = self.settings.my_language;
        let due = first_run::should_offer(&indexes, my, false);
        match self.downloads.first_run {
            FirstRun::Unasked if due => {
                let id = self.next_request();
                let send = self.send.clone();
                let downloader = Downloader::with_base_url(self.downloads.release.clone());
                let choice = first_run::path(self.downloads.dirs.config.as_deref());
                #[cfg(test)]
                let gate = self.downloads.read_gates.offer.clone();
                let job = self.work.run(
                    self.downloads.limits.manifest,
                    async move {
                        let chosen = first_run::read(choice.as_deref());
                        let result =
                            first_run::offered_manifest(indexes, my, chosen, &downloader).await;
                        #[cfg(test)]
                        if let Some(gate) = gate {
                            gate.wait().await;
                        }
                        result
                    },
                    move |outcome| {
                        let _ = send.send(Message::Downloads(Update::Offered(id, outcome)));
                    },
                );
                self.downloads.first_run = FirstRun::Asking(crate::Active { id, _job: job });
            }
            FirstRun::Unasked => self.downloads.done(),
            FirstRun::Asking(..)
            | FirstRun::Prepared(..)
            | FirstRun::Listing {
                presented: false, ..
            } if !due => {
                self.downloads.done();
                return true;
            }
            FirstRun::Prepared(..) => {
                let FirstRun::Prepared(id, manifest) =
                    std::mem::replace(&mut self.downloads.first_run, FirstRun::Done)
                else {
                    unreachable!()
                };
                self.show_offer(id, manifest, false);
                return true;
            }
            _ => {}
        }
        false
    }

    fn first_run_offered(
        &mut self,
        id: RequestId,
        outcome: Outcome<Result<Option<Manifest>, DownloadError>>,
    ) -> bool {
        if !matches!(self.downloads.first_run, FirstRun::Asking(ref active) if active.id == id) {
            return false;
        }
        self.downloads.first_run = FirstRun::Done;
        match outcome {
            Outcome::Finished(Ok(Some(manifest))) => {
                self.downloads.first_run = FirstRun::Prepared(id, manifest);
            }
            Outcome::Finished(Ok(None)) | Outcome::Cancelled => {}
            Outcome::Finished(Err(error)) => {
                eprintln!("addict: no dictionary download is offered: {error}");
            }
            Outcome::TimedOut => {
                eprintln!(
                    "addict: no dictionary download is offered: the manifest did not come in time"
                );
            }
            Outcome::Panicked => eprintln!("addict: no dictionary download is offered"),
        }
        false
    }

    fn listed(&self, manifest: &Manifest, recovery: bool) -> Vec<Published> {
        first_run::offered(&manifest.indexes, self.settings.my_language, |name| {
            !recovery
                && self
                    .downloads
                    .indexes
                    .known
                    .is_some_and(|indexes| first_run::is_present(&indexes, name))
        })
    }

    fn show_offer(&mut self, id: OfferId, manifest: Manifest, recovery: bool) {
        self.downloads.view.offer = Some(OfferView {
            id,
            recovery,
            my_language: self.settings.my_language,
            listed: self.listed(&manifest, recovery),
            phase: OfferPhase::Listing,
            finished_queue: None,
        });
        self.downloads.view.present += 1;
        self.downloads.first_run = FirstRun::Listing {
            manifest,
            presented: recovery,
        };
        if recovery {
            self.downloads.settings_line(false, view::DIALOG_OPEN);
        }
    }

    fn relist_offer(&mut self) -> bool {
        let FirstRun::Listing { manifest, .. } = &self.downloads.first_run else {
            return false;
        };
        let recovery = self.downloads.view.offer.as_ref().unwrap().recovery;
        let listed = self.listed(manifest, recovery);
        let my = self.settings.my_language;
        if recovery && listed.is_empty() {
            self.downloads.done();
            self.downloads.settings_line(false, view::nothing_text(my));
            return true;
        }
        let offer = self.downloads.view.offer.as_mut().unwrap();
        let changed = offer.my_language != my || offer.listed != listed;
        offer.my_language = my;
        offer.listed = listed;
        changed
    }

    fn offer_missing_for_my_language(&mut self) -> bool {
        if matches!(self.downloads.first_run, FirstRun::Listing { .. })
            && self
                .downloads
                .view
                .offer
                .as_ref()
                .is_some_and(|offer| offer.recovery)
        {
            self.downloads.view.present += 1;
            return true;
        }
        matches!(self.downloads.first_run, FirstRun::Done | FirstRun::Ended)
            && self.downloads.indexes.known.is_some_and(|indexes| {
                !first_run::missing(&indexes, self.settings.my_language).is_empty()
            })
            && self.settings_download()
    }

    pub(crate) fn settings_download(&mut self) -> bool {
        match &self.downloads.first_run {
            FirstRun::Recovering(..) => return false,
            FirstRun::Installing(_) => {
                self.downloads.view.present += 1;
                return true;
            }
            FirstRun::Listing { .. }
                if self
                    .downloads
                    .view
                    .offer
                    .as_ref()
                    .is_some_and(|o| o.recovery) =>
            {
                self.downloads.view.present += 1;
                return true;
            }
            _ => {}
        }
        let Some(dir) = self.downloads.dirs.data.clone() else {
            self.downloads
                .settings_line(false, view::recovery_failed_text(view::NO_PLACE));
            return true;
        };
        let id = self.next_request();
        let send = self.send.clone();
        let downloader = Downloader::with_base_url(self.downloads.release.clone());
        let manifest_limit = self.downloads.limits.manifest;
        let job = self.work.run(
            self.downloads.limits.download,
            async move {
                download::recovery_manifest(&downloader, dir, manifest_limit)
                    .await
                    .map_err(|error| error.to_string())
            },
            move |outcome| {
                let _ = send.send(Message::Downloads(Update::RecoveryPrepared(id, outcome)));
            },
        );
        self.downloads.view.offer = None;
        self.downloads.first_run = FirstRun::Recovering(crate::Active { id, _job: job });
        self.downloads.settings_line(true, view::FETCHING);
        true
    }

    fn recovery_prepared(
        &mut self,
        id: RequestId,
        outcome: Outcome<Result<Manifest, String>>,
    ) -> bool {
        if !matches!(self.downloads.first_run, FirstRun::Recovering(ref active) if active.id == id)
        {
            return false;
        }
        self.downloads.done();
        let line = match outcome {
            Outcome::Finished(Ok(manifest)) if !self.listed(&manifest, true).is_empty() => {
                self.show_offer(id, manifest, true);
                return true;
            }
            Outcome::Finished(Ok(_)) => view::nothing_text(self.settings.my_language),
            Outcome::Finished(Err(error)) => view::recovery_failed_text(&error),
            Outcome::TimedOut => view::recovery_failed_text(view::RECOVERY_TIMEOUT),
            Outcome::Cancelled | Outcome::Panicked => view::recovery_failed_text(view::STOPPED),
        };
        self.downloads.settings_line(false, line);
        true
    }

    fn has_offer(&self, id: OfferId) -> bool {
        self.downloads
            .view
            .offer
            .as_ref()
            .is_some_and(|offer| offer.id == id)
    }

    pub(crate) fn offer_presented(&mut self, id: OfferId) {
        if self.has_offer(id)
            && let FirstRun::Listing { presented, .. } = &mut self.downloads.first_run
        {
            *presented = true;
        }
    }

    pub(crate) fn offer_download(&mut self, id: OfferId) -> bool {
        if !self.has_offer(id) || !matches!(self.downloads.first_run, FirstRun::Listing { .. }) {
            return false;
        }
        let offer = self.downloads.view.offer.as_mut().unwrap();
        let mut queue = view::Queue::new(offer.listed.clone());
        let failure = self.downloads.dirs.data.is_none().then(|| {
            eprintln!("addict: the dictionary download failed: {}", view::NO_PLACE);
            queue.cancel_all();
            if offer.recovery {
                view::recovery_failed_text(view::NO_PLACE)
            } else {
                view::failed_text(view::NO_PLACE)
            }
        });
        offer.phase = OfferPhase::Installing(queue);
        self.downloads.first_run = FirstRun::Installing(None);
        if failure.is_some() {
            self.end_offer(failure);
        } else {
            self.install_next();
        }
        true
    }

    fn install_next(&mut self) {
        if !matches!(self.downloads.first_run, FirstRun::Installing(None)) {
            return;
        }
        let offer = self.downloads.view.offer.as_mut().unwrap();
        let OfferPhase::Installing(queue) = &mut offer.phase else {
            unreachable!()
        };
        let (offer_id, recovery) = (offer.id, offer.recovery);
        let Some(published) = queue.next() else {
            if queue.is_done() {
                self.end_offer(None);
            }
            return;
        };
        let id = self.next_request();
        let name = published.name.clone();
        let dir = self
            .downloads
            .dirs
            .data
            .clone()
            .expect("download directory");
        let send = self.send.clone();
        let progress = send.clone();
        let committed = send.clone();
        let (ready, refreshed) = oneshot::channel();
        let ready = Mutex::new(Some(ready));
        let downloader =
            Downloader::with_base_url(self.downloads.release.clone()).on_commit(move || {
                if let Some(ready) = ready.lock().expect("commit callback lock").take() {
                    let _ = committed.send(Message::Downloads(Update::Committed(
                        Refresh::Download(ready),
                    )));
                }
            });
        let downloader = if recovery {
            downloader.recovering()
        } else {
            downloader
        };
        let job = self.work.run(
            self.downloads.limits.download,
            async move {
                let path = downloader
                    .install(&published, &dir, move |received| {
                        let _ = progress
                            .send(Message::Downloads(Update::Progress(id, offer_id, received)));
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                refreshed
                    .await
                    .map_err(|_| "The dictionary refresh stopped.".to_owned())??;
                Ok(path)
            },
            move |outcome| {
                let _ = send.send(Message::Downloads(Update::Installed(id, offer_id, outcome)));
            },
        );
        self.downloads.first_run = FirstRun::Installing(Some(Install {
            id,
            name,
            _job: job,
        }));
    }

    fn end_offer(&mut self, failure: Option<String>) {
        let offer = self.downloads.view.offer.as_mut().unwrap();
        let OfferPhase::Installing(queue) =
            std::mem::replace(&mut offer.phase, OfferPhase::Listing)
        else {
            unreachable!()
        };
        let (heading, text, complete) = if let Some(text) = failure {
            (view::FAILED_HEADING, text, false)
        } else {
            let text = if offer.recovery {
                view::recovery_summary_text(&queue)
            } else {
                view::summary_text(&queue)
            };
            (view::ended_heading(&queue), text, view::is_complete(&queue))
        };
        offer.phase = OfferPhase::Ended {
            heading,
            text: text.clone(),
            complete,
        };
        offer.finished_queue = Some(queue);
        self.downloads.first_run = FirstRun::Ended;
        self.downloads.settings_line(false, text);
    }

    fn install_progress(&mut self, id: RequestId, offer_id: OfferId, received: u64) -> bool {
        if !self.has_offer(offer_id) {
            return false;
        }
        let FirstRun::Installing(Some(install)) = &self.downloads.first_run else {
            return false;
        };
        if install.id != id {
            return false;
        }
        let OfferPhase::Installing(queue) = &mut self.downloads.view.offer.as_mut().unwrap().phase
        else {
            unreachable!()
        };
        queue.progress(&install.name, received);
        true
    }

    pub(crate) fn offer_cancel(&mut self, id: OfferId, name: Option<String>) -> bool {
        if !self.has_offer(id) {
            return false;
        }
        let FirstRun::Installing(running) = &mut self.downloads.first_run else {
            return false;
        };
        let OfferPhase::Installing(queue) = &mut self.downloads.view.offer.as_mut().unwrap().phase
        else {
            unreachable!()
        };
        let was_running = match name {
            Some(name) => queue.cancel(&name),
            None => queue.cancel_all(),
        };
        if was_running {
            *running = None;
        }
        self.install_next();
        true
    }

    fn installed(
        &mut self,
        id: RequestId,
        offer_id: OfferId,
        outcome: Outcome<Result<PathBuf, String>>,
    ) -> bool {
        if !self.has_offer(offer_id) {
            return false;
        }
        let FirstRun::Installing(running) = &mut self.downloads.first_run else {
            return false;
        };
        if running.as_ref().is_none_or(|install| install.id != id) {
            return false;
        }
        let name = running.take().unwrap().name;
        let OfferPhase::Installing(queue) = &mut self.downloads.view.offer.as_mut().unwrap().phase
        else {
            unreachable!()
        };
        let result = match outcome {
            Outcome::Finished(result) => result.map(drop),
            Outcome::TimedOut => Err(view::late_text(self.downloads.limits.download)),
            Outcome::Cancelled | Outcome::Panicked => Err(view::STOPPED.to_owned()),
        };
        if let Err(error) = &result {
            eprintln!(
                "addict: the dictionary download failed: {}",
                view::failure_text(&name, error)
            );
        }
        queue.finished(&name, result);
        self.install_next();
        true
    }

    pub(crate) fn offer_not_now(&mut self, id: OfferId) -> bool {
        if !self.has_offer(id) || !matches!(self.downloads.first_run, FirstRun::Listing { .. }) {
            return false;
        }
        let recovery = self.downloads.view.offer.as_ref().unwrap().recovery;
        self.downloads.done();
        if recovery {
            self.downloads.settings_line(false, "");
            return true;
        }
        let id = self.next_request();
        let path = first_run::path(self.downloads.dirs.config.as_deref());
        let send = self.send.clone();
        let job = self.work.run(
            self.downloads.limits.choice,
            async move { first_run::write(path.as_deref()) },
            move |outcome| {
                let _ = send.send(Message::Downloads(Update::Declined(id, outcome)));
            },
        );
        self.downloads.decline = Some((id, job));
        true
    }

    pub(crate) fn update_downloads(&mut self, update: Update) -> bool {
        match update {
            Update::Checked(id, generation, outcome) => {
                if !self
                    .downloads
                    .check
                    .as_ref()
                    .is_some_and(|(current, _)| *current == id)
                {
                    return false;
                }
                self.downloads.check = None;
                #[cfg(test)]
                if let Some(checked) = &self.downloads.read_gates.checked {
                    let _ = checked.send(());
                }
                if let Outcome::Finished(Ok(indexes)) = outcome {
                    return self.observe_indexes(generation, indexes);
                }
                false
            }
            Update::Committed(refresh) => {
                self.start_index_refresh(refresh);
                false
            }
            Update::Refreshed(id, generation, outcome) => {
                self.index_refreshed(id, generation, outcome)
            }
            Update::Offered(id, outcome) => self.first_run_offered(id, outcome),
            Update::RecoveryPrepared(id, outcome) => self.recovery_prepared(id, outcome),
            Update::Progress(id, offer, received) => self.install_progress(id, offer, received),
            Update::Installed(id, offer, outcome) => self.installed(id, offer, outcome),
            Update::Declined(id, outcome) => {
                if self
                    .downloads
                    .decline
                    .as_ref()
                    .is_some_and(|(current, _)| *current == id)
                {
                    self.downloads.decline = None;
                    match outcome {
                        Outcome::Finished(Ok(())) | Outcome::Cancelled => {}
                        Outcome::Finished(Err(error)) => {
                            eprintln!(
                                "addict: the answer to the dictionary download was not kept: {error}"
                            );
                        }
                        Outcome::TimedOut | Outcome::Panicked => {
                            eprintln!("addict: the answer to the dictionary download was not kept");
                        }
                    }
                }
                false
            }
        }
    }
}
