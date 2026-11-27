use std::sync::Mutex;

use addict_core::translation::reading_languages;
use addict_engine::{online::is_long, translate::SENTENCE_LIMIT};

use super::*;

const RIVER: &str = "I sat on the bank of the river.";

#[derive(Clone)]
struct Canned {
    asked: Arc<Mutex<Vec<Request>>>,
    answer: Answer,
}

impl Translator for Canned {
    fn translate(&self, request: Request) -> Translating<'_> {
        Box::pin(async move {
            self.asked.lock().unwrap().push(request);
            self.answer.clone()
        })
    }
}

/// Drives the real controller to a settled snapshot after each action.
struct Desk {
    shell: FakeShell,
    view: ViewState,
    canned: Canned,
}

impl Desk {
    fn new(answer: Result<&str, Refusal>) -> Self {
        Self::with_data(answer, None)
    }

    fn with_data(answer: Result<&str, Refusal>, data: Option<PathBuf>) -> Self {
        let canned = Canned {
            asked: Arc::default(),
            answer: answer.map(str::to_owned),
        };
        Self {
            shell: FakeShell::with_translator(data, canned.clone(), false),
            view: ViewState::default(),
            canned,
        }
    }

    fn with_text(answer: Result<&str, Refusal>, text: &str) -> Self {
        let mut desk = Self::new(answer);
        desk.edit(text);
        desk
    }

    fn step(&mut self, event: Event) {
        self.shell.dispatch(event);
        loop {
            let (view, _) = self.shell.next();
            if !view.looking_up
                && view
                    .online
                    .iter()
                    .all(|row| row.state != OnlineState::Pending)
            {
                self.view = view;
                break;
            }
        }
    }

    fn edit(&mut self, text: &str) {
        self.step(Event::FieldEdited {
            text: text.to_owned(),
            replaced: false,
            source: None,
        });
    }

    fn paste(&mut self, text: &str) {
        self.edit("");
        self.step(Event::FieldEdited {
            text: text.to_owned(),
            replaced: true,
            source: None,
        });
        self.submit();
    }

    fn click(&mut self, index: usize) {
        self.step(Event::WordChosen {
            text: self.view.text_number,
            index,
        });
    }

    fn submit(&mut self) {
        self.step(Event::FieldSubmitted);
    }

    fn switch(&mut self, reading: Language) {
        self.step(Event::ReadingChosen(reading));
    }

    fn configure(&mut self, language: Language, reading: Language) {
        self.switch(reading);
        self.step(Event::MyLanguageChosen(language));
    }

    fn asked(&self) -> Vec<Request> {
        self.canned.asked.lock().unwrap().clone()
    }

    fn row(&self, kind: OnlineKind) -> Option<(OnlineState, &str)> {
        self.view
            .online
            .iter()
            .find(|row| row.kind == kind)
            .map(|row| (row.state, row.line.as_ref()))
    }
}

fn bank_request() -> Request {
    Request {
        text: "bank".to_owned(),
        context: Some(RIVER.to_owned()),
        source: Language::En,
        target: Language::Ru,
        english: English::Us,
    }
}

/// The request about the chosen word of `manual` in its sentence.
fn sentence_request() -> Request {
    Request {
        text: RIVER.to_owned(),
        context: None,
        source: Language::En,
        target: Language::Ru,
        english: English::Us,
    }
}

fn river_request() -> Request {
    Request {
        text: "river".to_owned(),
        context: Some(RIVER.to_owned()),
        source: Language::En,
        target: Language::Ru,
        english: English::Us,
    }
}

fn lone_request(word: &str) -> Request {
    Request {
        text: word.to_owned(),
        context: None,
        source: Language::En,
        target: Language::Ru,
        english: English::Us,
    }
}

/// The indices of `bank` and `river` in [`RIVER`].
const BANK: usize = 4;
const RIVER_WORD: usize = 7;

/// A text of exactly [`SENTENCE_LIMIT`] characters that starts with
/// [`RIVER`]; its bytes are more.
fn text_at_the_limit() -> String {
    let more = SENTENCE_LIMIT - RIVER.chars().count();
    let text = format!("{RIVER}{}{}", " я".repeat(more / 2), "!".repeat(more % 2));
    assert_eq!(text.chars().count(), SENTENCE_LIMIT);
    assert!(text.len() > SENTENCE_LIMIT);
    text
}

/// The Spanish sentence of the matrix, and `banco` in it.
const BANCO_SENTENCE: &str = "El banco cerró a las tres.";
const BANCO: usize = 1;

