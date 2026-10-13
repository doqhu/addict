//! The run of EXP-5 against a server on a loopback port: every row of the
//! matrix of the story, and that no output holds the key.

mod common;

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use addict_core::translation::Refusal;
use addict_deepl::{DeepL, stub};
use addict_exp5::{Failure, Loaded, When, explain, load_set, parse_set, run, sha256_hex};
use serde_json::{Value, json};

use common::{KEY, Reply, Script, Seen, Server, TempDir};

/// Longer than any answer of the loopback server takes.
const LIMIT: Duration = Duration::from_secs(5);

/// The limit of the tests that wait for a silent server.
const SHORT: Duration = Duration::from_millis(300);

const SET: &str = r#"{"items":[
    {"id":1,"word":"bank","sentence":"I sat on the bank of the river.","accepted":["берег"]},
    {"id":2,"word":"mean","sentence":"What does this word mean?","accepted":["значить","означать"]}
]}"#;

const LANGUAGES: &str = r#"[
    {"lang":"de","usable_as_source":true,"usable_as_target":true},
    {"lang":"en","usable_as_source":true,"usable_as_target":false},
    {"lang":"en-US","usable_as_source":false,"usable_as_target":true},
    {"lang":"ru","usable_as_source":true,"usable_as_target":true}
]"#;

const LANGUAGES_GET: &str = "GET /v3/languages?resource=translate_text";
const USAGE_GET: &str = "GET /v2/usage";
const TRANSLATE_POST: &str = "POST /v2/translate";

fn set() -> Loaded {
    parse_set(SET.as_bytes()).expect("the set of the tests")
}

