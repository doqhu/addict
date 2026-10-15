//! The AT-SPI adapter against a fake Android Studio on a private bus: every
//! row of the matrix that is reachable on the bus.

mod common;
mod fake_bus;

use std::{
    ffi::OsStr,
    time::{Duration, Instant},
};

use addict_platform::{
    atspi::{ContextFailure, Locate},
    foreground::Source,
};
use fake_bus::{
    Address, FakeNode, app, editor, hung_registry, plain, selected, text_node, unselected, world,
};

const STUDIO: &str = "demo - Android Studio";
const LIMIT: Duration = Duration::from_secs(5);

/// A world with one Android Studio holding the given text objects.
fn studio(name: &str, texts: Vec<fake_bus::FakeText>) -> fake_bus::World {
    world(name, Address::Plain, editor(STUDIO, texts), &[0])
}

#[test]
fn the_selected_word_comes_with_its_sentence() {
    let content = "// The bank was closed. We sat on the bank of the river.";
    let world = studio("happy", vec![selected(content, "bank", 1)]);
    let context = world.read(LIMIT).expect("context");
    assert_eq!(context.word, "bank");
    let sentence = context.sentence.expect("sentence");
    assert_eq!(sentence.text, "We sat on the bank of the river.");
    // The second `bank`, not the first.
    assert_eq!(sentence.word, 14..18);
    assert_eq!(&sentence.text[sentence.word], "bank");
}

#[test]
fn a_line_break_ends_the_sentence() {
    let content = "a.\n// Lead. We sat on the bank.\nnext.";
    let world = studio("line-break", vec![selected(content, "bank", 0)]);
    let context = world.read(LIMIT).expect("context");
    assert_eq!(
        context.sentence.expect("sentence").text,
        "We sat on the bank."
    );
}

#[test]
fn offsets_after_a_character_outside_the_bmp_are_characters() {
    let content =
        "// \u{1f600}\u{1f600} Done. \u{1f600} We sat on the b\u{e4}nk \u{1f600} of the river.";
    let world = studio("non-bmp", vec![selected(content, "b\u{e4}nk", 0)]);
    let context = world.read(LIMIT).expect("context");
    assert_eq!(context.word, "b\u{e4}nk");
    let sentence = context.sentence.expect("sentence");
    assert_eq!(
        sentence.text,
        "\u{1f600} We sat on the b\u{e4}nk \u{1f600} of the river."
    );
    assert_eq!(&sentence.text[sentence.word], "b\u{e4}nk");
}

#[test]
fn a_selection_far_into_a_long_text_is_read_in_its_window() {
    let before = "Filler words. ".repeat(200);
    let content = format!("{before}\n// Lead. We sat on the bank of the river.\nAfter. {before}");
    let world = studio("long", vec![selected(&content, "bank", 0)]);
    let context = world.read(LIMIT).expect("context");
    assert_eq!(context.word, "bank");
    assert_eq!(
        context.sentence.expect("sentence").text,
        "We sat on the bank of the river."
    );
}

#[test]
fn the_sentence_is_never_asked_of_the_application() {
    // The fake gives a wrong sentence to `GetStringAtOffset`, as EXP-2 found.
    let world = studio(
        "own-sentence",
        vec![selected("// Lead. Sit on the bank.", "bank", 0)],
    );
    let context = world.read(LIMIT).expect("context");
    assert_eq!(context.sentence.expect("sentence").text, "Sit on the bank.");
}

#[test]
fn without_a_wider_text_the_word_comes_alone() {
    let content = "We sat on the bank.";
    let mut text = selected(content, "bank", 0);
    text.only_selected_range = true;
    let world = studio("no-sentence", vec![text]);
    let context = world.read(LIMIT).expect("context");
    assert_eq!(context.word, "bank");
    assert_eq!(context.sentence, None);
}