#[test]
fn nothing_is_sent_for_a_russian_word_nor_for_its_sentence() {
    for language in reading_languages(Language::Ru) {
        // Alone, by Enter or by a paste.
        let mut desk = Desk::with_text(Ok("x"), "собака");
        desk.switch(language);
        desk.submit();
        desk.paste("«Дом»,");
        assert!(desk.asked().is_empty(), "{language:?}");
        assert_eq!(desk.row(OnlineKind::Lone), None);
        // In a sentence: neither the word nor the sentence, as if never
        // asked.
        let sentence = "Моя собака спит.";
        let mut desk = Desk::with_text(Ok("x"), sentence);
        desk.switch(language);
        desk.click(1);
        assert!(desk.asked().is_empty(), "{language:?}");
        assert_eq!(desk.row(OnlineKind::Word), None);
        assert_eq!(desk.row(OnlineKind::Sentence), None);
        // Clicked again, and after a switch: still nothing.
        desk.click(1);
        desk.switch(if language == Language::En {
            Language::Es
        } else {
            Language::En
        });
        desk.click(0);
        assert!(desk.asked().is_empty(), "{language:?}");
        assert_eq!(desk.row(OnlineKind::Sentence), None);
    }
    // A Latin word is sent as always, with its sentence, also in a
    // Russian sentence; a Russian word clicked after it leaves the
    // sentence's kept answer as it is and sends nothing more.
    let mut desk = Desk::with_text(Ok("банк"), "Мой bank.");
    desk.click(1);
    assert_eq!(desk.asked().len(), 2);
    assert_eq!(desk.asked()[0].text, "Мой bank.");
    assert_eq!(desk.asked()[1].text, "bank");
    desk.click(0);
    assert_eq!(desk.asked().len(), 2);
    assert_eq!(desk.row(OnlineKind::Word), None);
    assert_eq!(
        desk.row(OnlineKind::Sentence),
        Some((OnlineState::Translated, "банк"))
    );
}

/// The Russian sentence of the matrix, and `красный` in it.
const KRASNY_SENTENCE: &str = "Мне нравится красный цвет.";
const KRASNY: usize = 2;

/// The word `text` of a sentence, with it as context, and the sentence,
/// each sent from `source` into `target`.
fn word_and_sentence(
    text: &str,
    sentence: &str,
    source: Language,
    target: Language,
) -> [Request; 2] {
    [
        Request {
            text: sentence.to_owned(),
            context: None,
            source,
            target,
            english: English::Us,
        },
        Request {
            text: text.to_owned(),
            context: Some(sentence.to_owned()),
            source,
            target,
            english: English::Us,
        },
    ]
}

#[test]
fn a_russian_word_is_sent_from_ru_into_my_language_whatever_is_pressed() {
    // English mine: every button English mine offers, Ru among them.
    for language in reading_languages(Language::En) {
        let mut desk = Desk::with_text(Ok("red"), KRASNY_SENTENCE);
        desk.configure(Language::En, language);
        desk.click(KRASNY);
        assert_eq!(
            desk.asked(),
            word_and_sentence("красный", KRASNY_SENTENCE, Language::Ru, Language::En),
            "{language:?}"
        );
        assert_eq!(
            desk.row(OnlineKind::Word),
            Some((OnlineState::Translated, "red"))
        );
    }
    // A lone Russian word, Catalan mine.
    let mut desk = Desk::with_text(Ok("gos"), "собака");
    desk.configure(Language::Ca, Language::Es);
    desk.submit();
    assert_eq!(
        desk.asked(),
        [Request {
            text: "собака".to_owned(),
            context: None,
            source: Language::Ru,
            target: Language::Ca,
            english: English::Us,
        }]
    );
    assert_eq!(
        desk.row(OnlineKind::Lone),
        Some((OnlineState::Translated, "gos"))
    );
}

#[test]
fn a_latin_word_keeps_the_pressed_language_into_my_language() {
    // Ru pressed: a Latin word and its sentence go from the pressed
    // language, as Russian.
    let sentence = "Me gusta el color rojo.";
    let mut desk = Desk::with_text(Ok("red"), sentence);
    desk.configure(Language::En, Language::Ru);
    desk.click(4);
    assert_eq!(
        desk.asked(),
        word_and_sentence("rojo", sentence, Language::Ru, Language::En)
    );
    // A Latin word of a Russian sentence goes from the pressed language,
    // and so does the sentence.
    let mut desk = Desk::with_text(Ok("bank"), "Мой bank.");
    desk.configure(Language::En, Language::Es);
    desk.click(1);
    let [sentence_request, word_request] =
        word_and_sentence("bank", "Мой bank.", Language::Es, Language::En);
    assert_eq!(desk.asked(), [word_request, sentence_request]);
    // Spanish into Catalan.
    let mut desk = Desk::with_text(Ok("vermell"), sentence);
    desk.configure(Language::Ca, Language::Es);
    desk.click(4);
    let [sentence_request, word_request] =
        word_and_sentence("rojo", sentence, Language::Es, Language::Ca);
    assert_eq!(desk.asked(), [word_request, sentence_request]);
}