fn usage_body(count: u64) -> String {
    format!(r#"{{"character_count":{count},"character_limit":500000}}"#)
}

/// A script that offers EN→RU, counts 16 characters between its two usage
/// readings and leaves each translate request to `translate`.
fn script(translate: impl Fn(&Seen) -> Reply + Send + 'static) -> Script {
    scripted(LANGUAGES, translate)
}

fn scripted(
    languages: &'static str,
    translate: impl Fn(&Seen) -> Reply + Send + 'static,
) -> Script {
    Box::new(move |before, seen| match seen.target.as_str() {
        LANGUAGES_GET => Reply::ok(languages),
        USAGE_GET if before < 2 => Reply::ok(&usage_body(1000)),
        USAGE_GET => Reply::ok(&usage_body(1016)),
        TRANSLATE_POST => translate(seen),
        other => panic!("an unexpected request: {other}"),
    })
}

/// The Russian of a good day: the word alone gets its commonest meaning,
/// the word in its sentence the meaning of the sentence.
fn good(seen: &Seen) -> Reply {
    let text = match (seen.text.as_deref(), seen.context.is_some()) {
        (Some("bank"), false) => "банк",
        (Some("bank"), true) => "Берег.",
        (Some("mean"), false) => "среднее",
        (Some("mean"), true) => "означает",
        other => panic!("an unexpected translate request: {other:?}"),
    };
    Reply::translation(text)
}

/// A server with `script`, a client with a usable key file that asks it,
/// and where the results go.
fn asking(tag: &str, script: Script) -> (TempDir, Server, DeepL, PathBuf) {
    let dir = TempDir::new(tag);
    let server = Server::start(script);
    let client = DeepL::with_base_url(dir.key_file(&format!("{KEY}\n"), 0o600), server.base_url());
    let results = dir.path().join("out").join("results.jsonl");
    (dir, server, client, results)
}

fn lines(results: &Path) -> Vec<Value> {
    std::fs::read_to_string(results)
        .expect("the results file")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .collect()
}

fn kinds(lines: &[Value]) -> Vec<&str> {
    lines
        .iter()
        .map(|line| line["kind"].as_str().expect("a kind"))
        .collect()
}

#[tokio::test]
async fn a_full_run_writes_the_run_the_languages_the_usage_and_two_lines_per_item() {
    let (_dir, server, client, results) = asking("full", script(good));
    let summary = run(&client, &set(), &results, LIMIT).await.expect("a run");
    assert_eq!(summary.items, 2);
    assert_eq!(summary.refused, 0);
    assert_eq!(summary.matched_without_context, 0);
    assert_eq!(summary.matched_with_context, 1);
    assert_eq!(summary.before.character_count, 1000);
    assert_eq!(summary.after.character_count, 1016);

    // One request at a time, in this order, and none twice.
    assert_eq!(
        server.targets(),
        [
            LANGUAGES_GET,
            USAGE_GET,
            TRANSLATE_POST,
            TRANSLATE_POST,
            TRANSLATE_POST,
            TRANSLATE_POST,
            USAGE_GET,
        ]
    );
    let asked: Vec<_> = server
        .seen()
        .into_iter()
        .filter(|seen| seen.target == TRANSLATE_POST)
        .map(|seen| (seen.text, seen.context))
        .collect();
    let some = |text: &str| Some(text.to_owned());
    assert_eq!(
        asked,
        [
            (some("bank"), None),
            (some("bank"), some("I sat on the bank of the river.")),
            (some("mean"), None),
            (some("mean"), some("What does this word mean?")),
        ]
    );

    let lines = lines(&results);
    assert_eq!(
        kinds(&lines),
        [
            "run",
            "languages",
            "usage",
            "item",
            "item",
            "item",
            "item",
            "usage"
        ]
    );
    let run_line = &lines[0];
    assert_eq!(run_line["schema"], 1);
    assert_eq!(run_line["items"], 2);
    assert_eq!(run_line["limit_ms"], 5000);
    assert_eq!(run_line["set_sha256"], sha256_hex(SET.as_bytes()));
    let date = run_line["date"].as_str().expect("a date");
    assert!(date.len() == 20 && date.ends_with('Z'), "{date}");

    assert_eq!(lines[1]["en_as_source"], true);
    assert_eq!(lines[1]["ru_as_target"], true);
    assert_eq!(lines[1]["refusal"], Value::Null);
    assert_eq!(
        lines[1]["languages"].as_array().expect("languages").len(),
        4
    );
    assert_eq!(
        lines[1]["languages"][1],
        json!({"lang": "en", "usable_as_source": true, "usable_as_target": false})
    );
    for (line, when, count) in [(&lines[2], "before", 1000), (&lines[7], "after", 1016)] {
        assert_eq!(line["when"], when);
        assert_eq!(line["character_count"], count);
        assert_eq!(line["character_limit"], 500_000);
        assert_eq!(line["refusal"], Value::Null);
    }

    // The answer is kept as it came, beside its verdict.
    let expected = [
        (1, "bank", false, "банк", "mismatch"),
        (1, "bank", true, "Берег.", "match"),
        (2, "mean", false, "среднее", "mismatch"),
        (2, "mean", true, "означает", "mismatch"),
    ];
    for (line, (item, word, context, answer, verdict)) in lines[3..7].iter().zip(expected) {
        assert_eq!(line["item"], item);
        assert_eq!(line["word"], word);
        assert_eq!(line["context"], context);
        assert_eq!(line["answer"], answer);
        assert_eq!(line["verdict"], verdict);
        assert_eq!(line["refusal"], Value::Null);
        assert!(line["delay_ms"].is_u64(), "{line}");
        assert!(
            line["sentence"]
                .as_str()
                .expect("a sentence")
                .contains(word)
        );
        assert!(!line["accepted"].as_array().expect("accepted").is_empty());
    }
}

#[tokio::test]
async fn without_en_as_source_or_ru_as_target_no_translation_is_asked() {
    for (languages, en, ru) in [
        (
            r#"[{"lang":"en","usable_as_source":true,"usable_as_target":true},
                {"lang":"ru","usable_as_source":true,"usable_as_target":false}]"#,
            true,
            false,
        ),
        (
            r#"[{"lang":"en-US","usable_as_source":true,"usable_as_target":true},
                {"lang":"ru","usable_as_source":true,"usable_as_target":true}]"#,
            false,
            true,
        ),
        ("[]", false, false),
    ] {
        let (_dir, server, client, results) = asking("not-offered", scripted(languages, good));
        let failure = run(&client, &set(), &results, LIMIT)
            .await
            .expect_err("EN→RU is not offered");
        assert!(
            matches!(failure, Failure::NotOffered { en_as_source, ru_as_target }
                if en_as_source == en && ru_as_target == ru),
            "{failure:?}"
        );
        assert_eq!(server.targets(), [LANGUAGES_GET, USAGE_GET]);
        let lines = lines(&results);
        assert_eq!(kinds(&lines), ["run", "languages", "usage", "incomplete"]);
        assert_eq!(lines[1]["en_as_source"], en);
        assert_eq!(lines[1]["ru_as_target"], ru);
        assert_eq!(lines[2]["character_count"], 1000);
        assert_eq!(lines[3]["reason"], "en_ru_not_offered");
        assert!(explain(&failure, None).contains("EN→RU"));
    }
}

