use std::{fs, net::TcpListener, path::PathBuf};

use addict_core::failure::playback::PlaybackFailure;
use addict_engine::{
    recording::Credit,
    view::{PLAYBACK_FAILED, RECORDING_NEEDS_NETWORK},
};

use super::downloads::{TestDir, startup, wait_until};
use super::{FakeShell, TestData, WAIT, marker};
use crate::{
    ArticleId, Effect, Event, Message, Outcome, PlaybackState, PlaybackView, RequestId,
    stub::{Route, Stub},
};

const LOCAL: &str = "file:///tmp/a%20recording.ogg";
const COMMONS: &str = "https://commons.wikimedia.org/wiki/Special:FilePath/Fixture.ogg";
const COMMONS_HASH: &str = "8fce0b6d383f7dd767d591b5183140b98db00778e960da3898d6ee5a21d0d3cc";
const CREDIT_ROUTE: &str = "/credit?action=query&format=json&formatversion=2&titles=File%3AFixture.ogg&prop=imageinfo&iiprop=extmetadata&iiextmetadatafilter=Artist%7CAttribution%7CLicenseShortName";
const CREDIT: &[u8] = br#"{"query":{"pages":[{"ns":6,"imageinfo":[{"extmetadata":{"Artist":{"value":"The Shtooka Project"},"LicenseShortName":{"value":"CC BY 2.0 FR"}}}]}]}}"#;

fn fixture(cache: bool) -> (TestDir, TestData, FakeShell, ArticleId) {
    let temp = TestDir::new();
    let data = TestData::with_bank();
    let mut dirs = temp.dirs();
    dirs.data = Some(data.0.clone());
    dirs.cache = cache.then(|| temp.0.join("cache"));
    let mut shell = startup(dirs);
    shell.edit("bank");
    let (view, _) = shell.settled();
    assert!(matches!(
        view.shown.lookup(),
        Some(addict_engine::view::Lookup::Article(_))
    ));
    let article = view.article_id();
    (temp, data, shell, article)
}

fn press(shell: &mut FakeShell, article: ArticleId, url: &str) -> (PlaybackView, Vec<Effect>) {
    shell.dispatch(Event::PlayPressed {
        article,
        url: url.into(),
    });
    let (view, effects) = shell.next();
    let playback = view.playback.expect("playback snapshot");
    assert_eq!(playback.url, url);
    (playback, effects)
}

fn ready(shell: &mut FakeShell, id: RequestId) -> PathBuf {
    let (view, effects) = shell.next();
    let [Effect::Play { id: played, path }] = effects.as_slice() else {
        panic!("expected one Play, got {effects:?}");
    };
    assert_eq!(*played, id);
    assert_eq!(view.playback.unwrap().state, PlaybackState::Playing);
    path.clone()
}

fn finish(shell: &FakeShell, id: RequestId, result: Result<(), PlaybackFailure>) {
    shell.dispatch(Event::Played { id, result });
}

fn cache_commons(temp: &TestDir, shell: &FakeShell, server: &Stub) {
    let dir = temp.0.join("cache/recordings");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{COMMONS_HASH}.ogg")), b"cached audio").unwrap();
    shell
        .controller
        .dispatch
        .0
        .send(Message::CreditEndpoint(format!(
            "{}/credit",
            server.base_url()
        )))
        .unwrap();
}

fn start_commons(shell: &mut FakeShell, article: ArticleId) -> RequestId {
    let (pending, _) = press(shell, article, COMMONS);
    ready(shell, pending.id);
    pending.id
}

#[test]
fn fetches_remote_audio_atomically_and_replays_it_without_a_request() {
    let (temp, _data, mut shell, article) = fixture(true);
    let server =
        Stub::start([("/file.ogg".into(), Route::File(b"audio".to_vec()))].into()).unwrap();
    let url = format!("{}/file.ogg", server.base_url());
    let (pending, effects) = press(&mut shell, article, &url);
    assert_eq!(pending.state, PlaybackState::Fetching);
    assert!(effects.is_empty());
    let path = ready(&mut shell, pending.id);
    assert_eq!(path.parent().unwrap(), temp.0.join("cache/recordings"));
    assert_eq!(path.file_stem().unwrap().len(), 64);
    assert_eq!(path.extension().unwrap(), "ogg");
    assert_eq!(fs::read(&path).unwrap(), b"audio");
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    assert_eq!(server.seen(), ["GET /file.ogg HTTP/1.1"]);

    let (retry, effects) = press(&mut shell, article, &url);
    assert_eq!(effects, [Effect::Cancel(pending.id)]);
    assert_ne!(retry.id, pending.id);
    assert_eq!(ready(&mut shell, retry.id), path);
    assert_eq!(server.seen(), ["GET /file.ogg HTTP/1.1"]);
    drop(server);
    let (offline, _) = press(&mut shell, article, &url);
    assert_eq!(ready(&mut shell, offline.id), path);
}

