//! The client against the stub server: what it sends, what it makes of each
//! answer, and that without a usable key file nothing is sent.

mod common;

use std::{sync::Arc, time::Duration};

use addict_core::translation::{Refusal, Request, Translator};
use addict_deepl::{
    DeepL, Language, Usage,
    stub::{Reply, Stub},
};

use common::{KEY, TempDir};

const SENTENCE: &str = "I sat on the bank of the river.";

/// How long the silent stub is waited for before the request is given up:
/// long enough that the request has reached the stub on a loaded machine.
const DEADLINE: Duration = Duration::from_secs(2);

fn request() -> Request {
    Request {
        text: "bank".to_owned(),
        context: Some(SENTENCE.to_owned()),
    }
}

/// A stub with `reply`, and a client with a usable key file that asks it.
fn asking(tag: &str, reply: Reply) -> (TempDir, Stub, DeepL) {
    let dir = TempDir::new(tag);
    let stub = Stub::start(reply).expect("stub");
    let client = DeepL::with_base_url(dir.key_file(&format!("{KEY}\n"), 0o600), stub.base_url());
    (dir, stub, client)
}

#[tokio::test]
async fn a_word_with_its_sentence_is_sent_as_text_and_context_and_comes_back_russian() {
    let (_dir, stub, client) = asking("translate", Reply::translation("берег"));
    assert_eq!(client.translate(&request()).await, Ok("берег".to_owned()));
    let seen = stub.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].line, "POST /v2/translate HTTP/1.1");
    assert_eq!(
        seen[0].header("authorization"),
        Some(format!("DeepL-Auth-Key {KEY}").as_str())
    );
    assert_eq!(seen[0].header("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).expect("a JSON body");
    assert_eq!(
        body,
        serde_json::json!({
            "text": ["bank"],
            "context": SENTENCE,
            "source_lang": "EN",
            "target_lang": "RU",
        })
    );
}

#[tokio::test]
async fn a_request_without_a_context_sends_none() {
    let (_dir, stub, client) = asking("no-context", Reply::translation("банк"));
    let request = Request {
        text: "bank".to_owned(),
        context: None,
    };
    assert_eq!(client.translate(&request).await, Ok("банк".to_owned()));
    let body: serde_json::Value = serde_json::from_str(&stub.seen()[0].body).expect("JSON");
    assert_eq!(
        body,
        serde_json::json!({"text": ["bank"], "source_lang": "EN", "target_lang": "RU"})
    );
}

#[tokio::test]
async fn the_client_translates_through_the_contract_of_core() {
    let (_dir, stub, client) = asking("contract", Reply::translation("берег"));
    let translator: Arc<dyn Translator> = Arc::new(client);
    assert_eq!(
        translator.translate(request()).await,
        Ok("берег".to_owned())
    );
    assert_eq!(stub.seen().len(), 1);
}

#[tokio::test]
async fn every_status_of_refusal_is_its_refusal_and_is_asked_once() {
    for (status, refusal) in [
        (403, Refusal::RejectedKey),
        (456, Refusal::Quota),
        (429, Refusal::Other),
        (529, Refusal::Other),
        (500, Refusal::Other),
        (503, Refusal::Other),
        (400, Refusal::Other),
        (404, Refusal::Other),
        (302, Refusal::Other),
        (204, Refusal::Other),
    ] {
        let reply = Reply::answer(status, r#"{"message":"refused by the stub"}"#);
        let (_dir, stub, client) = asking("status", reply);
        assert_eq!(client.translate(&request()).await, Err(refusal), "{status}");
        assert_eq!(client.languages().await, Err(refusal), "{status}");
        assert_eq!(client.usage().await, Err(refusal), "{status}");
        // No retry: one request for each call.
        assert_eq!(stub.seen().len(), 3, "{status}");
    }
}

#[tokio::test]
async fn a_request_without_an_answer_is_pending_until_it_is_given_up() {
    let (_dir, stub, client) = asking("silence", Reply::Silence);
    // The deadline is the caller's: the client sets no timer of its own.
    let waited = tokio::time::timeout(DEADLINE, client.translate(&request())).await;
    assert!(waited.is_err(), "{waited:?}");
    assert_eq!(stub.seen().len(), 1);
    // The client is usable after a request that was given up.
    let waited = tokio::time::timeout(DEADLINE, client.usage()).await;
    assert!(waited.is_err(), "{waited:?}");
    assert_eq!(stub.seen().len(), 2);
}

#[tokio::test]
async fn an_answer_without_a_translation_is_a_refusal() {
    for body in [
        "",
        "not json",
        "{}",
        "[]",
        r#"{"translations":[]}"#,
        r#"{"translations":null}"#,
        r#"{"translations":"берег"}"#,
        r#"{"translations":[{}]}"#,
        r#"{"translations":[{"text":7}]}"#,
        r#"{"translations":[{"text":""}]}"#,
        r#"{"translations":[{"text":" \n"}]}"#,
        r#"{"translations":[{"text":"берег"}"#,
    ] {
        let (_dir, stub, client) = asking("malformed", Reply::answer(200, body));
        assert_eq!(
            client.translate(&request()).await,
            Err(Refusal::Other),
            "{body:?}"
        );
        assert_eq!(stub.seen().len(), 1, "{body:?}");
    }
}

#[tokio::test]
async fn the_first_translation_is_the_answer_without_the_white_space_around_it() {
    let body = r#"{"translations":[{"text":" берег\n"},{"text":"банк"}],"more":1}"#;
    let (_dir, _stub, client) = asking("first", Reply::answer(200, body));
    assert_eq!(client.translate(&request()).await, Ok("берег".to_owned()));
}

#[tokio::test]
async fn the_languages_are_asked_for_text_and_read() {
    let body = r#"[
        {"lang":"en","name":"English","usable_as_source":true,"usable_as_target":false,
         "status":"stable","features":{}},
        {"lang":"ru","name":"Russian","usable_as_source":true,"usable_as_target":true}
    ]"#;
    let (_dir, stub, client) = asking("languages", Reply::answer(200, body));
    let language = |lang: &str, usable_as_target| Language {
        lang: lang.to_owned(),
        usable_as_source: true,
        usable_as_target,
    };
    assert_eq!(
        client.languages().await,
        Ok(vec![language("en", false), language("ru", true)])
    );
    let seen = stub.seen();
    assert_eq!(
        seen[0].line,
        "GET /v3/languages?resource=translate_text HTTP/1.1"
    );
    assert_eq!(
        seen[0].header("authorization"),
        Some(format!("DeepL-Auth-Key {KEY}").as_str())
    );
    assert_eq!(seen[0].body, "");
}

#[tokio::test]
async fn the_usage_is_read() {
    let body = r#"{"character_count":180118,"character_limit":1250000,"more":[]}"#;
    let (_dir, stub, client) = asking("usage", Reply::answer(200, body));
    assert_eq!(
        client.usage().await,
        Ok(Usage {
            character_count: 180_118,
            character_limit: 1_250_000,
        })
    );
    let seen = stub.seen();
    assert_eq!(seen[0].line, "GET /v2/usage HTTP/1.1");
    assert_eq!(
        seen[0].header("authorization"),
        Some(format!("DeepL-Auth-Key {KEY}").as_str())
    );
}

#[tokio::test]
async fn languages_and_usage_that_cannot_be_read_are_refusals() {
    for body in ["", "{}", r#"[{"lang":"en"}]"#, r#"{"character_count":"x"}"#] {
        let (_dir, _stub, client) = asking("unread", Reply::answer(200, body));
        assert_eq!(client.usage().await, Err(Refusal::Other), "{body:?}");
        assert_eq!(client.languages().await, Err(Refusal::Other), "{body:?}");
    }
}

#[tokio::test]
async fn a_key_file_that_is_too_open_sends_nothing() {
    for mode in [0o644, 0o640, 0o604, 0o660, 0o700] {
        let dir = TempDir::new("open");
        let stub = Stub::start(Reply::translation("берег")).expect("stub");
        let client = DeepL::with_base_url(dir.key_file(KEY, mode), stub.base_url());
        assert_eq!(
            client.translate(&request()).await,
            Err(Refusal::UnsafeKey),
            "{mode:o}"
        );
        assert_eq!(
            client.languages().await,
            Err(Refusal::UnsafeKey),
            "{mode:o}"
        );
        assert_eq!(client.usage().await, Err(Refusal::UnsafeKey), "{mode:o}");
        assert!(stub.seen().is_empty(), "{mode:o}");
    }
}

#[tokio::test]
async fn no_key_file_an_empty_one_and_an_unreadable_one_send_nothing() {
    let dir = TempDir::new("no-key");
    let stub = Stub::start(Reply::translation("берег")).expect("stub");
    // Not there.
    let client = DeepL::with_base_url(dir.key_path(), stub.base_url());
    assert_eq!(client.translate(&request()).await, Err(Refusal::Key));
    // Empty, blank, and not a key.
    for content in ["", "\n", "  \n", "two words"] {
        dir.key_file(content, 0o600);
        assert_eq!(
            client.translate(&request()).await,
            Err(Refusal::Key),
            "{content:?}"
        );
    }
    // A directory in its place.
    let client = DeepL::with_base_url(dir.path(), stub.base_url());
    assert_eq!(
        client.translate(&request()).await,
        Err(Refusal::UnreadableKey)
    );
    assert_eq!(client.languages().await, Err(Refusal::UnreadableKey));
    assert_eq!(client.usage().await, Err(Refusal::UnreadableKey));
    assert!(stub.seen().is_empty());
}

#[tokio::test]
async fn a_key_written_after_a_refusal_is_found_by_the_next_request() {
    let dir = TempDir::new("later");
    let stub = Stub::start(Reply::translation("берег")).expect("stub");
    let client = DeepL::with_base_url(dir.key_path(), stub.base_url());
    assert_eq!(client.translate(&request()).await, Err(Refusal::Key));
    dir.key_file(KEY, 0o644);
    assert_eq!(client.translate(&request()).await, Err(Refusal::UnsafeKey));
    assert!(stub.seen().is_empty());
    dir.key_file(KEY, 0o600);
    assert_eq!(client.translate(&request()).await, Ok("берег".to_owned()));
    // And a key taken away is gone for the request after it.
    std::fs::remove_file(dir.key_path()).expect("remove");
    assert_eq!(client.translate(&request()).await, Err(Refusal::Key));
    assert_eq!(stub.seen().len(), 1);
}

#[tokio::test]
async fn a_server_that_is_not_there_is_a_refusal() {
    let dir = TempDir::new("no-server");
    // A loopback port that nothing listens on: taken and given back.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let address = listener.local_addr().expect("address");
    drop(listener);
    let client = DeepL::with_base_url(dir.key_file(KEY, 0o600), format!("http://{address}/"));
    assert_eq!(client.translate(&request()).await, Err(Refusal::Network));
    assert_eq!(client.usage().await, Err(Refusal::Network));
}

#[tokio::test]
async fn the_debug_output_of_the_client_and_of_its_refusals_does_not_hold_the_key() {
    let (dir, _stub, client) = asking("debug", Reply::answer(403, "{}"));
    let before = format!("{client:?}");
    let refused = client.translate(&request()).await;
    let after = format!("{client:?}");
    dir.key_file(KEY, 0o644);
    let no_key = client.usage().await;
    for shown in [before, after, format!("{refused:?}"), format!("{no_key:?}")] {
        assert!(!shown.contains(KEY), "{shown}");
        assert!(!shown.contains("test-key"), "{shown}");
    }
}

#[tokio::test]
async fn only_recognized_translation_language_errors_have_the_unsupported_kind() {
    for message in [
        "Value for 'source_lang' not supported.",
        "Value for 'target_lang' not supported.",
        "Source language not supported",
        "Target language not supported",
    ] {
        let body = serde_json::json!({"message": message}).to_string();
        let (_dir, stub, client) = asking("unsupported", Reply::answer(400, &body));
        assert_eq!(
            client.translate(&request()).await,
            Err(Refusal::UnsupportedLanguage)
        );
        assert_eq!(client.languages().await, Err(Refusal::Other));
        assert_eq!(client.usage().await, Err(Refusal::Other));
        assert_eq!(stub.seen().len(), 3);
    }
    for body in [
        "not json",
        "{}",
        r#"{"message":7}"#,
        r#"{"message":"Invalid text parameter"}"#,
        r#"{"message":"Value for 'context' not supported."}"#,
    ] {
        let (_dir, stub, client) = asking("bad-parameter", Reply::answer(400, body));
        assert_eq!(client.translate(&request()).await, Err(Refusal::Other));
        assert_eq!(stub.seen().len(), 1);
    }
}

#[test]
fn the_configured_key_path_is_captured_without_reading_it() {
    let dir = TempDir::new("captured-path");
    let path = dir.key_path();
    let client = DeepL::with_base_url(&path, "http://127.0.0.1:1");
    assert_eq!(client.key_file(), Some(path.as_path()));
    assert!(!path.exists());
}

#[tokio::test]
async fn an_interrupted_body_is_a_network_failure_even_when_its_json_is_valid() {
    use std::io::{Read, Write};

    for (status, body) in [
        (200, r#"{"translations":[{"text":"берег"}]}"#),
        (
            400,
            r#"{"message":"Value for 'target_lang' not supported."}"#,
        ),
    ] {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(DEADLINE))
                .expect("read timeout");
            // Consume the full request before sending the deliberately incomplete
            // response, so the refusal is caused by receiving its body.
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).expect("request header");
                headers.push(byte[0]);
            }
            let headers = String::from_utf8(headers).expect("headers");
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("request length"))
                })
                .expect("content length");
            stream
                .read_exact(&mut vec![0; length])
                .expect("request body");
            write!(stream,
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len() + 16,
            ).expect("partial response");
            stream
                .shutdown(std::net::Shutdown::Both)
                .expect("close stream");
        });
        let dir = TempDir::new("interrupted-body");
        let client = DeepL::with_base_url(dir.key_file(KEY, 0o600), format!("http://{address}"));
        assert_eq!(
            client.translate(&request()).await,
            Err(Refusal::Network),
            "{status}"
        );
        server.join().expect("server thread");
    }
}