#[tokio::test]
async fn without_a_usable_key_nothing_is_sent_and_the_message_names_the_key_file() {
    let (dir, server, client, results) = asking("no-key", script(good));
    for (content, mode) in [(None, 0), (Some(KEY), 0o644), (Some(""), 0o600)] {
        let _ = std::fs::remove_file(dir.key_path());
        let _ = std::fs::remove_file(&results);
        if let Some(content) = content {
            dir.key_file(content, mode);
        }
        let failure = run(&client, &set(), &results, LIMIT)
            .await
            .expect_err("no key");
        assert!(
            matches!(failure, Failure::Languages(Refusal::Key)),
            "{failure:?}"
        );
        let message = explain(&failure, Some(&dir.key_path()));
        assert!(
            message.contains(dir.key_path().to_str().expect("a path")),
            "{message}"
        );
        assert!(!message.contains(KEY), "{message}");
        // The refusal is recorded in the file that was begun.
        let lines = lines(&results);
        assert_eq!(kinds(&lines), ["run", "languages", "incomplete"]);
        assert_eq!(lines[1]["refusal"], "key");
        assert_eq!(lines[1]["en_as_source"], Value::Null);
        assert_eq!(lines[2]["reason"], "languages_key");
    }
    assert!(server.seen().is_empty());
}

#[tokio::test]
async fn a_rejected_key_stops_the_run_at_the_refused_request() {
    // The stub of the client: 403 to whatever is asked.
    let dir = TempDir::new("rejected");
    let server = stub::Stub::start(stub::Reply::answer(403, "{}")).expect("stub");
    let client = DeepL::with_base_url(dir.key_file(KEY, 0o600), server.base_url());
    let results = dir.path().join("results.jsonl");
    let failure = run(&client, &set(), &results, LIMIT)
        .await
        .expect_err("a rejected key");
    assert!(
        matches!(failure, Failure::Languages(Refusal::RejectedKey)),
        "{failure:?}"
    );
    assert_eq!(server.seen().len(), 1);
    let message = explain(&failure, Some(&dir.key_path()));
    assert!(message.contains("deepl.key"), "{message}");
    assert!(!message.contains(KEY), "{message}");
    let lines = lines(&results);
    assert_eq!(kinds(&lines), ["run", "languages", "incomplete"]);
    assert_eq!(lines[1]["refusal"], "rejected_key");

    // And in the middle of a run: nothing is sent after the refused request.
    let (dir, server, client, later) = asking(
        "rejected-later",
        script(|seen| match seen.context {
            None => Reply::translation("банк"),
            Some(_) => Reply::status(403),
        }),
    );
    let failure = run(&client, &set(), &later, LIMIT)
        .await
        .expect_err("a rejected key");
    assert!(
        matches!(
            failure,
            Failure::Item {
                id: 1,
                refusal: Refusal::RejectedKey
            }
        ),
        "{failure:?}"
    );
    assert_eq!(server.targets().len(), 4);
    assert_eq!(server.targets()[3], TRANSLATE_POST);
    let message = explain(&failure, Some(&dir.key_path()));
    assert!(message.contains("deepl.key"), "{message}");
    assert_eq!(
        kinds(&crate::lines(&later)),
        ["run", "languages", "usage", "item", "item", "incomplete"]
    );
}