#[test]
fn a_local_recording_uses_its_path_and_another_scheme_fails() {
    let (_temp, _data, mut shell, article) = fixture(true);
    let (playing, effects) = press(&mut shell, article, LOCAL);
    assert_eq!(playing.state, PlaybackState::Playing);
    assert_eq!(
        effects,
        [Effect::Play {
            id: playing.id,
            path: "/tmp/a recording.ogg".into()
        }]
    );
    let (failed, effects) = press(&mut shell, article, "ftp://example.org/audio");
    assert_eq!(
        failed.state,
        PlaybackState::Failed(PLAYBACK_FAILED.to_owned())
    );
    assert_eq!(effects, [Effect::Cancel(playing.id)]);
}

#[test]
fn remote_audio_without_a_cache_directory_fails_without_fetching() {
    let (_temp, _data, mut shell, article) = fixture(false);
    let server =
        Stub::start([("/file.ogg".into(), Route::File(b"audio".to_vec()))].into()).unwrap();
    let (failed, effects) = press(
        &mut shell,
        article,
        &format!("{}/file.ogg", server.base_url()),
    );
    assert_eq!(
        failed.state,
        PlaybackState::Failed(PLAYBACK_FAILED.to_owned())
    );
    assert!(effects.is_empty());
    assert!(server.seen().is_empty());
}

#[test]
fn network_and_deadline_failures_have_the_network_text_and_leave_no_cache_file() {
    let (temp, _data, mut shell, article) = fixture(true);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/file.ogg", listener.local_addr().unwrap());
    drop(listener);
    let (unreachable, _) = press(&mut shell, article, &url);
    let (failed, effects) = shell.next();
    assert!(effects.is_empty());
    assert_eq!(
        failed.playback.unwrap().state,
        PlaybackState::Failed(RECORDING_NEEDS_NETWORK.to_owned())
    );

    let stub = Stub::start(
        [(
            "/stall.ogg".into(),
            Route::Stall(b"unfinished recording".to_vec()),
        )]
        .into(),
    )
    .unwrap();
    let (pending, effects) = press(
        &mut shell,
        article,
        &format!("{}/stall.ogg", stub.base_url()),
    );
    assert_eq!(effects, [Effect::Cancel(unreachable.id)]);
    wait_until(WAIT, || !stub.seen().is_empty());
    shell
        .controller
        .dispatch
        .0
        .send(Message::AudioReady(pending.id, Outcome::TimedOut))
        .unwrap();
    assert_eq!(
        shell.next().0.playback.unwrap().state,
        PlaybackState::Failed(RECORDING_NEEDS_NETWORK.to_owned())
    );
    assert!(!temp.0.join("cache/recordings").exists());
}

#[test]
fn a_truncated_fetch_fails_without_leaving_a_partial_recording() {
    let (temp, _data, mut shell, article) = fixture(true);
    let server = Stub::start(
        [(
            "/short.ogg".into(),
            Route::Short(b"incomplete audio".to_vec()),
        )]
        .into(),
    )
    .unwrap();
    press(
        &mut shell,
        article,
        &format!("{}/short.ogg", server.base_url()),
    );
    assert_eq!(
        shell.next().0.playback.unwrap().state,
        PlaybackState::Failed(PLAYBACK_FAILED.to_owned())
    );
    assert!(!temp.0.join("cache/recordings").exists());
}

#[test]
fn commons_credit_is_fetched_only_after_playback_ends() {
    let (temp, _data, mut shell, article) = fixture(true);
    let server = Stub::start([(CREDIT_ROUTE.into(), Route::File(CREDIT.to_vec()))].into()).unwrap();
    cache_commons(&temp, &shell, &server);
    let id = start_commons(&mut shell, article);
    assert!(server.seen().is_empty());
    finish(&shell, id, Ok(()));
    assert_eq!(
        shell.next().0.playback.unwrap().state,
        PlaybackState::Ended { credit: None }
    );
    assert_eq!(
        shell.next().0.playback.unwrap().state,
        PlaybackState::Ended {
            credit: Some("The Shtooka Project · CC BY 2.0 FR".into())
        },
    );
    assert_eq!(server.seen(), [format!("GET {CREDIT_ROUTE} HTTP/1.1")]);
}

#[test]
fn a_failed_credit_leaves_the_ended_recording_without_credit() {
    let (temp, _data, mut shell, article) = fixture(true);
    let server = Stub::start(
        [(
            CREDIT_ROUTE.into(),
            Route::Status("503 Service Unavailable"),
        )]
        .into(),
    )
    .unwrap();
    cache_commons(&temp, &shell, &server);
    let id = start_commons(&mut shell, article);
    finish(&shell, id, Ok(()));
    assert_eq!(
        shell.next().0.playback.unwrap().state,
        PlaybackState::Ended { credit: None }
    );
    wait_until(WAIT, || !server.seen().is_empty());
    assert_eq!(
        marker(&mut shell).playback.unwrap().state,
        PlaybackState::Ended { credit: None }
    );
}

