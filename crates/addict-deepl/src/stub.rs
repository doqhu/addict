//! A stub of the DeepL server for tests: a listener on a loopback port that
//! reads each request, keeps it for the test and answers as scripted.
//!
//! The stub runs on a thread of its own, so a test starts it from any thread
//! and with any runtime, or with none. Dropping it closes its connections.

use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex, PoisonError},
    thread::JoinHandle,
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

/// What the stub does with every request after reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Answers with this status and body once `after` has passed.
    Answer {
        status: u16,
        body: String,
        after: Duration,
    },
    /// Never answers and keeps the connection open.
    Silence,
}

impl Reply {
    /// An answer with `status` and `body`, sent at once.
    pub fn answer(status: u16, body: &str) -> Self {
        Self::Answer {
            status,
            body: body.to_owned(),
            after: Duration::ZERO,
        }
    }

    /// The 200 answer of a translation into `text`.
    pub fn translation(text: &str) -> Self {
        Self::answer(200, &translation_body(text))
    }
}

/// The body DeepL answers a translate request with, for `text`. `text` must
/// need no JSON escaping.
pub fn translation_body(text: &str) -> String {
    format!(r#"{{"translations":[{{"detected_source_language":"EN","text":"{text}"}}]}}"#)
}

/// One request as the stub read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// The request line, such as `POST /v2/translate HTTP/1.1`.
    pub line: String,
    /// The headers in the order they came, the names in lowercase.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Seen {
    /// The value of the header `name`, given in lowercase.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// The running stub.
#[derive(Debug)]
pub struct Stub {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// Dropping it ends the thread.
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Stub {
    /// Starts a stub that gives `reply` to every request.
    pub fn start(reply: Reply) -> io::Result<Self> {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (stop, stopped) = oneshot::channel::<()>();
        let kept = seen.clone();
        let thread = std::thread::Builder::new()
            .name("addict-deepl-stub".to_owned())
            .spawn(move || {
                runtime.block_on(async move {
                    let Ok(listener) = TcpListener::from_std(listener) else {
                        return;
                    };
                    tokio::select! {
                        () = serve(listener, reply, kept) => {}
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

    /// The base URL of the stub, without a slash at its end.
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
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Accepts connections for ever; each is served as a task of its own.
async fn serve(listener: TcpListener, reply: Reply, seen: Arc<Mutex<Vec<Seen>>>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(converse(stream, reply.clone(), seen.clone()));
    }
}

/// Reads one request from `stream`, keeps it and gives the `reply`.
async fn converse(mut stream: TcpStream, reply: Reply, seen: Arc<Mutex<Vec<Seen>>>) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    seen.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request);
    match reply {
        Reply::Answer {
            status,
            body,
            after,
        } => {
            tokio::time::sleep(after).await;
            let response = format!(
                "HTTP/1.1 {status} Stub\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
        // Holds the connection until the stub is dropped.
        Reply::Silence => std::future::pending().await,
    }
}

/// Reads the request line, the headers and as much body as `content-length`
/// says. `None` when the connection ends first or the request is not text.
async fn read_request(stream: &mut TcpStream) -> Option<Seen> {
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
        match stream.read(&mut chunk).await {
            Ok(read) if read > 0 => bytes.extend_from_slice(&chunk[..read]),
            _ => return None,
        }
    };
    let head = std::str::from_utf8(&bytes[..head_len]).ok()?;
    let mut lines = head.split("\r\n");
    let line = lines.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|header| header.split_once(':'))
        .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_owned()))
        .collect();
    let body_len = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = bytes[head_len + HEAD_END.len()..].to_vec();
    while body.len() < body_len {
        match stream.read(&mut chunk).await {
            Ok(read) if read > 0 => body.extend_from_slice(&chunk[..read]),
            _ => return None,
        }
    }
    Some(Seen {
        line,
        headers,
        body: String::from_utf8(body).ok()?,
    })
}
