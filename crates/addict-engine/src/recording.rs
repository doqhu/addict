//! Credits for Wikimedia Commons recordings referenced by a dictionary.

use std::{error::Error, fmt, io, net::ToSocketAddrs, sync::Arc, time::Duration};

use percent_encoding::percent_decode_str;
use quick_xml::{Reader, escape::resolve_html5_entity, events::Event};
use reqwest::{
    Client, Url,
    dns::{Addrs, Name, Resolve, Resolving},
    redirect::Policy,
};
use serde_json::Value;

const COMMONS: &str = "https://commons.wikimedia.org";
const API: &str = "https://commons.wikimedia.org/w/api.php";
const USER_AGENT: &str = concat!(
    "Addict/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/doqhu/addict)"
);

/// The worker deadline, with time for both requests when needed.
pub const CREDIT_LIMIT: Duration = Duration::from_secs(10);

/// Deadline and size bound for buffering a remote pronunciation for GTK.
pub const AUDIO_LIMIT: Duration = Duration::from_secs(20);
const AUDIO_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFailure {
    Network,
    Failed,
}

/// Fetches the URL from the dictionary independently of desktop VFS helpers.
pub async fn fetch_audio(url: String) -> Result<Vec<u8>, AudioFailure> {
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .dns_resolver(Arc::new(AudioResolver))
        .timeout(Duration::from_secs(18))
        .build()
        .map_err(audio_failure)?;
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(audio_failure)?;
    if response
        .content_length()
        .is_some_and(|bytes| bytes > AUDIO_MAX_BYTES as u64)
    {
        return Err(AudioFailure::Failed);
    }
    let mut audio = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(audio_failure)? {
        if chunk.len() > AUDIO_MAX_BYTES - audio.len() {
            return Err(AudioFailure::Failed);
        }
        audio.extend_from_slice(&chunk);
    }
    if audio.is_empty() {
        return Err(AudioFailure::Failed);
    }
    Ok(audio)
}

struct AudioResolver;

impl Resolve for AudioResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            // Use the system's getaddrinfo on a blocking thread, like reqwest's default.
            let addrs = tokio::task::spawn_blocking(move || (host.as_str(), 0).to_socket_addrs())
                .await?
                .map_err(AudioDnsError)?;
            Ok(Box::new(addrs) as Addrs)
        })
    }
}

#[derive(Debug)]
struct AudioDnsError(io::Error);

impl fmt::Display for AudioDnsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "recording host resolution failed: {}", self.0)
    }
}

impl Error for AudioDnsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

fn audio_failure(error: reqwest::Error) -> AudioFailure {
    if error.is_timeout() {
        return AudioFailure::Network;
    }
    let mut cause = error.source();
    while let Some(error) = cause {
        if error.is::<AudioDnsError>()
            || error.downcast_ref::<io::Error>().is_some_and(|error| {
                matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionRefused
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::HostUnreachable
                        | io::ErrorKind::NetworkDown
                        | io::ErrorKind::NetworkUnreachable
                        | io::ErrorKind::TimedOut
                )
            })
        {
            return AudioFailure::Network;
        }
        cause = error.source();
    }
    AudioFailure::Failed
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommonsFile(String);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Credit {
    pub author: Option<String>,
    pub license: Option<String>,
}

impl CommonsFile {
    /// Finds the original file name, including when the recording is a transcode.
    pub fn from_recording_url(recording: &str) -> Option<Self> {
        let url = Url::parse(recording).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let path: Vec<_> = url.path_segments()?.collect();
        let filename = match (url.host_str()?, path.as_slice()) {
            ("commons.wikimedia.org", ["wiki", special, filename])
                if special.eq_ignore_ascii_case("Special:FilePath") =>
            {
                filename
            }
            ("upload.wikimedia.org", ["wikipedia", "commons", _, _, filename])
            | ("upload.wikimedia.org", ["wikipedia", "commons", "transcoded", _, _, filename, _]) => {
                filename
            }
            _ => return None,
        };
        let filename = percent_decode_str(filename).decode_utf8().ok()?;
        if filename.trim().is_empty()
            || filename.contains(['#', '<', '>', '[', ']', '|', '{', '}'])
            || filename.chars().any(char::is_control)
        {
            return None;
        }
        Some(Self(filename.into_owned()))
    }

    fn title(&self) -> String {
        format!("File:{}", self.0)
    }

    /// The description page holds the original file and its license.
    pub fn page_url(&self) -> Url {
        let mut url = Url::parse(COMMONS).expect("Commons base URL");
        url.path_segments_mut()
            .expect("hierarchical URL")
            .extend(["wiki", &self.title()]);
        url
    }

