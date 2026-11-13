//! A static-file server on a loopback port, for the tests: each path has a
//! body or a redirect, and every other path is a 404. Hand-written HTTP/1.1,
//! one request per connection.

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
    /// 200 that declares these bytes and closes after the first half.
    Short(Vec<u8>),
    /// 200 that declares these bytes, sends the first half and then
    /// holds the connection open, silent, until the stub stops.
    Stall(Vec<u8>),
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
    let route = routes.get(path);
    let (status, extra, body): (&str, String, &[u8]) = match route {
        Some(Route::File(bytes) | Route::Short(bytes) | Route::Stall(bytes)) => {
            ("200 OK", String::new(), bytes)
        }
        Some(Route::Redirect(to)) => ("302 Found", format!("location: {to}\r\n"), b""),
        None => ("404 Not Found", String::new(), b"Not Found"),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/octet-stream\r\n{extra}\
         content-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let sent = match route {
        Some(Route::Short(_) | Route::Stall(_)) => &body[..body.len() / 2],
        _ => body,
    };
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(sent).await;
    let _ = stream.flush().await;
    if matches!(route, Some(Route::Stall(_))) {
        // Ends when the stub's runtime is dropped, which closes the
        // connection.
        std::future::pending::<()>().await;
    }
    let _ = stream.shutdown().await;
}
