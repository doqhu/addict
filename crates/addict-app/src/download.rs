//! The download of the published indexes: the manifest of the data release
//! of this index format and the assets it lists, from fixed release-asset
//! URLs, `<base>/indexes-v<VERSION>/<asset>`. The GitHub REST API and
//! `releases/latest` are never asked; GitHub answers an asset URL with a
//! redirect, which is followed.
//!
//! An asset is read whole into memory, never beyond the size the manifest
//! gives, and its size and then its SHA-256 are checked against the manifest
//! before it is unpacked; `addict_dictionary::release::install_unless` then
//! puts it in the data directory, so an install that fails leaves the
//! previous index as it was. The hash and the install run on tokio's blocking
//! threads, off the runner's one thread; when the install's future is dropped
//! (the job was cancelled or ran out of time), the blocking part is told to
//! stop and renames nothing into place.
//!
//! The release uploads the manifest last, so an asset can be newer than the
//! manifest read before it. On a checksum mismatch the manifest is therefore
//! fetched once more, and the index is downloaded again when its entry there
//! differs; it is never fetched a third time.
//!
//! What the release is and how an index is installed is
//! `addict_dictionary::release`'s; when the download is offered is
//! [`first_run`](crate::first_run)'s.

use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use addict_dictionary::{
    Index, format, import,
    release::{self, InstallError, Manifest, Published},
};

/// Where the releases of the repository keep their assets.
pub const BASE_URL: &str = "https://github.com/doqhu/addict/releases/download";

/// How long the fetch of the manifest may take on the runner.
pub const MANIFEST_LIMIT: Duration = Duration::from_secs(60);

/// How long the download and the install of one index may take on the
/// runner.
pub const DOWNLOAD_LIMIT: Duration = Duration::from_secs(30 * 60);

/// How long a connection may take to open.
const CONNECT_LIMIT: Duration = Duration::from_secs(30);

/// The largest manifest read, in bytes.
const MANIFEST_MAX: u64 = 1 << 20;

/// What the errors about the manifest call it.
const MANIFEST_WHAT: &str = "the index manifest";

/// One install at a time in the whole process, including manual imports:
/// both writers use a per-process temporary index name. The blocking part
/// of an install that was given up still runs to its end.
static INSTALLS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Shared by release installs and manual imports. Move the permit into the
/// blocking work so cancelling its async caller cannot release it early.
pub(crate) async fn installation_permit() -> tokio::sync::SemaphorePermit<'static> {
    INSTALLS.acquire().await.expect("installs are never closed")
}

