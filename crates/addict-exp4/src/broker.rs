use crate::{
    Evidence, append,
    metrics::{self, ClockMapping},
    protocol::{self, Backend, Command, FrontendEvent, Invocation, Message, Mode, Origin},
};
use ashpd::desktop::{
    CreateSessionOptions,
    global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut},
};
use futures_util::StreamExt;
use serde_json::json;
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
    process::{Child, Command as ProcessCommand},
    sync::mpsc,
};

#[derive(Debug, Clone)]
pub struct Args {
    pub backend: Backend,
    pub mode: Mode,
    pub origin: Origin,
    pub frontend: PathBuf,
    pub socket: PathBuf,
    pub results: PathBuf,
    pub trigger: Option<String>,
    pub timeout: Duration,
    pub binding_timeout: Duration,
    pub local_only: bool,
    pub clock_mapping: Option<ClockMapping>,
    pub fixture: PathBuf,
    /// Glob for `VK_LOADER_DRIVERS_SELECT` in the frontend environment.
    pub vulkan_drivers: Option<String>,
}
enum Incoming {
    Frontend {
        pid: u32,
        backend: Backend,
        writer: OwnedWriteHalf,
    },
    Event {
        pid: u32,
        value: FrontendEvent,
    },
    Control {
        origin: Origin,
        quit: bool,
        writer: OwnedWriteHalf,
    },
    Failure(String),
}
struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn listener(path: &Path) -> io::Result<(UnixListener, SocketGuard)> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("socket needs a private parent directory"))?;
    if !parent.exists() {
        std::fs::create_dir(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let metadata = std::fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "socket directory must be owned by you with mode 0700",
        ));
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok((listener, SocketGuard(path.to_owned())))
}
async fn serve(listener: UnixListener, sender: mpsc::Sender<Incoming>) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            break;
        };
        let sender = sender.clone();
        tokio::spawn(async move {
            let credentials = match stream.peer_cred() {
                Ok(c) => c,
                Err(_) => return,
            };
            if credentials.uid() != unsafe { libc::geteuid() } {
                return;
            }
            let handshake =
                tokio::time::timeout(protocol::IPC_TIMEOUT, protocol::read_async(&mut stream))
                    .await;
            match handshake {
                Ok(Ok(Message::Hello {
                    version: protocol::WIRE_VERSION,
                    backend,
                    pid,
                })) if credentials.pid() == Some(pid as i32) => {
                    let (mut reader, writer) = stream.into_split();
                    if sender
                        .send(Incoming::Frontend {
                            pid,
                            backend,
                            writer,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                    loop {
                        match protocol::read_async(&mut reader).await {
                            Ok(Message::Event { value }) => {
                                if sender.send(Incoming::Event { pid, value }).await.is_err() {
                                    break;
                                }
                            }
                            Ok(_) => {
                                let _ = sender
                                    .send(Incoming::Failure("unexpected frontend message".into()))
                                    .await;
                                break;
                            }
                            Err(e) => {
                                let _ = sender
                                    .send(Incoming::Failure(format!("frontend IPC ended: {e}")))
                                    .await;
                                break;
                            }
                        }
                    }
                }
                Ok(Ok(Message::Control { origin, quit })) => {
                    let (_, writer) = stream.into_split();
                    let _ = sender
                        .send(Incoming::Control {
                            origin,
                            quit,
                            writer,
                        })
                        .await;
                }
                _ => {
                    let _ = sender
                        .send(Incoming::Failure(
                            "invalid or timed-out IPC handshake".into(),
                        ))
                        .await;
                }
            }
        });
    }
}
async fn send(writer: &mut OwnedWriteHalf, value: Command) -> io::Result<()> {
    tokio::time::timeout(
        protocol::IPC_TIMEOUT,
        protocol::write_async(writer, &Message::Command { value }),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "frontend write deadline"))?
}
fn launch(args: &Args, invocation: Option<&Invocation>) -> io::Result<Child> {
    let mut command = ProcessCommand::new(&args.frontend);
    command
        .arg("--socket")
        .arg(&args.socket)
        .arg("--mode")
        .arg(match args.mode {
            Mode::Resident => "resident",
            Mode::OnDemand => "on-demand",
        })
        .arg("--fixture")
        .arg(&args.fixture)
        .kill_on_drop(true);
    command
        .env_remove("DESKTOP_STARTUP_ID")
        .env_remove("XDG_ACTIVATION_TOKEN");
    // Both frontends consume the token once through the first IPC invocation.
    // Never also expose that single-use token through their startup environment.
    let _ = invocation;
    // Enumerating every Vulkan driver wakes a runtime-suspended discrete GPU.
    if let Some(drivers) = &args.vulkan_drivers {
        command.env("VK_LOADER_DRIVERS_SELECT", drivers);
    }
    command.process_group(0);
    command.spawn()
}
fn record(
    args: &Args,
    invocation: Option<&Invocation>,
    event: &str,
    details: serde_json::Value,
) -> io::Result<()> {
    append(
        &args.results,
        &Evidence::new(
            args.backend,
            args.mode,
            invocation.map_or(args.origin, |i| i.origin),
            invocation.map(|i| i.id.clone()),
            event,
            details,
        ),
    )
}
async fn stop(child: &mut Option<Child>) {
    if let Some(mut child) = child.take() {
        if matches!(
            tokio::time::timeout(Duration::from_secs(3), child.wait()).await,
            Ok(Ok(_))
        ) {
            return;
        }
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
    }
    *child = None;
}