#[tokio::test]
async fn a_used_up_quota_stops_the_run_and_keeps_the_lines_so_far() {
    let (_dir, server, client, results) = asking(
        "quota",
        script(|seen| match seen.text.as_deref() {
            Some("bank") => good(seen),
            _ => Reply::status(456),
        }),
    );
    let failure = run(&client, &set(), &results, LIMIT)
        .await
        .expect_err("the quota");
    assert!(
        matches!(
            failure,
            Failure::Item {
                id: 2,
                refusal: Refusal::Quota
            }
        ),
        "{failure:?}"
    );
    // No retry, and nothing after the refusal: not even the usage.
    assert_eq!(
        server.targets(),
        [
            LANGUAGES_GET,
            USAGE_GET,
            TRANSLATE_POST,
            TRANSLATE_POST,
            TRANSLATE_POST
        ]
    );
    let lines = lines(&results);
    assert_eq!(
        kinds(&lines),
        [
            "run",
            "languages",
            "usage",
            "item",
            "item",
            "item",
            "incomplete"
        ]
    );
    assert_eq!(lines[4]["answer"], "Берег.");
    assert_eq!(lines[5]["item"], 2);
    assert_eq!(lines[5]["context"], false);
    assert_eq!(lines[5]["refusal"], "quota");
    assert_eq!(lines[5]["answer"], Value::Null);
    assert_eq!(lines[5]["verdict"], Value::Null);
    assert_eq!(lines[6]["reason"], "item_2_quota");
    assert!(explain(&failure, None).contains("quota"));
}

#[tokio::test]
async fn another_refusal_of_an_item_is_written_and_the_run_goes_on() {
    for status in [429, 500, 503, 400] {
        let (_dir, server, client, results) = asking(
            "other",
            script(move |seen| match (seen.text.as_deref(), &seen.context) {
                (Some("bank"), Some(_)) => Reply::status(status),
                _ => good(seen),
            }),
        );
        let summary = run(&client, &set(), &results, LIMIT).await.expect("a run");
        assert_eq!(summary.refused, 1, "{status}");
        // No retry: seven requests, as in a full run.
        assert_eq!(server.targets().len(), 7, "{status}");
        let lines = lines(&results);
        assert_eq!(
            kinds(&lines),
            [
                "run",
                "languages",
                "usage",
                "item",
                "item",
                "item",
                "item",
                "usage"
            ]
        );
        assert_eq!(lines[4]["refusal"], "other", "{status}");
        assert_eq!(lines[4]["answer"], Value::Null);
        assert_eq!(lines[5]["answer"], "среднее");
    }
}

#[tokio::test]
async fn an_item_without_an_answer_in_time_is_written_and_the_run_goes_on() {
    let (_dir, server, client, results) = asking(
        "silence",
        script(|seen| match (seen.text.as_deref(), &seen.context) {
            (Some("bank"), None) => Reply::Silence,
            _ => good(seen),
        }),
    );
    let summary = run(&client, &set(), &results, SHORT).await.expect("a run");
    assert_eq!(summary.refused, 1);
    assert_eq!(summary.matched_with_context, 1);
    assert_eq!(server.targets().len(), 7);
    let lines = lines(&results);
    assert_eq!(lines[0]["limit_ms"], 300);
    assert_eq!(lines[3]["refusal"], "timed_out");
    assert_eq!(lines[3]["answer"], Value::Null);
    let waited = lines[3]["delay_ms"].as_u64().expect("a delay");
    assert!((300..2000).contains(&waited), "{waited}");
    assert_eq!(lines[4]["verdict"], "match");
    assert_eq!(lines[7]["when"], "after");
}