/// The downloader of the indexes of one release.
pub struct Downloader {
    /// Without a slash at its end.
    base_url: String,
    /// Built at the first request.
    http: OnceLock<reqwest::Client>,
    recovery: bool,
    committed: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Downloader {
    /// The downloader of the product: the releases of the repository.
    /// Nothing is connected here.
    pub fn new() -> Self {
        Self::with_base_url(BASE_URL)
    }

    /// A downloader that asks `base_url` instead of the repository's
    /// releases, for the tests.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            http: OnceLock::new(),
            recovery: false,
            committed: None,
        }
    }

    /// Recovery preserves an edition that became healthy after selection.
    pub fn recovering(mut self) -> Self {
        self.recovery = true;
        self
    }

    /// Called from the blocking writer after a successful commit, even if
    /// the async caller is cancelled before it receives the result. The
    /// callback must return promptly and own any required finalization.
    pub fn on_commit(mut self, committed: impl Fn() + Send + Sync + 'static) -> Self {
        self.committed = Some(Arc::new(committed));
        self
    }

    /// The URL of `asset` in the release of this index format.
    fn url(&self, asset: &str) -> String {
        format!("{}/{}/{asset}", self.base_url, release::tag())
    }

    /// The manifest of the release.
    pub async fn manifest(&self) -> Result<Manifest, DownloadError> {
        let body = self
            .fetch(MANIFEST_WHAT, release::MANIFEST, MANIFEST_MAX, |_| {})
            .await?;
        Manifest::parse(&body).map_err(|cause| DownloadError::Manifest { cause })
    }

    /// Downloads the index `published` lists, checks it against the entry
    /// and installs it into the data directory `dir`; the path of the
    /// index. On any error nothing is written. `progress` hears how many
    /// bytes of the asset came so far, in steps of about one percent and at
    /// the end. On a checksum mismatch the manifest is fetched once more:
    /// when its entry of the index differs, the index is downloaded again by
    /// that entry; otherwise, or when the manifest cannot be had, the error
    /// is the mismatch.
    pub async fn install(
        &self,
        published: &Published,
        dir: &Path,
        mut progress: impl FnMut(u64),
    ) -> Result<PathBuf, DownloadError> {
        let error = match self.install_once(published, dir, &mut progress).await {
            Err(error @ DownloadError::Checksum { .. }) => error,
            done => return done,
        };
        let fresh = match self.manifest().await {
            Ok(manifest) => manifest
                .indexes
                .into_iter()
                .find(|fresh| fresh.name == published.name),
            Err(_) => None,
        };
        match fresh {
            Some(fresh) if fresh != *published => {
                self.install_once(&fresh, dir, &mut progress).await
            }
            _ => Err(error),
        }
    }

    /// [`install`](Self::install) by the entry `published`, without asking
    /// the manifest again.
    async fn install_once(
        &self,
        published: &Published,
        dir: &Path,
        progress: impl FnMut(u64),
    ) -> Result<PathBuf, DownloadError> {
        let name = published.name.clone();
        let source = release::source_of(&name)
            .ok_or_else(|| DownloadError::Unknown { name: name.clone() })?;
        if published.format != format::VERSION {
            return Err(DownloadError::Format {
                name,
                format: published.format,
            });
        }
        // Held until the blocking part ends, even when this future is
        // dropped first. The semaphore is never closed.
        let mut permit = installation_permit().await;
        if self.recovery {
            let path = import::index_path(dir, source);
            let checked = path.clone();
            // Move the permit into the scan too: cancellation cannot let
            // another writer race an unfinished blocking health check.
            let (held, healthy) = tokio::task::spawn_blocking(move || {
                let healthy = Index::open(&checked)
                    .and_then(|index| index.validate())
                    .is_ok();
                (permit, healthy)
            })
            .await
            .map_err(|_| DownloadError::Stopped { name: name.clone() })?;
            permit = held;
            if healthy {
                if let Some(committed) = &self.committed {
                    committed();
                }
                return Ok(path);
            }
        }
        let body = self
            .fetch(&name, &published.asset(), published.size, progress)
            .await?;
        if body.len() as u64 != published.size {
            return Err(DownloadError::Size {
                what: name,
                expected: published.size,
                actual: body.len() as u64,
            });
        }
        let expected = published.sha256.to_ascii_lowercase();
        let dir = dir.to_owned();
        let what = name.clone();
        // Set when this future is dropped, which the blocking part, that
        // cannot be aborted, sees before it renames anything into place.
        let stop = Arc::new(AtomicBool::new(false));
        let _guard = StopOnDrop(stop.clone());
        let committed = self.committed.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let actual = release::sha256(&body);
            if actual != expected {
                return Err(DownloadError::Checksum {
                    name: what,
                    expected,
                    actual,
                });
            }
            let path = release::install_unless(&body, source, &dir, || stop.load(Ordering::SeqCst))
                .map_err(DownloadError::Install)?;
            if let Some(committed) = committed {
                committed();
            }
            Ok(path)
        })
        .await
        .map_err(|_| DownloadError::Stopped { name })?
    }

    /// The body of `asset`, at most `limit` bytes, which `what` names.
    /// `progress` hears the bytes read so far each time about one percent
    /// of `limit` more came, and once at the end.
    async fn fetch(
        &self,
        what: &str,
        asset: &str,
        limit: u64,
        mut progress: impl FnMut(u64),
    ) -> Result<Vec<u8>, DownloadError> {
        let connection = |error: reqwest::Error| DownloadError::Connection {
            what: what.to_owned(),
            cause: causes(&error),
        };
        let mut response = self
            .http(what)?
            .get(self.url(asset))
            .send()
            .await
            .map_err(connection)?;
        let status = response.status();
        if !status.is_success() {
            return Err(DownloadError::Status {
                what: what.to_owned(),
                status: status.as_u16(),
            });
        }
        let too_large = || DownloadError::TooLarge {
            what: what.to_owned(),
            limit,
        };
        let length = response.content_length();
        if length.is_some_and(|length| length > limit) {
            return Err(too_large());
        }
        let mut body = Vec::with_capacity(length.unwrap_or(0) as usize);
        let step = (limit / 100).max(1);
        let mut told = 0;
        while let Some(chunk) = response.chunk().await.map_err(connection)? {
            if (body.len() + chunk.len()) as u64 > limit {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk);
            let received = body.len() as u64;
            if received - told >= step {
                progress(received);
                told = received;
            }
        }
        let received = body.len() as u64;
        if received != told || received == 0 {
            progress(received);
        }
        Ok(body)
    }

    /// The HTTP client, built at the first request.
    fn http(&self, what: &str) -> Result<&reqwest::Client, DownloadError> {
        if let Some(http) = self.http.get() {
            return Ok(http);
        }
        let built = reqwest::Client::builder()
            .user_agent(concat!("addict/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_LIMIT)
            // No connection outlives its answer, so nothing here wakes an
            // idle process.
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|error| DownloadError::Connection {
                what: what.to_owned(),
                cause: causes(&error),
            })?;
        Ok(self.http.get_or_init(|| built))
    }
}

