//! Shared helpers of the run tests: a private temporary directory with a key
//! file, and a loopback server that answers each DeepL path as scripted.
//!
//! The stub of `addict-deepl` gives one reply to every request; a whole run
//! needs the languages, the usage and a translation for each request, so
//! this server chooses its reply by the path and the body.
#![allow(dead_code)]

use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::JoinHandle,
};

/// A key no service knows; the tests look for it where it must not be.
pub const KEY: &str = "0a1b2c3d-test-key-of-exp5";

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory of one test, removed when the test ends.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "addict-exp5-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).expect("create the temporary directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Where the key file of this directory is, written or not.
    pub fn key_path(&self) -> PathBuf {
        self.0.join("deepl.key")
    }

    /// Writes the key file with `content` and `mode` and gives its path.
    pub fn key_file(&self, content: &str, mode: u32) -> PathBuf {
        let path = self.key_path();
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, content).expect("write the key file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("set the mode of the key file");
        path
    }

    /// Writes `content` to the file `name` and gives its path.
    pub fn file(&self, name: &str, content: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, content).expect("write the file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What the server does with a request after reading it.
#[derive(Debug, Clone)]
pub enum Reply {
    /// Answers with this status and body at once.
    Answer(u16, String),
    /// Never answers and keeps the connection open.
    Silence,
}

impl Reply {
    pub fn ok(body: &str) -> Self {
        Self::Answer(200, body.to_owned())
    }

    pub fn status(status: u16) -> Self {
        Self::Answer(
            status,
            r#"{"message":"refused by the server of the test"}"#.to_owned(),
        )
    }

    /// The 200 answer of a translation into `text`.
    pub fn translation(text: &str) -> Self {
        Self::ok(&addict_deepl::stub::translation_body(text))
    }
}

/// One request as the server read it.
#[derive(Debug, Clone)]
pub struct Seen {
    /// The method and the path, such as `POST /v2/translate`.
    pub target: String,
    /// Everything of the request, the headers included, as text.
    pub raw: String,
    /// The `text` of a translate request.
    pub text: Option<String>,
    /// The `context` of a translate request.
    pub context: Option<String>,
}

/// Chooses the reply of each request; `usize` counts the requests before it.
pub type Script = Box<dyn Fn(usize, &Seen) -> Reply + Send>;

/// The running server. Requests are served one after another, in the order
/// they come.
pub struct Server {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(script: Script) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let seen = Arc::new(Mutex::new(Vec::<Seen>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (kept, stopped) = (seen.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            // The connections that get no answer, held until the server ends.
            let mut silent = Vec::new();
            for stream in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(mut stream) = stream else { continue };
                let Some(request) = read_request(&mut stream) else {
                    continue;
                };
                let reply = {
                    let mut seen = kept.lock().unwrap_or_else(PoisonError::into_inner);
                    let reply = script(seen.len(), &request);
                    seen.push(request);
                    reply
                };
                match reply {
                    Reply::Answer(status, body) => {
                        let response = format!(
                            "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Reply::Silence => silent.push(stream),
                }
            }
        });
        Self {
            address,
            seen,
            stop,
            thread: Some(thread),
        }
    }

    /// The base URL of the server, without a slash at its end.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// The requests read so far, oldest first.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The method and path of each request read so far.
    pub fn targets(&self) -> Vec<String> {
        self.seen().into_iter().map(|seen| seen.target).collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the thread out of its wait for a connection.
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Reads the head of one request and as much body as `content-length` says.
fn read_request(stream: &mut TcpStream) -> Option<Seen> {
    const HEAD_END: &[u8] = b"\r\n\r\n";
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let head_len = loop {
        if let Some(at) = bytes
            .windows(HEAD_END.len())
            .position(|window| window == HEAD_END)
        {
            break at;
        }
        match stream.read(&mut chunk) {
            Ok(read) if read > 0 => bytes.extend_from_slice(&chunk[..read]),
            _ => return None,
        }
    };
    let head = std::str::from_utf8(&bytes[..head_len]).ok()?.to_owned();
    let body_len = head
        .split("\r\n")
        .filter_map(|header| header.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < head_len + HEAD_END.len() + body_len {
        match stream.read(&mut chunk) {
            Ok(read) if read > 0 => bytes.extend_from_slice(&chunk[..read]),
            _ => return None,
        }
    }
    let raw = String::from_utf8(bytes).ok()?;
    let body: serde_json::Value =
        serde_json::from_str(&raw[head_len + HEAD_END.len()..]).unwrap_or_default();
    let mut line = head.split("\r\n").next()?.split(' ');
    let target = format!("{} {}", line.next()?, line.next()?);
    Some(Seen {
        target,
        text: body["text"][0].as_str().map(str::to_owned),
        context: body["context"].as_str().map(str::to_owned),
        raw,
    })
}