#[test]
fn a_spanish_word_is_sent_from_es_with_its_sentence() {
    let mut desk = Desk::with_text(Ok("банк"), BANCO_SENTENCE);
    desk.switch(Language::Es);
    desk.click(BANCO);
    assert_eq!(
        desk.asked(),
        [
            Request {
                text: "banco".to_owned(),
                context: Some(BANCO_SENTENCE.to_owned()),
                source: Language::Es,
                target: Language::Ru,
                english: English::Us,
            },
            Request {
                text: BANCO_SENTENCE.to_owned(),
                context: None,
                source: Language::Es,
                target: Language::Ru,
                english: English::Us,
            },
        ]
    );
    assert_eq!(
        desk.row(OnlineKind::Word),
        Some((OnlineState::Translated, "банк"))
    );
}

#[test]
fn a_catalan_sentence_is_sent_from_ca_without_context() {
    let text = "La col·lecció d'art és gran.";
    let mut desk = Desk::with_text(Ok("коллекция"), text);
    desk.switch(Language::Ca);
    // `col·lecció` is one word, the second.
    assert_eq!(&text[desk.view.words[1].clone()], "col·lecció");
    desk.click(1);
    assert_eq!(
        desk.asked()[1],
        Request {
            text: text.to_owned(),
            context: None,
            source: Language::Ca,
            target: Language::Ru,
            english: English::Us,
        }
    );
    assert_eq!(desk.asked()[0].text, "col·lecció");
    assert_eq!(desk.asked()[0].source, Language::Ca);
}

#[test]
fn changing_the_language_sends_the_shown_word_and_its_sentence_again() {
    let mut desk = Desk::with_text(Ok("банк"), BANCO_SENTENCE);
    desk.click(BANCO);
    assert_eq!(desk.asked().len(), 2);
    assert!(
        desk.asked()
            .iter()
            .all(|asked| asked.source == Language::En)
    );
    desk.switch(Language::Es);
    assert_eq!(
        desk.asked()[2..],
        [
            Request {
                text: "banco".to_owned(),
                context: Some(BANCO_SENTENCE.to_owned()),
                source: Language::Es,
                target: Language::Ru,
                english: English::Us,
            },
            Request {
                text: BANCO_SENTENCE.to_owned(),
                context: None,
                source: Language::Es,
                target: Language::Ru,
                english: English::Us,
            },
        ]
    );
    // The same language again sends nothing.
    desk.switch(Language::Es);
    assert_eq!(desk.asked().len(), 4);
    // A submitted lone word is sent again; one that was not, is not.
    let mut lone = Desk::with_text(Ok("банк"), "banco");
    lone.switch(Language::Es);
    assert!(lone.asked().is_empty());
    lone.submit();
    lone.switch(Language::Ca);
    assert_eq!(
        lone.asked(),
        [
            Request {
                text: "banco".to_owned(),
                context: None,
                source: Language::Es,
                target: Language::Ru,
                english: English::Us,
            },
            Request {
                text: "banco".to_owned(),
                context: None,
                source: Language::Ca,
                target: Language::Ru,
                english: English::Us,
            },
        ]
    );
}

#[test]
fn changing_the_language_without_a_word_sends_nothing() {
    let mut desk = Desk::with_text(Ok("банк"), BANCO_SENTENCE);
    desk.switch(Language::Es);
    desk.switch(Language::Ca);
    assert!(desk.asked().is_empty());
    let mut empty = Desk::new(Ok("банк"));
    empty.switch(Language::Es);
    assert!(empty.asked().is_empty());
}

#[test]
fn the_first_click_sends_the_word_with_its_sentence_and_the_sentence() {
    let mut desk = Desk::with_text(Ok("берег"), RIVER);
    desk.click(BANK);
    assert_eq!(desk.asked(), [sentence_request(), bank_request()]);
    assert_eq!(
        desk.row(OnlineKind::Word),
        Some((OnlineState::Translated, "берег"))
    );
    assert_eq!(
        desk.row(OnlineKind::Sentence),
        Some((OnlineState::Translated, "берег"))
    );
}

/// The text of 21-27-22 of EXP-8 at 2×, psm 6, as Tesseract gave it.
const PITFALL: &str = "When we talk about problem size, we have to be careful about which size, or sizes, we\nare talking about. This example demonstrates a pitfall of algorithm analysis: the\ntempting shortcut of counting loops. If there is one loop, the algorithm is often linear.\nIf there are two loops (one nested inside the other), the algorithm is often quadratic.\nBut be careful! You have to think about how many times each loop runs. If the num-\nber of iterations is proportional to n for all loops, you can get away with just counting\nthe loops. But if, as in this example, the number of iterations is not always propor-\ntional to n, you have to give it more thought.";

