//! Persistent EXP-4 broker against an isolated portal and a real child process.
use addict_exp4::{
    broker, metrics,
    protocol::{Backend, Mode, Origin},
};
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::HashMap,
    path::PathBuf,
    process::{Child, Stdio},
    time::{Duration, Instant},
};
use zbus::{
    Connection, ObjectServer, fdo,
    message::Header,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};
const PATH: &str = "/org/freedesktop/portal/desktop";
struct Bus {
    daemon: Child,
    address: String,
    dir: PathBuf,
}
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn bus(name: &str) -> Bus {
    let dir = std::env::temp_dir().join(format!(
        "exp4-{name}-{}-{}",
        std::process::id(),
        metrics::monotonic_ns()
    ));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.join("bus");
    let config = dir.join("bus.conf");
    std::fs::write(&config,format!(r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd"><busconfig><type>session</type><listen>unix:path={}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>"#,socket.display())).unwrap();
    let mut daemon = std::process::Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .arg("--nofork")
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !socket.exists() {
        assert!(daemon.try_wait().unwrap().is_none());
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    Bus {
        daemon,
        address: format!("unix:path={}", socket.display()),
        dir,
    }
}
fn request(header: &Header<'_>, options: &HashMap<String, OwnedValue>) -> OwnedObjectPath {
    let sender = header
        .sender()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let token = <&str>::try_from(options.get("handle_token").unwrap()).unwrap();
    format!("{PATH}/request/{sender}/{token}")
        .try_into()
        .unwrap()
}
async fn response(
    connection: &Connection,
    request: &OwnedObjectPath,
    code: u32,
    result: HashMap<&str, Value<'_>>,
) -> fdo::Result<()> {
    connection
        .emit_signal(
            None::<&str>,
            request,
            "org.freedesktop.portal.Request",
            "Response",
            &(code, result),
        )
        .await
        .map_err(|e| fdo::Error::Failed(e.to_string()))
}
struct Registry;
#[zbus::interface(name = "org.freedesktop.host.portal.Registry")]
impl Registry {
    fn register(&self, _app_id: String, _options: HashMap<String, OwnedValue>) {}
}
#[derive(Clone, Copy)]
enum Behavior {
    Success,
    Refused,
    Empty,
    Response1,
    Response2,
    Timeout,
}
struct Shortcuts(Behavior);
struct Session;
#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl Session {
    fn close(&self) {}
}
#[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts")]
impl Shortcuts {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
    async fn create_session(
        &self,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<OwnedObjectPath> {
        let request = request(&header, &options);
        let sender = header
            .sender()
            .unwrap()
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");
        let token = <&str>::try_from(options.get("session_handle_token").unwrap()).unwrap();
        let path = format!("{PATH}/session/{sender}/{token}");
        server.at(path.as_str(), Session).await.unwrap();
        response(
            connection,
            &request,
            0,
            HashMap::from([("session_handle", Value::from(path))]),
        )
        .await?;
        Ok(request)
    }
    async fn bind_shortcuts(
        &self,
        _session: OwnedObjectPath,
        _shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
        _parent: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<OwnedObjectPath> {
        let request = request(&header, &options);
        match self.0 {
            Behavior::Success => {
                let shortcuts = vec![(
                    "exp4-invoke".to_owned(),
                    HashMap::from([
                        ("description", Value::from("EXP-4 test")),
                        ("trigger_description", Value::from("Test shortcut")),
                    ]),
                )];
                response(
                    connection,
                    &request,
                    0,
                    HashMap::from([("shortcuts", Value::from(shortcuts))]),
                )
                .await?;
                let connection = connection.clone();
                tokio::spawn(async move {
                    let foreign: OwnedObjectPath = format!("{PATH}/session/foreign/ignored")
                        .try_into()
                        .unwrap();
                    for (session, stamp, token) in [
                        (&foreign, 1233, Some("foreign-token")),
                        (&_session, 1234, Some("fresh-token-1")),
                        (&_session, 1235, Some("fresh-token-2")),
                        (&_session, 1236, None),
                    ] {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        let options = token
                            .map(|token| HashMap::from([("activation_token", Value::from(token))]))
                            .unwrap_or_default();
                        connection
                            .emit_signal(
                                None::<&str>,
                                PATH,
                                "org.freedesktop.portal.GlobalShortcuts",
                                "Activated",
                                &(session, "exp4-invoke", stamp as u64, options),
                            )
                            .await
                            .unwrap();
                    }
                });
            }
            Behavior::Refused => return Err(fdo::Error::AccessDenied("binding refused".into())),
            Behavior::Empty => {
                response(
                    connection,
                    &request,
                    0,
                    HashMap::from([(
                        "shortcuts",
                        Value::from(Vec::<(String, HashMap<String, Value>)>::new()),
                    )]),
                )
                .await?
            }
            Behavior::Response1 => response(connection, &request, 1, HashMap::new()).await?,
            Behavior::Response2 => response(connection, &request, 2, HashMap::new()).await?,
            Behavior::Timeout => {}
        }
        Ok(request)
    }
}
fn frontend(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("frontend");
    std::fs::write(&path,r#"#!/usr/bin/python3
import socket,struct,json,os,sys,time
s=socket.socket(socket.AF_UNIX);s.connect(sys.argv[sys.argv.index('--socket')+1]);mode=sys.argv[sys.argv.index('--mode')+1]
def send(v):
 b=json.dumps(v).encode();s.sendall(struct.pack('!I',len(b))+b)
def readn(n):
 b=b''
 while len(b)<n:
  x=s.recv(n-len(b))
  if not x: raise EOFError()
  b+=x
 return b
def receive():return json.loads(readn(struct.unpack('!I',readn(4))[0]))
send({'type':'hello','version':1,'backend':'gtk4','pid':os.getpid()})
while True:
 c=receive()['value']
 if c['command']=='quit':break
 i=c['invocation']['id']
 token=c['invocation']['activation_token']
 assert 'XDG_ACTIVATION_TOKEN' not in os.environ
 with open(os.path.join(os.path.dirname(sys.argv[0]),'received-tokens.jsonl'),'a') as received:received.write(json.dumps({'id':i,'token':token})+'\n')
 send({'type':'event','value':{'event':'activated','id':i,'token_consumed':bool(token)}})
 send({'type':'event','value':{'event':'focus','id':i,'focused':True}})
 send({'type':'event','value':{'event':'presented','id':i,'time_ns':None,'clock_id':None,'usable':True,'diagnostic_draw_ns':0,'limitation':'mock has no display'}})
 send({'type':'event','value':{'event':'closed','id':i}})
 if mode=='on-demand':
  time.sleep(0.25)
  break
"#).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn portal_success_filters_session_routes_fresh_tokens_and_correlates_frontend_events() {
    let bus = bus("success");
    let _portal = zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.freedesktop.portal.Desktop")
        .unwrap()
        .serve_at(PATH, Registry)
        .unwrap()
        .serve_at(PATH, Shortcuts(Behavior::Success))
        .unwrap()
        .build()
        .await
        .unwrap();
    let socket = bus.dir.join("broker.sock");
    let results = bus.dir.join("results.jsonl");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_addict-exp4"))
        .args([
            "broker",
            "--backend",
            "gtk4",
            "--mode",
            "resident",
            "--origin",
            "foliate",
            "--binding-timeout-seconds",
            "1",
            "--timeout-seconds",
            "10",
        ])
        .arg("--frontend")
        .arg(frontend(&bus.dir))
        .arg("--socket")
        .arg(&socket)
        .arg("--results")
        .arg(&results)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(&results, "closed", 3).await;
    broker::control(&socket, Origin::Local, true).await.unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        assert!(start.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let rows = evidence(&results);
    let invocations: Vec<_> = rows.iter().filter(|r| r.event == "invocation").collect();
    assert_eq!(invocations.len(), 3, "foreign session must be ignored");
    let received: Vec<serde_json::Value> =
        std::fs::read_to_string(bus.dir.join("received-tokens.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(
        received
            .iter()
            .map(|r| r["token"].clone())
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!("fresh-token-1"),
            serde_json::json!("fresh-token-2"),
            serde_json::Value::Null
        ]
    );
    let pid = rows
        .iter()
        .find(|r| r.event == "frontend-connected")
        .unwrap()
        .details["pid"]
        .as_u64()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    for (n, invocation) in invocations.iter().enumerate() {
        assert_eq!(invocation.origin, Origin::Foliate);
        assert_eq!(invocation.details["portal_timestamp_ms"], 1234 + n as u64);
        assert_eq!(invocation.details["activation_token_present"], n < 2);
        assert_eq!(
            received[n]["id"].as_str(),
            invocation.invocation_id.as_deref()
        );
        let events: Vec<_> = rows
            .iter()
            .filter(|r| r.invocation_id == invocation.invocation_id)
            .collect();
        let activated = events
            .iter()
            .find(|r| r.event == "frontend-event" && r.details["event"] == "activated")
            .unwrap();
        assert_eq!(activated.details["token_consumed"], n < 2);
        assert!(events.iter().any(|r| r.event == "frontend-event"
            && r.details["event"] == "focus"
            && r.details["focused"] == true));
        let frame = events.iter().find(|r| r.event == "presentation").unwrap();
        assert_eq!(
            frame.details["id"].as_str(),
            invocation.invocation_id.as_deref()
        );
        assert!(frame.details["acceptance_latency_ns"].is_null());
    }
    let raw = std::fs::read_to_string(&results).unwrap();
    assert!(!raw.contains("fresh-token") && !raw.contains("foreign-token"));
}
fn evidence(path: &std::path::Path) -> Vec<addict_exp4::Evidence> {
    metrics::read_evidence(path).unwrap_or_default()
}
async fn wait_for(path: &std::path::Path, event: &str, count: usize) {
    let start = Instant::now();
    while evidence(path).iter().filter(|r| r.event == event).count() < count {
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "missing {event} {count}: {:?}",
            evidence(path)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn refused_empty_response1_response2_timeout_keep_local_fallback() {
    for (name, behavior) in [
        ("refused", Behavior::Refused),
        ("empty", Behavior::Empty),
        ("response1", Behavior::Response1),
        ("response2", Behavior::Response2),
        ("timeout", Behavior::Timeout),
    ] {
        let bus = bus(name);
        let _portal = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name("org.freedesktop.portal.Desktop")
            .unwrap()
            .serve_at(PATH, Registry)
            .unwrap()
            .serve_at(PATH, Shortcuts(behavior))
            .unwrap()
            .build()
            .await
            .unwrap();
        let socket = bus.dir.join("broker.sock");
        let results = bus.dir.join("results.jsonl");
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_addict-exp4"))
            .args([
                "broker",
                "--backend",
                "gtk4",
                "--mode",
                "resident",
                "--binding-timeout-seconds",
                "1",
                "--timeout-seconds",
                "10",
            ])
            .arg("--frontend")
            .arg(frontend(&bus.dir))
            .arg("--socket")
            .arg(&socket)
            .arg("--results")
            .arg(&results)
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for(&results, "binding-failed", 1).await;
        assert_eq!(
            evidence(&results)
                .iter()
                .find(|r| r.event == "binding-failed")
                .unwrap()
                .details["local_invoke_available"],
            true
        );
        broker::control(&socket, Origin::Local, false)
            .await
            .unwrap();
        wait_for(&results, "presentation", 1).await;
        broker::control(&socket, Origin::Local, true).await.unwrap();
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(start.elapsed() < Duration::from_secs(5));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let rows = evidence(&results);
        assert!(rows.iter().any(|r| r.event == "broker-ended"));
        assert!(!rows.iter().any(|r| r.event == "shortcut-bound"));
        let errors: Vec<_> = rows
            .iter()
            .filter(|r| r.event == "binding-failed")
            .collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].details["reason_inferred"], false);
        let error = errors[0].details["error"].as_str().unwrap();
        match behavior {
            Behavior::Response1 => assert!(error.contains("Cancelled"), "{error}"),
            Behavior::Response2 => assert!(error.contains("Other"), "{error}"),
            Behavior::Timeout => assert!(error.contains("Elapsed"), "{error}"),
            _ => {}
        }
    }
}
#[tokio::test]
async fn repeated_close_reinvoke_keeps_resident_pid_and_relaunches_on_demand() {
    for mode in [Mode::Resident, Mode::OnDemand] {
        let bus = bus(if mode == Mode::Resident {
            "resident"
        } else {
            "ondemand"
        });
        let socket = bus.dir.join("broker.sock");
        let results = bus.dir.join("results.jsonl");
        let args = broker::Args {
            backend: Backend::Gtk4,
            mode,
            origin: Origin::Local,
            frontend: frontend(&bus.dir),
            socket: socket.clone(),
            results: results.clone(),
            trigger: None,
            timeout: Duration::from_secs(10),
            binding_timeout: Duration::from_secs(1),
            local_only: true,
            clock_mapping: None,
            fixture: PathBuf::from("unused"),
            vulkan_drivers: None,
        };
        let task = tokio::spawn(broker::run(args));
        while !socket.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for n in 1..=2 {
            broker::control(&socket, Origin::Local, false)
                .await
                .unwrap();
            wait_for(&results, "closed", n).await;
        }
        broker::control(&socket, Origin::Local, true).await.unwrap();
        task.await.unwrap().unwrap();
        let rows = evidence(&results);
        let pids: Vec<_> = rows
            .iter()
            .filter(|r| r.event == "frontend-connected")
            .map(|r| r.details["pid"].as_u64().unwrap())
            .collect();
        if mode == Mode::Resident {
            assert_eq!(pids.len(), 1);
        } else {
            assert_eq!(pids.len(), 2);
            assert_ne!(pids[0], pids[1]);
        }
        for pid in pids {
            assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        }
        let ids: Vec<_> = rows
            .iter()
            .filter(|r| r.event == "presentation")
            .map(|r| r.invocation_id.as_ref().unwrap())
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
    }
}
