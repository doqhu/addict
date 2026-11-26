//! The DeepL client: translate, the languages and the usage.
//!
//! Every request snapshots the caller's key source first, so a key installed
//! while the application runs is used by the next request. A request without a
//! usable key sends nothing. The HTTP client, and with it the TLS state, is
//! built by the first request that has a key. No request is repeated, nothing
//! is cached in this crate and no connection is kept after its answer: between
//! two requests the client holds nothing that wakes. [`sent`] names every
//! field a translate request sends, so a cache outside this crate can key on
//! exactly what goes on the wire.
//!
//! No timer runs here. A request that gets no answer stays pending until its
//! future is dropped; the deadline is the caller's.

use std::sync::OnceLock;

use addict_core::translation::{English, Refusal, Request, Source, Translating, Translator};
use reqwest::{
    StatusCode,
    header::{AUTHORIZATION, HeaderValue},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::key::{KeyError, KeySource, Secret};

/// The host of the keys of the Free plan.
const FREE_HOST: &str = "https://api-free.deepl.com";

/// The host of every other key.
const PRO_HOST: &str = "https://api.deepl.com";

/// The `source_lang` of a request in `source`, as DeepL writes it. No
/// wildcard arm: a new language does not compile until it is given its code.
fn source_lang(source: Source) -> &'static str {
    match source {
        Source::En => "EN",
        Source::Es => "ES",
        Source::Ca => "CA",
        Source::Ru => "RU",
    }
}

/// The `target_lang` of a request to `target`, as DeepL writes it. English
/// always names its variant, `english`, US or British, so the answer does
/// not hang on a server default; any other target ignores it. No wildcard
/// arm: a new language or variant does not compile until it is given its
/// code.
fn target_lang(target: Source, english: English) -> &'static str {
    match (target, english) {
        (Source::En, English::Us) => "EN-US",
        (Source::En, English::Gb) => "EN-GB",
        (Source::Es, _) => "ES",
        (Source::Ca, _) => "CA",
        (Source::Ru, _) => "RU",
    }
}

/// What stands before the key in the `Authorization` header.
const AUTH_SCHEME: &str = "DeepL-Auth-Key ";

/// The status DeepL answers with when the quota is used up.
const QUOTA_EXCEEDED: u16 = 456;

/// The base URL of the requests made with `key`.
pub fn host(key: &Secret) -> &'static str {
    if key.is_free() { FREE_HOST } else { PRO_HOST }
}

/// One language DeepL knows for the translation of text.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Language {
    /// The code of the language, as DeepL writes it.
    pub lang: String,
    pub usable_as_source: bool,
    pub usable_as_target: bool,
}

/// How much of the quota of the key is used, in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Usage {
    pub character_count: u64,
    pub character_limit: u64,
}

/// The model every translate request asks for. Naming it keeps the answer
/// independent of a server default that can change.
const MODEL_TYPE: &str = "prefer_quality_optimized";

/// The formality of every translate request. It is not sent, so DeepL uses
/// its default; it is named here so a cache key can hold it.
const FORMALITY: &str = "default";

/// Every field of a translate request that can change its answer, with the
/// exact values the client sends for `request`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent<'a> {
    pub text: &'a str,
    pub context: Option<&'a str>,
    pub source_lang: &'static str,
    pub target_lang: &'static str,
    pub model_type: &'static str,
    /// Not sent: DeepL's default formality applies.
    pub formality: &'static str,
}

/// What a translate request for `request` sends. The body is built from it,
/// so a key built from it cannot drift from the wire.
pub fn sent(request: &Request) -> Sent<'_> {
    Sent {
        text: &request.text,
        context: request.context.as_deref(),
        source_lang: source_lang(request.source),
        target_lang: target_lang(request.target, request.english),
        model_type: MODEL_TYPE,
        formality: FORMALITY,
    }
}

/// The body of a translate request.
#[derive(Serialize)]
struct TranslateBody<'a> {
    text: [&'a str; 1],
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'a str>,
    source_lang: &'static str,
    target_lang: &'static str,
    model_type: &'static str,
}

impl<'a> From<Sent<'a>> for TranslateBody<'a> {
    fn from(sent: Sent<'a>) -> Self {
        Self {
            text: [sent.text],
            context: sent.context,
            source_lang: sent.source_lang,
            target_lang: sent.target_lang,
            model_type: sent.model_type,
        }
    }
}