#[test]
fn a_word_of_a_recognized_text_sends_its_sentence_only() {
    let mut desk = Desk::new(Ok("число"));
    let text = addict_core::sentence::recognized(PITFALL).expect("text");
    desk.step(Event::FieldEdited {
        text,
        replaced: true,
        source: Some(Source::Ocr),
    });
    // The field is over the limit; the sentence is not.
    assert!(is_long(desk.view.sentence.as_str()));
    let iterations = desk
        .view
        .words
        .iter()
        .position(|range| &desk.view.sentence.as_str()[range.clone()] == "iterations")
        .expect("word");
    desk.click(iterations);
    let sentence = "If the number of iterations is proportional to n for all loops, you can get away with just counting the loops.";
    assert_eq!(
        desk.asked(),
        [
            Request {
                text: sentence.to_owned(),
                context: None,
                source: Language::En,
                target: Language::Ru,
                english: English::Us,
            },
            Request {
                text: "iterations".to_owned(),
                context: Some(sentence.to_owned()),
                source: Language::En,
                target: Language::Ru,
                english: English::Us,
            },
        ]
    );
    // A word of another sentence sends that sentence.
    let pitfall = desk
        .view
        .words
        .iter()
        .position(|range| &desk.view.sentence.as_str()[range.clone()] == "pitfall")
        .expect("word");
    desk.click(pitfall);
    let other = "This example demonstrates a pitfall of algorithm analysis: the tempting shortcut of counting loops.";
    assert_eq!(
        desk.asked()[2..],
        [
            Request {
                text: other.to_owned(),
                context: None,
                source: Language::En,
                target: Language::Ru,
                english: English::Us,
            },
            Request {
                text: "pitfall".to_owned(),
                context: Some(other.to_owned()),
                source: Language::En,
                target: Language::Ru,
                english: English::Us,
            },
        ]
    );
}

#[test]
fn the_same_word_again_sends_nothing() {
    let mut desk = Desk::with_text(Ok("берег"), RIVER);
    desk.click(BANK);
    desk.click(BANK);
    desk.click(BANK);
    assert_eq!(desk.asked(), [sentence_request(), bank_request()]);
    assert_eq!(
        desk.row(OnlineKind::Word),
        Some((OnlineState::Translated, "берег"))
    );
}

#[test]
fn another_word_sends_itself_only_and_going_back_sends_nothing() {
    let mut desk = Desk::with_text(Ok("берег"), RIVER);
    desk.click(BANK);
    desk.click(RIVER_WORD);
    let sent = [sentence_request(), bank_request(), river_request()];
    assert_eq!(desk.asked(), sent);
    // The sentence stays while another word is the chosen one.
    assert_eq!(
        desk.row(OnlineKind::Sentence),
        Some((OnlineState::Translated, "берег"))
    );
    desk.click(BANK);
    desk.click(RIVER_WORD);
    assert_eq!(desk.asked(), sent);
}

#[test]
fn what_was_refused_is_sent_again_by_the_next_click() {
    let mut desk = Desk::with_text(Err(Refusal::Quota), RIVER);
    desk.click(BANK);
    assert_eq!(desk.row(OnlineKind::Word).unwrap().0, OnlineState::Failed);
    assert_eq!(
        desk.row(OnlineKind::Sentence).unwrap().0,
        OnlineState::Failed
    );
    desk.click(BANK);
    // The sentence again by a click on any word.
    desk.click(RIVER_WORD);
    assert_eq!(
        desk.asked(),
        [
            sentence_request(),
            bank_request(),
            sentence_request(),
            bank_request(),
            sentence_request(),
            river_request(),
        ]
    );
}

#[test]
fn a_long_text_sends_the_word_with_the_text_as_context_and_no_sentence() {
    let text = format!("{}.", text_at_the_limit());
    assert_eq!(text.chars().count(), SENTENCE_LIMIT + 1);
    let mut desk = Desk::with_text(Ok("берег"), &text);
    desk.click(BANK);
    desk.click(BANK);
    assert_eq!(
        desk.asked(),
        [Request {
            text: "bank".to_owned(),
            context: Some(text.clone()),
            source: Language::En,
            target: Language::Ru,
            english: English::Us,
        }]
    );
    assert_eq!(
        desk.row(OnlineKind::Sentence),
        Some((
            OnlineState::Failed,
            "The sentence is too long to translate."
        ))
    );
    desk.click(RIVER_WORD);
    assert_eq!(desk.asked().len(), 2);
    assert_eq!(
        desk.asked()[1],
        Request {
            text: "river".to_owned(),
            context: Some(text),
            source: Language::En,
            target: Language::Ru,
            english: English::Us,
        }
    );
}

