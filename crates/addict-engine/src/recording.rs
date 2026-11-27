//! Pronunciation audio and credits for Wikimedia Commons recordings.

use std::{
    error::Error,
    fmt, fs,
    io::{self, Write},
    net::ToSocketAddrs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use addict_core::failure::playback::PlaybackFailure;

use percent_encoding::percent_decode_str;
use quick_xml::{Reader, escape::resolve_html5_entity, events::Event};
use reqwest::{
    Client, Url,
    dns::{Addrs, Name, Resolve, Resolving},
    redirect::Policy,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

const COMMONS: &str = "https://commons.wikimedia.org";
const API: &str = "https://commons.wikimedia.org/w/api.php";
const USER_AGENT: &str = concat!(
    "Addict/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/doqhu/addict)"
);

const AUDIO_MAX_BYTES: usize = 16 * 1024 * 1024;
const CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;

// Serialize publication and pruning when an earlier cache job is still
// writing on a blocking thread after its request was cancelled.
static CACHE_WRITE: Mutex<()> = Mutex::new(());

/// Resolves file URLs locally. HTTP(S) URLs are left for the cache job.
pub fn local_audio(url: &str) -> Result<Option<PathBuf>, PlaybackFailure> {
    let url = Url::parse(url).map_err(|error| PlaybackFailure::Failed(error.to_string()))?;
    match url.scheme() {
        "http" | "https" => Ok(None),
        "file" => url
            .to_file_path()
            .map(Some)
            .map_err(|()| PlaybackFailure::Failed("invalid recording file URL".into())),
        _ => Err(PlaybackFailure::Failed(
            "unsupported recording URL scheme".into(),
        )),
    }
}

/// Reuses a cached pronunciation or fetches and atomically publishes it.
pub async fn cached_audio(cache_dir: &Path, url: &str) -> Result<PathBuf, PlaybackFailure> {
    let parsed = Url::parse(url).map_err(|error| PlaybackFailure::Failed(error.to_string()))?;
    let extension = parsed
        .path()
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, suffix)| suffix)
        .filter(|suffix| {
            !suffix.is_empty()
                && suffix.len() <= 10
                && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .unwrap_or("audio");
    let digest = Sha256::digest(url.as_bytes());
    let dir = cache_dir.join("recordings");
    let path = dir.join(format!("{digest:x}.{extension}"));
    let check = path.clone();
    if tokio::task::spawn_blocking(move || check.is_file())
        .await
        .map_err(|error| PlaybackFailure::Failed(error.to_string()))?
    {
        return Ok(path);
    }

    let audio = fetch_audio(url.to_owned()).await?;
    tokio::task::spawn_blocking(move || {
        cache_audio(&dir, &path, &audio)?;
        Ok(path)
    })
    .await
    .map_err(|error| PlaybackFailure::Failed(error.to_string()))?
    .map_err(|error: io::Error| PlaybackFailure::Failed(error.to_string()))
}

fn cache_audio(dir: &Path, path: &Path, audio: &[u8]) -> io::Result<()> {
    let _guard = CACHE_WRITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if path.is_file() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(audio)?;
    temp.persist(path)?;

    if let Err(error) = prune_recordings(dir, path) {
        eprintln!("addict: could not read the recording cache: {error}");
    }
    Ok(())
}

fn prune_recordings(dir: &Path, path: &Path) -> io::Result<()> {
    let mut entries = Vec::new();
    let mut total = 0_u64;
    for entry in fs::read_dir(dir)? {
        let (entry, metadata) = match entry.and_then(|entry| Ok((entry.path(), entry.metadata()?)))
        {
            Ok(file) => file,
            Err(error) => {
                eprintln!("addict: could not inspect a recording cache entry: {error}");
                continue;
            }
        };
        if !metadata.is_file() {
            continue;
        }
        total = total.saturating_add(metadata.len());
        if entry != path {
            match metadata.modified() {
                Ok(modified) => entries.push((modified, entry, metadata.len())),
                Err(error) => {
                    eprintln!(
                        "addict: could not read the age of {}: {error}",
                        entry.display()
                    );
                }
            }
        }
    }
    entries.sort_unstable();
    for (_, old, size) in entries {
        if total <= CACHE_MAX_BYTES {
            break;
        }
        if let Err(error) = fs::remove_file(&old) {
            eprintln!(
                "addict: could not remove cached recording {}: {error}",
                old.display()
            );
            continue;
        }
        total -= size;
    }
    Ok(())
}

/// Fetches the URL from the dictionary independently of desktop VFS helpers.
pub async fn fetch_audio(url: String) -> Result<Vec<u8>, PlaybackFailure> {
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
        return Err(PlaybackFailure::Failed(
            "recording exceeds the size limit".into(),
        ));
    }
    let mut audio = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(audio_failure)? {
        if chunk.len() > AUDIO_MAX_BYTES - audio.len() {
            return Err(PlaybackFailure::Failed(
                "recording exceeds the size limit".into(),
            ));
        }
        audio.extend_from_slice(&chunk);
    }
    if audio.is_empty() {
        return Err(PlaybackFailure::Failed("empty recording".into()));
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

fn audio_failure(error: reqwest::Error) -> PlaybackFailure {
    if error.is_timeout() {
        return PlaybackFailure::Network;
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
            return PlaybackFailure::Network;
        }
        cause = error.source();
    }
    PlaybackFailure::Failed(error.without_url().to_string())
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
        self.fetch_credit_from(API).await
    }

    /// Lets the controller's stub-server tests supply a MediaWiki API endpoint.
    #[doc(hidden)]
    pub async fn fetch_credit_from(&self, endpoint: &str) -> Credit {
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
                endpoint,
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
                    endpoint,
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

    async fn query_page(
        &self,
        client: &Client,
        endpoint: &str,
        props: &[(&str, &str)],
    ) -> Option<Value> {
        let value = client
            .get(endpoint)
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
        assert_eq!(audio_failure(error), PlaybackFailure::Network);
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
            assert!(matches!(
                fetch_audio(url(path)).await,
                Err(PlaybackFailure::Failed(_))
            ));
        }
    }

    #[tokio::test]
    async fn buffered_recording_can_be_reopened_from_the_beginning() {
        use std::io::Read;

        let audio = b"buffered recording";
        let temp = tempfile::tempdir().unwrap();
        let stub = Stub::start(HashMap::from([(
            "/audio.ogg".into(),
            Route::File(audio.to_vec()),
        )]))
        .unwrap();
        let url = format!("{}/audio.ogg", stub.base_url());
        let path = cached_audio(temp.path(), &url)
            .await
            .expect("cached recording");
        let mut opened = fs::File::open(&path).unwrap();
        opened.read_exact(&mut [0; 5]).unwrap();
        drop(stub);
        let reopened = cached_audio(temp.path(), &url).await.unwrap();
        assert_eq!(reopened, path);
        assert_eq!(fs::read(reopened).unwrap(), audio);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cached_audio_returns_the_path_when_pruning_cannot_read_the_directory() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("recordings");
        fs::create_dir(&dir).unwrap();
        let permissions = fs::metadata(&dir).unwrap().permissions();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o300)).unwrap();
        if fs::read_dir(&dir).is_ok() {
            // A privileged test process can list the directory anyway.
            fs::set_permissions(&dir, permissions).unwrap();
            return;
        }

        let stub = Stub::start(HashMap::from([(
            "/audio.ogg".into(),
            Route::File(b"playable bytes".to_vec()),
        )]))
        .unwrap();
        let url = format!("{}/audio.ogg", stub.base_url());
        let outcome = cached_audio(temp.path(), &url).await;
        fs::set_permissions(&dir, permissions).unwrap();

        let path = outcome.expect("audio was published despite the pruning error");
        assert_eq!(path.parent(), Some(dir.as_path()));
        assert_eq!(fs::read(path).unwrap(), b"playable bytes");
    }

    #[test]
    fn the_recording_cache_prunes_oldest_files_to_the_byte_limit() {
        let temp = tempfile::tempdir().unwrap();
        let oldest = temp.path().join("oldest.ogg");
        for (name, seconds) in [("oldest", 1), ("old", 2), ("newer", 3), ("newest", 4)] {
            let file = fs::File::create(temp.path().join(format!("{name}.ogg"))).unwrap();
            file.set_len(AUDIO_MAX_BYTES as u64).unwrap();
            file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(seconds))
                .unwrap();
        }
        let added = temp.path().join("added.ogg");
        cache_audio(temp.path(), &added, b"recording").unwrap();
        assert!(!oldest.exists());
        assert_eq!(fs::read(&added).unwrap(), b"recording");
        let entries = fs::read_dir(temp.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 4);
        let bytes: u64 = entries
            .iter()
            .map(|entry| entry.metadata().unwrap().len())
            .sum();
        assert!(bytes <= CACHE_MAX_BYTES);
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
