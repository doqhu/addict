//! The foreground listener against fake applications on a private bus: every
//! row of the matrix that is reachable on the bus.

mod fake_bus;

use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use addict_platform::{
    atspi::Locate,
    foreground::{Foreground, Source, watch_at},
};
use fake_bus::{Address, World, status_world, world};

const STUDIO: &str = "demo - Android Studio";
const WAIT: Duration = Duration::from_secs(5);
/// How long nothing is expected to happen.
const QUIET: Duration = Duration::from_millis(300);

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
    let (foreground, receiver) = listen(&world);
    subscribed(&world);
    // A client that serves no root: its name cannot be had.
    let mute = world.open_mute_client();
    mute.window_activated();
    assert_eq!(
        receiver.recv_timeout(QUIET),
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