#[tokio::test]
async fn languages_or_usage_without_an_answer_stop_the_run_before_any_translation() {
    // The stub of the client: silent to whatever is asked.
    let dir = TempDir::new("silent-languages");
    let server = stub::Stub::start(stub::Reply::Silence).expect("stub");
    let client = DeepL::with_base_url(dir.key_file(KEY, 0o600), server.base_url());
    let results = dir.path().join("results.jsonl");
    let failure = run(&client, &set(), &results, SHORT)
        .await
        .expect_err("no languages");
    assert!(
        matches!(failure, Failure::Languages(Refusal::TimedOut)),
        "{failure:?}"
    );
    assert_eq!(server.seen().len(), 1);
    assert_eq!(lines(&results)[2]["reason"], "languages_timed_out");

    let (_dir, server, client, results) = asking(
        "no-usage",
        Box::new(|_, seen| match seen.target.as_str() {
            LANGUAGES_GET => Reply::ok(LANGUAGES),
            _ => Reply::status(500),
        }),
    );
    let failure = run(&client, &set(), &results, LIMIT)
        .await
        .expect_err("no usage");
    assert!(
        matches!(failure, Failure::Usage(When::Before, Refusal::Other)),
        "{failure:?}"
    );
    assert_eq!(server.targets(), [LANGUAGES_GET, USAGE_GET]);
    let lines = lines(&results);
    assert_eq!(kinds(&lines), ["run", "languages", "usage", "incomplete"]);
    assert_eq!(lines[2]["refusal"], "other");
    assert_eq!(lines[2]["character_count"], Value::Null);
}

#[tokio::test]
async fn a_refused_usage_after_the_items_keeps_their_lines_and_fails_the_run() {
    let (_dir, _server, client, results) = asking(
        "no-usage-after",
        Box::new(|before, seen| match seen.target.as_str() {
            LANGUAGES_GET => Reply::ok(LANGUAGES),
            USAGE_GET if before < 2 => Reply::ok(&usage_body(1000)),
            USAGE_GET => Reply::status(503),
            _ => good(seen),
        }),
    );
    let failure = run(&client, &set(), &results, LIMIT)
        .await
        .expect_err("no usage");
    assert!(
        matches!(failure, Failure::Usage(When::After, Refusal::Other)),
        "{failure:?}"
    );
    let lines = lines(&results);
    assert_eq!(
        kinds(&lines),
        [
            "run",
            "languages",
            "usage",
            "item",
            "item",
            "item",
            "item",
            "usage",
            "incomplete"
        ]
    );
    assert_eq!(lines[8]["reason"], "usage_after_other");
}

#[tokio::test]
async fn an_existing_results_file_is_left_as_it_is_and_nothing_is_asked() {
    let (dir, server, client, _) = asking("exists", script(good));
    let results = dir.file("results.jsonl", "kept\n");
    let failure = run(&client, &set(), &results, LIMIT)
        .await
        .expect_err("the file exists");
    assert!(
        matches!(&failure, Failure::Exists(path) if path == &results),
        "{failure:?}"
    );
    assert!(explain(&failure, None).contains("results.jsonl"));
    assert!(server.seen().is_empty());
    assert_eq!(std::fs::read_to_string(&results).expect("read"), "kept\n");
}