pub async fn run(mut args: Args) -> io::Result<()> {
    if args.timeout.is_zero() || args.timeout > Duration::from_secs(3600) {
        return Err(io::Error::other("broker deadline must be 1..=3600 s"));
    }
    let frontend = args.frontend.canonicalize()?;
    if !frontend.is_file() {
        return Err(io::Error::other("frontend binary does not exist"));
    }
    let (listener, _guard) = listener(&args.socket)?;
    let (sender, mut incoming) = mpsc::channel(64);
    let server = tokio::spawn(serve(listener, sender));
    let environment = addict_probe::record::capture_environment(Duration::from_secs(2)).await;
    if let Some(mapping) = args.clock_mapping.as_ref()
        && let Err(error) = mapping.validate_environment(&environment)
    {
        record(
            &args,
            None,
            "clock-certificate-invalid",
            json!({"error":error}),
        )?;
        args.clock_mapping = None;
    }
    record(
        &args,
        None,
        "broker-started",
        json!({"pid":std::process::id(),"environment":environment,"frontend":frontend,"clock_mapping":args.clock_mapping,"socket":args.socket,"protocol":protocol::WIRE_VERSION,"vulkan_drivers":args.vulkan_drivers}),
    )?;
    let mut child = if args.mode == Mode::Resident {
        Some(launch(&args, None)?)
    } else {
        None
    };
    if let Some(frontend) = child.as_ref() {
        record(
            &args,
            None,
            "frontend-spawned",
            json!({"pid":frontend.id(),"new_process":true,"resident_prelaunch":true}),
        )?;
    }
    let result = run_session(&args, &mut child, &mut incoming).await;
    stop(&mut child).await;
    server.abort();
    record(
        &args,
        None,
        "broker-ended",
        json!({"outcome":if result.is_ok(){"completed"}else{"error"}, "error":result.as_ref().err().map(ToString::to_string)}),
    )?;
    result
}
async fn run_session(
    args: &Args,
    child: &mut Option<Child>,
    incoming: &mut mpsc::Receiver<Incoming>,
) -> io::Result<()> {
    let progress = addict_probe::record::Progress::new();
    let mut session = None;
    let mut activations: Option<
        std::pin::Pin<
            Box<
                dyn futures_util::Stream<Item = ashpd::desktop::global_shortcuts::Activated> + Send,
            >,
        >,
    > = None;
    let mut session_path = String::new();
    if !args.local_only {
        let setup = async {
            let connection = addict_probe::portal::connect(&progress)
                .await
                .map_err(|e| io::Error::other(format!("{:?}: {:?}", e.outcome, e.error)))?;
            addict_probe::portal::register(&connection, Some(args.backend.app_id()), &progress)
                .await
                .map_err(|e| io::Error::other(format!("{:?}: {:?}", e.outcome, e.error)))?;
            let portal = GlobalShortcuts::with_connection(connection)
                .await
                .map_err(io::Error::other)?;
            let stream = portal.receive_activated().await.map_err(io::Error::other)?;
            let created = portal
                .create_session(CreateSessionOptions::default())
                .await
                .map_err(io::Error::other)?;
            let shortcut = NewShortcut::new("exp4-invoke", "Addict EXP-4 window comparison")
                .preferred_trigger(args.trigger.as_deref());
            let bound = portal
                .bind_shortcuts(&created, &[shortcut], None, BindShortcutsOptions::default())
                .await
                .and_then(|r| r.response())
                .map_err(io::Error::other)?;
            if !bound
                .shortcuts()
                .iter()
                .any(|s| s.id() == "exp4-invoke" && !s.trigger_description().trim().is_empty())
            {
                return Err(io::Error::other(
                    "not-bound: shortcut trigger is empty/refused",
                ));
            }
            session_path = serde_json::to_value(&created)?
                .as_str()
                .ok_or_else(|| io::Error::other("missing session path"))?
                .to_owned();
            record(
                args,
                None,
                "shortcut-bound",
                json!({"trigger":bound.shortcuts()[0].trigger_description(),"app_id":args.backend.app_id()}),
            )?;
            session = Some(created);
            activations = Some(Box::pin(stream));
            Ok::<_, io::Error>(())
        };
        match tokio::time::timeout(args.binding_timeout, setup).await {
            Ok(Ok(())) => {}
            failure => {
                record(
                    args,
                    None,
                    "binding-failed",
                    json!({"error":format!("{failure:?}"),"local_invoke_available":true,"reason_inferred":false}),
                )?;
            }
        }
    }
    let deadline = tokio::time::sleep(args.timeout);
    tokio::pin!(deadline);
    let mut timer = tokio::time::interval(Duration::from_millis(100));
    let mut writer = None;
    let mut current: Option<Invocation> = None;
    let mut dispatched = false;
    let mut completed = false;
    let mut token_present = false;
    let mut token_consumed = false;
    let mut count = 0u64;
    let mut launches = 0u64;
    let mut history = HashMap::<String, Invocation>::new();
    loop {
        let mut activation = None;
        tokio::select! {
            _ = &mut deadline => { record(args,current.as_ref(),"session-timeout",json!({"limit_seconds":args.timeout.as_secs()}))?; break; },
            _ = tokio::signal::ctrl_c() => { record(args,current.as_ref(),"quit",json!({"source":"sigint"}))?; break; },
            signal = async { match activations.as_mut() { Some(s) => s.next().await, None => std::future::pending().await } } => {
                if let Some(signal) = signal {
                    if signal.session_handle().as_str() == session_path && signal.shortcut_id() == "exp4-invoke" {
                        let token = signal.options().get("activation_token").and_then(|v| <&str>::try_from(v).ok()).map(str::to_owned).filter(|s| !s.is_empty());
                        activation = Some((args.origin,Some(signal.timestamp().as_millis() as u64),token));
                    }
                } else { record(args,None,"portal-stream-ended",json!({}))?; activations=None; }
            },
            value = incoming.recv() => match value {
                Some(Incoming::Frontend { pid, backend, writer: frontend_writer }) => {
                    if backend != args.backend || child.as_ref().and_then(Child::id) != Some(pid) { record(args,current.as_ref(),"ipc-rejected",json!({"pid":pid,"backend":backend}))?; continue; }
                    writer = Some(frontend_writer); record(args,current.as_ref(),"frontend-connected",json!({"pid":pid}))?;
                },
                Some(Incoming::Control { origin, quit, writer: mut reply }) => {
                    protocol::write_async(&mut reply,&Message::Ack {accepted:true,detail: if quit {"quitting"}else{"invocation queued"}.into()}).await?;
                    if quit { record(args,current.as_ref(),"quit",json!({"source":"local-control"}))?; break; }
                    activation=Some((origin,None,None));
                },
                Some(Incoming::Event {pid,value}) => {
                    if child.as_ref().and_then(Child::id)!=Some(pid) {record(args,None,"ipc-rejected",json!({"pid":pid,"error":"event from unauthenticated child"}))?;continue;}
                    let details=serde_json::to_value(&value)?;
                    match &value {
                        FrontendEvent::Presented {id,time_ns,clock_id,usable,limitation, ..} => {
                            if current.as_ref().is_none_or(|i| &i.id != id) { record(args,None,"uncorrelated-presentation",details)?; continue; }
                            let i=current.as_ref().unwrap();
                            let latency = match (&args.clock_mapping,i.portal_timestamp_ms,time_ns,clock_id) {
                                (Some(mapping),Some(raw),Some(time),Some(clock)) if *usable && limitation.is_none() => acceptance_latency(mapping,i,token_present,token_consumed,raw,*time,*clock),
                                _ => Err("missing clock mapping, portal activation, usable frame or presentation feedback".into())
                            };
                            let mut evidence=details; evidence["acceptance_latency_ns"]=json!(latency.as_ref().ok()); evidence["clock_error"]=json!(latency.err());
                            record(args,current.as_ref(),"presentation",evidence)?; completed=true;
                        },
                        FrontendEvent::Closed { id } => {
                            if current.as_ref().is_some_and(|i| &i.id == id) { record(args,current.as_ref(),"closed",details)?; current=None; dispatched=false; completed=false;
                                if args.mode == Mode::OnDemand {
                                    writer=None;
                                    stop(child).await;
                                    record(args,None,"frontend-exited",json!({"reason":"closed on-demand child reaped"}))?;
                                }
                            }
                        },
                        FrontendEvent::Quit => { record(args,current.as_ref(),"quit",details)?; break; },
                        FrontendEvent::Activated {id,token_consumed: consumed} => {
                            if dispatched && current.as_ref().is_some_and(|i| &i.id == id) { token_consumed = *consumed; }
                            record(args,history.get(id),"frontend-event",details)?;
                        },
                        FrontendEvent::Work {id,..} | FrontendEvent::Focus {id,..} => {
                            record(args,history.get(id),"frontend-event",details)?;
                        },
                        _ => record(args,current.as_ref(),"frontend-event",details)?,
                    }
                },
                Some(Incoming::Failure(error)) => record(args,current.as_ref(),"ipc-error",json!({"error":error}))?,
                None => break,
            },
            _ = timer.tick() => {
                if let Some(frontend)=child.as_mut() && let Some(status)=frontend.try_wait()? {
                    record(args,current.as_ref(),"frontend-exited",json!({"status":status.to_string()}))?; *child=None; writer=None;
                    if current.is_some() && !completed { record(args,current.as_ref(),"invocation-failed",json!({"error":"child exited before usable presentation"}))?; }
                    current=None; dispatched=false; completed=false;
                }
                if current.as_ref().is_some_and(|i| !completed && metrics::monotonic_ns().saturating_sub(i.receipt_monotonic_ns)>30_000_000_000) {
                    record(args,current.as_ref(),"invocation-timeout",json!({"deadline_seconds":30}))?; current=None; dispatched=false; completed=false;
                }
            }
        }
        if let Some((origin, portal_timestamp_ms, activation_token)) = activation {
            if current.is_some() && !completed {
                record(
                    args,
                    current.as_ref(),
                    "invocation-busy",
                    json!({"origin":origin}),
                )?;
                continue;
            }
            count += 1;
            let cold = if args.mode == Mode::Resident {
                count == 1
            } else {
                launches == 0
            };
            let invocation = Invocation {
                id: format!("{}-{}-{count}", std::process::id(), metrics::monotonic_ns()),
                origin,
                receipt_monotonic_ns: metrics::monotonic_ns(),
                portal_timestamp_ms,
                activation_token,
                cold,
            };
            let frontend_spawned = child.is_none();
            record(
                args,
                Some(&invocation),
                "invocation",
                json!({"cold":cold,"portal_timestamp_ms":portal_timestamp_ms,"receipt_monotonic_ns":invocation.receipt_monotonic_ns,"activation_token_present":invocation.activation_token.is_some(),"frontend_spawned":frontend_spawned,"frontend_start":"new-process-if-spawned; cold flag describes first broker/cache invocation"}),
            )?;
            if child.is_none() {
                *child = Some(launch(args, Some(&invocation))?);
                launches += 1;
                record(
                    args,
                    Some(&invocation),
                    "frontend-spawned",
                    json!({"pid":child.as_ref().and_then(Child::id),"new_process":true,"resident_prelaunch":false}),
                )?;
            }
            let mut remembered = invocation.clone();
            remembered.activation_token = None;
            history.insert(remembered.id.clone(), remembered);
            token_present = invocation.activation_token.is_some();
            token_consumed = false;
            current = Some(invocation);
            dispatched = false;
            completed = false;
        }
        if !dispatched && let (Some(writer), Some(invocation)) = (writer.as_mut(), current.as_ref())
        {
            match send(
                writer,
                Command::Invoke {
                    invocation: invocation.clone(),
                },
            )
            .await
            {
                Ok(()) => {
                    dispatched = true;
                    if let Some(i) = current.as_mut() {
                        i.activation_token = None;
                    }
                }
                Err(e) => {
                    record(
                        args,
                        current.as_ref(),
                        "ipc-send-failed",
                        json!({"error":e.to_string()}),
                    )?;
                    current = None;
                }
            }
        }
    }
    if let Some(writer) = writer.as_mut() {
        let _ = send(writer, Command::Quit).await;
    }
    if let Some(session) = session {
        let outcome = tokio::time::timeout(Duration::from_secs(3), session.close()).await;
        record(
            args,
            None,
            "session-closed",
            json!({"outcome":format!("{outcome:?}")}),
        )?;
    }
    Ok(())
}
pub async fn control(socket: &Path, origin: Origin, quit: bool) -> io::Result<Message> {
    tokio::time::timeout(protocol::IPC_TIMEOUT, async {
        let mut stream = UnixStream::connect(socket).await?;
        protocol::write_async(&mut stream, &Message::Control { origin, quit }).await?;
        protocol::read_async(&mut stream).await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "local control deadline"))?
}

