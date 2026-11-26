//! Shared product state and its event loop. The first migrated flow is the
//! offline lookup of a typed word.

pub mod deadlines;

use std::{
    io,
    sync::{Arc, Weak, mpsc},
    thread::{self, JoinHandle},
};

use addict_core::{dirs::Dirs, translation::Source as Language};
use addict_deepl::key::KeySource;
use addict_engine::{
    dictionaries::{Dictionaries, Indexes, Looked},
    evidence::{self, Answered},
    manual,
    view::Lookup,
    work::{Job, Outcome, Work},
};

/// Platform facts supplied by the shell. Typed lookup has no platform branches.
pub struct Platform {
    pub name: String,
}

/// The existing key source may still be loading when the controller starts.
pub struct StartFacts {
    pub key_source: KeySource,
}

/// Identity of a controller request, issued in increasing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId(u64);

/// The word and the dictionary context at the time of the edit.
#[derive(Debug)]
pub struct TypedWord {
    /// The number of this lookup in the shell's `Manual` state.
    pub run: u64,
    pub word: String,
    pub reading: Language,
    pub target: Language,
    pub known_indexes: Option<Indexes>,
    pub index_generation: u64,
}

/// Facts sent to the controller by the shell.
pub enum Event {
    WordTyped(TypedWord),
    TypedLookupRetired { run: u64 },
}

/// Commands for the shell. Offline lookup uses only the Rust runner.
#[derive(Debug)]
pub enum Effect {}

/// The entire controller-owned view state at one revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewState {
    pub revision: u64,
    pub typed_lookup: Option<TypedLookup>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedLookup {
    pub id: RequestId,
    pub run: u64,
    pub index_generation: u64,
    pub state: LookupState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupState {
    Pending,
    Resolved(Box<Resolution>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub lookup: Lookup,
    pub answered: Option<Language>,
    pub indexes: Option<Indexes>,
    pub evidence: Option<Answered>,
    pub check_indexes: bool,
}

/// Receives snapshots on the controller thread. Renderers enqueue their work
/// for delivery on the shell's UI thread, in order.
pub trait Renderer: Send + 'static {
    fn render(&self, state: ViewState, effects: Vec<Effect>);
}

impl<F: Fn(ViewState, Vec<Effect>) + Send + 'static> Renderer for F {
    fn render(&self, state: ViewState, effects: Vec<Effect>) {
        self(state, effects);
    }
}

/// Enqueue-only access for an event producer. It does not own the loop.
#[derive(Clone)]
pub struct Dispatcher(mpsc::Sender<Message>);

impl Dispatcher {
    /// Returns whether the event was accepted by the queue.
    pub fn dispatch(&self, event: Event) -> bool {
        self.0.send(Message::Event(event)).is_ok()
    }
}

/// The owner of the controller loop and its one work runner.
pub struct Controller {
    dispatch: Dispatcher,
    thread: Option<JoinHandle<()>>,
    work: Weak<Work>,
    dictionaries: Arc<Dictionaries>,
}

impl Controller {
    /// Returns whether the event was accepted by the queue.
    pub fn dispatch(&self, event: Event) -> bool {
        self.dispatch.dispatch(event)
    }

    pub fn dispatcher(&self) -> Dispatcher {
        self.dispatch.clone()
    }

    /// Legacy jobs use this runner until their orchestration is migrated.
    pub fn work(&self) -> Weak<Work> {
        self.work.clone()
    }

