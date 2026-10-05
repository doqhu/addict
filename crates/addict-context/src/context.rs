//! The CAP-2 read: find the selection, read the word, ask the application
//! for the sentence, read wider text and derive a sentence from it, and
//! name the outcome.

use std::time::Duration;

use addict_probe::record::{Bounded, Progress, bounded};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    bus::{Object, TEXT},
    record::{Finish, Outcome},
    text,
    tree::{TextObject, Walk},
};

pub const EXPECTED: &str = "outcome context: exactly one Text object of the application reports a \
     non-empty selection; its character offsets, the selected word, the sentence the application \
     returns for that offset and a sentence derived from wider text are recorded, each sentence \
     with its source";

/// `Granularity` of `GetStringAtOffset`.
const GRANULARITY_SENTENCE: u32 = 2;
const GRANULARITY_PARAGRAPH: u32 = 4;
/// `BoundaryType` of `GetTextAtOffset`.
const BOUNDARY_SENTENCE_START: u32 = 3;

/// Candidates written to an `ambiguous` record.
pub const LISTED_CANDIDATES: usize = 10;
/// Characters of any one text kept in a record.
const RECORDED_CHARS: usize = 4000;
const CANDIDATE_WORD_CHARS: usize = 200;

pub struct Args {
    /// Characters read on each side of the selection for the derived sentence.
    pub context_chars: usize,
    pub expect_word: Option<String>,
    pub expect_sentence: Option<String>,
    /// Time one read of a candidate gets while candidates are being sorted.
    pub per_call: Duration,
}

/// A Text object with a non-empty selection.
struct Candidate {
    text_object: TextObject,
    object: Object,
    n_selections: i32,
    start: usize,
    end: usize,
}

fn offset(value: i32) -> usize {
    usize::try_from(value).unwrap_or(0)
}

