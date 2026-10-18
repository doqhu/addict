//! The foreground listener against fake applications on a private bus: every
//! row of the matrix that is reachable on the bus.

mod fake_bus;

use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use addict_platform::{
    atspi::Locate,
    foreground::{Accessibility, Foreground, Source, watch_at},
};
use fake_bus::{Address, World, editor, selected, status_world, world};

const STUDIO: &str = "demo - Android Studio";
const WAIT: Duration = Duration::from_secs(5);
/// How long nothing is expected to happen.
const QUIET: Duration = Duration::from_millis(300);
/// What a probe may take when Android Studio is silent: the adapter's
/// limit of one call, 500 ms, and some slack.
const PROBE_BOUND: Duration = Duration::from_millis(1500);

/// A bus with the accessibility registry and the status, accessibility on.
fn a11y(name: &str) -> World {
    world(name, Address::Launcher, Vec::new(), &[])
}

fn listen(world: &World) -> (Foreground, Receiver<Source>) {
    listen_with(Some(&world.bus.address), world)
}

fn listen_with(status: Option<&str>, world: &World) -> (Foreground, Receiver<Source>) {
    let (sender, receiver) = mpsc::channel();
    let foreground = watch_at(
        status,
        Locate::Address(world.bus.address.clone()),
        move |source| {
            let _ = sender.send(source);
        },
    );
    (foreground, receiver)
}