    /// Loads structured credit from Commons, falling back to an explicit
    /// Shtooka credit in the description when no author was supplied.
    pub async fn fetch_credit(&self) -> Credit {
        let Ok(client) = Client::builder()
            .user_agent(USER_AGENT)
            .redirect(Policy::none())
            .timeout(Duration::from_secs(4))
            .build()
        else {
            return Credit::default();
        };
        let Some(page) = self
            .query_page(
                &client,
                &[
                    ("prop", "imageinfo"),
                    ("iiprop", "extmetadata"),
                    ("iiextmetadatafilter", "Artist|Attribution|LicenseShortName"),
                ],
            )
            .await
        else {
            return Credit::default();
        };
        let mut credit = structured_credit(&page);
        if credit.author.is_none() {
            let revision = self
                .query_page(
                    &client,
                    &[
                        ("prop", "revisions"),
                        ("rvprop", "content"),
                        ("rvslots", "main"),
                    ],
                )
                .await;
            credit.author = revision
                .as_ref()
                .and_then(shtooka_credit)
                .map(str::to_owned);
        }
        credit
    }

    async fn query_page(&self, client: &Client, props: &[(&str, &str)]) -> Option<Value> {
        let value = client
            .get(API)
            .query(&[
                ("action", "query"),
                ("format", "json"),
                ("formatversion", "2"),
                ("titles", &self.title()),
            ])
            .query(props)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .bytes()
            .await
            .ok()?;
        first_file_page(serde_json::from_slice(&value).ok()?)
    }
}

fn first_file_page(mut response: Value) -> Option<Value> {
    let page = response.pointer_mut("/query/pages/0")?;
    (page["ns"].as_u64() == Some(6) && page.get("missing").is_none()).then(|| page.take())
}

fn structured_credit(page: &Value) -> Credit {
    let metadata = &page["imageinfo"][0]["extmetadata"];
    let field = |name: &str| plain_text(metadata[name]["value"].as_str()?);
    let author = field("Attribution").or_else(|| field("Artist"));
    let license = field("LicenseShortName").map(|license| {
        if license.eq_ignore_ascii_case("CC BY 2.0 FR") {
            "CC BY 2.0 FR".to_owned()
        } else {
            license
        }
    });
    Credit { author, license }
}

fn shtooka_credit(page: &Value) -> Option<&'static str> {
    let source = page.pointer("/revisions/0/slots/main/content")?.as_str()?;
    source
        .contains("Audio sample provided by [[Commons:Shtooka|The Shtooka Project]]")
        .then_some("The Shtooka Project")
}

