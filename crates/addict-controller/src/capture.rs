//! Controller transitions for the area picker and text recognition.

use std::{any::Any, collections::HashMap, fmt, future, sync::Arc, time::Duration};

use addict_core::{
    failure::{Ended, capture::CaptureFailure, recognize::OcrFailure},
    input::Source,
    sentence,
    translation::Source as Language,
};
use addict_engine::{
    capture::Timing,
    evidence::Read,
    view::{capture_failure_text, recognize_failure_text},
    work::{Job, Outcome},
};

use crate::{
    Effect, Message, Model, RequestId,
    deadlines::{CAPTURE_LIMIT, OCR_LIMIT},
};

/// A shell-owned image. An adapter can recover its own type without exposing
/// it to the controller. The last reference releases the image.
#[derive(Clone)]
pub struct ImageRef(Arc<dyn Any + Send + Sync>);

impl ImageRef {
    pub fn new(image: impl Any + Send + Sync) -> Self {
        Self(Arc::new(image))
    }

    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl fmt::Debug for ImageRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ImageRef")
    }
}

impl PartialEq for ImageRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

pub(crate) struct Capture {
    picker_limit: Duration,
    recognition_limit: Duration,
    active: Option<Active>,
    pub(crate) source_failure: Option<String>,
    /// A timeout is shown immediately. Its evidence waits until the shell
    /// acknowledges cancellation and its files and children have been reaped.
    timed_out: HashMap<RequestId, (Read, Option<u64>)>,
}

struct Active {
    id: RequestId,
    language: Language,
    stage: Stage,
    _deadline: Job,
}

enum Stage {
    Picker,
    Recognition { capture_ms: Option<u64> },
}

impl Default for Capture {
    fn default() -> Self {
        Self::with_limits(CAPTURE_LIMIT, OCR_LIMIT)
    }
}

impl Capture {
    pub(crate) fn with_limits(picker_limit: Duration, recognition_limit: Duration) -> Self {
        Self {
            picker_limit,
            recognition_limit,
            active: None,
            source_failure: None,
            timed_out: HashMap::new(),
        }
    }

    pub(crate) fn picker_open(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| matches!(active.stage, Stage::Picker))
    }

    pub(crate) fn recognizing(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| matches!(active.stage, Stage::Recognition { .. }))
    }

    fn take(&mut self, id: RequestId, picker: bool) -> Option<Active> {
        let active = self.active.as_ref()?;
        if active.id == id && matches!(active.stage, Stage::Picker) == picker {
            self.active.take()
        } else {
            None
        }
    }
}

impl Model {
    fn capture_timer(&self, id: RequestId, limit: Duration) -> Job {
        let send = self.send.clone();
        self.work
            .run(limit, future::pending::<()>(), move |outcome| {
                if outcome == Outcome::TimedOut {
                    let _ = send.send(Message::CaptureDeadline(id));
                }
            })
    }

    pub(crate) fn capture_pressed(
        &mut self,
        activation_token: Option<String>,
        effects: &mut Vec<Effect>,
    ) -> bool {
        if self.capture.picker_open() {
            return false;
        }
        self.cancel_capture(effects);
        self.capture.source_failure = None;
        self.manual.set_reading(None);
        let id = self.next_request();
        self.capture.active = Some(Active {
            id,
            language: self.settings.reading(),
            stage: Stage::Picker,
            _deadline: self.capture_timer(id, self.capture.picker_limit),
        });
        effects.push(Effect::CaptureArea {
            id,
            read_sequence: self.read_sequence,
            activation_token,
        });
        true
    }

    pub(crate) fn picker_started(&mut self, id: RequestId) {
        if let Some(active) = &mut self.capture.active
            && active.id == id
            && matches!(active.stage, Stage::Picker)
        {
            active.language = self.settings.reading();
        }
    }

    pub(crate) fn cancel_capture(&mut self, effects: &mut Vec<Effect>) {
        if let Some(active) = self.capture.active.take() {
            effects.push(Effect::Cancel(active.id));
            if matches!(active.stage, Stage::Recognition { .. }) {
                self.manual.set_reading(None);
            }
        }
    }

    pub(crate) fn clear_capture_source(&mut self, effects: &mut Vec<Effect>) -> bool {
        let had_failure = self.capture.source_failure.take().is_some();
        let recognizing = self.capture.recognizing();
        if recognizing {
            self.cancel_capture(effects);
        }
        had_failure || recognizing
    }