    /// Both paths use the same open files and see index imports together.
    pub fn dictionaries(&self) -> Arc<Dictionaries> {
        self.dictionaries.clone()
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let _ = self.dispatch.0.send(Message::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Opens no dictionary files. The event loop and the Rust jobs each get
/// their own thread, and both end when the controller is dropped.
pub fn start(
    dirs: Dirs,
    platform: Platform,
    start_facts: StartFacts,
    renderer: impl Renderer,
) -> io::Result<Controller> {
    let work = Arc::new(Work::start()?);
    let worker = Arc::downgrade(&work);
    let dictionaries = Arc::new(Dictionaries::new(dirs.data.clone()));
    let (send, receive) = mpsc::channel();
    let model = Model {
        _dirs: dirs,
        _platform: platform,
        _start_facts: start_facts,
        work,
        dictionaries: dictionaries.clone(),
        send: send.clone(),
        request: 0,
        revision: 0,
        current: None,
    };
    let thread = thread::Builder::new()
        .name("addict-controller".to_owned())
        .spawn(move || model.serve(receive, renderer))?;
    Ok(Controller {
        dispatch: Dispatcher(send),
        thread: Some(thread),
        work: worker,
        dictionaries,
    })
}

enum Message {
    Event(Event),
    LookedUp(RequestId, Language, Box<Outcome<Looked>>),
    Stop,
}

struct Active {
    word: String,
    snapshot: TypedLookup,
    job: Option<Job>,
}

struct Model {
    _dirs: Dirs,
    _platform: Platform,
    _start_facts: StartFacts,
    work: Arc<Work>,
    dictionaries: Arc<Dictionaries>,
    send: mpsc::Sender<Message>,
    request: u64,
    revision: u64,
    current: Option<Active>,
}

impl Model {
    fn serve(mut self, receive: mpsc::Receiver<Message>, renderer: impl Renderer) {
        self.render(&renderer);
        while let Ok(message) = receive.recv() {
            let changed = match message {
                Message::Event(Event::WordTyped(word)) => {
                    self.look_up(word);
                    true
                }
                Message::Event(Event::TypedLookupRetired { run }) => self.retire(run),
                Message::LookedUp(id, language, outcome) => self.looked_up(id, language, *outcome),
                Message::Stop => break,
            };
            if changed {
                self.render(&renderer);
            }
        }
    }

    fn render(&mut self, renderer: &impl Renderer) {
        self.revision += 1;
        renderer.render(
            ViewState {
                revision: self.revision,
                typed_lookup: self.current.as_ref().map(|active| active.snapshot.clone()),
            },
            Vec::new(),
        );
    }

    fn retire(&mut self, run: u64) -> bool {
        if self.current.as_ref().is_some_and(|a| a.snapshot.run == run) {
            self.current = None;
            return true;
        }
        false
    }

    fn look_up(&mut self, typed: TypedWord) {
        // Dropping the old job cancels it. Any outcome already queued still
        // carries its old request id.
        self.current = None;
        self.request += 1;
        let id = RequestId(self.request);
        let send = self.send.clone();
        let started = manual::offline(
            Some(&self.work),
            deadlines::LOOKUP_LIMIT,
            self.dictionaries.clone(),
            typed.word.clone(),
            typed.reading,
            typed.target,
            typed.known_indexes,
            move |language, outcome| {
                let _ = send.send(Message::LookedUp(id, language, Box::new(outcome)));
            },
        );
        let (job, state) = match started {
            Ok(job) => (Some(job), LookupState::Pending),
            Err(lookup) => (
                None,
                LookupState::Resolved(Box::new(Resolution {
                    check_indexes: manual::checks_again(&lookup, typed.reading),
                    lookup: *lookup,
                    answered: None,
                    indexes: None,
                    evidence: None,
                })),
            ),
        };
        self.current = Some(Active {
            word: typed.word,
            snapshot: TypedLookup {
                id,
                run: typed.run,
                index_generation: typed.index_generation,
                state,
            },
            job,
        });
    }

    fn looked_up(&mut self, id: RequestId, language: Language, outcome: Outcome<Looked>) -> bool {
        let Some(current) = &mut self.current else {
            return false;
        };
        if current.snapshot.id != id || current.job.take().is_none() {
            return false;
        }
        let indexes = match &outcome {
            Outcome::Finished(looked) => Some(looked.indexes),
            _ => None,
        };
        let evidence = evidence::answered(&outcome);
        let Some(lookup) = manual::result_in(outcome, &current.word, language) else {
            return false;
        };
        current.snapshot.state = LookupState::Resolved(Box::new(Resolution {
            lookup,
            answered: indexes.map(|_| language),
            indexes,
            evidence,
            check_indexes: false,
        }));
        true
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, time::Duration};

    use addict_deepl::key::KeySource;
    use addict_dictionary::{
        format::{Contents, Headword, SenseRecord, encode},
        import::{self, Source},
    };

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);
    const DEFINITION: &str = "the ground alongside a river";

    struct TestData(PathBuf);

    impl TestData {
        fn with_bank() -> Self {
            let dir = std::env::temp_dir().join(format!("addict-tracer-{}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            let data = Self(dir);
            let index = encode(&Contents {
                source: "oewn".to_owned(),
                senses: vec![SenseRecord {
                    definition: DEFINITION.to_owned(),
                    members: vec!["bank".to_owned()],
                    ..Default::default()
                }],
                headwords: vec![Headword {
                    word: "bank".to_owned(),
                    senses: vec![0],
                    ..Default::default()
                }],
                ..Default::default()
            })
            .unwrap();
            fs::write(import::index_path(&data.0, Source::Oewn), index).unwrap();
            data
        }
    }

    impl Drop for TestData {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FakeShell {
        controller: Controller,
        renders: mpsc::Receiver<ViewState>,
        revision: u64,
    }

    impl FakeShell {
        fn start(data: Option<PathBuf>) -> Self {
            let caller = thread::current().id();
            let (send, renders) = mpsc::channel();
            let controller = start(
                Dirs {
                    data,
                    ..Dirs::default()
                },
                Platform {
                    name: "test".to_owned(),
                },
                StartFacts {
                    key_source: KeySource::pending(),
                },
                move |view, effects: Vec<Effect>| {
                    assert_ne!(thread::current().id(), caller);
                    assert!(effects.is_empty());
                    send.send(view).unwrap();
                },
            )
            .unwrap();
            let mut shell = Self {
                controller,
                renders,
                revision: 0,
            };
            assert!(shell.next().is_none());
            shell
        }

        fn next(&mut self) -> Option<TypedLookup> {
            let view = self.renders.recv_timeout(WAIT).expect("controller render");
            assert!(view.revision > self.revision);
            self.revision = view.revision;
            view.typed_lookup
        }

        fn type_word(&self, run: u64, word: &str, reading: Language) {
            assert!(self.controller.dispatch(Event::WordTyped(TypedWord {
                run,
                word: word.to_owned(),
                reading,
                target: Language::Ru,
                known_indexes: Some(Indexes::MISSING),
                index_generation: 7,
            })));
        }
    }

    #[test]
    fn a_typed_word_renders_an_article_in_a_newer_snapshot() {
        let data = TestData::with_bank();
        let mut shell = FakeShell::start(Some(data.0.clone()));
        shell.type_word(3, "bank", Language::En);
        let pending = shell.next().unwrap();
        assert_eq!(pending.state, LookupState::Pending);
        let completed = shell.next().unwrap();
        assert_eq!(
            (completed.run, completed.id, completed.index_generation),
            (3, pending.id, 7)
        );
        let LookupState::Resolved(resolution) = completed.state else {
            panic!("lookup must resolve");
        };
        let Lookup::Article(article) = resolution.lookup else {
            panic!("bank must have an article");
        };
        assert_eq!(article.senses[0].definition, DEFINITION);
        assert_eq!(resolution.answered, Some(Language::En));
        assert_eq!(resolution.evidence, Some(Answered::Article));
        assert!(resolution.indexes.is_some());

        let dispatcher = shell.controller.dispatcher();
        let work = shell.controller.work();
        assert!(work.upgrade().is_some());
        drop(shell);
        assert!(work.upgrade().is_none());
        assert!(!dispatcher.dispatch(Event::TypedLookupRetired { run: 3 }));
    }

    #[test]
    fn a_replaced_lookup_cannot_settle_the_new_one_and_a_refusal_asks_for_indexes() {
        let mut shell = FakeShell::start(None);
        // Hold the worker while both edits are handled on the controller.
        let work = shell.controller.work().upgrade().unwrap();
        let (started, waiting) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let _gate = work.run(
            WAIT,
            async move {
                started.send(()).unwrap();
                let _ = blocked.recv_timeout(WAIT);
            },
            |_| {},
        );
        waiting.recv_timeout(WAIT).unwrap();

        shell.type_word(3, "bank", Language::En);
        let old = shell.next().unwrap();
        assert_eq!(old.state, LookupState::Pending);
        shell.type_word(5, "river", Language::En);
        let current = shell.next().unwrap();
        assert_eq!(current.state, LookupState::Pending);
        assert!(current.id.0 > old.id.0);
        assert!(
            shell
                .controller
                .dispatch(Event::TypedLookupRetired { run: old.run })
        );
        // A worker callback may have succeeded in enqueuing before cancellation.
        shell
            .controller
            .dispatch
            .0
            .send(Message::LookedUp(
                old.id,
                Language::En,
                Box::new(Outcome::Panicked),
            ))
            .unwrap();
        release.send(()).unwrap();
        let completed = shell.next().unwrap();
        assert_eq!(completed.id, current.id);
        let LookupState::Resolved(resolved) = completed.state else {
            panic!("current lookup must resolve");
        };
        assert_eq!(
            resolved.lookup,
            Lookup::NotImported {
                word: "river".to_owned()
            }
        );

        assert!(
            shell
                .controller
                .dispatch(Event::TypedLookupRetired { run: current.run })
        );
        assert!(shell.next().is_none());

        shell.type_word(7, "banco", Language::Es);
        let refused = shell.next().unwrap();
        assert_eq!(refused.run, 7);
        let LookupState::Resolved(refusal) = refused.state else {
            panic!("a missing dictionary starts no job");
        };
        assert_eq!(refusal.lookup, Lookup::NoDictionary(Language::Es));
        assert!(refusal.check_indexes);
        assert!(refusal.evidence.is_none());
    }
}