/// The API wraps fields in HTML fragments. Keep their text and resolve
/// entities before passing the fields to the UI as plain text.
fn plain_text(html: &str) -> Option<String> {
    let mut reader = Reader::from_str(html);
    // HTML has unclosed void elements and need not have a single root.
    reader.config_mut().check_end_names = false;
    reader.config_mut().allow_dangling_amp = true;
    let separates_words = |name: &[u8]| matches!(name, b"br" | b"div" | b"li" | b"p");
    let mut text = String::new();
    loop {
        match reader.read_event().ok()? {
            Event::Text(part) => text.push_str(&part.decode().ok()?),
            Event::GeneralRef(reference) => {
                if let Some(character) = reference.resolve_char_ref().ok()? {
                    text.push(character);
                } else if let Some(value) = resolve_html5_entity(&reference.decode().ok()?) {
                    text.push_str(value);
                }
            }
            Event::Start(tag) | Event::Empty(tag) if separates_words(tag.name().as_ref()) => {
                text.push(' ');
            }
            Event::End(tag) if separates_words(tag.name().as_ref()) => text.push(' '),
            Event::Eof => break,
            _ => {}
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::download::stub::{Route, Stub};

    #[tokio::test]
    async fn audio_dns_errors_keep_their_type_through_reqwest() {
        struct FailedResolver;

        impl Resolve for FailedResolver {
            fn resolve(&self, _: Name) -> Resolving {
                Box::pin(async {
                    Err(Box::new(AudioDnsError(io::Error::other("DNS fixture"))) as _)
                })
            }
        }

        let error = Client::builder()
            .no_proxy()
            .dns_resolver(Arc::new(FailedResolver))
            .build()
            .expect("test HTTP client")
            .get("http://recording.invalid")
            .send()
            .await
            .expect_err("injected DNS failure");
        assert_eq!(audio_failure(error), AudioFailure::Network);
    }

    #[tokio::test]
    async fn remote_audio_follows_redirects_and_rejects_failed_or_oversized_responses() {
        let stub = Stub::start(HashMap::from([
            ("/file".into(), Route::Redirect("/file.ogg".into())),
            ("/file.ogg".into(), Route::File(b"recording bytes".to_vec())),
            (
                "/server-error".into(),
                Route::Status("503 Service Unavailable"),
            ),
            (
                "/oversized".into(),
                Route::File(vec![0; AUDIO_MAX_BYTES + 1]),
            ),
        ]))
        .expect("HTTP fixture");
        let url = |path| format!("{}{path}", stub.base_url());
        assert_eq!(
            fetch_audio(url("/file")).await,
            Ok(b"recording bytes".to_vec())
        );
        for path in ["/server-error", "/missing", "/oversized"] {
            assert_eq!(fetch_audio(url(path)).await, Err(AudioFailure::Failed));
        }
    }

    #[test]
    fn commons_urls_identify_the_original_file() {
        let file = CommonsFile("Ru-собака.ogg".into());
        for recording in [
            "https://commons.wikimedia.org/wiki/Special:FilePath/Ru-собака.ogg",
            "https://commons.wikimedia.org/wiki/Special:FilePath/Ru-%D1%81%D0%BE%D0%B1%D0%B0%D0%BA%D0%B0.ogg?x=1",
            "https://upload.wikimedia.org/wikipedia/commons/a/ab/Ru-%D1%81%D0%BE%D0%B1%D0%B0%D0%BA%D0%B0.ogg",
            "https://upload.wikimedia.org/wikipedia/commons/transcoded/a/ab/Ru-%D1%81%D0%BE%D0%B1%D0%B0%D0%BA%D0%B0.ogg/Ru-собака.ogg.mp3",
        ] {
            assert_eq!(
                CommonsFile::from_recording_url(recording),
                Some(file.clone())
            );
        }
        assert_eq!(
            file.page_url().as_str(),
            "https://commons.wikimedia.org/wiki/File:Ru-%D1%81%D0%BE%D0%B1%D0%B0%D0%BA%D0%B0.ogg"
        );
        assert_eq!(
            CommonsFile::from_recording_url(
                "https://commons.wikimedia.org/wiki/Special:FilePath/En-R%26D%20%2B%2050%25.ogg"
            ),
            Some(CommonsFile("En-R&D + 50%.ogg".into()))
        );
        for recording in [
            "file:///tmp/Ru-собака.ogg",
            "https://commons.wikimedia.org.example.com/wiki/Special:FilePath/Ru-собака.ogg",
            "https://upload.wikimedia.org/wikipedia/en/a/ab/Ru-собака.ogg",
            "https://commons.wikimedia.org/wiki/Special:FilePath/",
            "https://commons.wikimedia.org/wiki/Special:FilePath/A%7CFile%3AB.ogg",
        ] {
            assert_eq!(CommonsFile::from_recording_url(recording), None);
        }
    }

    fn imageinfo(metadata: Value) -> Value {
        first_file_page(json!({
            "query": {"pages": [{"ns": 6, "imageinfo": [{"extmetadata": metadata}]}]}
        }))
        .expect("Commons file page")
    }

    #[test]
    fn structured_credit_is_plain_text_and_prefers_explicit_attribution() {
        let page = imageinfo(json!({
            "Artist": {"value": "<a href=\"/wiki/User:Jane\">Jane &amp; Jo&#235;l</a>"},
            "LicenseShortName": {"value": "<span>CC BY-SA&nbsp;4.0</span>"}
        }));
        assert_eq!(
            structured_credit(&page),
            Credit {
                author: Some("Jane & Joël".into()),
                license: Some("CC BY-SA 4.0".into())
            }
        );
        let page = imageinfo(json!({
            "Artist": {"value": "Artist"},
            "Attribution": {"value": "<b>Jane &eacute;</b><br>attribution"}
        }));
        assert_eq!(
            structured_credit(&page),
            Credit {
                author: Some("Jane é attribution".into()),
                license: None
            }
        );
    }

    #[test]
    fn structured_credit_preserves_block_boundaries_and_literal_ampersands() {
        let page = imageinfo(json!({
            "Artist": {"value": "<div>Alice & Bob</div><a>Studio</a><p>Research &amp; Development</p>Team &"}
        }));
        assert_eq!(
            structured_credit(&page).author.as_deref(),
            Some("Alice & Bob Studio Research & Development Team &")
        );
    }

    #[test]
    fn incomplete_credit_keeps_only_the_verified_fields() {
        let page = imageinfo(json!({
            "Artist": {"value": "<span> </span>"},
            "LicenseShortName": {"value": "CC BY 2.0 fr"},
            "User": {"value": "Upload bot"}
        }));
        assert_eq!(
            structured_credit(&page),
            Credit {
                author: None,
                license: Some("CC BY 2.0 FR".into())
            }
        );
        assert_eq!(structured_credit(&imageinfo(json!({}))), Credit::default());
        let missing = json!({"query": {"pages": [{"ns": 6, "missing": true}]}});
        assert!(first_file_page(missing).is_none());
    }

    #[test]
    fn shtooka_credit_requires_the_explicit_description_attribution() {
        let revision = |content: &str| {
            json!({
                "revisions": [{"user": "Upload bot", "slots": {"main": {"content": content}}}]
            })
        };
        let dog = revision(
            "Russian pronunciation.<br/>Audio sample provided by [[Commons:Shtooka|The Shtooka Project]].{{Cc-by-2.0-fr}}",
        );
        assert_eq!(shtooka_credit(&dog), Some("The Shtooka Project"));
        let other = revision("[[Category:The Shtooka Project]]");
        assert_eq!(shtooka_credit(&other), None);
    }
}