#[test]
fn the_name_of_the_application_is_matched_without_case() {
    for name in [
        "android studio",
        "Android Studio",
        "x - ANDROID STUDIO 2026",
    ] {
        let world = world(
            "name-case",
            Address::Plain,
            editor(name, vec![selected("Sit on the bank.", "bank", 0)]),
            &[0],
        );
        assert_eq!(world.read(LIMIT).expect("context").word, "bank", "{name}");
    }
}

#[test]
fn only_android_studio_is_read() {
    let mut nodes = editor("Reader", vec![selected("Read the other word.", "other", 0)]);
    let studio_nodes = editor(STUDIO, vec![selected("Sit on the bank.", "bank", 0)]);
    // Renumber the second editor's nodes after the first's.
    let shift = nodes.len();
    for mut node in studio_nodes {
        node.children = node.children.iter().map(|id| id + shift).collect();
        nodes.push(node);
    }
    let world = world("only-studio", Address::Plain, nodes, &[0, shift]);
    assert_eq!(world.read(LIMIT).expect("context").word, "bank");
}

#[test]
fn an_application_that_is_not_android_studio_in_front_reads_no_text() {
    let world = studio(
        "not-from-studio",
        vec![selected("Sit on the bank.", "bank", 0)],
    );
    for source in [Source::Other, Source::Unknown] {
        assert_eq!(
            world.read_from(source, LIMIT),
            Err(ContextFailure::NotFromStudio(source))
        );
    }
    // Not a single text object was asked, though the selection is there.
    assert_eq!(world.text_calls(), 0);
    assert_eq!(world.read(LIMIT).expect("context").word, "bank");
    assert!(world.text_calls() > 0);
}

#[test]
fn another_source_is_not_from_studio_and_not_unavailable_on_an_absent_bus() {
    let missing = std::env::temp_dir().join("addict-atspi-test-no-such-bus-gate");
    let locate = Locate::Address(format!("unix:path={}", missing.display()));
    let world = studio("gate-absent", vec![]);
    for source in [Source::Other, Source::Unknown] {
        assert_eq!(
            world.read_with(&locate, source, LIMIT),
            Err(ContextFailure::NotFromStudio(source))
        );
    }
    // The gate does not even look for the session's bus.
    assert_eq!(
        world.read_with(&Locate::Session, Source::Other, LIMIT),
        Err(ContextFailure::NotFromStudio(Source::Other))
    );
    // Android Studio in front on the same absent bus is what it was.
    assert!(matches!(
        world.read_with(&locate, Source::AndroidStudio, LIMIT),
        Err(ContextFailure::Unavailable(_))
    ));
}