/// Waits until the listener has told the registry what it wants, which it
/// does after its match rules are in place.
fn subscribed(world: &World) {
    let started = Instant::now();
    while world.registered_events().len() < 3 {
        assert!(started.elapsed() < WAIT, "the listener did not subscribe");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn next(receiver: &Receiver<Source>) -> Source {
    receiver.recv_timeout(WAIT).expect("a change of the source")
}

fn quiet(receiver: &Receiver<Source>) {
    assert_eq!(
        receiver.recv_timeout(QUIET),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
}

#[test]
fn a_listener_that_has_seen_nothing_is_unknown() {
    let world = a11y("fg-nothing");
    let (foreground, receiver) = listen(&world);
    subscribed(&world);
    assert_eq!(foreground.source(), Source::Unknown);
    quiet(&receiver);
}

#[test]
fn the_listener_asks_for_the_three_events() {
    let world = a11y("fg-registered");
    let (_foreground, _receiver) = listen(&world);
    subscribed(&world);
    assert_eq!(
        world.registered_events(),
        [
            "window:activate",
            "window:deactivate",
            "object:state-changed:active"
        ]
    );
}

#[test]
fn android_studio_activated_is_android_studio() {
    let world = a11y("fg-studio");
    let studio = world.open_app(STUDIO);
    let (foreground, receiver) = listen(&world);
    subscribed(&world);
    studio.window_activated();
    assert_eq!(next(&receiver), Source::AndroidStudio);
    assert_eq!(foreground.source(), Source::AndroidStudio);
}

#[test]
fn another_application_activated_after_studio_is_other() {
    let world = a11y("fg-other");
    let studio = world.open_app(STUDIO);
    let foliate = world.open_app("foliate");
    let (foreground, receiver) = listen(&world);
    subscribed(&world);
    studio.window_activated();
    assert_eq!(next(&receiver), Source::AndroidStudio);
    foliate.window_activated();
    // Studio is not told that it lost the focus: the next activation is
    // the whole news.
    assert_eq!(next(&receiver), Source::Unknown);
    assert_eq!(next(&receiver), Source::Other);
    assert_eq!(foreground.source(), Source::Other);
    // And back.
    studio.window_activated();
    assert_eq!(next(&receiver), Source::Unknown);
    assert_eq!(next(&receiver), Source::AndroidStudio);
}

#[test]
fn the_active_application_deactivated_leaves_nothing_active() {
    let world = a11y("fg-deactivated");
    let studio = world.open_app(STUDIO);
    let (foreground, receiver) = listen(&world);
    subscribed(&world);
    studio.window_activated();
    assert_eq!(next(&receiver), Source::AndroidStudio);
    studio.window_deactivated();
    assert_eq!(next(&receiver), Source::Unknown);
    assert_eq!(foreground.source(), Source::Unknown);
}

#[test]
fn deactivating_an_application_that_is_not_active_changes_nothing() {
    let world = a11y("fg-deactivated-other");
    let studio = world.open_app(STUDIO);
    let foliate = world.open_app("foliate");
    let (foreground, receiver) = listen(&world);
    subscribed(&world);
    studio.window_activated();
    assert_eq!(next(&receiver), Source::AndroidStudio);
    foliate.window_deactivated();
    quiet(&receiver);
    assert_eq!(foreground.source(), Source::AndroidStudio);
}

#[test]
fn the_active_state_changing_is_heard_like_a_window_event() {
    let world = a11y("fg-state");
    let studio = world.open_app(STUDIO);
    let (foreground, receiver) = listen(&world);
    subscribed(&world);
    studio.state_changed("active", true);
    assert_eq!(next(&receiver), Source::AndroidStudio);
    assert_eq!(foreground.source(), Source::AndroidStudio);
    studio.state_changed("active", false);
    assert_eq!(next(&receiver), Source::Unknown);
}

#[test]
fn another_state_is_not_an_activation() {
    let world = a11y("fg-other-state");
    let studio = world.open_app(STUDIO);
    let (foreground, receiver) = listen(&world);
    subscribed(&world);
    studio.state_changed("focused", true);
    quiet(&receiver);
    assert_eq!(foreground.source(), Source::Unknown);
}

#[test]
fn both_signals_of_one_activation_are_one_activation() {
    let world = a11y("fg-both");
    let studio = world.open_app(STUDIO);
    let (_foreground, receiver) = listen(&world);
    subscribed(&world);
    studio.window_activated();
    studio.state_changed("active", true);
    assert_eq!(next(&receiver), Source::AndroidStudio);
    quiet(&receiver);
}

#[test]
fn an_application_without_a_name_is_unknown() {
    let world = a11y("fg-nameless");
    let studio = world.open_app(STUDIO);
    let (foreground, receiver) = listen(&world);
    subscribed(&world);
    // Studio first, so that the activation of the client is a change.
    studio.window_activated();
    assert_eq!(next(&receiver), Source::AndroidStudio);
    // A client that serves no root: its name cannot be had.
    let mute = world.open_mute_client();
    mute.window_activated();
    // Its activation was heard, and its name lookup changes nothing: the
    // wait outlasts the lookup, which ends at the adapter's call limit of
    // 500 ms.
    assert_eq!(next(&receiver), Source::Unknown);
    let lookup = Duration::from_millis(800);
    assert_eq!(
        receiver.recv_timeout(lookup),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
    assert_eq!(foreground.source(), Source::Unknown);
}

#[test]
fn with_accessibility_off_nothing_is_held_until_it_turns_on() {
    // The status is on one bus and the accessibility bus is another.
    let status = status_world("fg-status", false);
    let a11y = a11y("fg-off");
    let studio = a11y.open_app(STUDIO);
    let before = a11y.client_count();
    let (foreground, receiver) = listen_with(Some(&status.bus.address), &a11y);

    // Off: no connection to the accessibility bus, nothing asked of its
    // registry, and what happens there is not seen.
    std::thread::sleep(QUIET);
    assert_eq!(a11y.client_count(), before);
    assert!(a11y.registered_events().is_empty());
    studio.window_activated();
    quiet(&receiver);
    assert_eq!(foreground.source(), Source::Unknown);

    // On: it subscribes and sees the activations from then on.
    status.set_enabled(true);
    subscribed(&a11y);
    assert_eq!(a11y.client_count(), before + 1);
    studio.window_activated();
    assert_eq!(next(&receiver), Source::AndroidStudio);

    // Off again: what it knew and its connection go.
    status.set_enabled(false);
    assert_eq!(next(&receiver), Source::Unknown);
    let started = Instant::now();
    while a11y.client_count() != before {
        assert!(started.elapsed() < WAIT, "the connection was kept");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(foreground.source(), Source::Unknown);
}

/// Waits until the listener says that accessibility is `wanted`.
fn accessibility_is(foreground: &Foreground, wanted: Accessibility) {
    let started = Instant::now();
    while foreground.accessibility() != wanted {
        assert!(
            started.elapsed() < WAIT,
            "accessibility stayed {:?}, not {wanted:?}",
            foreground.accessibility()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_accessibility_follows_is_enabled() {
    let status = status_world("fg-a11y-status", false);
    let a11y = a11y("fg-a11y");
    let (foreground, _receiver) = listen_with(Some(&status.bus.address), &a11y);
    accessibility_is(&foreground, Accessibility::Off);
    status.set_enabled(true);
    accessibility_is(&foreground, Accessibility::On);
    subscribed(&a11y);
    status.set_enabled(false);
    accessibility_is(&foreground, Accessibility::Off);
}

#[test]
fn accessibility_on_at_the_start_is_on() {
    let world = a11y("fg-a11y-on");
    let (foreground, _receiver) = listen(&world);
    accessibility_is(&foreground, Accessibility::On);
}

#[test]
fn a_status_that_cannot_be_read_is_unknown() {
    // Nothing owns `org.a11y.Bus` on this bus: `IsEnabled` is not answered.
    let world = world("fg-a11y-none", Address::Plain, Vec::new(), &[]);
    let (foreground, receiver) = listen(&world);
    // The worker gives up, and its callback goes with it.
    assert_eq!(
        receiver.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
    assert_eq!(foreground.accessibility(), Accessibility::Unknown);
    assert_eq!(foreground.source(), Source::Unknown);

    // A status read once and then lost is unknown again: the bus of the
    // status goes, with its daemon, and the worker gives up.
    let status = status_world("fg-a11y-lost-status", true);
    let a11y = a11y("fg-a11y-lost");
    let (foreground, _receiver) = listen_with(Some(&status.bus.address), &a11y);
    accessibility_is(&foreground, Accessibility::On);
    subscribed(&a11y);
    drop(status);
    accessibility_is(&foreground, Accessibility::Unknown);
}

#[test]
fn dropping_the_listener_stops_it_and_closes_its_connection() {
    let world = a11y("fg-drop");
    let before = world.client_count();
    let (foreground, _receiver) = listen(&world);
    subscribed(&world);
    assert!(world.client_count() > before);
    let started = Instant::now();
    drop(foreground);
    assert!(started.elapsed() < Duration::from_secs(2));
    let started = Instant::now();
    while world.client_count() != before {
        assert!(started.elapsed() < WAIT, "the connection was kept");
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ------------------------------------------------------------- the probe

/// The answer of one probe, and how long it took; an unanswered probe is
/// a failure of the test.
fn probe(foreground: &Foreground) -> (bool, Duration) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let started = Instant::now();
    let answer = runtime
        .block_on(async { tokio::time::timeout(WAIT, foreground.probe()).await })
        .expect("the probe was answered")
        .expect("the probe was not dropped");
    (answer, started.elapsed())
}

/// Android Studio (0) with one frame (1) that holds a selected text (2).
fn studio_world(name: &str, application: &'static str) -> World {
    let nodes = editor(
        application,
        vec![selected("// the bank of the river.", "bank", 0)],
    );
    world(name, Address::Launcher, nodes, &[0])
}

#[test]
fn an_active_frame_of_studio_is_in_front_without_any_event() {
    let world = studio_world("fg-probe-active", STUDIO);
    world.set_active(1, true);
    let (foreground, receiver) = listen(&world);
    subscribed(&world);
    assert!(probe(&foreground).0);
    // Asked, not heard: the observed source is unchanged, and no text was
    // read.
    quiet(&receiver);
    assert_eq!(foreground.source(), Source::Unknown);
    assert_eq!(world.text_calls(), 0);
}

#[test]
fn studio_in_the_back_is_not_in_front() {
    let world = studio_world("fg-probe-back", STUDIO);
    let (foreground, _receiver) = listen(&world);
    subscribed(&world);
    assert!(!probe(&foreground).0);
    // It follows the state at each press.
    world.set_active(1, true);
    assert!(probe(&foreground).0);
    world.set_active(1, false);
    assert!(!probe(&foreground).0);
    assert_eq!(world.text_calls(), 0);
}

#[test]
fn without_studio_an_active_frame_is_not_studio() {
    let world = studio_world("fg-probe-absent", "foliate");
    world.set_active(1, true);
    let (foreground, _receiver) = listen(&world);
    subscribed(&world);
    assert!(!probe(&foreground).0);
    assert_eq!(world.text_calls(), 0);
}

#[test]
fn with_accessibility_off_the_probe_is_answered_false_at_once() {
    let status = status_world("fg-probe-status", false);
    let a11y = studio_world("fg-probe-off", STUDIO);
    a11y.set_active(1, true);
    let (foreground, _receiver) = listen_with(Some(&status.bus.address), &a11y);
    accessibility_is(&foreground, Accessibility::Off);
    let (active, took) = probe(&foreground);
    assert!(!active);
    assert!(took < QUIET, "the probe took {took:?}");
    // On: the same frame is asked on the connection.
    status.set_enabled(true);
    subscribed(&a11y);
    assert!(probe(&foreground).0);
}

#[test]
fn without_a_status_the_probe_is_answered_false() {
    // Nothing owns `org.a11y.Bus`: the worker gives up.
    let world = world("fg-probe-none", Address::Plain, Vec::new(), &[]);
    let (foreground, receiver) = listen(&world);
    assert_eq!(
        receiver.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
    let (active, took) = probe(&foreground);
    assert!(!active);
    assert!(took < QUIET, "the probe took {took:?}");
}

#[test]
fn a_silent_studio_is_not_in_front_within_the_limit() {
    let mut nodes = editor(STUDIO, vec![selected("// the bank.", "bank", 0)]);
    nodes[1].active = true;
    nodes[1].silent_state = true;
    let world = world("fg-probe-silent", Address::Launcher, nodes, &[0]);
    let (foreground, _receiver) = listen(&world);
    subscribed(&world);
    let (active, took) = probe(&foreground);
    assert!(!active);
    assert!(took < PROBE_BOUND, "the probe took {took:?}");
    assert_eq!(world.text_calls(), 0);
}

#[test]
fn a_name_given_while_studio_starts_is_asked_again() {
    let world = studio_world("fg-probe-renamed", "loading");
    world.set_active(1, true);
    let (foreground, _receiver) = listen(&world);
    subscribed(&world);
    assert!(!probe(&foreground).0);
    world.set_name(0, STUDIO);
    assert!(probe(&foreground).0);
    assert_eq!(world.text_calls(), 0);
}