#[test]
fn a_text_of_exactly_the_limit_is_sent_as_a_sentence() {
    let text = text_at_the_limit();
    let mut desk = Desk::with_text(Ok("берег"), &text);
    desk.click(BANK);
    assert_eq!(desk.asked().len(), 2);
    assert_eq!(
        desk.asked()[0],
        Request {
            text,
            context: None,
            source: Language::En,
            target: Language::Ru,
            english: English::Us,
        }
    );
}

#[test]
fn a_new_text_drops_the_online_texts_and_asks_anew() {
    let mut desk = Desk::with_text(Ok("берег"), RIVER);
    desk.click(BANK);
    let edited = "I sat on the bank of the river";
    desk.edit(edited);
    assert_eq!(desk.view.online, []);
    assert_eq!(desk.asked().len(), 2);
    desk.click(BANK);
    assert_eq!(desk.asked().len(), 4);
    assert_eq!(
        desk.asked()[2..],
        [
            lone_request(edited),
            Request {
                text: "bank".to_owned(),
                context: Some(edited.to_owned()),
                source: Language::En,
                target: Language::Ru,
                english: English::Us,
            },
        ]
    );
}

#[test]
fn a_pasted_sentence_sends_nothing_until_a_word_is_clicked() {
    let mut desk = Desk::new(Ok("берег"));
    desk.paste(RIVER);
    // Enter in it sends nothing either.
    desk.submit();
    assert!(desk.asked().is_empty());
    assert_eq!(desk.view.online, []);
    desk.click(BANK);
    assert_eq!(desk.asked(), [sentence_request(), bank_request()]);
    desk.submit();
    assert_eq!(desk.asked().len(), 2);
}

#[test]
fn a_typed_lone_word_sends_nothing_typed_or_clicked() {
    let mut desk = Desk::new(Ok("банк"));
    for text in ["b", "ba", "ban", "bank"] {
        desk.edit(text);
        assert_eq!(desk.view.input.as_ref().expect("input").word(), text);
    }
    desk.click(0);
    desk.click(0);
    assert!(desk.asked().is_empty());
    assert_eq!(desk.view.online, []);
}

#[test]
fn enter_sends_a_lone_word_without_context_once() {
    let mut desk = Desk::with_text(Ok("банк"), "bank");
    desk.submit();
    assert_eq!(desk.asked(), [lone_request("bank")]);
    assert_eq!(
        desk.row(OnlineKind::Lone),
        Some((OnlineState::Translated, "банк"))
    );
    desk.submit();
    desk.click(0);
    assert_eq!(desk.asked(), [lone_request("bank")]);
}

#[test]
fn a_pasted_lone_word_is_sent_as_enter_sends_it() {
    let mut desk = Desk::new(Ok("банк"));
    desk.paste(" Bank. ");
    assert_eq!(desk.asked(), [lone_request("Bank")]);
    assert_eq!(
        desk.row(OnlineKind::Lone),
        Some((OnlineState::Translated, "банк"))
    );
    desk.submit();
    assert_eq!(desk.asked().len(), 1);
    // The same word pasted again is a new text.
    desk.paste(" Bank. ");
    assert_eq!(desk.asked(), [lone_request("Bank"), lone_request("Bank")]);
}

#[test]
fn a_refused_lone_word_is_sent_again_by_the_next_enter() {
    let mut desk = Desk::with_text(Err(Refusal::Key), "bank");
    desk.submit();
    assert_eq!(desk.row(OnlineKind::Lone).unwrap().0, OnlineState::Failed);
    desk.submit();
    assert_eq!(desk.asked(), [lone_request("bank"), lone_request("bank")]);
}

#[test]
fn enter_in_an_empty_field_sends_nothing() {
    let mut desk = Desk::new(Ok("банк"));
    desk.submit();
    desk.edit(" 12 ");
    desk.submit();
    assert!(desk.asked().is_empty());
    assert_eq!(desk.view.online, []);
}

#[test]
fn every_online_failure_preserves_the_field_and_dictionary_article() {
    for refusal in [
        Refusal::Key,
        Refusal::UnreadableKey,
        Refusal::UnsafeKey,
        Refusal::RejectedKey,
        Refusal::Network,
        Refusal::Quota,
        Refusal::UnsupportedLanguage,
        Refusal::TimedOut,
        Refusal::Other,
    ] {
        let data = TestData::with_bank();
        let mut desk = Desk::with_data(Err(refusal), Some(data.0.clone()));
        desk.edit(RIVER);
        desk.click(BANK);
        assert_eq!(desk.row(OnlineKind::Word).unwrap().0, OnlineState::Failed);
        assert_eq!(
            desk.view.input.as_ref().expect("input").sentence(),
            Some(RIVER)
        );
        assert_eq!(article_definition(&desk.view), DEFINITION);
        desk.edit("river");
        assert_eq!(desk.view.sentence.as_str(), "river");
        desk.submit();
        assert_eq!(desk.row(OnlineKind::Lone).unwrap().0, OnlineState::Failed);
    }
}