impl Default for Downloader {
    fn default() -> Self {
        Self::new()
    }
}

/// Sets its flag when dropped.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Why a download gave nothing; nothing was written. The text is one line
/// and names the index, or the manifest.
#[derive(Debug)]
pub enum DownloadError {
    /// The request could not be sent or its answer not read.
    Connection { what: String, cause: String },
    /// The server answered with this HTTP status.
    Status { what: String, status: u16 },
    /// The answer is longer than `limit` bytes.
    TooLarge { what: String, limit: u64 },
    /// The asset is shorter than the manifest says.
    Size {
        what: String,
        expected: u64,
        actual: u64,
    },
    /// The SHA-256 of the asset is not the manifest's.
    Checksum {
        name: String,
        expected: String,
        actual: String,
    },
    /// The manifest is not one this build reads.
    Manifest { cause: String },
    /// The manifest lists an index this build does not know.
    Unknown { name: String },
    /// The manifest gives the index a format this build does not read.
    Format { name: String, format: u32 },
    /// The checked asset could not be installed.
    Install(InstallError),
    /// The check and the install stopped before their end.
    Stopped { name: String },
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection { what, cause } => write!(f, "cannot download {what}: {cause}"),
            Self::Status { what, status } => {
                write!(
                    f,
                    "cannot download {what}: the server answered HTTP {status}"
                )
            }
            Self::TooLarge { what, limit } => {
                write!(f, "cannot download {what}: it is larger than {limit} bytes")
            }
            Self::Size {
                what,
                expected,
                actual,
            } => write!(
                f,
                "cannot download {what}: it is {actual} bytes, the manifest says {expected}"
            ),
            Self::Checksum {
                name,
                expected,
                actual,
            } => write!(
                f,
                "cannot install {name}: the checksum does not match: its SHA-256 is {actual}, \
                 the manifest says {expected}"
            ),
            Self::Manifest { cause } => write!(f, "cannot read {MANIFEST_WHAT}: {cause}"),
            Self::Unknown { name } => {
                write!(f, "cannot install {name}: this version does not know it")
            }
            Self::Format { name, format } => write!(
                f,
                "cannot install {name}: it is of index format {format}, this version reads {}",
                format::VERSION
            ),
            Self::Install(error) => write!(f, "{error}"),
            Self::Stopped { name } => write!(f, "cannot install {name}: the install stopped"),
        }
    }
}