#[test]
fn failed_streams_have_the_correct_text_and_do_not_request_credit() {
    let (temp, _data, mut shell, article) = fixture(true);
    let server = Stub::start(Default::default()).unwrap();
    cache_commons(&temp, &shell, &server);
    for (failure, text) in [
        (PlaybackFailure::Network, RECORDING_NEEDS_NETWORK),
        (
            PlaybackFailure::Failed("bad stream".into()),
            PLAYBACK_FAILED,
        ),
    ] {
        let id = start_commons(&mut shell, article);
        finish(&shell, id, Err(failure));
        let (failed, effects) = shell.next();
        assert!(effects.is_empty());
        assert_eq!(
            failed.playback.unwrap().state,
            PlaybackState::Failed(text.to_owned())
        );
        shell.quiet(Event::Played { id, result: Ok(()) });
    }
    assert!(server.seen().is_empty());
}

#[test]
fn a_new_play_retires_the_fetch_and_ignores_late_results() {
    let (temp, _data, mut shell, article) = fixture(true);
    let server = Stub::start(
        [(
            "/stall.ogg".into(),
            Route::Stall(b"unfinished recording".to_vec()),
        )]
        .into(),
    )
    .unwrap();
    let (pending, _) = press(
        &mut shell,
        article,
        &format!("{}/stall.ogg", server.base_url()),
    );
    wait_until(WAIT, || !server.seen().is_empty());
    let (playing, effects) = press(&mut shell, article, LOCAL);
    assert!(
        matches!(effects.as_slice(), [Effect::Cancel(old), Effect::Play { id, .. }] if *old == pending.id && *id == playing.id)
    );
    shell
        .controller
        .dispatch
        .0
        .send(Message::AudioReady(
            pending.id,
            Outcome::Finished(Ok("/stale.ogg".into())),
        ))
        .unwrap();
    shell.quiet(Event::Played {
        id: pending.id,
        result: Err(PlaybackFailure::Cancelled),
    });
    assert_eq!(marker(&mut shell).playback, Some(playing));
    assert!(!temp.0.join("cache/recordings").exists());
}

#[test]
fn new_field_text_keeps_playback_while_the_same_result_is_shown() {
    let (_temp, _data, mut shell, article) = fixture(true);
    let before = marker(&mut shell);
    let (playing, _) = press(&mut shell, article, LOCAL);
    shell
        .controller
        .dispatch
        .0
        .send(Message::FieldEditedKeepingArticle("river".into()))
        .unwrap();
    let (after, effects) = shell.next();
    assert_eq!(after.field.text, "river");
    assert_ne!(after.article_id(), article);
    assert_eq!(after.shown, before.shown);
    assert_eq!(after.playback, Some(playing));
    assert!(effects.is_empty());
    shell.quiet(Event::PlayPressed {
        article,
        url: LOCAL.into(),
    });
}

#[test]
fn retrying_the_same_recording_ignores_the_previous_credit() {
    let (temp, _data, mut shell, article) = fixture(true);
    let server =
        Stub::start([(CREDIT_ROUTE.into(), Route::Stall(CREDIT.to_vec()))].into()).unwrap();
    cache_commons(&temp, &shell, &server);
    let old = start_commons(&mut shell, article);
    finish(&shell, old, Ok(()));
    assert_eq!(
        shell.next().0.playback.unwrap().state,
        PlaybackState::Ended { credit: None }
    );
    wait_until(WAIT, || server.seen().len() == 1);

    let (retry, effects) = press(&mut shell, article, COMMONS);
    assert_eq!(effects, [Effect::Cancel(old)]);
    assert_ne!(retry.id, old);
    ready(&mut shell, retry.id);
    finish(&shell, retry.id, Ok(()));
    let before = shell.next().0.playback.unwrap();
    assert_eq!(before.state, PlaybackState::Ended { credit: None });
    assert_eq!(before.id, retry.id);
    wait_until(WAIT, || server.seen().len() == 2);

    shell
        .controller
        .dispatch
        .0
        .send(Message::CreditReady(
            old,
            Outcome::Finished(Credit {
                author: Some("late credit".into()),
                license: None,
            }),
        ))
        .unwrap();
    assert_eq!(marker(&mut shell).playback, Some(before));
}

#[test]
fn a_new_article_retires_the_credit_and_rejects_a_stale_play() {
    let (temp, _data, mut shell, article) = fixture(true);
    let server =
        Stub::start([(CREDIT_ROUTE.into(), Route::Stall(CREDIT.to_vec()))].into()).unwrap();
    cache_commons(&temp, &shell, &server);
    let id = start_commons(&mut shell, article);
    finish(&shell, id, Ok(()));
    assert_eq!(
        shell.next().0.playback.unwrap().state,
        PlaybackState::Ended { credit: None }
    );
    wait_until(WAIT, || !server.seen().is_empty());
    shell.edit("river");
    let (view, effects) = shell.next();
    assert_eq!(effects, [Effect::Cancel(id)]);
    assert_ne!(view.article_id(), article);
    assert!(view.playback.is_none());
    shell.settled();
    shell
        .controller
        .dispatch
        .0
        .send(Message::CreditReady(
            id,
            Outcome::Finished(Credit {
                author: Some("late credit".into()),
                license: None,
            }),
        ))
        .unwrap();
    shell.quiet(Event::PlayPressed {
        article,
        url: LOCAL.into(),
    });
    assert!(marker(&mut shell).playback.is_none());
}
