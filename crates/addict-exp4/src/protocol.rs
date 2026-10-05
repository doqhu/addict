use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const WIRE_VERSION: u32 = 1;
pub const MAX_MESSAGE: usize = 16 * 1024;
pub const IPC_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    Gpui,
    Gtk4,
}
impl Backend {
    pub fn app_id(self) -> &'static str {
        match self {
            Self::Gpui => "dev.addict.Exp4.Gpui",
            Self::Gtk4 => "dev.addict.Exp4.Gtk4",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Resident,
    OnDemand,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    Foliate,
    IntelliJ,
    AndroidStudio,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    pub id: String,
    pub origin: Origin,
    pub receipt_monotonic_ns: u64,
    pub portal_timestamp_ms: Option<u64>,
    pub activation_token: Option<String>,
    pub cold: bool,
}
impl Invocation {
    pub fn validate(&self) -> io::Result<()> {
        if self.id.is_empty()
            || self.id.len() > 128
            || self
                .activation_token
                .as_ref()
                .is_some_and(|t| t.is_empty() || t.len() > 4096)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid invocation/token",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Command {
    Invoke { invocation: Invocation },
    Quit,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FrontendEvent {
    Ready {
        pid: u32,
        backend: Backend,
        version: u32,
    },
    Activated {
        id: String,
        token_consumed: bool,
    },
    Presented {
        id: String,
        time_ns: Option<u64>,
        clock_id: Option<i32>,
        usable: bool,
        diagnostic_draw_ns: u64,
        limitation: Option<String>,
    },
    Focus {
        id: String,
        focused: bool,
    },
    Closed {
        id: String,
    },
    Work {
        id: String,
        outcome: String,
        child_pid: Option<u32>,
    },
    Error {
        id: Option<String>,
        error: String,
    },
    Quit,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Message {
    Hello {
        version: u32,
        backend: Backend,
        pid: u32,
    },
    Control {
        origin: Origin,
        quit: bool,
    },
    Command {
        value: Command,
    },
    Event {
        value: FrontendEvent,
    },
    Ack {
        accepted: bool,
        detail: String,
    },
}

fn encode(message: &Message) -> io::Result<Vec<u8>> {
    let body = serde_json::to_vec(message)?;
    if body.is_empty() || body.len() > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC frame too large",
        ));
    }
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend(body);
    Ok(frame)
}
fn size(header: [u8; 4]) -> io::Result<usize> {
    let size = u32::from_be_bytes(header) as usize;
    if size == 0 || size > MAX_MESSAGE {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid IPC length",
        ))
    } else {
        Ok(size)
    }
}
pub fn read(stream: &mut impl Read) -> io::Result<Message> {
    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    let mut body = vec![0; size(header)?];
    stream.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}
pub fn write(stream: &mut impl Write, message: &Message) -> io::Result<()> {
    stream.write_all(&encode(message)?)
}
pub async fn read_async(stream: &mut (impl tokio::io::AsyncRead + Unpin)) -> io::Result<Message> {
    let mut header = [0; 4];
    // Waiting idle for an event is distinct from an incomplete frame. Once its
    // first byte arrives, the entire remaining message has a 3 s deadline.
    stream.read_exact(&mut header[..1]).await?;
    tokio::time::timeout(IPC_TIMEOUT, async {
        stream.read_exact(&mut header[1..]).await?;
        let mut body = vec![0; size(header)?];
        stream.read_exact(&mut body).await?;
        Ok(serde_json::from_slice(&body)?)
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "incomplete IPC frame deadline"))?
}
pub async fn write_async(
    stream: &mut (impl tokio::io::AsyncWrite + Unpin),
    message: &Message,
) -> io::Result<()> {
    stream.write_all(&encode(message)?).await
}