fn wire(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Text for a record: clipped, with its real length and embedded objects.
fn recorded(text: &str) -> Value {
    let (clipped, truncated) = text::clip(text, RECORDED_CHARS);
    json!({
        "text": clipped,
        "chars": text::char_len(text),
        "truncated": truncated,
        "embedded_object_chars": text::embedded_object_count(text),
    })
}

/// One read of the sentence from the application.
#[derive(Debug, Serialize)]
struct Attempt {
    method: &'static str,
    argument: u32,
    ok: bool,
    text: Option<Value>,
    start: Option<i32>,
    end: Option<i32>,
    /// Whether the returned range holds the whole selection.
    covers_selection: Option<bool>,
    error: Option<String>,
}

async fn attempt(
    object: &Object,
    method: &'static str,
    argument: u32,
    start: usize,
    end: usize,
) -> (Attempt, Option<String>) {
    let reply: Result<(String, i32, i32), String> =
        object.call(TEXT, method, &(wire(start), argument)).await;
    match reply {
        Ok((sentence, from, to)) if !sentence.trim().is_empty() => (
            Attempt {
                method,
                argument,
                ok: true,
                text: Some(recorded(&sentence)),
                start: Some(from),
                end: Some(to),
                covers_selection: Some(offset(from) <= start && end <= offset(to)),
                error: None,
            },
            Some(sentence),
        ),
        Ok((_, from, to)) => (
            Attempt {
                method,
                argument,
                ok: false,
                text: None,
                start: Some(from),
                end: Some(to),
                covers_selection: None,
                error: Some("the application returned an empty string".to_owned()),
            },
            None,
        ),
        Err(error) => (
            Attempt {
                method,
                argument,
                ok: false,
                text: None,
                start: None,
                end: None,
                covers_selection: None,
                error: Some(error),
            },
            None,
        ),
    }
}

/// Selection reads that failed on objects reporting a selection.
#[derive(Default)]
struct ReadFailures {
    errors: Vec<String>,
    timed_out: bool,
}

async fn candidates(
    walk: &Walk,
    per_call: Duration,
    progress: &Progress,
) -> (Vec<Candidate>, ReadFailures) {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    let mut timed_out = false;
    for text_object in walk.text_objects.iter().filter(|t| t.reports_selection()) {
        let Some(object) = text_object.object.clone() else {
            continue;
        };
        let read = bounded(
            per_call,
            object.call::<_, (i32, i32)>(TEXT, "GetSelection", &(0i32,)),
        )
        .await;
        match read {
            Bounded::Done(Ok((a, b))) => {
                let (start, end) = (offset(a.min(b)), offset(a.max(b)));
                if end > start {
                    found.push(Candidate {
                        text_object: text_object.clone(),
                        object,
                        n_selections: text_object.n_selections.unwrap_or(0),
                        start,
                        end,
                    });
                }
            }
            Bounded::Done(Err(e)) => errors.push(format!(
                "{} {}: GetSelection: {e}",
                text_object.bus_name, text_object.object_path
            )),
            Bounded::TimedOut => {
                timed_out = true;
                errors.push(format!(
                    "{} {}: GetSelection: no answer within {} s",
                    text_object.bus_name,
                    text_object.object_path,
                    per_call.as_secs_f64()
                ));
            }
        }
    }
    if !errors.is_empty() {
        progress.put("selection_read_errors", &errors);
    }
    (found, ReadFailures { errors, timed_out })
}

/// Reads the selection of the walked application and classifies the run.
pub async fn run(walk: &Walk, args: &Args, progress: &Progress) -> Finish {
    progress.phase("selections");
    let (found, read_failures) = candidates(walk, args.per_call, progress).await;

    if found.is_empty() {
        // Incomplete evidence has not shown that nothing is selected.
        if walk.cut == Some("time") {
            return Finish::with_error(
                Outcome::Timeout,
                "the walk was cut by its time limit before a selection was found",
            );
        }
        if !read_failures.errors.is_empty() {
            let outcome = if read_failures.timed_out {
                Outcome::Timeout
            } else {
                Outcome::Error
            };
            return Finish::with_error(
                outcome,
                format!(
                    "a selection was reported but could not be read: {}",
                    read_failures.errors.join("; ")
                ),
            );
        }
        if walk.unanswered > 0 {
            return Finish::with_error(
                Outcome::Timeout,
                format!(
                    "{} nodes did not answer before a selection was found",
                    walk.unanswered
                ),
            );
        }
        if walk.cut == Some("nodes") {
            return Finish::with_error(
                Outcome::Error,
                "the walk was cut at --max-nodes before a selection was found",
            );
        }
        return if walk.text_objects_with_characters == 0 {
            Finish::ok(Outcome::NoText)
        } else {
            Finish::ok(Outcome::NoSelection)
        };
    }

    if found.len() > 1 {
        progress.phase("ambiguous-candidates");
        let mut listed = Vec::new();
        for candidate in found.iter().take(LISTED_CANDIDATES) {
            let word = bounded(
                args.per_call,
                candidate.object.call::<_, String>(
                    TEXT,
                    "GetText",
                    &(wire(candidate.start), wire(candidate.end)),
                ),
            )
            .await;
            let (word, error) = match word {
                Bounded::Done(Ok(word)) => (Some(text::clip(&word, CANDIDATE_WORD_CHARS).0), None),
                Bounded::Done(Err(e)) => (None, Some(e)),
                Bounded::TimedOut => (None, Some("no answer".to_owned())),
            };
            listed.push(json!({
                "object": candidate.text_object,
                "selection": { "start": candidate.start, "end": candidate.end,
                               "n_selections": candidate.n_selections },
                "word": word,
                "error": error,
            }));
        }
        progress.put("candidate_count", found.len());
        progress.put("candidates", &listed);
        return Finish::with_error(
            Outcome::Ambiguous,
            format!("{} objects report a non-empty selection", found.len()),
        );
    }

    let candidate = &found[0];
    let object = &candidate.object;
    let (start, end) = (candidate.start, candidate.end);
    progress.put("object", &candidate.text_object);
    progress.put(
        "selection",
        json!({ "start": start, "end": end, "n_selections": candidate.n_selections,
                "unit": "characters" }),
    );

    progress.phase("word");
    let word: String = match object
        .call(TEXT, "GetText", &(wire(start), wire(end)))
        .await
    {
        Ok(word) => word,
        Err(e) => {
            return Finish::with_error(
                Outcome::Error,
                format!("selection {start}..{end} was reported but GetText failed: {e}"),
            );
        }
    };
    progress.put("word", recorded(&word));
    if let Some(expected) = &args.expect_word {
        progress.put("word_match", text::matches_expectation(expected, &word));
    }

    // What the toolkit implements: the granularity call, then the older
    // boundary call. Every attempt is recorded, also after a success.
    progress.phase("sentence-application");
    let mut attempts = Vec::new();
    let mut failures = Vec::new();
    let mut from_application: Option<(String, &'static str)> = None;
    // The first non-empty reply, whether or not its range holds the selection.
    let mut first_reply: Option<String> = None;
    for (method, argument) in [
        ("GetStringAtOffset", GRANULARITY_SENTENCE),
        ("GetTextAtOffset", BOUNDARY_SENTENCE_START),
    ] {
        let (made, sentence) = attempt(object, method, argument, start, end).await;
        if let Some(error) = &made.error {
            failures.push(format!("{method}({argument}): {error}"));
        }
        let covers = made.covers_selection == Some(true);
        if sentence.is_some() && !covers {
            failures.push(format!(
                "{method}({argument}): returned range {}..{} does not hold the selection \
                 {start}..{end}",
                made.start.unwrap_or(-1),
                made.end.unwrap_or(-1)
            ));
        }
        attempts.push(made);
        if let Some(sentence) = sentence {
            if first_reply.is_none() {
                first_reply = Some(sentence.clone());
            }
            // Only a reply whose range holds the selection is its sentence.
            if covers && from_application.is_none() {
                from_application = Some((sentence, method));
            }
        }
        progress.put("sentence_attempts", &attempts);
    }
    progress.phase("paragraph-application");
    let (paragraph, _) = attempt(
        object,
        "GetStringAtOffset",
        GRANULARITY_PARAGRAPH,
        start,
        end,
    )
    .await;
    progress.put("paragraph_attempt", &paragraph);

    // Whether the product could segment wider text itself.
    progress.phase("wider-text");
    let count = candidate
        .text_object
        .character_count
        .map(offset)
        .filter(|count| *count >= end);
    let wide_start = start.saturating_sub(args.context_chars);
    let wide_end = match count {
        Some(count) => count.min(end.saturating_add(args.context_chars)),
        None => end.saturating_add(args.context_chars),
    };
    let wider: Result<String, String> = object
        .call(TEXT, "GetText", &(wire(wide_start), wire(wide_end)))
        .await;
    let mut derived: Option<String> = None;
    match &wider {
        Ok(wider) => {
            let (rel_start, rel_end) = (start - wide_start, end - wide_start);
            // False when the application's offsets are not character
            // offsets of the text it returns (e.g. UTF-16 units).
            let consistent = text::char_slice(wider, rel_start, rel_end) == word;
            let sentence = text::derive_sentence(wider, rel_start, rel_end);
            let mut block = json!({
                "start": wide_start,
                "end": wide_end,
                "wider_text": recorded(wider),
                "selection_matches_word": consistent,
                "sentence": Value::Null,
            });
            match sentence {
                Some(sentence) => {
                    block["sentence"] = json!({
                        "text": recorded(&sentence.text),
                        "start": wide_start + sentence.start,
                        "end": wide_start + sentence.end,
                        // The sentence runs into the edge of the window that
                        // was read, not into the edge of the object's text.
                        "cut_at_window_start": sentence.open_start && wide_start > 0,
                        "cut_at_window_end": sentence.open_end
                            && count.is_none_or(|count| wide_end < count),
                    });
                    derived = Some(sentence.text);
                }
                None => failures
                    .push("derived: no sentence around the selection in the wider text".to_owned()),
            }
            progress.put("derived", block);
        }
        Err(e) => {
            failures.push(format!("GetText({wide_start}, {wide_end}): {e}"));
            progress.put(
                "derived",
                json!({ "start": wide_start, "end": wide_end, "error": e }),
            );
        }
    }

    progress.phase("classify");
    if let Some(expected) = &args.expect_sentence {
        progress.put(
            "sentence_match_application",
            first_reply
                .as_ref()
                .map(|sentence| text::matches_expectation(expected, sentence)),
        );
        progress.put(
            "sentence_match_derived",
            derived
                .as_ref()
                .map(|sentence| text::matches_expectation(expected, sentence)),
        );
    }
    let chosen = match (&from_application, &derived) {
        (Some((sentence, method)), _) => Some((sentence, "application", Some(*method))),
        (None, Some(sentence)) => Some((sentence, "derived", None)),
        (None, None) => None,
    };
    match chosen {
        Some((sentence, source, method)) => {
            progress.put(
                "sentence",
                json!({ "text": recorded(sentence), "source": source, "method": method }),
            );
            if let Some(expected) = &args.expect_sentence {
                progress.put(
                    "sentence_match",
                    text::matches_expectation(expected, sentence),
                );
            }
            progress.phase("done");
            Finish::ok(Outcome::Context)
        }
        None => {
            if args.expect_sentence.is_some() {
                progress.put("sentence_match", false);
            }
            progress.phase("done");
            Finish::with_error(Outcome::WordOnly, failures.join("; "))
        }
    }
}
