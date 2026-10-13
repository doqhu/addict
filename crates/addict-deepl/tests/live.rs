//! The live check of the user's key: asks DeepL itself, so it runs only when
//! asked for, with `cargo test -p addict-deepl -- --ignored --nocapture`.

use addict_deepl::DeepL;

#[tokio::test]
#[ignore = "calls DeepL with the key of ~/.config/addict/deepl.key"]
async fn the_key_of_the_user_reads_the_languages_and_the_usage() {
    let client = DeepL::new();
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
