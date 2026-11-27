use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use addict_core::{dirs::Dirs, translation::Source as Language};
use addict_deepl::key::{KeyError, KeySource};
use addict_dictionary::{
    format::{Contents, Headword, VERSION, encode},
    import::{self, Source},
    release::{self, Manifest, Published},
};
use addict_engine::{
    dictionaries::{Answer, Indexes, Looked, State},
    evidence::Answered,
    view::{Lookup, first_run},
    work::Outcome,
};

use super::{FakeShell, Silent};
use crate::{
    Effect, Event, LookedUp, Message, OfferId, OfferPhase, RequestId, ViewState,
    downloads::{Limits, ReadGates},
    settings::ReadGate,
    stub::{Route, Stub},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) struct TestDir(pub PathBuf);

impl TestDir {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "addict-controller-downloads-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub fn dirs(&self) -> Dirs {
        Dirs {
            config: Some(self.0.join("config")),
            data: Some(self.0.join("data")),
            cache: None,
        }
    }

    fn data(&self) -> PathBuf {
        let data = self.0.join("data");
        fs::create_dir_all(&data).unwrap();
        data
    }

    fn decline(&self) {
        let path = crate::first_run_choice_path(self.dirs().config.as_deref()).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "declined\n").unwrap();
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn gate() -> Arc<ReadGate> {
    Arc::new(ReadGate::default())
}

pub(super) fn shell(
    dirs: Dirs,
    release: String,
    settings: Option<Arc<ReadGate>>,
    reads: ReadGates,
    limits: Limits,
) -> FakeShell {
    FakeShell::with_downloads(
        dirs,
        Silent,
        false,
        KeySource::new(Err(KeyError::Missing)),
        Default::default(),
        Default::default(),
        Default::default(),
        false,
        settings,
        release,
        limits,
        reads,
    )
}

pub(super) fn startup(dirs: Dirs) -> FakeShell {
    let mut shell = shell(
        dirs,
        String::new(),
        None,
        ReadGates::default(),
        Limits::default(),
    );
    settled_start(&mut shell);
    shell
}

fn settled_start(shell: &mut FakeShell) -> ViewState {
    shell.wait_for(|view| view.settings.languages_read && view.downloads.indexes.is_some())
}

pub(super) fn wait_until(limit: Duration, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !condition() {
        assert!(Instant::now() < deadline, "condition did not arrive");
        thread::sleep(Duration::from_millis(4));
    }
}

pub(super) fn mark(shell: &mut FakeShell) -> ViewState {
    shell.dispatch(Event::SetReadingSource(Some(
        addict_core::input::Source::Clipboard,
    )));
    let view = shell.wait_for(|view| view.reading_source.is_some());
    shell.dispatch(Event::SetReadingSource(None));
    shell.wait_for(|view| view.reading_source.is_none());
    view
}

