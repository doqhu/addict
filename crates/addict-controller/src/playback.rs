//! One pronunciation request, from fetch to playback and optional credit.

use std::path::PathBuf;

use addict_core::failure::playback::PlaybackFailure;
use addict_engine::{
    recording::{self, CommonsFile, Credit},
    view::{Lookup, playback::playback_text, recording_credit_text},
    work::{Job, Outcome},
};

use crate::{
    ArticleId, Effect, Message, Model, RequestId,
    deadlines::{AUDIO_LIMIT, CREDIT_LIMIT},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackView {
    pub id: RequestId,
    pub url: String,
    pub state: PlaybackState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackState {
    Fetching,
    Playing,
    Ended { credit: Option<String> },
    Failed(String),
}

pub(crate) struct Playback {
    pub view: PlaybackView,
    shown_generation: u64,
    job: Option<Job>,
}

impl Model {
    pub(crate) fn play_pressed(
        &mut self,
        article: ArticleId,
        url: String,
        effects: &mut Vec<Effect>,
    ) -> bool {
        if article != self.article_id() || !matches!(self.shown.lookup(), Some(Lookup::Article(_)))
        {
            return false;
        }
        self.cancel_playback(effects);
        let id = self.next_request();
        let mut playback = Playback {
            view: PlaybackView {
                id,
                url,
                state: PlaybackState::Fetching,
            },
            shown_generation: self.shown.generation(),
            job: None,
        };
        match recording::local_audio(&playback.view.url) {
            Ok(Some(path)) => playback.play(path, effects),
            Ok(None) => {
                if let Some(cache) = self.downloads.dirs.cache.clone() {
                    let url = playback.view.url.clone();
                    let send = self.send.clone();
                    playback.job = Some(self.work.run(
                        AUDIO_LIMIT,
                        async move { recording::cached_audio(&cache, &url).await },
                        move |outcome| {
                            let _ = send.send(Message::AudioReady(id, outcome));
                        },
                    ));
                } else {
                    eprintln!("addict: could not cache recording: no cache directory");
                    playback.fail(PlaybackFailure::Failed("missing cache directory".into()));
                }
            }
            Err(failure) => playback.fail(failure),
        }
        self.playback = Some(playback);
        true
    }

    pub(crate) fn cancel_playback(&mut self, effects: &mut Vec<Effect>) {
        if let Some(playback) = self.playback.take() {
            effects.push(Effect::Cancel(playback.view.id));
        }
    }

    pub(crate) fn cancel_stale_playback(&mut self, effects: &mut Vec<Effect>) {
        if self.playback.as_ref().is_some_and(|current| {
            current.shown_generation != self.shown.generation()
                || !matches!(self.shown.lookup(), Some(Lookup::Article(_)))
        }) {
            self.cancel_playback(effects);
        }
    }

    pub(crate) fn audio_ready(
        &mut self,
        id: RequestId,
        outcome: Outcome<Result<PathBuf, PlaybackFailure>>,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let Some(playback) = self.playback.as_mut().filter(|current| {
            current.view.id == id && current.view.state == PlaybackState::Fetching
        }) else {
            return false;
        };
        playback.job = None;
        let result = match outcome {
            Outcome::Finished(result) => result,
            Outcome::TimedOut => Err(PlaybackFailure::Network),
            Outcome::Panicked => Err(PlaybackFailure::Failed("recording fetch panicked".into())),
            Outcome::Cancelled => Err(PlaybackFailure::Cancelled),
        };
        match result {
            Ok(path) => playback.play(path, effects),
            Err(PlaybackFailure::Cancelled) => self.playback = None,
            Err(failure) => playback.fail(failure),
        }
        true
    }

    pub(crate) fn played(&mut self, id: RequestId, result: Result<(), PlaybackFailure>) -> bool {
        let Some(playback) = self.playback.as_mut().filter(|current| {
            current.view.id == id && current.view.state == PlaybackState::Playing
        }) else {
            return false;
        };
        match result {
            Err(PlaybackFailure::Cancelled) => self.playback = None,
            Err(failure) => playback.fail(failure),
            Ok(()) => {
                playback.view.state = PlaybackState::Ended { credit: None };
                if let Some(file) = CommonsFile::from_recording_url(&playback.view.url) {
                    let send = self.send.clone();
                    #[cfg(test)]
                    let endpoint = self.credit_endpoint.clone();
                    playback.job = Some(self.work.run(
                        CREDIT_LIMIT,
                        async move {
                            #[cfg(test)]
                            if let Some(endpoint) = endpoint {
                                return file.fetch_credit_from(&endpoint).await;
                            }
                            file.fetch_credit().await
                        },
                        move |outcome| {
                            let _ = send.send(Message::CreditReady(id, outcome));
                        },
                    ));
                }
            }
        }
        true
    }

    pub(crate) fn credit_ready(&mut self, id: RequestId, outcome: Outcome<Credit>) -> bool {
        let Some(playback) = self
            .playback
            .as_mut()
            .filter(|current| current.view.id == id)
        else {
            return false;
        };
        if playback.job.is_none() || playback.view.state != (PlaybackState::Ended { credit: None })
        {
            return false;
        }
        playback.job = None;
        let Outcome::Finished(credit) = outcome else {
            return false;
        };
        let Some(credit) = recording_credit_text(&credit) else {
            return false;
        };
        playback.view.state = PlaybackState::Ended {
            credit: Some(credit),
        };
        true
    }
}

impl Playback {
    fn play(&mut self, path: PathBuf, effects: &mut Vec<Effect>) {
        self.view.state = PlaybackState::Playing;
        effects.push(Effect::Play {
            id: self.view.id,
            path,
        });
    }

    fn fail(&mut self, failure: PlaybackFailure) {
        if let Some(text) = playback_text(&failure) {
            self.view.state = PlaybackState::Failed(text.to_owned());
        }
    }
}