#[test]
fn a_bus_that_is_not_there_is_unavailable() {
    let missing = std::env::temp_dir().join("addict-atspi-test-no-such-bus");
    let locate = Locate::Address(format!("unix:path={}", missing.display()));
    let world = studio("absent", vec![]);
    match world.read_at(&locate, LIMIT) {
        Err(ContextFailure::Unavailable(detail)) => assert!(!detail.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_registry_without_android_studio_is_app_not_found() {
    let nodes = editor(
        "org.example.Reader",
        vec![selected("Sit on the bank.", "bank", 0)],
    );
    let world = world("not-found", Address::Plain, nodes, &[0]);
    assert_eq!(world.read(LIMIT), Err(ContextFailure::AppNotFound));
    let empty = world_without_applications();
    assert_eq!(empty.read(LIMIT), Err(ContextFailure::AppNotFound));
}

fn world_without_applications() -> fake_bus::World {
    world("empty-registry", Address::Plain, vec![], &[])
}

#[test]
fn text_objects_without_a_selection_are_no_selection() {
    let world = studio(
        "no-selection",
        vec![unselected("Sit on the bank."), unselected("Another line.")],
    );
    assert_eq!(world.read(LIMIT), Err(ContextFailure::NoSelection));
}

#[test]
fn an_empty_selection_is_no_selection() {
    let mut text = unselected("Sit on the bank.");
    text.selections = vec![(4, 4)];
    let world = studio("empty-selection", vec![text]);
    assert_eq!(world.read(LIMIT), Err(ContextFailure::NoSelection));
}

#[test]
fn a_selection_of_blanks_is_no_selection() {
    let mut text = unselected("Sit   on the bank.");
    text.selections = vec![(3, 6)];
    let world = studio("blank-selection", vec![text]);
    assert_eq!(world.read(LIMIT), Err(ContextFailure::NoSelection));
}

#[test]
fn several_selections_are_ambiguous() {
    let world = studio(
        "ambiguous",
        vec![
            selected("Sit on the bank.", "bank", 0),
            selected("Read the other word.", "other", 0),
            unselected("Nothing here."),
        ],
    );
    assert_eq!(world.read(LIMIT), Err(ContextFailure::Ambiguous(2)));
}

#[test]
fn a_selection_that_cannot_be_read_is_a_failure() {
    let mut text = selected("Sit on the bank.", "bank", 0);
    text.broken_selection = Some(false);
    let world = studio("broken-selection", vec![text]);
    match world.read(LIMIT) {
        Err(ContextFailure::Failed(detail)) => assert!(detail.contains("selection refused")),
        other => panic!("{other:?}"),
    }
}

/// How long a bounded read may overrun its limit in these tests.
const SLACK: Duration = Duration::from_secs(2);

fn assert_times_out(world: &fake_bus::World, limit: Duration) {
    let started = Instant::now();
    assert_eq!(world.read(limit), Err(ContextFailure::TimedOut));
    let elapsed = started.elapsed();
    assert!(elapsed < limit + SLACK, "{elapsed:?}");
}

#[test]
fn a_registry_that_is_silent_times_out_at_the_deadline() {
    let world = hung_registry("hung-registry");
    let limit = Duration::from_millis(600);
    let started = Instant::now();
    assert_times_out(&world, limit);
    assert!(started.elapsed() >= limit);
}

#[test]
fn an_application_that_is_silent_times_out() {
    let mut nodes = editor(STUDIO, vec![selected("Sit on the bank.", "bank", 0)]);
    nodes[0].hang = true;
    let world = world("hung-root", Address::Plain, nodes, &[0]);
    assert_times_out(&world, Duration::from_millis(700));
}

#[test]
fn an_application_that_does_not_give_its_name_times_out_when_nothing_matches() {
    let mut nodes = editor("org.example.Reader", vec![]);
    nodes[0].silent_name = true;
    let world = world("silent-name", Address::Plain, nodes, &[0]);
    assert_times_out(&world, LIMIT);
}

#[test]
fn a_selection_that_never_answers_times_out() {
    let mut text = selected("Sit on the bank.", "bank", 0);
    text.broken_selection = Some(true);
    let world = studio("silent-selection", vec![text]);
    assert_times_out(&world, LIMIT);
}

#[test]
fn a_node_that_never_answers_is_skipped_when_the_selection_is_elsewhere() {
    let mut nodes = editor(STUDIO, vec![selected("Sit on the bank.", "bank", 0)]);
    // Another child of the frame that never answers `GetInterfaces`.
    let hung = FakeNode {
        hang: true,
        ..plain(vec![])
    };
    nodes.push(hung);
    let last = nodes.len() - 1;
    nodes[1].children.insert(0, last);
    let world = world("hung-node", Address::Plain, nodes, &[0]);
    assert_eq!(world.read(LIMIT).expect("context").word, "bank");
}

#[test]
fn a_node_that_never_answers_is_not_proof_of_no_selection() {
    let mut nodes = editor(STUDIO, vec![unselected("Sit on the bank.")]);
    let hung = FakeNode {
        hang: true,
        ..plain(vec![])
    };
    nodes.push(hung);
    let last = nodes.len() - 1;
    nodes[1].children.push(last);
    let world = world("hung-node-none", Address::Plain, nodes, &[0]);
    assert_eq!(world.read(LIMIT), Err(ContextFailure::TimedOut));
}

#[test]
fn the_tree_is_walked_below_the_frame() {
    // application → frame → pane → text, three levels down.
    let nodes = vec![
        app(STUDIO, vec![1]),
        plain(vec![2]),
        plain(vec![3]),
        text_node(selected("Sit on the bank.", "bank", 0)),
    ];
    let world = world("deep", Address::Plain, nodes, &[0]);
    assert_eq!(world.read(LIMIT).expect("context").word, "bank");
}

// ---------------------------------------------- the session's own bus

/// Set only on the child of a session test.
const CHILD: &str = "ADDICT_TEST_ATSPI_CHILD";
const CHILD_TIMEOUT: Duration = Duration::from_secs(40);

fn is_child() -> bool {
    std::env::var_os(CHILD).is_some()
}

fn child_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn the_session_bus_names_the_accessibility_bus() {
    const TEST: &str = "the_session_bus_names_the_accessibility_bus";
    if is_child() {
        let context = child_runtime()
            .block_on(addict_platform::atspi::read(
                &Locate::Session,
                Source::AndroidStudio,
                LIMIT,
            ))
            .expect("context");
        assert_eq!(context.word, "bank");
        return;
    }
    let content = "Sit on the bank.";
    let world = world(
        "session-launcher",
        Address::Launcher,
        editor(STUDIO, vec![selected(content, "bank", 0)]),
        &[0],
    );
    common::run_child(
        TEST,
        CHILD_TIMEOUT,
        &[
            (CHILD, OsStr::new("1")),
            ("DBUS_SESSION_BUS_ADDRESS", OsStr::new(&world.bus.address)),
            ("AT_SPI_BUS_ADDRESS", OsStr::new("")),
        ],
    );
}

#[test]
fn at_spi_bus_address_wins_over_the_session_bus() {
    const TEST: &str = "at_spi_bus_address_wins_over_the_session_bus";
    if is_child() {
        let context = child_runtime()
            .block_on(addict_platform::atspi::read(
                &Locate::Session,
                Source::AndroidStudio,
                LIMIT,
            ))
            .expect("context");
        assert_eq!(context.word, "bank");
        return;
    }
    let world = studio("session-env", vec![selected("Sit on the bank.", "bank", 0)]);
    common::run_child(
        TEST,
        CHILD_TIMEOUT,
        &[
            (CHILD, OsStr::new("1")),
            // Nothing is there to be found through the session bus.
            (
                "DBUS_SESSION_BUS_ADDRESS",
                OsStr::new("unix:path=/nonexistent"),
            ),
            ("AT_SPI_BUS_ADDRESS", OsStr::new(&world.bus.address)),
        ],
    );
}

#[test]
fn a_session_without_the_launcher_or_the_variable_is_unavailable() {
    const TEST: &str = "a_session_without_the_launcher_or_the_variable_is_unavailable";
    if is_child() {
        let failure = child_runtime()
            .block_on(addict_platform::atspi::read(
                &Locate::Session,
                Source::AndroidStudio,
                LIMIT,
            ))
            .expect_err("failure");
        assert!(
            matches!(failure, ContextFailure::Unavailable(_)),
            "{failure:?}"
        );
        return;
    }
    // A session bus on which nothing owns `org.a11y.Bus`.
    let world = studio(
        "session-bare",
        vec![selected("Sit on the bank.", "bank", 0)],
    );
    common::run_child(
        TEST,
        CHILD_TIMEOUT,
        &[
            (CHILD, OsStr::new("1")),
            ("DBUS_SESSION_BUS_ADDRESS", OsStr::new(&world.bus.address)),
            ("AT_SPI_BUS_ADDRESS", OsStr::new("")),
        ],
    );
}
