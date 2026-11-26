//! The live check of the user's key: asks DeepL itself, so it runs only when
//! asked for, with `cargo test -p addict-deepl -- --ignored --nocapture`.
//! The same command sends a word in its sentence for every ordered pair of
//! the four languages, and a Spanish sentence to US and to British
//! English.

use std::{env, path::PathBuf};

use addict_core::translation::{English, Request, Source, Translator};
use addict_deepl::{
    DeepL,
    key::{self, KeyError, KeySource},
};

/// The Linux key location used by the opt-in live tests.
fn key_file() -> Option<PathBuf> {
    let config = match env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        Some(config) if config.is_absolute() => config,
        _ => PathBuf::from(env::var_os("HOME").filter(|home| !home.is_empty())?).join(".config"),
    };
    key::key_path(Some(&config.join("addict")))
}

fn live_client() -> DeepL {
    let initial = key_file()
        .as_deref()
        .map_or(Err(KeyError::NoDirectory), key::read);
    DeepL::new(KeySource::new(initial))
}

#[tokio::test]
#[ignore = "calls DeepL with the key of ~/.config/addict/deepl.key"]
async fn the_key_of_the_user_reads_the_languages_and_the_usage() {
    let client = live_client();
    let languages = client.languages().await.expect("the languages");
    println!("languages: {}", languages.len());
    let find = |code: &str| {
        languages
            .iter()
            .find(|language| language.lang.eq_ignore_ascii_case(code))
    };
    let en = find("en").expect("en is listed");
    let ru = find("ru").expect("ru is listed");
    println!("en usable as source: {}", en.usable_as_source);
    println!("ru usable as target: {}", ru.usable_as_target);
    let usage = client.usage().await.expect("the usage");
    println!(
        "usage: {} of {} characters",
        usage.character_count, usage.character_limit
    );
    assert!(en.usable_as_source);
    assert!(ru.usable_as_target);
}

/// The word and its sentence of every language, as the spike sent them.
const WORDS: [(Source, &str, &str); 4] = [
    (Source::En, "red", "I like the colour red."),
    (Source::Es, "rojo", "Me gusta el color rojo."),
    (Source::Ca, "vermell", "M'agrada el color vermell."),
    (Source::Ru, "красный", "Мне нравится красный цвет."),
];

#[tokio::test]
#[ignore = "calls DeepL with the key of ~/.config/addict/deepl.key"]
async fn every_ordered_pair_translates_a_word_in_its_sentence() {
    let client = live_client();
    let mut pairs = 0;
    for (source, word, sentence) in WORDS {
        for target in Source::ALL {
            if target == source {
                continue;
            }
            let request = Request {
                text: word.to_owned(),
                context: Some(sentence.to_owned()),
                source,
                target,
                english: English::Us,
            };
            let answer = Translator::translate(&client, request).await;
            println!(
                "{} -> {}: {word} in “{sentence}”: {answer:?}",
                source.code(),
                target.code()
            );
            let answer = answer
                .unwrap_or_else(|refusal| panic!("{source:?} -> {target:?} refused: {refusal:?}"));
            assert!(!answer.trim().is_empty(), "{source:?} -> {target:?}");
            pairs += 1;
        }
    }
    assert_eq!(pairs, 12);
}

#[tokio::test]
#[ignore = "calls DeepL with the key of ~/.config/addict/deepl.key"]
async fn us_and_british_english_spell_the_colour_their_own_way() {
    let client = live_client();
    for (english, spelling) in [(English::Us, "color"), (English::Gb, "colour")] {
        let request = Request {
            text: "Me gusta el color rojo".to_owned(),
            context: None,
            source: Source::Es,
            target: Source::En,
            english,
        };
        let answer = Translator::translate(&client, request).await;
        println!("es -> en {}: {answer:?}", english.code());
        let answer = answer.unwrap_or_else(|refusal| panic!("{english:?} refused: {refusal:?}"));
        assert!(
            answer.to_lowercase().contains(spelling),
            "{english:?}: {answer:?}"
        );
    }
}