fn acceptance_latency(
    mapping: &ClockMapping,
    invocation: &Invocation,
    token_present: bool,
    token_consumed: bool,
    raw: u64,
    time: u64,
    clock: i32,
) -> Result<u64, String> {
    if !token_present || !token_consumed {
        return Err("activation token absent or no correlated token consumption".into());
    }
    mapping.latency(raw, time, clock, invocation.receipt_monotonic_ns)
}
#[cfg(test)]
mod acceptance_tests {
    use super::*;
    #[test]
    fn valid_clock_and_frame_require_present_and_consumed_token() {
        let mapping: ClockMapping = serde_json::from_value(json!({"portal_backend":"gnome","backend_version":"50","evidence_path":"proof","evidence_sha256":"a".repeat(64),"verified":true,"portal_clock_id":1,"presentation_clock_id":1,"portal_to_presentation_offset_ns":0,"uncertainty_ns":0,"boot_id":"boot","session_type":"wayland","shell_version":"50","portal_version":"1","portal_timestamp_bits":64})).unwrap();
        let invocation = Invocation {
            id: "test".into(),
            origin: Origin::Foliate,
            receipt_monotonic_ns: 1_000_000_000,
            portal_timestamp_ms: Some(1000),
            activation_token: None,
            cold: false,
        };
        for (present, consumed) in [(false, false), (false, true), (true, false)] {
            assert!(
                acceptance_latency(
                    &mapping,
                    &invocation,
                    present,
                    consumed,
                    1000,
                    1_100_000_000,
                    1
                )
                .is_err()
            );
        }
        assert_eq!(
            acceptance_latency(&mapping, &invocation, true, true, 1000, 1_100_000_000, 1).unwrap(),
            101_000_000
        );
    }
}