impl Error for DownloadError {}

/// The text of `error` and of its causes, on one line.
fn causes(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(error) = cause {
        text.push_str(": ");
        text.push_str(&error.to_string());
        cause = error.source();
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A static-file server on a loopback port, for the tests: each path has a
/// body or a redirect, and every other path is a 404. Hand-written HTTP/1.1,
/// one request per connection.
#[cfg(test)]
pub(crate) mod stub {
    use std::{
        collections::HashMap,
        io,
        net::{Ipv4Addr, SocketAddr},
        sync::{Arc, Mutex, PoisonError},
        thread::JoinHandle,
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        sync::oneshot,
    };

    /// What a path answers.
    #[derive(Debug, Clone)]
    pub enum Route {
        /// 200 with these bytes.
        File(Vec<u8>),
        /// 302 to this path of the stub.
        Redirect(String),
    }

    /// The running stub; dropping it stops it.
    pub struct Stub {
        address: SocketAddr,
        seen: Arc<Mutex<Vec<String>>>,
        stop: Option<oneshot::Sender<()>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Stub {
        /// Serves `routes`, by path, such as `/indexes-v3/manifest.json`.
        pub fn start(routes: HashMap<String, Route>) -> io::Result<Self> {
            let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            listener.set_nonblocking(true)?;
            let address = listener.local_addr()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let seen = Arc::new(Mutex::new(Vec::new()));
            let kept = seen.clone();
            let routes = Arc::new(routes);
            let (stop, stopped) = oneshot::channel::<()>();
            let thread = std::thread::Builder::new()
                .name("addict-download-stub".to_owned())
                .spawn(move || {
                    runtime.block_on(async move {
                        let Ok(listener) = TcpListener::from_std(listener) else {
                            return;
                        };
                        tokio::select! {
                            () = serve(listener, routes, kept) => {}
                            _ = stopped => {}
                        }
                    });
                })?;
            Ok(Self {
                address,
                seen,
                stop: Some(stop),
                thread: Some(thread),
            })
        }

        /// The base URL, without a slash at its end.
        pub fn base_url(&self) -> String {
            format!("http://{}", self.address)
        }

        /// The request lines read so far, oldest first.
        pub fn seen(&self) -> Vec<String> {
            self.seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            self.stop.take();
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    async fn serve(
        listener: TcpListener,
        routes: Arc<HashMap<String, Route>>,
        seen: Arc<Mutex<Vec<String>>>,
    ) {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(answer(stream, routes.clone(), seen.clone()));
        }
    }

    /// Reads the head of one request and answers it.
    async fn answer(
        mut stream: TcpStream,
        routes: Arc<HashMap<String, Route>>,
        seen: Arc<Mutex<Vec<String>>>,
    ) {
        let mut head = Vec::new();
        let mut chunk = [0_u8; 4096];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut chunk).await {
                Ok(read) if read > 0 => head.extend_from_slice(&chunk[..read]),
                _ => return,
            }
        }
        let head = String::from_utf8_lossy(&head);
        let line = head.lines().next().unwrap_or_default().to_owned();
        seen.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.clone());
        let path = line.split(' ').nth(1).unwrap_or_default();
        let (status, extra, body): (&str, String, &[u8]) = match routes.get(path) {
            Some(Route::File(bytes)) => ("200 OK", String::new(), bytes),
            Some(Route::Redirect(to)) => ("302 Found", format!("location: {to}\r\n"), b""),
            None => ("404 Not Found", String::new(), b"Not Found"),
        };
        let head = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/octet-stream\r\n{extra}\
             content-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(body).await;
        let _ = stream.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
    };

    use addict_dictionary::{
        Index,
        format::{Contents, Headword, SenseRecord, encode},
        release::{self, Manifest, Published},
    };

    use super::{
        DownloadError, Downloader,
        stub::{Route, Stub},
    };

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A private data directory of one test, removed when the test ends. It
    /// does not exist until an install makes it.
    struct DataDir(PathBuf);

    impl DataDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "addict-download-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }

        fn data(&self) -> PathBuf {
            self.0.join("addict")
        }

        /// The entries of the data directory, sorted; none when it is not
        /// there.
        fn entries(&self) -> Vec<String> {
            let Ok(read) = fs::read_dir(self.data()) else {
                return Vec::new();
            };
            let mut names: Vec<String> = read
                .map(|entry| {
                    entry
                        .expect("entry")
                        .file_name()
                        .into_string()
                        .expect("utf-8")
                })
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for DataDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn a_manual_import_waits_for_the_release_installation_permit() {
        use futures_util::{pin_mut, poll};
        let dir = DataDir::new("manual-overlap");
        let existing = super::installation_permit().await;
        let importing = crate::file_import::import_file(dir.0.join("missing-file"), dir.data());
        pin_mut!(importing);
        assert!(poll!(importing.as_mut()).is_pending());
        drop(existing);
        // acquire() reserves the released permit for its queued waiter
        // before that future is polled again. Merely spawning an unlocked
        // worker would also return Pending, but would leave this permit
        // available. No timing assumption or sleep is needed here.
        assert!(
            super::INSTALLS.try_acquire().is_err(),
            "the manual import must have queued on the shared semaphore"
        );
        let error = importing.await.unwrap_err();
        assert!(error.contains("missing-file"), "{error}");
        assert!(!dir.data().exists());
    }

    #[tokio::test]
    async fn recovery_keeps_a_manual_edition_imported_after_manifest_selection() {
        let xz = xz_of(&index());
        let stub = release(manifest_of(vec![Published::of("oewn", &xz)]), None);
        let downloader = Downloader::with_base_url(stub.base_url()).recovering();
        let dir = DataDir::new("manual-after-selection");
        let manifest = crate::first_run::recovery_manifest(&downloader, dir.data())
            .await
            .unwrap();
        assert_eq!(manifest.indexes.len(), 1);
        let manual = encode(&Contents {
            source: "a-manual-edition".to_owned(),
            headwords: vec![Headword {
                word: "manual".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        fs::create_dir_all(dir.data()).unwrap();
        fs::write(dir.data().join("oewn.index"), &manual).unwrap();
        downloader
            .install(&manifest.indexes[0], &dir.data(), |_| {
                panic!("no asset request")
            })
            .await
            .unwrap();
        assert_eq!(fs::read(dir.data().join("oewn.index")).unwrap(), manual);
        assert_eq!(
            stub.seen().len(),
            1,
            "only the selection manifest was fetched"
        );
    }

    #[tokio::test]
    async fn recovery_rechecks_after_a_queued_manual_writer_releases_the_permit() {
        use futures_util::{pin_mut, poll};
        let stub = Stub::start(HashMap::new()).unwrap();
        let downloader = Downloader::with_base_url(stub.base_url()).recovering();
        let dir = DataDir::new("manual-before-recovery");
        let published = Published::of("oewn", b"unused");
        let manual = index();
        let path = dir.data().join("oewn.index");
        let held = super::installation_permit().await;
        // The same shared permit and blocking-writer lifetime as import_file.
        // Queue that writer first, then queue recovery before either runs.
        let importing = async {
            let permit = super::installation_permit().await;
            crate::file_import::blocking(move || {
                let _permit = permit;
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, &manual).unwrap();
            })
            .await
            .unwrap();
        };
        let data = dir.data();
        let recovery = downloader.install(&published, &data, |_| panic!("no asset request"));
        pin_mut!(importing, recovery);
        assert!(poll!(importing.as_mut()).is_pending());
        assert!(poll!(recovery.as_mut()).is_pending());
        drop(held);
        let ((), result) = tokio::join!(importing, recovery);
        result.unwrap();
        assert_eq!(fs::read(dir.data().join("oewn.index")).unwrap(), index());
        assert!(
            stub.seen().is_empty(),
            "the recheck happens after waiting for the writer"
        );
    }

    /// A small English index whose one headword is `bank`.
    fn index() -> Vec<u8> {
        encode(&Contents {
            source: "oewn-fixture".to_owned(),
            senses: vec![SenseRecord {
                part_of_speech: None,
                definition: "sloping land beside a body of water".to_owned(),
                members: vec!["bank".to_owned(), "shore".to_owned()],
                metadata: None,
            }],
            headwords: vec![Headword {
                entries: Vec::new(),
                word: "bank".to_owned(),
                senses: vec![0],
                translations: Vec::new(),
            }],
            forms: Vec::new(),
            analyses: Vec::new(),
        })
        .expect("encode")
    }

    fn xz_of(bytes: &[u8]) -> Vec<u8> {
        let mut packed = Vec::new();
        lzma_rs::xz_compress(&mut &bytes[..], &mut packed).expect("compress");
        packed
    }

    fn manifest_of(published: Vec<Published>) -> Vec<u8> {
        Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: published,
        }
        .to_json()
        .into_bytes()
    }

    /// A stub of the release with `manifest` and, when given, the asset of
    /// OEWN, which is reached through a redirect, as GitHub's is.
    fn release(manifest: Vec<u8>, asset: Option<Vec<u8>>) -> Stub {
        let tag = release::tag();
        let mut routes = HashMap::from([(format!("/{tag}/manifest.json"), Route::File(manifest))]);
        if let Some(asset) = asset {
            routes.insert(
                format!("/{tag}/oewn.index.xz"),
                Route::Redirect("/objects/oewn".to_owned()),
            );
            routes.insert("/objects/oewn".to_owned(), Route::File(asset));
        }
        Stub::start(routes).expect("the stub")
    }

    #[tokio::test]
    async fn the_published_index_is_installed_into_an_empty_data_directory() {
        let xz = xz_of(&index());
        let stub = release(manifest_of(vec![Published::of("oewn", &xz)]), Some(xz));
        let downloader = Downloader::with_base_url(stub.base_url());
        let dir = DataDir::new("happy");

        let manifest = downloader.manifest().await.expect("the manifest");
        assert_eq!(manifest.indexes.len(), 1);
        let path = downloader
            .install(&manifest.indexes[0], &dir.data(), |_| {})
            .await
            .expect("installed");

        assert_eq!(path, dir.data().join("oewn.index"));
        let index = Index::open(&path).expect("open");
        assert!(index.find("bank").expect("find").is_some());
        assert_eq!(dir.entries(), ["oewn.index"]);
        let tag = release::tag();
        assert_eq!(
            stub.seen(),
            [
                format!("GET /{tag}/manifest.json HTTP/1.1"),
                format!("GET /{tag}/oewn.index.xz HTTP/1.1"),
                "GET /objects/oewn HTTP/1.1".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn an_asset_of_another_checksum_writes_nothing() {
        let xz = xz_of(&index());
        let mut published = Published::of("oewn", &xz);
        published.sha256 = "0".repeat(64);
        let stub = release(manifest_of(vec![published]), Some(xz));
        let downloader = Downloader::with_base_url(stub.base_url());
        let dir = DataDir::new("checksum");

        let manifest = downloader.manifest().await.expect("the manifest");
        let error = downloader
            .install(&manifest.indexes[0], &dir.data(), |_| {})
            .await
            .expect_err("another checksum");

        assert!(matches!(error, DownloadError::Checksum { .. }), "{error:?}");
        let text = error.to_string();
        assert!(text.contains("oewn") && text.contains("checksum"), "{text}");
        assert!(dir.entries().is_empty());
        // The manifest was asked once more, and the asset not again: the
        // fresh entry is the one that failed.
        let tag = release::tag();
        assert_eq!(
            stub.seen(),
            [
                format!("GET /{tag}/manifest.json HTTP/1.1"),
                format!("GET /{tag}/oewn.index.xz HTTP/1.1"),
                "GET /objects/oewn HTTP/1.1".to_owned(),
                format!("GET /{tag}/manifest.json HTTP/1.1"),
            ]
        );
    }

    #[tokio::test]
    async fn a_stale_entry_is_fixed_by_the_manifest_fetched_again() {
        let xz = xz_of(&index());
        // The entry of the last run: the same size, another checksum.
        let mut stale = Published::of("oewn", &xz);
        stale.sha256 = "0".repeat(64);
        let stub = release(manifest_of(vec![Published::of("oewn", &xz)]), Some(xz));
        let downloader = Downloader::with_base_url(stub.base_url());
        let dir = DataDir::new("stale");

        // Asked first, as the app does; then the stale entry is installed.
        downloader.manifest().await.expect("the manifest");
        let path = downloader
            .install(&stale, &dir.data(), |_| {})
            .await
            .expect("installed by the fresh entry");

        assert!(Index::open(&path).is_ok());
        assert_eq!(dir.entries(), ["oewn.index"]);
        let manifests = stub
            .seen()
            .iter()
            .filter(|line| line.contains("manifest.json"))
            .count();
        assert_eq!(manifests, 2, "{:?}", stub.seen());
        assert_eq!(stub.seen().len(), 6, "{:?}", stub.seen());
    }

    #[tokio::test]
    async fn the_progress_reaches_the_size_in_about_one_percent_steps() {
        // Text that xz packs poorly, so the asset spans many chunks.
        let mut seed = 0x2545_f491_u32;
        let noise: Vec<u8> = (0..400_000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed.to_le_bytes()[0]
            })
            .collect();
        let xz = xz_of(&noise);
        let published = Published::of("oewn", &xz);
        let stub = release(manifest_of(vec![published.clone()]), Some(xz));
        let dir = DataDir::new("progress");
        let mut heard = Vec::new();
        let error = Downloader::with_base_url(stub.base_url())
            .install(&published, &dir.data(), |received| heard.push(received))
            .await
            .expect_err("noise is no index");
        assert!(matches!(error, DownloadError::Install(_)), "{error:?}");
        assert_eq!(heard.last(), Some(&published.size), "{heard:?}");
        assert!(heard.windows(2).all(|pair| pair[0] < pair[1]), "{heard:?}");
        assert!(heard.len() <= 101, "{}", heard.len());
        let step = published.size / 100;
        assert!(
            heard
                .windows(2)
                .take(heard.len().saturating_sub(2))
                .all(|pair| pair[1] - pair[0] >= step),
            "{heard:?}"
        );
    }

    #[tokio::test]
    async fn a_missing_asset_names_the_index_and_the_status() {
        let xz = xz_of(&index());
        let stub = release(manifest_of(vec![Published::of("oewn", &xz)]), None);
        let downloader = Downloader::with_base_url(stub.base_url());
        let dir = DataDir::new("missing");

        let manifest = downloader.manifest().await.expect("the manifest");
        let error = downloader
            .install(&manifest.indexes[0], &dir.data(), |_| {})
            .await
            .expect_err("404");

        assert!(
            matches!(error, DownloadError::Status { status: 404, .. }),
            "{error:?}"
        );
        let text = error.to_string();
        assert!(text.contains("oewn") && text.contains("404"), "{text}");
        assert!(dir.entries().is_empty());
    }

    #[tokio::test]
    async fn a_garbled_or_missing_manifest_is_an_error() {
        let stub = release(b"<html>not a manifest</html>".to_vec(), None);
        let error = Downloader::with_base_url(stub.base_url())
            .manifest()
            .await
            .expect_err("garbled");
        assert!(matches!(error, DownloadError::Manifest { .. }), "{error:?}");
        assert!(error.to_string().contains("manifest"), "{error}");

        let empty = Stub::start(HashMap::new()).expect("the stub");
        let error = Downloader::with_base_url(empty.base_url())
            .manifest()
            .await
            .expect_err("missing");
        assert!(
            matches!(error, DownloadError::Status { status: 404, .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("manifest"), "{error}");
    }

    #[tokio::test]
    async fn an_asset_of_another_size_writes_nothing() {
        let xz = xz_of(&index());
        let dir = DataDir::new("size");
        for size in [xz.len() as u64 - 1, xz.len() as u64 + 1] {
            let mut published = Published::of("oewn", &xz);
            published.size = size;
            let stub = release(manifest_of(vec![published.clone()]), Some(xz.clone()));
            let error = Downloader::with_base_url(stub.base_url())
                .install(&published, &dir.data(), |_| {})
                .await
                .expect_err("another size");
            assert!(
                matches!(
                    error,
                    DownloadError::TooLarge { .. } | DownloadError::Size { .. }
                ),
                "{error:?}"
            );
            assert!(error.to_string().contains("oewn"), "{error}");
            assert!(dir.entries().is_empty());
        }
    }

    #[tokio::test]
    async fn a_checked_asset_that_is_no_index_leaves_the_old_one() {
        let dir = DataDir::new("bad-index");
        fs::create_dir_all(dir.data()).expect("the data directory");
        let path = dir.data().join("oewn.index");
        fs::write(&path, b"the old index").expect("write");
        for asset in [index(), xz_of(b"not an index")] {
            let published = Published::of("oewn", &asset);
            let stub = release(manifest_of(vec![published.clone()]), Some(asset));
            let error = Downloader::with_base_url(stub.base_url())
                .install(&published, &dir.data(), |_| {})
                .await
                .expect_err("not an index");
            assert!(matches!(error, DownloadError::Install(_)), "{error:?}");
            assert!(error.to_string().contains("oewn"), "{error}");
            assert_eq!(fs::read(&path).expect("index"), b"the old index");
            assert_eq!(dir.entries(), ["oewn.index"]);
        }
    }

    #[tokio::test]
    async fn an_unknown_index_or_format_is_not_downloaded() {
        let stub = Stub::start(HashMap::new()).expect("the stub");
        let downloader = Downloader::with_base_url(stub.base_url());
        let dir = DataDir::new("unknown");
        let mut published = Published::of("tessdata", b"");
        let error = downloader
            .install(&published, &dir.data(), |_| {})
            .await
            .expect_err("unknown");
        assert!(matches!(error, DownloadError::Unknown { .. }), "{error:?}");
        published.name = "oewn".to_owned();
        published.format = addict_dictionary::format::VERSION + 1;
        let error = downloader
            .install(&published, &dir.data(), |_| {})
            .await
            .expect_err("a newer format");
        assert!(matches!(error, DownloadError::Format { .. }), "{error:?}");
        assert!(error.to_string().contains("oewn"), "{error}");
        assert!(stub.seen().is_empty());
        assert!(dir.entries().is_empty());
    }

    /// A cancelled or timed-out job drops the install's future: the flag
    /// the blocking part asks before the rename is then set.
    #[test]
    fn dropping_the_install_tells_the_blocking_part_to_stop() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let stop = Arc::new(AtomicBool::new(false));
        let guard = super::StopOnDrop(stop.clone());
        assert!(!stop.load(Ordering::SeqCst));
        drop(guard);
        assert!(stop.load(Ordering::SeqCst));
    }

    #[test]
    fn the_product_asks_the_fixed_release_of_this_format() {
        let downloader = Downloader::new();
        let tag = release::tag();
        assert_eq!(
            downloader.url("manifest.json"),
            format!("https://github.com/doqhu/addict/releases/download/{tag}/manifest.json")
        );
        assert_eq!(
            downloader.url("oewn.index.xz"),
            format!("https://github.com/doqhu/addict/releases/download/{tag}/oewn.index.xz")
        );
    }
}
