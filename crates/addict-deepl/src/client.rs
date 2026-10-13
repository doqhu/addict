//! The DeepL client: translate, the languages and the usage.
//!
//! Every request reads the key file first, so a key written while the
//! application runs is found by the next request, and a request without a
//! usable key sends nothing. The HTTP client, and with it the TLS state, is
//! built by the first request that has a key. No request is repeated, nothing
//! is cached and no connection is kept after its answer: between two requests
//! the client holds nothing that wakes.
//!
//! No timer runs here. A request that gets no answer stays pending until its
//! future is dropped; the deadline is the caller's.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use addict_core::translation::{Refusal, Request, Translating, Translator};
use reqwest::{
    StatusCode,
    header::{AUTHORIZATION, HeaderValue},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::key::{self, Key};

/// The host of the keys of the Free plan.
const FREE_HOST: &str = "https://api-free.deepl.com";

/// The host of every other key.
const PRO_HOST: &str = "https://api.deepl.com";

/// The language of what is translated and the language it is translated to.
const SOURCE_LANG: &str = "EN";
const TARGET_LANG: &str = "RU";

/// What stands before the key in the `Authorization` header.
const AUTH_SCHEME: &str = "DeepL-Auth-Key ";

/// The status DeepL answers with when the quota is used up.
const QUOTA_EXCEEDED: u16 = 456;

/// The base URL of the requests made with `key`.
pub fn host(key: &Key) -> &'static str {
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

/// The body of a translate request.
#[derive(Serialize)]
struct TranslateBody<'a> {
    text: [&'a str; 1],
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'a str>,
    source_lang: &'static str,
    target_lang: &'static str,
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

/// The client. It holds the path of the key file and never the key.
#[derive(Debug)]
pub struct DeepL {
    /// `None` when the environment names no configuration directory.
    key_file: Option<PathBuf>,
    /// The base URL of every request; `None` chooses the host by the key.
    base_url: Option<String>,
    /// Built by the first request that has a key.
    http: OnceLock<reqwest::Client>,
}

impl DeepL {
    /// The client of the product: the key file of [`key::key_path`] and the
    /// host of the key. Nothing is read or opened here.
    pub fn new() -> Self {
        Self {
            key_file: key::key_path(),
            base_url: None,
            http: OnceLock::new(),
        }
    }

    /// A client that reads the key from `key_file` and sends every request
    /// to `base_url`, whatever the key is.
    pub fn with_base_url(key_file: impl Into<PathBuf>, base_url: impl Into<String>) -> Self {
        Self {
            key_file: Some(key_file.into()),
            base_url: Some(base_url.into().trim_end_matches('/').to_owned()),
            http: OnceLock::new(),
        }
    }

    /// The configured path captured when this client was created.
    pub fn key_file(&self) -> Option<&Path> {
        self.key_file.as_deref()
    }

    /// The Russian text for the English `request.text`, read in
    /// `request.context` when there is one. The context is not translated.
    pub async fn translate(&self, request: &Request) -> Result<String, Refusal> {
        let (http, base, authorization) = self.prepared()?;
        let body = TranslateBody {
            text: [&request.text],
            context: request.context.as_deref(),
            source_lang: SOURCE_LANG,
            target_lang: TARGET_LANG,
        };
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
        let (http, base, authorization) = self.prepared()?;
        let sent = http
            .get(format!("{base}/v3/languages?resource=translate_text"))
            .header(AUTHORIZATION, authorization);
        answer(sent, false).await
    }

    /// How much of the quota of the key is used.
    pub async fn usage(&self) -> Result<Usage, Refusal> {
        let (http, base, authorization) = self.prepared()?;
        let sent = http
            .get(format!("{base}/v2/usage"))
            .header(AUTHORIZATION, authorization);
        answer(sent, false).await
    }

    /// What every request needs: the HTTP client, the base URL and the
    /// header with the key, read from its file now. Without a usable key
    /// nothing else is done.
    fn prepared(&self) -> Result<(&reqwest::Client, &str, HeaderValue), Refusal> {
        let key_file = self.key_file.as_deref().ok_or(Refusal::Key)?;
        let key = key::read(key_file).map_err(|error| match error {
            key::KeyError::Missing | key::KeyError::Empty => Refusal::Key,
            key::KeyError::Unreadable => Refusal::UnreadableKey,
            key::KeyError::TooOpen => Refusal::UnsafeKey,
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

impl Default for DeepL {
    fn default() -> Self {
        Self::new()
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
    use addict_core::translation::Refusal;
    use reqwest::StatusCode;

    use super::refused;

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