// Idle waiting is unbounded. Once one byte arrives, header and body share a deadline.
fn read_frontend_frame(socket: &mut UnixStream, limit: Duration) -> io::Result<Message> {
    socket.set_read_timeout(None)?;
    let mut header = [0; 4];
    socket.read_exact(&mut header[..1])?;
    let deadline = Instant::now() + limit;
    read_before(socket, &mut header[1..], deadline)?;
    let mut body = vec![0; size(header)?];
    read_before(socket, &mut body, deadline)?;
    Ok(serde_json::from_slice(&body)?)
}
fn read_before(socket: &mut UnixStream, mut bytes: &mut [u8], deadline: Instant) -> io::Result<()> {
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "incomplete IPC frame deadline")
            })?;
        socket.set_read_timeout(Some(remaining))?;
        match socket.read(bytes) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => bytes = &mut bytes[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Err(io::ErrorKind::TimedOut.into());
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

type Wake = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;
fn wake(hook: &Wake) {
    if let Ok(hook) = hook.lock()
        && let Some(hook) = hook.as_ref()
    {
        hook();
    }
}
/// Background-only receiver; the UI thread drains its bounded channel when woken.
pub struct Client {
    writer: Arc<Mutex<UnixStream>>,
    pub commands: std::sync::mpsc::Receiver<Command>,
    wake: Wake,
}
impl Client {
    pub fn connect(path: &Path, backend: Backend) -> io::Result<Self> {
        let mut socket = UnixStream::connect(path)?;
        socket.set_write_timeout(Some(IPC_TIMEOUT))?;
        write(
            &mut socket,
            &Message::Hello {
                version: WIRE_VERSION,
                backend,
                pid: std::process::id(),
            },
        )?;
        let writer = Arc::new(Mutex::new(socket.try_clone()?));
        let (sender, commands) = std::sync::mpsc::sync_channel(8);
        let hook = Wake::default();
        let reader_hook = hook.clone();
        std::thread::spawn(move || {
            loop {
                let message = read_frontend_frame(&mut socket, IPC_TIMEOUT);
                match message {
                    Ok(Message::Command { value }) => {
                        if let Command::Invoke { invocation } = &value
                            && invocation.validate().is_err()
                        {
                            break;
                        }
                        if sender.try_send(value).is_err() {
                            break;
                        }
                        wake(&reader_hook);
                    }
                    _ => break,
                }
            }
            let _ = sender.try_send(Command::Quit);
            wake(&reader_hook);
        });
        Ok(Self {
            writer,
            commands,
            wake: hook,
        })
    }
    /// Called from any thread after a command or an observed event is queued, so the UI
    /// thread can sleep instead of polling. The hook must not block.
    pub fn on_activity(&self, hook: impl Fn() + Send + 'static) {
        *self.wake.lock().unwrap() = Some(Box::new(hook));
    }
    /// Queue a small event; socket I/O happens off the UI thread.
    pub fn emitter(&self) -> Emitter {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<FrontendEvent>(64);
        let writer = self.writer.clone();
        std::thread::spawn(move || {
            while let Ok(value) = receiver.recv() {
                let Ok(mut socket) = writer.lock() else {
                    break;
                };
                if write(&mut *socket, &Message::Event { value }).is_err() {
                    break;
                }
            }
        });
        Emitter {
            sender,
            observers: Arc::new(Mutex::new(Vec::new())),
            wake: self.wake.clone(),
        }
    }
}
#[derive(Clone)]
pub struct Emitter {
    sender: std::sync::mpsc::SyncSender<FrontendEvent>,
    observers: Arc<Mutex<Vec<std::sync::mpsc::SyncSender<FrontendEvent>>>>,
    wake: Wake,
}
impl Emitter {
    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<FrontendEvent> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(64);
        self.observers.lock().unwrap().push(sender);
        receiver
    }
    pub fn send(&self, event: FrontendEvent) {
        for observer in self.observers.lock().unwrap().iter() {
            let _ = observer.try_send(event.clone());
            wake(&self.wake);
        }
        if self.sender.try_send(event).is_err() {
            eprintln!("EXP-4 event queue full/disconnected; evidence is incomplete");
        }
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    #[test]
    fn partial_header_has_whole_frame_deadline() {
        let (mut receiver, mut sender) = UnixStream::pair().unwrap();
        sender.write_all(&[0, 0]).unwrap();
        let start = Instant::now();
        assert_eq!(
            read_frontend_frame(&mut receiver, Duration::from_millis(100))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() < Duration::from_millis(500));
    }
    #[test]
    fn trickling_body_cannot_extend_whole_frame_deadline() {
        let (mut receiver, mut sender) = UnixStream::pair().unwrap();
        sender.write_all(&100u32.to_be_bytes()).unwrap();
        let producer = std::thread::spawn(move || {
            for _ in 0..50 {
                if sender.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let start = Instant::now();
        assert_eq!(
            read_frontend_frame(&mut receiver, Duration::from_millis(100))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() < Duration::from_millis(500));
        drop(receiver);
        producer.join().unwrap();
    }
}