#[tokio::test]
async fn no_output_of_a_run_holds_the_key() {
    let (dir, server, client, results) = asking("key", script(good));
    let summary = run(&client, &set(), &results, LIMIT).await.expect("a run");
    // The key did go out, in the header and nowhere else.
    assert!(
        server
            .seen()
            .iter()
            .all(|seen| seen.raw.contains(&format!("DeepL-Auth-Key {KEY}")))
    );
    let written = std::fs::read_to_string(&results).expect("the results");
    let (_dir2, _server2, refused, results2) = asking(
        "key-refused",
        Box::new(|_, _| Reply::Answer(403, format!(r#"{{"message":"{KEY}"}}"#))),
    );
    let failure = run(&refused, &set(), &results2, LIMIT)
        .await
        .expect_err("a rejected key");
    for shown in [
        written,
        std::fs::read_to_string(&results2).expect("the results"),
        format!("{summary:?}"),
        format!("{failure:?}"),
        explain(&failure, Some(&dir.key_path())),
        format!("{client:?}"),
    ] {
        assert!(!shown.contains(KEY), "{shown}");
        assert!(!shown.contains("test-key"), "{shown}");
    }
}

/// The set of the experiment itself, as the repository tracks it.
fn tracked_set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../experiments/exp5/set.json")
}

#[test]
fn the_tracked_set_is_the_twelve_approved_items() {
    let loaded = load_set(&tracked_set()).expect("the tracked set");
    // The set of the run of 2026-10-13: it is not edited after its first
    // request, and a change is a new set and a new dated run.
    assert_eq!(
        loaded.sha256,
        "654f33a255a91663a462a377b02b1e03cdd1258407c23925a8a0ad7a0b73a878"
    );
    let items = &loaded.set.items;
    assert_eq!(
        items.iter().map(|item| item.id).collect::<Vec<_>>(),
        (1..=12).collect::<Vec<_>>()
    );
    for word in ["bank", "charge", "issue", "mean"] {
        assert_eq!(
            items.iter().filter(|item| item.word == word).count(),
            3,
            "{word}"
        );
    }
    let forms: usize = items.iter().map(|item| item.accepted.len()).sum();
    assert_eq!(forms, 26);
    // Every accepted form is Russian: no Latin letter slipped in.
    for item in items {
        for form in &item.accepted {
            assert!(
                form.chars()
                    .all(|c| c == ' ' || ('а'..='я').contains(&c) || c == 'ё'),
                "{form}"
            );
        }
    }
    // A full run sends each word twice; the context is not billed.
    let billed: usize = items.iter().map(|item| 2 * item.word.len()).sum();
    assert!(billed < 150, "{billed}");
}

/// Runs the binary with a configuration directory of the test, so that it
/// can never find the key of the user and never asks DeepL.
fn binary(config: &Path, arguments: &[&Path]) -> (Option<i32>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_addict-exp5"))
        .args(arguments)
        .env("XDG_CONFIG_HOME", config)
        .output()
        .expect("run the binary");
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.code(), shown)
}

#[test]
fn the_binary_refuses_an_invalid_set_before_anything_else() {
    let dir = TempDir::new("bin-set");
    let results = dir.path().join("results.jsonl");
    let bad = dir.file(
        "set.json",
        r#"{"items":[{"id":4,"word":"charge","sentence":"No fee for delivery.","accepted":["плата"]}]}"#,
    );
    let (code, shown) = binary(dir.path(), &[&bad, &results]);
    assert_eq!(code, Some(2), "{shown}");
    assert!(shown.contains("item 4"), "{shown}");
    assert!(!results.exists());

    let (code, shown) = binary(dir.path(), &[&dir.path().join("none.json"), &results]);
    assert_eq!(code, Some(2), "{shown}");
    assert!(!results.exists());

    let (code, shown) = binary(dir.path(), &[&bad]);
    assert_eq!(code, Some(2), "{shown}");
    assert!(shown.contains("usage"), "{shown}");
}

#[test]
fn the_binary_refuses_an_existing_results_file_and_names_it() {
    let dir = TempDir::new("bin-exists");
    let results = dir.file("results.jsonl", "kept\n");
    let (code, shown) = binary(dir.path(), &[&tracked_set(), &results]);
    assert_eq!(code, Some(2), "{shown}");
    assert!(shown.contains(results.to_str().expect("a path")), "{shown}");
    assert_eq!(std::fs::read_to_string(&results).expect("read"), "kept\n");
}

#[test]
fn the_binary_without_a_usable_key_names_the_key_file_and_begins_no_results() {
    let dir = TempDir::new("bin-key");
    let results = dir.path().join("results.jsonl");
    let key_file = dir.path().join("addict").join("deepl.key");
    // No key file at all.
    let (code, shown) = binary(dir.path(), &[&tracked_set(), &results]);
    assert_eq!(code, Some(2), "{shown}");
    assert!(
        shown.contains(key_file.to_str().expect("a path")),
        "{shown}"
    );
    assert!(!results.exists());

    // A key file that others can read: its content is not read, and not shown.
    std::fs::create_dir(dir.path().join("addict")).expect("create");
    std::fs::write(&key_file, KEY).expect("write the key file");
    std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o644))
        .expect("set the mode of the key file");
    let (code, shown) = binary(dir.path(), &[&tracked_set(), &results]);
    assert_eq!(code, Some(2), "{shown}");
    assert!(
        shown.contains(key_file.to_str().expect("a path")),
        "{shown}"
    );
    assert!(shown.contains("0600"), "{shown}");
    assert!(!shown.contains(KEY), "{shown}");
    assert!(!results.exists());
}