fn index(word: &str) -> Vec<u8> {
    encode(&Contents {
        source: "download-refresh-regression".to_owned(),
        headwords: vec![Headword {
            word: word.to_owned(),
            translations: vec!["банк".to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    })
    .unwrap()
}

pub(super) fn write_index(dir: &Path, source: Source, word: &str) {
    fs::write(import::index_path(dir, source), index(word)).unwrap();
}

fn asset(name: &str, word: &str) -> (Published, Vec<u8>) {
    let mut xz = Vec::new();
    lzma_rs::xz_compress(&mut index(word).as_slice(), &mut xz).unwrap();
    (Published::of(name, &xz), xz)
}

fn route(asset: &str) -> String {
    format!("/{}/{}", release::tag(), asset)
}

fn stub(indexes: Vec<Published>, routes: impl IntoIterator<Item = (String, Route)>) -> Stub {
    let manifest = Manifest {
        format: VERSION,
        indexes,
    };
    let mut routes: HashMap<_, _> = routes.into_iter().collect();
    routes.insert(
        route(release::MANIFEST_3),
        Route::File(manifest.to_json().into_bytes()),
    );
    Stub::start(routes).unwrap()
}

fn at_release(dirs: Dirs, stub: &Stub) -> FakeShell {
    shell(
        dirs,
        stub.base_url(),
        None,
        ReadGates::default(),
        Limits::default(),
    )
}

fn listing(shell: &mut FakeShell) -> ViewState {
    shell.wait_for(|view| {
        view.downloads
            .offer
            .as_ref()
            .is_some_and(|offer| offer.phase == OfferPhase::Listing)
    })
}

fn row(view: &ViewState, name: &str) -> Option<first_run::RowState> {
    let queue = view.downloads.offer.as_ref()?.queue()?;
    queue
        .rows()
        .iter()
        .find(|(entry, _)| entry.name == name)
        .map(|(_, state)| state.clone())
}

fn ended(shell: &mut FakeShell) -> ViewState {
    shell.wait_for(|view| {
        view.downloads
            .offer
            .as_ref()
            .is_some_and(|offer| matches!(offer.phase, OfferPhase::Ended { .. }))
    })
}

fn choose(shell: &mut FakeShell, my: Language) -> ViewState {
    shell.dispatch(Event::MyLanguageChosen(my));
    shell.wait_for(|view| view.settings.my_chosen > 0 && view.settings.my_language == my)
}

fn recover(shell: &mut FakeShell) -> OfferId {
    shell.dispatch(Event::DownloadPressed);
    assert_eq!(
        shell
            .wait_for(|view| view.downloads.settings_busy)
            .downloads
            .settings_line,
        first_run::FETCHING
    );
    let view = listing(shell);
    assert!(!view.downloads.settings_busy);
    let offer = view.downloads.offer.unwrap();
    assert!(offer.recovery);
    offer.id
}

pub(super) fn stale_lookup(shell: &mut FakeShell, indexes: Indexes) {
    shell
        .controller
        .dispatch
        .0
        .send(Message::LookedUp(LookedUp {
            id: RequestId(u64::MAX),
            run: 0,
            generation: Some(0),
            language: Language::En,
            outcome: Box::new(Outcome::Finished(Looked {
                indexes,
                answer: Answer::NoEntry,
            })),
        }))
        .unwrap();
    let (_, effects) = shell.next();
    assert!(
        effects.contains(&Effect::RecordLookup(Answered::NoEntry)),
        "{effects:?}"
    );
}

pub(super) fn assert_pair_lookup(shell: &mut FakeShell, language: Language, word: &str) {
    shell.dispatch(Event::ReadingChosen(language));
    shell.edit(word);
    let view = shell.wait_for(|view| view.sentence == word && !view.looking_up);
    assert_eq!(view.answered, Some(language));
    let Some(Lookup::Grouped { grouped, .. }) = view.shown.lookup() else {
        panic!(
            "expected the imported language entry: {:?}",
            view.shown.lookup()
        );
    };
    assert!(
        grouped
            .translations_into(Language::Ru)
            .unwrap_or_default()
            .iter()
            .any(|text| text == "банк")
    );
}

#[test]
fn first_run_uses_the_saved_language_in_either_order() {
    for my in [Language::En, Language::Ru] {
        for indexes_first in [false, true] {
            let temp = TestDir::new();
            let dir = temp.data();
            write_index(&dir, Source::WikdictRuEs, "слово");
            addict_engine::my_language::write(
                addict_engine::my_language::path(temp.dirs().config.as_deref()).as_deref(),
                my,
            )
            .unwrap();
            let server = stub(vec![Published::of("oewn", b"xz")], []);
            let settings = gate();
            let indexes = gate();
            let mut shell = shell(
                temp.dirs(),
                server.base_url(),
                Some(settings.clone()),
                ReadGates {
                    startup: Some(indexes.clone()),
                    ..ReadGates::default()
                },
                Limits::default(),
            );
            assert!(shell.next().0.downloads.offer.is_none());
            let first = if indexes_first {
                indexes.release();
                shell.wait_for(|view| view.downloads.indexes.is_some())
            } else {
                settings.release();
                shell.wait_for(|view| view.settings.languages_read)
            };
            assert!(first.downloads.offer.is_none());
            assert!(server.seen().is_empty());
            indexes.release();
            settings.release();
            let settled = settled_start(&mut shell);
            assert_eq!(settled.settings.my_language, my);
            assert!(
                settled
                    .downloads
                    .indexes
                    .unwrap()
                    .presence
                    .contains(Source::WikdictRuEs)
            );
            if my == Language::En {
                let view = listing(&mut shell);
                let offer = view.downloads.offer.unwrap();
                assert!(!offer.recovery);
                assert_eq!(offer.my_language, my);
                assert_eq!(offer.listed[0].name, "oewn");
                assert_eq!(server.seen().len(), 1);
            } else {
                assert!(mark(&mut shell).downloads.offer.is_none());
                assert!(server.seen().is_empty());
            }
        }
    }
}

#[test]
fn an_offer_prepared_during_an_import_waits_for_current_indexes() {
    let temp = TestDir::new();
    let server = stub(vec![Published::of("oewn", b"entry")], []);
    let offered = gate();
    let refresh = gate();
    let mut shell = shell(
        temp.dirs(),
        server.base_url(),
        None,
        ReadGates {
            offer: Some(offered.clone()),
            refresh: Some(refresh.clone()),
            ..Default::default()
        },
        Limits::default(),
    );
    let original = settled_start(&mut shell);
    wait_until(Duration::from_secs(4), || offered.is_waiting());
    let id = super::import::chooser(&mut shell);
    shell.dispatch(Event::FileChosen {
        id,
        result: Ok(super::import::kaikki_fixture(&temp.0)),
    });
    shell.wait_for(|view| view.downloads.import.busy());
    wait_until(Duration::from_secs(4), || refresh.is_waiting());
    assert_eq!(
        mark(&mut shell).downloads.indexes,
        original.downloads.indexes
    );
    offered.release();
    stale_lookup(&mut shell, Indexes::OPEN);
    let during = mark(&mut shell);
    assert_eq!(during.downloads.indexes, original.downloads.indexes);
    assert!(during.downloads.offer.is_none());
    refresh.release();
    let view = shell.wait_for(|view| {
        matches!(
            view.downloads.import,
            addict_engine::view::file_choice::ImportState::Imported(_)
        )
    });
    assert!(view.downloads.offer.is_none());
    assert!(
        view.downloads
            .indexes
            .unwrap()
            .presence
            .contains(Source::KaikkiEs)
    );
    assert_eq!(server.seen().len(), 1);
}

#[test]
fn the_initial_offer_decision_waits_for_an_already_committed_import() {
    let temp = TestDir::new();
    let server = stub(vec![Published::of("oewn", b"entry")], []);
    let settings = gate();
    let refresh = gate();
    let mut shell = shell(
        temp.dirs(),
        server.base_url(),
        Some(settings.clone()),
        ReadGates {
            refresh: Some(refresh.clone()),
            ..Default::default()
        },
        Limits::default(),
    );
    shell.wait_for(|view| view.downloads.indexes.is_some());
    let id = super::import::chooser(&mut shell);
    shell.dispatch(Event::FileChosen {
        id,
        result: Ok(super::import::kaikki_fixture(&temp.0)),
    });
    shell.wait_for(|view| view.downloads.import.busy());
    wait_until(Duration::from_secs(4), || refresh.is_waiting());
    settings.release();
    let during = shell.wait_for(|view| view.settings.languages_read);
    assert!(during.downloads.offer.is_none());
    assert!(server.seen().is_empty());
    refresh.release();
    let view = shell.wait_for(|view| {
        matches!(
            view.downloads.import,
            addict_engine::view::file_choice::ImportState::Imported(_)
        )
    });
    assert!(view.downloads.offer.is_none());
    assert!(server.seen().is_empty());
}

#[test]
fn downloaded_dictionaries_refresh_even_when_cancelled_after_commit_before_refresh() {
    for (language, source, name, word) in [
        (Language::Es, Source::WikdictEsRu, "wikdict-es-ru", "banco"),
        (Language::Ca, Source::WikdictCaRu, "wikdict-ca-ru", "banc"),
    ] {
        let temp = TestDir::new();
        temp.decline();
        let dir = temp.data();
        let (published, packed) = asset(name, word);
        let asset_path = route(&published.asset());
        let server = stub(vec![published], [(asset_path, Route::File(packed))]);
        let refresh = gate();
        let mut shell = shell(
            temp.dirs(),
            server.base_url(),
            None,
            ReadGates {
                refresh: Some(refresh.clone()),
                ..ReadGates::default()
            },
            Limits::default(),
        );
        let stale = settled_start(&mut shell).downloads.indexes.unwrap();
        assert_eq!(stale.spanish, State::Missing);
        assert_eq!(stale.catalan, State::Missing);
        let id = recover(&mut shell);
        shell.dispatch(Event::OfferDownloadPressed(id));
        shell.wait_for(|view| matches!(row(view, name), Some(first_run::RowState::Running { .. })));
        wait_until(Duration::from_secs(4), || {
            import::index_path(&dir, source).is_file()
        });
        assert!(
            import::index_path(&dir, source).is_file(),
            "committed before cancellation"
        );
        wait_until(Duration::from_secs(4), || refresh.is_waiting());
        shell.dispatch(Event::OfferCancelPressed(id, Some(name.to_owned())));
        let cancelled =
            shell.wait_for(|view| row(view, name) == Some(first_run::RowState::Cancelled));
        assert_eq!(cancelled.downloads.indexes, Some(stale));
        assert!(matches!(
            cancelled.downloads.offer.unwrap().phase,
            OfferPhase::Ended {
                complete: false,
                ..
            }
        ));

        stale_lookup(&mut shell, Indexes::OPEN);
        assert_eq!(
            mark(&mut shell).downloads.indexes,
            Some(stale),
            "advance before accepting old checks"
        );
        refresh.release();
        let current = shell.wait_for(|view| {
            view.downloads
                .indexes
                .is_some_and(|indexes| indexes.presence.contains(source))
        });
        stale_lookup(&mut shell, stale);
        assert_eq!(
            mark(&mut shell).downloads.indexes,
            current.downloads.indexes
        );
        assert_pair_lookup(&mut shell, language, word);
    }
}

#[test]
fn a_delayed_typed_snapshot_refreshes_a_refusal_without_overwriting_newer_indexes() {
    let temp = TestDir::new();
    temp.decline();
    let dir = temp.data();
    let refresh = gate();
    let mut shell = shell(
        temp.dirs(),
        String::new(),
        None,
        ReadGates {
            refresh: Some(refresh.clone()),
            ..ReadGates::default()
        },
        Limits::default(),
    );
    settled_start(&mut shell);
    write_index(&dir, Source::WikdictEsRu, "banco");
    shell.dispatch(Event::ReadingChosen(Language::Es));
    shell.edit("banco");
    let refused = shell.wait_for(|view| view.sentence == "banco");
    assert_eq!(
        refused.shown.lookup(),
        Some(&Lookup::NoDictionary(Language::Es))
    );
    shell.edit("new text");
    let newer = shell.wait_for(|view| {
        view.sentence == "new text"
            && view
                .downloads
                .indexes
                .is_some_and(|indexes| indexes.spanish == State::Open)
    });
    assert!(
        newer.revision > refused.revision,
        "a stale refusal still refreshes"
    );

    shell.dispatch(Event::ImportPressed);
    let (_, effects) = shell.next();
    let [Effect::ChooseFile { id }] = effects.as_slice() else {
        panic!("chooser")
    };
    shell.dispatch(Event::FileChosen {
        id: *id,
        result: Ok(super::import::kaikki_fixture(&temp.0)),
    });
    shell.wait_for(|view| view.downloads.import.busy());
    wait_until(Duration::from_secs(4), || {
        dir.join("kaikki-es-Espanol.index").is_file()
    });
    wait_until(Duration::from_secs(4), || refresh.is_waiting());
    let before = mark(&mut shell);
    shell.dispatch(Event::ReadingChosen(Language::Ca));
    shell.edit("altre");
    let refused = shell.wait_for(|view| view.sentence == "altre");
    assert_eq!(
        refused.shown.lookup(),
        Some(&Lookup::NoDictionary(Language::Ca))
    );
    assert_eq!(
        refused.downloads.indexes, before.downloads.indexes,
        "the import already owns the refresh"
    );
    stale_lookup(&mut shell, Indexes::OPEN);
    refresh.release();
    shell.wait_for(|view| {
        matches!(
            view.downloads.import,
            addict_engine::view::file_choice::ImportState::Imported(_)
        )
    });

    shell.dispatch(Event::ReadingChosen(Language::En));
    shell.edit("bank");
    let observed =
        shell.wait_for(|view| view.sentence == "bank" && view.answered == Some(Language::En));
    let current = observed.downloads.indexes.expect("observed indexes");
    assert_eq!(current.definitions, State::Missing);
    stale_lookup(&mut shell, Indexes::OPEN);
    assert_eq!(mark(&mut shell).downloads.indexes, Some(current));
}

#[test]
fn a_start_check_that_times_out_leaves_indexes_unknown() {
    let temp = TestDir::new();
    let server = stub(vec![Published::of("oewn", b"entry")], []);
    let startup = gate();
    let mut shell = shell(
        temp.dirs(),
        server.base_url(),
        None,
        ReadGates {
            startup: Some(startup.clone()),
            ..ReadGates::default()
        },
        Limits {
            check: Duration::from_millis(15),
            ..Limits::default()
        },
    );
    shell.wait_for(|view| view.settings.languages_read);
    thread::sleep(Duration::from_millis(100));
    startup.release();
    let view = mark(&mut shell);
    assert_eq!(view.downloads.indexes, None);
    assert!(view.downloads.offer.is_none());
    assert!(server.seen().is_empty());
}

#[test]
fn a_user_choice_before_the_read_starts_recovery_only_after_it() {
    let temp = TestDir::new();
    let dir = temp.data();
    write_index(&dir, Source::Oewn, "bank");
    write_index(&dir, Source::WikdictEsRu, "banco");
    let server = stub(vec![Published::of("wikdict-es-en", b"entry")], []);
    let settings = gate();
    let mut shell = shell(
        temp.dirs(),
        server.base_url(),
        Some(settings.clone()),
        ReadGates::default(),
        Limits::default(),
    );
    shell.wait_for(|view| view.downloads.indexes.is_some());
    let chosen = choose(&mut shell, Language::Es);
    assert!(!chosen.settings.languages_read);
    assert_eq!(chosen.downloads.settings_line, "");
    assert!(chosen.downloads.offer.is_none());
    assert!(server.seen().is_empty());
    settings.release();
    let offer = listing(&mut shell).downloads.offer.unwrap();
    assert!(offer.recovery);
    assert_eq!(offer.my_language, Language::Es);
    assert_eq!(offer.listed[0].name, "wikdict-es-en");
}

#[test]
fn declining_closes_the_first_run_and_keeps_the_choice_during_recovery() {
    let temp = TestDir::new();
    let server = stub(vec![Published::of("oewn", b"entry")], []);
    let mut shell = at_release(temp.dirs(), &server);
    let id = listing(&mut shell).downloads.offer.unwrap().id;
    shell.dispatch(Event::OfferNotNowPressed(id));
    assert!(shell.next().0.downloads.offer.is_none());
    let recovery = recover(&mut shell);
    assert_ne!(recovery, id);
    shell.quiet(Event::OfferDownloadPressed(id));
    shell.quiet(Event::OfferNotNowPressed(id));
    shell.quiet(Event::OfferCancelPressed(id, None));
    shell.dispatch(Event::OfferNotNowPressed(recovery));
    let closed = shell.next().0;
    assert!(closed.downloads.offer.is_none());
    assert_eq!(closed.downloads.settings_line, "");
    let path = crate::first_run_choice_path(temp.dirs().config.as_deref()).unwrap();
    wait_until(Duration::from_secs(4), || path.is_file());
    assert_eq!(fs::read_to_string(&path).unwrap(), "declined\n");
    drop(shell);
    let mut restarted = at_release(temp.dirs(), &server);
    let requests = server.seen();
    assert!(settled_start(&mut restarted).downloads.offer.is_none());
    assert!(mark(&mut restarted).downloads.offer.is_none());
    assert_eq!(server.seen(), requests);
}

#[test]
fn download_cancel_one_starts_the_next_and_cancel_all_leaves_no_partial_file() {
    let temp = TestDir::new();
    let (one, first) = asset("oewn", "bank");
    let (two, second) = asset("freedict", "river");
    let (three, third) = asset("wikdict-es-ru", "banco");
    let paths = [&one, &two, &three].map(|entry| route(&entry.asset()));
    let server = stub(
        vec![one, two, three],
        [
            (paths[0].clone(), Route::Stall(first)),
            (paths[1].clone(), Route::Stall(second)),
            (paths[2].clone(), Route::File(third)),
        ],
    );
    let mut shell = at_release(temp.dirs(), &server);
    let offer = listing(&mut shell).downloads.offer.unwrap();
    assert!(!offer.recovery);
    let id = offer.id;
    shell.dispatch(Event::OfferDownloadPressed(id));
    shell.wait_for(|view| matches!(row(view, "oewn"), Some(first_run::RowState::Running { received }) if received > 0));
    shell.dispatch(Event::OfferCancelPressed(id, Some("oewn".to_owned())));
    let second = shell.wait_for(|view| matches!(row(view, "freedict"), Some(first_run::RowState::Running { received }) if received > 0));
    assert_eq!(row(&second, "oewn"), Some(first_run::RowState::Cancelled));
    assert_eq!(
        row(&second, "wikdict-es-ru"),
        Some(first_run::RowState::Queued)
    );
    shell.dispatch(Event::OfferCancelPressed(id, None));
    let end = ended(&mut shell);
    assert_eq!(row(&end, "freedict"), Some(first_run::RowState::Cancelled));
    assert_eq!(
        row(&end, "wikdict-es-ru"),
        Some(first_run::RowState::Cancelled)
    );
    assert_eq!(fs::read_dir(temp.data()).unwrap().count(), 0);
    assert_eq!(
        server.seen(),
        [
            format!("GET {} HTTP/1.1", route(release::MANIFEST_3)),
            format!("GET {} HTTP/1.1", paths[0]),
            format!("GET {} HTTP/1.1", paths[1])
        ]
    );
}

#[test]
fn successful_install_reports_only_after_indexes_refresh() {
    let temp = TestDir::new();
    let (entry, bytes) = asset("wikdict-es-ru", "banco");
    let server = stub(
        vec![entry.clone()],
        [(route(&entry.asset()), Route::File(bytes))],
    );
    let gate = gate();
    let mut shell = shell(
        temp.dirs(),
        server.base_url(),
        None,
        ReadGates {
            refresh: Some(gate.clone()),
            ..ReadGates::default()
        },
        Limits::default(),
    );
    let id = listing(&mut shell).downloads.offer.unwrap().id;
    shell.dispatch(Event::OfferDownloadPressed(id));
    shell.wait_for(|view| {
        matches!(
            row(view, "wikdict-es-ru"),
            Some(first_run::RowState::Running { .. })
        )
    });
    wait_until(Duration::from_secs(4), || {
        temp.0.join("data/wikdict-es-ru.index").exists()
    });
    let view = mark(&mut shell);
    assert_eq!(view.downloads.indexes.unwrap().spanish, State::Missing);
    assert!(matches!(
        row(&view, "wikdict-es-ru"),
        Some(first_run::RowState::Running { .. })
    ));
    gate.release();
    let ready =
        shell.wait_for(|view| row(view, "wikdict-es-ru") == Some(first_run::RowState::Installed));
    assert_eq!(ready.downloads.indexes.unwrap().spanish, State::Open);
    assert!(matches!(
        ready.downloads.offer.unwrap().phase,
        OfferPhase::Ended { complete: true, .. }
    ));
}

#[test]
fn recovery_failure_and_timeout_are_visible_and_retryable() {
    let mut unavailable = startup(Dirs::default());
    unavailable.dispatch(Event::DownloadPressed);
    assert_eq!(
        unavailable.next().0.downloads.settings_line,
        first_run::recovery_failed_text(first_run::NO_PLACE)
    );

    let temp = TestDir::new();
    temp.decline();
    let server = Stub::start(HashMap::from([(
        route(release::MANIFEST_3),
        Route::Stall(vec![b' '; 20]),
    )]))
    .unwrap();
    let mut shell = shell(
        temp.dirs(),
        server.base_url(),
        None,
        ReadGates::default(),
        Limits {
            download: Duration::from_millis(100),
            ..Limits::default()
        },
    );
    settled_start(&mut shell);
    for _ in 0..2 {
        shell.dispatch(Event::DownloadPressed);
        assert!(shell.next().0.downloads.settings_busy);
        let view = shell.wait_for(|view| !view.downloads.settings_busy);
        assert_eq!(
            view.downloads.settings_line,
            first_run::recovery_failed_text("the dictionary check did not finish in time")
        );
        assert!(view.downloads.offer.is_none());
    }
    assert_eq!(server.seen().len(), 2);
}

#[test]
fn timed_out_install_fails_its_row() {
    let temp = TestDir::new();
    let (entry, bytes) = asset("oewn", "bank");
    let server = stub(
        vec![entry.clone()],
        [(route(&entry.asset()), Route::Stall(bytes))],
    );
    let limit = Duration::from_millis(200);
    let mut shell = shell(
        temp.dirs(),
        server.base_url(),
        None,
        ReadGates::default(),
        Limits {
            download: limit,
            ..Limits::default()
        },
    );
    let id = listing(&mut shell).downloads.offer.unwrap().id;
    shell.dispatch(Event::OfferDownloadPressed(id));
    let end = ended(&mut shell);
    assert_eq!(
        row(&end, "oewn"),
        Some(first_run::RowState::Failed(first_run::failure_text(
            "oewn",
            &first_run::late_text(limit),
        )))
    );
    assert!(matches!(
        end.downloads.offer.unwrap().phase,
        OfferPhase::Ended {
            complete: false,
            ..
        }
    ));
    assert_eq!(fs::read_dir(temp.data()).unwrap().count(), 0);
}

#[test]
fn choosing_a_missing_set_after_an_empty_relist_fetches_recovery_again() {
    let temp = TestDir::new();
    temp.decline();
    let dirs = temp.dirs();
    let saved = addict_engine::my_language::path(dirs.config.as_deref());
    addict_engine::my_language::write(saved.as_deref(), Language::En).unwrap();
    let server = stub(vec![Published::of("wikdict-en-es", b"entry")], []);
    let mut shell = at_release(dirs, &server);
    settled_start(&mut shell);
    recover(&mut shell);

    let chosen = choose(&mut shell, Language::Ru);
    assert!(chosen.downloads.offer.is_none());
    assert!(chosen.downloads.settings_busy);
    assert_eq!(chosen.downloads.settings_line, first_run::FETCHING);

    let checked = shell.wait_for(|view| !view.downloads.settings_busy);
    assert!(checked.downloads.offer.is_none());
    assert_eq!(
        checked.downloads.settings_line,
        first_run::nothing_text(Language::Ru)
    );
    assert_eq!(server.seen().len(), 2);
}

#[test]
fn only_a_presented_first_run_offer_relists_a_partly_installed_set() {
    for presented in [false, true] {
        let temp = TestDir::new();
        write_index(&temp.data(), Source::WikdictEnEs, "bank");
        let server = stub(vec![Published::of("oewn", b"entry")], []);
        let mut shell = at_release(temp.dirs(), &server);
        let first = listing(&mut shell).downloads.offer.unwrap();
        assert!(!first.recovery);
        if presented {
            shell.quiet(Event::OfferPresented(first.id));
        }
        let chosen = choose(&mut shell, Language::En);
        if presented {
            let offer = chosen.downloads.offer.unwrap();
            assert_eq!(offer.id, first.id);
            assert!(!offer.recovery);
            assert_eq!(offer.my_language, Language::En);
            assert_eq!(server.seen().len(), 1);
        } else {
            assert!(chosen.downloads.settings_busy);
            let offer = listing(&mut shell).downloads.offer.unwrap();
            assert!(offer.recovery);
            assert_ne!(offer.id, first.id);
            assert_eq!(offer.my_language, Language::En);
            assert_eq!(server.seen().len(), 2);
        }
    }
}

#[test]
fn empty_and_invalid_manifests_leave_no_first_run_offer_or_decline() {
    let empty = Manifest {
        format: VERSION,
        indexes: Vec::new(),
    }
    .to_json();
    for body in [empty.into_bytes(), b"not a manifest".to_vec()] {
        let temp = TestDir::new();
        let server = Stub::start(HashMap::from([(
            route(release::MANIFEST_3),
            Route::File(body),
        )]))
        .unwrap();
        let gate = gate();
        let mut shell = shell(
            temp.dirs(),
            server.base_url(),
            None,
            ReadGates {
                offer: Some(gate.clone()),
                ..Default::default()
            },
            Limits::default(),
        );
        settled_start(&mut shell);
        wait_until(Duration::from_secs(4), || gate.is_waiting());
        gate.release();
        let (release, _held) = shell.hold_worker();
        let view = mark(&mut shell);
        assert!(view.downloads.offer.is_none());
        assert!(view.downloads.settings_line.is_empty());
        assert_eq!(server.seen().len(), 1);
        assert!(!temp.0.join("config/first-run-download").exists());
        drop(release);
    }
}

#[test]
fn stale_preparation_progress_and_install_outcomes_do_not_change_a_new_recovery() {
    let temp = TestDir::new();
    temp.decline();
    let (entry, bytes) = asset("oewn", "bank");
    let server = stub(
        vec![entry.clone()],
        [(route(&entry.asset()), Route::Stall(bytes))],
    );
    let mut shell = at_release(temp.dirs(), &server);
    settled_start(&mut shell);
    let old = recover(&mut shell);
    shell.dispatch(Event::OfferNotNowPressed(old));
    assert!(shell.next().0.downloads.offer.is_none());

    let (resume, _held) = shell.hold_worker();
    shell.dispatch(Event::DownloadPressed);
    assert!(
        shell
            .wait_for(|view| view.downloads.settings_busy)
            .downloads
            .offer
            .is_none()
    );
    let send = shell.controller.dispatch.0.clone();
    let stale = Manifest {
        format: VERSION,
        indexes: vec![entry],
    };
    send.send(Message::Downloads(
        crate::downloads::Update::RecoveryPrepared(old, Outcome::Finished(Ok(stale))),
    ))
    .unwrap();
    let preparing = mark(&mut shell);
    assert!(preparing.downloads.settings_busy);
    assert!(preparing.downloads.offer.is_none());
    drop(resume);

    let id = listing(&mut shell).downloads.offer.unwrap().id;
    assert_ne!(id, old);
    let (resume, _held) = shell.hold_worker();
    shell.dispatch(Event::OfferDownloadPressed(id));
    shell.wait_for(|view| row(view, "oewn") == Some(first_run::RowState::Running { received: 0 }));
    send.send(Message::Downloads(crate::downloads::Update::Progress(
        RequestId(u64::MAX),
        old,
        100,
    )))
    .unwrap();
    send.send(Message::Downloads(crate::downloads::Update::Installed(
        RequestId(u64::MAX),
        old,
        Outcome::Finished(Ok(PathBuf::new())),
    )))
    .unwrap();
    shell.quiet(Event::OfferDownloadPressed(id));
    let current = mark(&mut shell);
    assert_eq!(
        row(&current, "oewn"),
        Some(first_run::RowState::Running { received: 0 })
    );
    drop(resume);
    shell.wait_for(|view| matches!(row(view, "oewn"), Some(first_run::RowState::Running { received }) if received > 0));
    shell.dispatch(Event::OfferCancelPressed(id, None));
    ended(&mut shell);
}