/// The answer to a translate request.
#[derive(Deserialize)]
struct Translated {
    translations: Vec<Translation>,
}

#[derive(Deserialize)]
struct Translation {
    text: String,
}

/// The client. Requests use a snapshot of the caller's key source.
#[derive(Debug)]
pub struct DeepL {
    key_source: KeySource,
    /// The base URL of every request; `None` chooses the host by the key.
    base_url: Option<String>,
    /// Built by the first request that has a key.
    http: OnceLock<reqwest::Client>,
}

impl DeepL {
    /// Uses the supplied source and the host of its current key.
    pub fn new(key_source: KeySource) -> Self {
        Self {
            key_source,
            base_url: None,
            http: OnceLock::new(),
        }
    }

    /// A client that uses `key_source` and sends every request
    /// to `base_url`, whatever the key is.
    pub fn with_base_url(key_source: KeySource, base_url: impl Into<String>) -> Self {
        Self {
            key_source,
            base_url: Some(base_url.into().trim_end_matches('/').to_owned()),
            http: OnceLock::new(),
        }
    }

    /// The text for `request.text` in the language of `request.target`.
    /// `request.text` is in the language of `request.source` and is read in
    /// `request.context` when there is one. The context is not translated.
    pub async fn translate(&self, request: &Request) -> Result<String, Refusal> {
        let (http, base, authorization) = self.prepared().await?;
        let body = TranslateBody::from(sent(request));
        let sent = http
            .post(format!("{base}/v2/translate"))
            .header(AUTHORIZATION, authorization)
            .json(&body);
        let translated: Translated = answer(sent, true).await?;
        let first = translated.translations.into_iter().next();
        let text = first.map(|translation| translation.text.trim().to_owned());
        text.filter(|text| !text.is_empty()).ok_or(Refusal::Other)
    }

    /// The languages DeepL translates text from and to.
    pub async fn languages(&self) -> Result<Vec<Language>, Refusal> {
        let (http, base, authorization) = self.prepared().await?;
        let sent = http
            .get(format!("{base}/v3/languages?resource=translate_text"))
            .header(AUTHORIZATION, authorization);
        answer(sent, false).await
    }

    /// How much of the quota of the key is used.
    pub async fn usage(&self) -> Result<Usage, Refusal> {
        let (http, base, authorization) = self.prepared().await?;
        let sent = http
            .get(format!("{base}/v2/usage"))
            .header(AUTHORIZATION, authorization);
        answer(sent, false).await
    }

    /// What every request needs: the HTTP client, the base URL and the
    /// header with the key from one snapshot. Without a usable key
    /// nothing else is done.
    async fn prepared(&self) -> Result<(&reqwest::Client, &str, HeaderValue), Refusal> {
        let key = self.key_source.ready().await.map_err(|error| match error {
            KeyError::NoDirectory | KeyError::Missing | KeyError::Empty => Refusal::Key,
            KeyError::Unreadable => Refusal::UnreadableKey,
            KeyError::TooOpen => Refusal::UnsafeKey,
        })?;
        let mut authorization = HeaderValue::from_str(&format!("{AUTH_SCHEME}{}", key.secret()))
            .map_err(|_| Refusal::Key)?;
        // Keeps the key out of the `Debug` output of the request.
        authorization.set_sensitive(true);
        let base = self.base_url.as_deref().unwrap_or_else(|| host(&key));
        Ok((self.http()?, base, authorization))
    }

    /// The HTTP client, built at the first call.
    fn http(&self) -> Result<&reqwest::Client, Refusal> {
        if let Some(http) = self.http.get() {
            return Ok(http);
        }
        let built = reqwest::Client::builder()
            .user_agent(concat!("addict/", env!("CARGO_PKG_VERSION")))
            // The key goes to the host it was sent to and nowhere else.
            .redirect(reqwest::redirect::Policy::none())
            // No connection outlives its answer, so nothing here wakes an
            // idle process.
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|_| Refusal::Other)?;
        Ok(self.http.get_or_init(|| built))
    }
}

impl Translator for DeepL {
    fn translate(&self, request: Request) -> Translating<'_> {
        Box::pin(async move { DeepL::translate(self, &request).await })
    }
}