    pub(crate) fn area_captured(
        &mut self,
        id: RequestId,
        result: Result<ImageRef, Ended<CaptureFailure>>,
        capture_ms: Option<u64>,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let Some(active) = self.capture.take(id, true) else {
            drop(result);
            self.record_timeout(
                id,
                Timing {
                    capture_ms,
                    ..Timing::default()
                },
                effects,
            );
            return false;
        };
        match result {
            Ok(image) => {
                self.manual.set_reading(Some(Source::Ocr));
                self.present_capture(false, effects);
                let id = self.next_request();
                let language = active.language;
                self.capture.active = Some(Active {
                    id,
                    language,
                    stage: Stage::Recognition { capture_ms },
                    _deadline: self.capture_timer(id, self.capture.recognition_limit),
                });
                effects.push(Effect::Recognize {
                    id,
                    image,
                    language,
                    read_sequence: self.read_sequence,
                });
            }
            Err(failure) => {
                eprintln!("addict: no text from the capture: {failure:?}");
                self.capture.source_failure = Some(capture_failure_text(&failure).into_owned());
                self.present_capture(true, effects);
                self.record_capture(
                    Read::capture_failed(&failure),
                    Timing {
                        capture_ms,
                        ..Timing::default()
                    },
                    effects,
                );
            }
        }
        true
    }

    pub(crate) fn text_recognized(
        &mut self,
        id: RequestId,
        result: Result<String, Ended<OcrFailure>>,
        mut timing: Timing,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let Some(active) = self.capture.take(id, false) else {
            self.record_timeout(id, timing, effects);
            return false;
        };
        let Stage::Recognition { capture_ms } = active.stage else {
            unreachable!("recognition stage");
        };
        timing.capture_ms = capture_ms;
        self.manual.set_reading(None);
        let result = result.and_then(|text| {
            sentence::recognized(&text)
                .filter(|line| !sentence::words(line).is_empty())
                .ok_or(Ended::Shell(OcrFailure::Empty))
        });
        let (read, retry) = match result {
            Ok(text) => {
                let edit = self.manual.recognized(text);
                self.set_field(self.manual.sentence().to_owned());
                self.ocr_field_revision = Some(self.field.revision);
                self.history.record(self.manual.sentence());
                self.text_replaced(edit);
                (Read::Recognized, false)
            }
            Err(failure) => {
                eprintln!("addict: no text from the capture: {failure:?}");
                self.capture.source_failure =
                    Some(recognize_failure_text(&failure, active.language).into_owned());
                (Read::recognize_failed(&failure), true)
            }
        };
        self.present_capture(retry, effects);
        self.record_capture(read, timing, effects);
        true
    }

    pub(crate) fn capture_deadline(&mut self, id: RequestId, effects: &mut Vec<Effect>) -> bool {
        let Some(active) = self
            .capture
            .active
            .as_ref()
            .filter(|active| active.id == id)
        else {
            return false;
        };
        let (read, capture_ms, message) = match active.stage {
            Stage::Picker => (
                Read::capture_failed(&Ended::DeadlinePassed),
                None,
                capture_failure_text(&Ended::DeadlinePassed),
            ),
            Stage::Recognition { capture_ms } => (
                Read::recognize_failed(&Ended::DeadlinePassed),
                capture_ms,
                recognize_failure_text(&Ended::DeadlinePassed, active.language),
            ),
        };
        self.cancel_capture(effects);
        self.capture.source_failure = Some(message.into_owned());
        self.present_capture(true, effects);
        if self.recording {
            self.capture.timed_out.insert(id, (read, capture_ms));
        }
        true
    }

    fn present_capture(&self, retry: bool, effects: &mut Vec<Effect>) {
        effects.push(Effect::PresentCapture {
            read_sequence: self.read_sequence,
            retry,
        });
    }

    fn record_timeout(&mut self, id: RequestId, mut timing: Timing, effects: &mut Vec<Effect>) {
        if let Some((read, capture_ms)) = self.capture.timed_out.remove(&id) {
            timing.capture_ms = timing.capture_ms.or(capture_ms);
            self.record_capture(read, timing, effects);
        }
    }

    fn record_capture(&self, read: Read, timing: Timing, effects: &mut Vec<Effect>) {
        if self.recording {
            effects.push(Effect::RecordCapture { read, timing });
        }
    }
}