fn click(text: u64, index: usize) -> Event {
    Event::WordChosen { text, index }
}

fn sense(article: ArticleId, index: usize) -> Event {
    Event::SenseChosen { article, index }
}

fn definition(article: ArticleId, index: usize) -> Event {
    Event::DefinitionAction {
        article,
        choice: Choice::Sense(index),
    }
}

fn next_request(asked: &mpsc::Receiver<Asked>, expected: &Request) -> oneshot::Sender<Answer> {
    let Asked { request, answer } = asked.recv_timeout(WAIT).unwrap();
    assert_eq!(&request, expected);
    answer
}

fn assert_cancelled(answer: &oneshot::Sender<Answer>) {
    let until = std::time::Instant::now() + WAIT;
    while !answer.is_canceled() {
        assert!(
            std::time::Instant::now() < until,
            "translation was not cancelled"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn assert_record(effects: &[Effect], subject: &str, outcome: &str) {
    let [Effect::RecordOnline(record)] = effects else {
        panic!("expected only an online evidence record: {effects:?}");
    };
    assert_eq!(record.details()["subject"], subject);
    assert_eq!(record.details()["source"], "en");
    assert_eq!(record.details()["outcome"], outcome);
    assert!(record.details()["elapsed_ms"].is_u64());
}

#[test]
fn two_clicks_share_a_pending_sentence_and_only_the_current_word_waits_for_lookup() {
    let data = TestData::with_bank();
    let (mut shell, asked) = FakeShell::scripted(Some(data.0.clone()), true);
    shell.edit(RIVER);
    let (typed, _) = shell.next();
    let (release, _gate) = shell.hold_worker();
    shell.dispatch(click(typed.text_number, BANK));
    let (bank, _) = shell.next();
    assert!(bank.looking_up);
    assert!(pending(&bank, OnlineKind::Word));
    assert!(pending(&bank, OnlineKind::Sentence));

    shell.dispatch(click(typed.text_number, RIVER_WORD));
    let (river, _) = shell.next();
    assert!(river.looking_up);
    assert!(pending(&river, OnlineKind::Sentence));
    release.send(()).unwrap();

    let sentence = next_request(&asked, &sentence_request());
    let (river, _) = shell.settled();
    assert_eq!(article_definition(&river), super::RIVER);
    let word = next_request(&asked, &river_request());
    assert!(asked.try_recv().is_err());
    sentence.send(Ok("whole".into())).unwrap();
    let (shown, effects) = shell.next();
    assert_record(&effects, "sentence", "translated");
    assert!(pending(&shown, OnlineKind::Word));
    assert_eq!(shown.online[1].line, "whole");
    word.send(Ok("word".into())).unwrap();
    let (shown, effects) = shell.next();
    assert_record(&effects, "word", "translated");
    assert_eq!(shown.online[0].line, "word");
}

#[test]
fn stale_completions_record_evidence_and_cancelled_jobs_leave_no_rows_or_record() {
    let data = TestData::with_bank();
    let (mut shell, asked) = FakeShell::scripted(Some(data.0.clone()), true);
    shell.edit(RIVER);
    let (typed, _) = shell.next();
    shell.dispatch(click(typed.text_number, BANK));
    shell.next();
    let sentence = next_request(&asked, &sentence_request());
    shell.settled();
    let old = next_request(&asked, &bank_request());
    shell.dispatch(click(typed.text_number, RIVER_WORD));
    shell.next();
    shell.settled();
    let river = next_request(&asked, &river_request());

    // The first word was issued after lookup 1 and sentence 2, and carried
    // Kept's first pending identity. Simulate its completion already queued
    // behind the newer click.
    shell
        .controller
        .dispatch
        .0
        .send(crate::online::finished_word(
            RequestId(4),
            typed.text_number,
            BANK,
            1,
            Outcome::Finished(Ok("stale".into())),
        ))
        .unwrap();
    let (shown, effects) = shell.next();
    assert_record(&effects, "word", "translated");
    assert_eq!(shown.chosen, Some(RIVER_WORD));
    assert!(pending(&shown, OnlineKind::Word));
    assert!(pending(&shown, OnlineKind::Sentence));
    assert_cancelled(&old);

    shell.dispatch(Event::FieldCleared);
    let (cleared, effects) = shell.next();
    assert!(effects.is_empty());
    assert!(cleared.online.is_empty());
    assert!(cleared.shown.lookup().is_none());
    assert_cancelled(&sentence);
    assert_cancelled(&river);
    // A queued cancellation produces neither a render nor evidence.
    shell
        .controller
        .dispatch
        .0
        .send(crate::online::finished_word(
            RequestId(6),
            typed.text_number,
            RIVER_WORD,
            3,
            Outcome::Cancelled,
        ))
        .unwrap();
    shell.quiet(click(typed.text_number, BANK));
}

#[test]
fn definition_worker_sends_only_explicit_actions_and_allows_guarded_recovery() {
    let data = TestData::with_bank();
    let (mut shell, asked) = FakeShell::scripted(Some(data.0.clone()), true);
    shell.edit("bank");
    let (shown, _) = shell.settled();
    let article = shown.article_id();
    shell.dispatch(sense(article, 0));
    let (chosen, _) = shell.next();
    assert!(chosen.shown.definition().is_none());
    assert!(asked.try_recv().is_err());

    let request = lone_request(DEFINITION);
    shell.dispatch(definition(article, 0));
    let (pending, _) = shell.next();
    assert_eq!(pending.shown.definition(), Some(&Online::Pending));
    let answer = next_request(&asked, &request);
    shell.quiet(definition(article, 0));
    answer.send(Err(Refusal::Network)).unwrap();
    let (refused, effects) = shell.next();
    assert_eq!(
        refused.shown.definition(),
        Some(&Online::Refused(Refusal::Network))
    );
    assert_record(&effects, "definition", "network");

    shell.dispatch(definition(article, 0));
    shell.next();
    let answer = next_request(&asked, &request);
    // Spelling only changes English-target requests, so Russian mine keeps this job.
    shell.dispatch(Event::EnglishChosen(English::Gb));
    let (pending, _) = shell.next();
    assert_eq!(pending.shown.definition(), Some(&Online::Pending));
    answer.send(Ok("берег".into())).unwrap();
    let (translated, effects) = shell.next();
    assert_record(&effects, "definition", "translated");
    assert_eq!(
        translated.shown.definition(),
        Some(&Online::Translated("берег".into()))
    );
    shell.quiet(definition(article, 0));
}

#[test]
fn changing_sense_cancels_a_definition_worker_and_returning_requires_a_new_action() {
    let data = TestData::with_bank();
    let (mut shell, asked) = FakeShell::scripted(Some(data.0.clone()), true);
    shell.edit("bank");
    let (shown, _) = shell.settled();
    let article = shown.article_id();
    shell.dispatch(sense(article, 0));
    shell.next();
    shell.dispatch(definition(article, 0));
    shell.next();
    let request = lone_request(DEFINITION);
    let old = next_request(&asked, &request);

    shell.dispatch(sense(article, 1));
    let (changed, _) = shell.next();
    assert_eq!(changed.shown.choice(), Some(Choice::Sense(1)));
    assert!(changed.shown.definition().is_none());
    shell.quiet(definition(article, 0));
    shell.dispatch(sense(article, 0));
    let (chosen, _) = shell.next();
    assert!(chosen.shown.definition().is_none());
    assert!(asked.try_recv().is_err());

    shell.dispatch(definition(article, 0));
    shell.next();
    let new = next_request(&asked, &request);
    assert_cancelled(&old);
    new.send(Ok("fresh".into())).unwrap();
    let (ready, effects) = shell.next();
    assert_eq!(
        ready.shown.definition(),
        Some(&Online::Translated("fresh".into()))
    );
    assert_record(&effects, "definition", "translated");

    shell.edit("river");
    shell.settled();
    shell.quiet(sense(article, 0));
    shell.quiet(definition(article, 0));
}

#[test]
fn a_controller_translation_times_out_and_can_be_retried() {
    let (mut shell, asked) = FakeShell::scripted(None, true);
    shell.edit("bank");
    shell.settled();
    shell.dispatch(Event::FieldSubmitted);
    let (pending, _) = shell.next();
    assert!(super::pending(&pending, OnlineKind::Lone));
    let blocked = next_request(&asked, &lone_request("bank"));
    let (failed, effects) = shell.next_within(deadlines::TRANSLATE_LIMIT + WAIT);
    assert_eq!(failed.online[0].state, OnlineState::Failed);
    assert_record(&effects, "word", "timed-out");
    assert!(blocked.is_canceled());

    shell.dispatch(Event::FieldSubmitted);
    shell.next();
    let retry = next_request(&asked, &lone_request("bank"));
    retry.send(Ok("translated".into())).unwrap();
    let (shown, effects) = shell.next();
    assert_eq!(shown.online[0].line, "translated");
    assert_record(&effects, "word", "translated");
}

#[test]
fn an_english_gloss_link_submits_its_replacement_without_sending_the_old_word_again() {
    let data = TestData::with_banco();
    let (mut shell, asked) = FakeShell::scripted(Some(data.0.clone()), false);
    shell.dispatch(Event::ReadingChosen(Language::Es));
    shell.next();
    shell.edit("banco");
    let (shown, _) = shell.settled();
    let article = shown.article_id();
    shell.dispatch(Event::FieldSubmitted);
    shell.next();
    let old = next_request(
        &asked,
        &Request {
            source: Language::Es,
            ..lone_request("banco")
        },
    );
    shell.dispatch(Event::GlossChosen { article, index: 0 });
    shell.next();
    let gloss = next_request(&asked, &lone_request("bank"));

    let (release, _gate) = shell.hold_worker();
    shell.dispatch(Event::LookUpEnglish {
        article,
        index: 0,
        word: "bank".into(),
    });
    shell.dispatch(Event::MyLanguageChosen(Language::Ca));
    let (replacement, effects) = shell.next();
    assert_eq!(replacement.field.text, "bank");
    assert!(replacement.can_back);
    assert!(pending(&replacement, OnlineKind::Lone));
    assert!(replacement.shown.definition().is_none());
    assert!(
        effects.is_empty(),
        "the reading choice is written on the runner"
    );
    let (reconfigured, _) = shell.next();
    assert_eq!(reconfigured.shown.translating().my, Language::Ca);
    assert_eq!(reconfigured.shown.translating().pressed, Language::En);
    release.send(()).unwrap();
    let (bank, _) = shell.settled();
    assert_eq!(bank.answered, Some(Language::En));
    assert!(matches!(
        bank.shown.lookup(),
        Some(Lookup::Grouped {
            language: Language::En,
            ..
        })
    ));
    let current = next_request(
        &asked,
        &Request {
            target: Language::Ca,
            ..lone_request("bank")
        },
    );
    assert!(asked.try_recv().is_err());
    assert_cancelled(&old);
    assert_cancelled(&gloss);
    current.send(Ok("берег".into())).unwrap();
    let translated = shell.wait_for(|view| !pending(view, OnlineKind::Lone));
    assert_eq!(translated.online[0].line, "берег");
}

#[test]
fn changing_applicable_spelling_replaces_the_pending_request_for_the_same_text() {
    let (mut shell, asked) = FakeShell::scripted(None, false);
    shell.dispatch(Event::MyLanguageChosen(Language::En));
    shell.next();
    shell.edit("banco");
    shell.settled();
    shell.dispatch(Event::FieldSubmitted);
    let (us, _) = shell.next();
    let request = Request {
        source: Language::Es,
        target: Language::En,
        ..lone_request("banco")
    };
    let old = next_request(&asked, &request);

    shell.dispatch(Event::EnglishChosen(English::Gb));
    let (gb, _) = shell.next();
    assert_eq!(us.field, gb.field);
    assert_eq!(us.text_number, gb.text_number);
    assert!(pending(&gb, OnlineKind::Lone));
    let current = next_request(
        &asked,
        &Request {
            english: English::Gb,
            ..request
        },
    );
    assert_cancelled(&old);
    current.send(Ok("bank".into())).unwrap();
    let (translated, _) = shell.next();
    assert_eq!(translated.online[0].line, "bank");
}

#[test]
fn sentence_at_once_changes_the_next_eligible_read_only() {
    let (mut shell, asked) = FakeShell::scripted(None, false);
    let read = |text: &str, source| Event::FieldEdited {
        text: text.to_owned(),
        replaced: true,
        source: Some(source),
    };
    shell.dispatch(Event::AtOnceChosen(false));
    shell.next();
    shell.dispatch(read(RIVER, Source::Clipboard));
    let (shown, _) = shell.next();
    assert!(shown.online.is_empty());
    shell.dispatch(Event::AtOnceChosen(true));
    shell.next();
    assert!(asked.try_recv().is_err());

    // Capture and one word never ask for a whole sentence.
    shell.dispatch(read(RIVER, Source::Ocr));
    let (captured, _) = shell.next();
    assert!(captured.online.is_empty());
    shell.dispatch(read("bank", Source::Clipboard));
    let (lone, _) = shell.settled();
    assert!(lone.online.is_empty());
    assert!(asked.try_recv().is_err());

    shell.dispatch(read(
        &format!("{}.", text_at_the_limit()),
        Source::Clipboard,
    ));
    let (long, _) = shell.next();
    assert_eq!(
        long.online[0].line,
        "The sentence is too long to translate."
    );
    assert!(asked.try_recv().is_err());

    shell.dispatch(read(RIVER, Source::Clipboard));
    let (whole, _) = shell.next();
    assert_eq!(whole.chosen, None);
    let sentence = next_request(&asked, &sentence_request());
    shell.dispatch(Event::AtOnceChosen(false));
    shell.next();
    sentence.send(Ok("whole".into())).unwrap();
    let (answered, _) = shell.next();
    assert_eq!(answered.online[0].line, "whole");
}