/// Sends `request` once and reads the answer. The error of the HTTP client
/// is reduced to its kind here and goes no further.
async fn answer<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    translation: bool,
) -> Result<T, Refusal> {
    let response = request.send().await.map_err(|error| refusal(&error))?;
    match response.status() {
        StatusCode::OK => {
            let body = response.bytes().await.map_err(|_| Refusal::Network)?;
            serde_json::from_slice(&body).map_err(|_| Refusal::Other)
        }
        StatusCode::BAD_REQUEST if translation => {
            #[derive(Deserialize)]
            struct ErrorBody {
                message: String,
            }
            let body = response.bytes().await.map_err(|_| Refusal::Network)?;
            let error = serde_json::from_slice::<ErrorBody>(&body);
            // Match only recognized language rejection messages. Other bad
            // parameters and malformed error bodies remain generic.
            let unsupported = error.is_ok_and(|body| {
                let message = body
                    .message
                    .trim()
                    .trim_end_matches('.')
                    .to_ascii_lowercase();
                matches!(
                    message.as_str(),
                    "value for 'source_lang' not supported"
                        | "value for 'target_lang' not supported"
                        | "source language not supported"
                        | "target language not supported"
                )
            });
            Err(if unsupported {
                Refusal::UnsupportedLanguage
            } else {
                Refusal::Other
            })
        }
        status => Err(refused(status)),
    }
}

/// The refusal of a status that is not 200. No wildcard beyond the last arm:
/// 429, 5xx and everything else are one kind.
fn refused(status: StatusCode) -> Refusal {
    match status.as_u16() {
        403 => Refusal::RejectedKey,
        QUOTA_EXCEEDED => Refusal::Quota,
        _ => Refusal::Other,
    }
}

/// Transport errors are distinct from malformed responses. There is no timeout, so a
/// request that ran out of time is known only by the caller's deadline.
fn refusal(error: &reqwest::Error) -> Refusal {
    if error.is_connect() || error.is_body() || (error.is_request() && !error.is_builder()) {
        Refusal::Network
    } else {
        Refusal::Other
    }
}

#[cfg(test)]
mod tests {
    use addict_core::translation::{English, Refusal, Request, Source};
    use reqwest::StatusCode;

    use super::{refused, sent};

    #[test]
    fn the_target_of_a_request_is_its_target_lang() {
        for (target, english, code) in [
            (Source::Ru, English::Us, "RU"),
            (Source::En, English::Us, "EN-US"),
            (Source::En, English::Gb, "EN-GB"),
            (Source::Es, English::Us, "ES"),
            (Source::Ca, English::Us, "CA"),
        ] {
            let request = Request {
                text: "banco".to_owned(),
                context: None,
                source: Source::Es,
                target,
                english,
            };
            assert_eq!(sent(&request).target_lang, code, "{target:?} {english:?}");
            assert_eq!(sent(&request).source_lang, "ES", "{target:?}");
        }
    }

    #[test]
    fn the_english_variant_changes_nothing_for_any_other_target() {
        for target in [Source::Es, Source::Ca, Source::Ru] {
            let request = |english| Request {
                text: "bank".to_owned(),
                context: Some("the bank of the river".to_owned()),
                source: Source::En,
                target,
                english,
            };
            assert_eq!(
                sent(&request(English::Us)),
                sent(&request(English::Gb)),
                "{target:?}"
            );
        }
    }

    #[test]
    fn russian_as_a_source_is_ru() {
        let request = Request {
            text: "берег".to_owned(),
            context: None,
            source: Source::Ru,
            target: Source::En,
            english: English::Us,
        };
        assert_eq!(sent(&request).source_lang, "RU");
        assert_eq!(sent(&request).target_lang, "EN-US");
    }

    #[test]
    fn a_rejected_key_and_a_used_up_quota_have_their_own_refusals() {
        let status = |code| StatusCode::from_u16(code).expect("status");
        assert_eq!(refused(status(403)), Refusal::RejectedKey);
        assert_eq!(refused(status(456)), Refusal::Quota);
        for other in [201, 204, 301, 400, 401, 404, 413, 429, 500, 503, 529] {
            assert_eq!(refused(status(other)), Refusal::Other, "{other}");
        }
    }
}
