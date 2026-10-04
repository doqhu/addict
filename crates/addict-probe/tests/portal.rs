//! The matrix rows that need a portal, end to end: the built binary runs
//! against a private `dbus-daemon` on which this test owns
//! `org.freedesktop.portal.Desktop` and serves just enough of the portal.
//! The user's bus and desktop are never touched.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::Value as Json;
use zbus::{
    Connection, ObjectServer, fdo,
    message::Header,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const TRIGGER: &str = "Press <Control><Alt>d";

// ---------------------------------------------------------------- the bus

/// A private bus that dies with the test.
struct Bus {
    daemon: Child,
    address: String,
    dir: PathBuf,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

fn start_bus(name: &str) -> Bus {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("portal-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("bus");
    let config = dir.join("bus.conf");
    std::fs::write(
        &config,
        format!(
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
            socket.display()
        ),
    )
    .unwrap();
    let mut daemon = std::process::Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .arg("--nofork")
        .stdin(Stdio::null())
        .spawn()
        .expect("dbus-daemon is required for the portal tests and could not be started");
    let started = Instant::now();
    while !socket.exists() {
        if let Some(status) = daemon.try_wait().unwrap() {
            panic!("dbus-daemon exited at once with {status}");
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "dbus-daemon did not open its socket within 5 s"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    Bus {
        daemon,
        address: format!("unix:path={}", socket.display()),
        dir,
    }
}

// ------------------------------------------------------- the fake portal

#[derive(Clone, Copy, PartialEq)]
enum Bind {
    /// Response 0 with one bound shortcut.
    Accept,
    /// Response 1: the dialog was dismissed.
    Dismiss,
    /// Response 0 with zero shortcuts.
    Zero,
}

#[derive(Clone, PartialEq)]
enum Shot {
    Uri(String),
    Cancel,
}

#[derive(Clone)]
struct Script {
    versions: [u32; 5],
    available_targets: u32,
    bind: Bind,
    press: bool,
    shot: Shot,
    /// Whether anything owns `org.freedesktop.portal.Desktop` on the bus.
    portal: bool,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            versions: [1, 2, 1, 2, 1],
            available_targets: 0,
            bind: Bind::Accept,
            press: true,
            shot: Shot::Cancel,
            portal: true,
        }
    }
}

/// What the probe sent, for the test to assert on.
#[derive(Default)]
struct Seen {
    registered: Vec<String>,
    bound_ids: Vec<String>,
    preferred_triggers: Vec<Option<String>>,
    screenshot_option_keys: Vec<String>,
    sessions_closed: usize,
}

type Shared = Arc<Mutex<Seen>>;

fn token(options: &HashMap<String, OwnedValue>, key: &str) -> fdo::Result<String> {
    options
        .get(key)
        .and_then(|v| <&str>::try_from(v).ok())
        .map(str::to_owned)
        .ok_or_else(|| fdo::Error::InvalidArgs(format!("missing {key}")))
}

fn sender_element(header: &Header<'_>) -> String {
    let sender = header.sender().expect("a method call has a sender");
    sender.trim_start_matches(':').replace('.', "_")
}

fn request_path(
    header: &Header<'_>,
    options: &HashMap<String, OwnedValue>,
) -> fdo::Result<OwnedObjectPath> {
    let path = format!(
        "{DESKTOP_PATH}/request/{}/{}",
        sender_element(header),
        token(options, "handle_token")?
    );
    Ok(ObjectPath::try_from(path).unwrap().into())
}

async fn respond(
    connection: &Connection,
    request: &OwnedObjectPath,
    code: u32,
    results: HashMap<&str, Value<'_>>,
) -> fdo::Result<()> {
    connection
        .emit_signal(
            None::<&str>,
            request,
            "org.freedesktop.portal.Request",
            "Response",
            &(code, results),
        )
        .await
        .map_err(|e| fdo::Error::Failed(e.to_string()))
}

fn shortcut_list(ids: &[String]) -> Value<'static> {
    let list: Vec<(String, HashMap<&str, Value<'_>>)> = ids
        .iter()
        .map(|id| {
            (
                id.clone(),
                HashMap::from([
                    ("description", Value::from("Addict probe: invoke")),
                    ("trigger_description", Value::from(TRIGGER)),
                ]),
            )
        })
        .collect();
    Value::from(list)
}

struct Session(Shared);

#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl Session {
    fn close(&self) {
        self.0.lock().unwrap().sessions_closed += 1;
    }
}

struct GlobalShortcuts(Script, Shared);

#[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts")]
impl GlobalShortcuts {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.0.versions[0]
    }

    async fn create_session(
        &self,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<OwnedObjectPath> {
        let request = request_path(&header, &options)?;
        let session = format!(
            "{DESKTOP_PATH}/session/{}/{}",
            sender_element(&header),
            token(&options, "session_handle_token")?
        );
        server
            .at(session.as_str(), Session(self.1.clone()))
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        let results = HashMap::from([("session_handle", Value::from(session))]);
        respond(connection, &request, 0, results).await?;
        Ok(request)
    }

    async fn bind_shortcuts(
        &self,
        _session: OwnedObjectPath,
        shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
        _parent_window: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<OwnedObjectPath> {
        let request = request_path(&header, &options)?;
        let ids: Vec<String> = shortcuts.iter().map(|(id, _)| id.clone()).collect();
        {
            let mut seen = self.1.lock().unwrap();
            seen.bound_ids = ids.clone();
            seen.preferred_triggers = shortcuts
                .iter()
                .map(|(_, info)| token(info, "preferred_trigger").ok())
                .collect();
        }
        match self.0.bind {
            Bind::Dismiss => respond(connection, &request, 1, HashMap::new()).await?,
            Bind::Zero => {
                let results = HashMap::from([("shortcuts", shortcut_list(&[]))]);
                respond(connection, &request, 0, results).await?
            }
            Bind::Accept => {
                let results = HashMap::from([("shortcuts", shortcut_list(&ids))]);
                respond(connection, &request, 0, results).await?
            }
        }
        Ok(request)
    }

    async fn list_shortcuts(
        &self,
        session: OwnedObjectPath,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<OwnedObjectPath> {
        let request = request_path(&header, &options)?;
        let ids = self.1.lock().unwrap().bound_ids.clone();
        let results = HashMap::from([("shortcuts", shortcut_list(&ids))]);
        respond(connection, &request, 0, results).await?;

        if self.0.press {
            let connection = connection.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                // First a press that belongs to somebody else's session: the
                // probe must not take it for its own.
                let foreign =
                    ObjectPath::try_from(format!("{DESKTOP_PATH}/session/9_999/someone_else"))
                        .unwrap();
                activate(&connection, &foreign, "foreign-shortcut", HashMap::new()).await;
                tokio::time::sleep(Duration::from_millis(150)).await;
                let options = HashMap::from([("activation_token", Value::from("fake-token-1"))]);
                activate(&connection, &session, &ids[0], options).await;
            });
        }
        Ok(request)
    }
}

async fn activate(
    connection: &Connection,
    session: &ObjectPath<'_>,
    id: &str,
    options: HashMap<&str, Value<'_>>,
) {
    connection
        .emit_signal(
            None::<&str>,
            DESKTOP_PATH,
            "org.freedesktop.portal.GlobalShortcuts",
            "Activated",
            &(session, id, 1_791_144_707_000u64, options),
        )
        .await
        .unwrap();
}

struct Screenshot(Script, Shared);

#[zbus::interface(name = "org.freedesktop.portal.Screenshot")]
impl Screenshot {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.0.versions[1]
    }

    #[zbus(property)]
    fn available_targets(&self) -> u32 {
        self.0.available_targets
    }

    async fn screenshot(
        &self,
        _parent_window: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<OwnedObjectPath> {
        let request = request_path(&header, &options)?;
        let mut keys: Vec<String> = options.keys().cloned().collect();
        keys.sort();
        self.1.lock().unwrap().screenshot_option_keys = keys;
        match &self.0.shot {
            Shot::Cancel => respond(connection, &request, 1, HashMap::new()).await?,
            Shot::Uri(uri) => {
                let results = HashMap::from([("uri", Value::from(uri.as_str()))]);
                respond(connection, &request, 0, results).await?
            }
        }
        Ok(request)
    }
}

struct Clipboard(Script);

#[zbus::interface(name = "org.freedesktop.portal.Clipboard")]
impl Clipboard {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.0.versions[2]
    }
}

struct RemoteDesktop(Script);

#[zbus::interface(name = "org.freedesktop.portal.RemoteDesktop")]
impl RemoteDesktop {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.0.versions[3]
    }
}

struct Registry(Script, Shared);

#[zbus::interface(name = "org.freedesktop.host.portal.Registry")]
impl Registry {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.0.versions[4]
    }

    fn register(&self, app_id: String, _options: HashMap<String, OwnedValue>) {
        self.1.lock().unwrap().registered.push(app_id);
    }
}

// -------------------------------------------------------------- the run

struct Run {
    code: Option<i32>,
    record: Json,
    seen: Shared,
    elapsed: Duration,
}

async fn served(
    builder: zbus::connection::Builder<'_>,
    script: &Script,
    seen: &Shared,
) -> Connection {
    builder
        .serve_at(DESKTOP_PATH, GlobalShortcuts(script.clone(), seen.clone()))
        .unwrap()
        .serve_at(DESKTOP_PATH, Screenshot(script.clone(), seen.clone()))
        .unwrap()
        .serve_at(DESKTOP_PATH, Clipboard(script.clone()))
        .unwrap()
        .serve_at(DESKTOP_PATH, RemoteDesktop(script.clone()))
        .unwrap()
        .serve_at(DESKTOP_PATH, Registry(script.clone(), seen.clone()))
        .unwrap()
        .name("org.freedesktop.portal.Desktop")
        .unwrap()
        .build()
        .await
        .expect("the fake portal could not join the private bus")
}

/// Starts a private bus with the scripted portal and runs the built binary
/// against it. The Wayland display is a socket that does not exist.
async fn probe(name: &str, script: Script, args: &[&str]) -> Run {
    let bus = start_bus(name);
    let seen = Shared::default();
    let builder = zbus::connection::Builder::address(bus.address.as_str()).unwrap();
    let _portal = if script.portal {
        Some(served(builder, &script, &seen).await)
    } else {
        None
    };

    let out = bus.dir.join("results.jsonl");
    let binary = env!("CARGO_BIN_EXE_addict-probe");
    let mut command = std::process::Command::new(binary);
    command
        .args(args)
        .arg("--out")
        .arg(&out)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("WAYLAND_DISPLAY", bus.dir.join("no-such-wayland-socket"))
        .env("HOME", &bus.dir)
        .env_remove("ADDICT_PROBE_OUT")
        .stdin(Stdio::null());
    let started = Instant::now();
    let output = tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap();
    let elapsed = started.elapsed();

    let text = std::fs::read_to_string(&out).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one record per run: {output:?}");
    Run {
        code: output.status.code(),
        record: serde_json::from_str(lines[0]).unwrap(),
        seen,
        elapsed,
    }
}

fn write_png(path: &Path, width: u32, height: u32) {
    let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
    std::fs::write(path, png).unwrap();
}

// ------------------------------------------------- matrix: Capabilities

#[tokio::test(flavor = "multi_thread")]
async fn caps_records_what_the_portal_serves() {
    let script = Script {
        versions: [7, 8, 9, 10, 11],
        available_targets: 5,
        ..Script::default()
    };
    let run = probe("caps", script, &["caps"]).await;
    assert_eq!(run.code, Some(0), "{}", run.record);
    assert_eq!(run.record["probe"], "caps");
    assert_eq!(run.record["outcome"], "reported");
    assert_eq!(run.record["error"], Json::Null);

    let interfaces = &run.record["details"]["interfaces"];
    for (name, version) in [
        ("GlobalShortcuts", 7),
        ("Screenshot", 8),
        ("Clipboard", 9),
        ("RemoteDesktop", 10),
        ("Registry", 11),
    ] {
        assert_eq!(interfaces[name]["present"], true, "{name}");
        assert_eq!(interfaces[name]["version"], version, "{name}");
    }
    let targets = &run.record["details"]["screenshot_available_targets"];
    assert_eq!(targets["present"], true);
    assert_eq!(targets["bits"], 5);
    assert_eq!(targets["names"], serde_json::json!(["screen", "area"]));

    // Session type is the one the process was given.
    assert_eq!(
        run.record["details"]["session_type"].as_str(),
        std::env::var("XDG_SESSION_TYPE").ok().as_deref()
    );
    // There is no compositor behind this test: the Wayland side must say so
    // rather than invent globals, and must not turn the run into a failure.
    assert_eq!(run.record["details"]["wayland"]["reachable"], false);
    assert!(run.record["details"]["wayland"]["error"].is_string());
    assert!(run.record["details"]["wayland"].get("globals").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn caps_records_live_version_shape_of_this_install() {
    // The versions the plan's verification expects from this machine.
    let run = probe("caps-live-shape", Script::default(), &["caps"]).await;
    assert_eq!(run.code, Some(0));
    let interfaces = &run.record["details"]["interfaces"];
    assert_eq!(interfaces["GlobalShortcuts"]["version"], 1);
    assert_eq!(interfaces["Screenshot"]["version"], 2);
    let targets = &run.record["details"]["screenshot_available_targets"];
    assert_eq!(targets["bits"], 0);
    assert_eq!(targets["names"], serde_json::json!([]));
}

// ----------------------------------------------- matrix: Shortcut fires

#[tokio::test(flavor = "multi_thread")]
async fn shortcut_fires_for_its_own_session_only() {
    let run = probe(
        "shortcut-fires",
        Script::default(),
        &[
            "shortcut",
            "--app-id",
            "dev.addict.Probe",
            "--timeout",
            "20",
        ],
    )
    .await;
    assert_eq!(run.code, Some(0), "{}", run.record);
    assert_eq!(run.record["outcome"], "activated");
    assert_eq!(run.record["error"], Json::Null);
    let details = &run.record["details"];

    // Bound trigger.
    assert_eq!(details["bound"][0]["id"], "probe-invoke");
    assert_eq!(details["bound"][0]["trigger_description"], TRIGGER);
    assert_eq!(details["listed"][0]["trigger_description"], TRIGGER);
    // Time from bind to press: the fake presses 300 ms after the list.
    let bind_to_press = details["bind_to_press_ms"].as_u64().unwrap();
    assert!((250..10_000).contains(&bind_to_press), "{bind_to_press}");
    assert!(details["bind_ms"].is_u64());
    // Whether an activation token came.
    assert_eq!(details["activation_token_present"], true);
    assert_eq!(
        details["activated_option_keys"],
        serde_json::json!(["activation_token"])
    );
    // The foreign session's press, sent first and without a token, was
    // ignored: the recorded activation is the probe's own.
    assert_eq!(details["activated_shortcut_id"], "probe-invoke");

    assert_eq!(details["registration"], "registered");
    assert_eq!(details["interface_version"], 1);
    assert_eq!(details["session_closed"], true);
    assert_eq!(details["phase"], "done");

    let seen = run.seen.lock().unwrap();
    assert_eq!(seen.registered, ["dev.addict.Probe"]);
    assert_eq!(seen.bound_ids, ["probe-invoke"]);
    assert_eq!(seen.preferred_triggers, [Some("CTRL+ALT+d".to_owned())]);
    assert_eq!(seen.sessions_closed, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn shortcut_without_app_id_registers_nothing() {
    let run = probe(
        "shortcut-unregistered",
        Script::default(),
        &["shortcut", "--no-trigger", "--timeout", "20"],
    )
    .await;
    assert_eq!(run.code, Some(0), "{}", run.record);
    assert_eq!(run.record["outcome"], "activated");
    assert_eq!(run.record["details"]["registration"], "not-requested");
    let seen = run.seen.lock().unwrap();
    assert!(seen.registered.is_empty());
    assert_eq!(seen.preferred_triggers, [None]);
}

// ------------------------------------ matrix: Binding refused or empty

#[tokio::test(flavor = "multi_thread")]
async fn dismissed_binding_is_not_bound() {
    let script = Script {
        bind: Bind::Dismiss,
        ..Script::default()
    };
    let run = probe(
        "shortcut-dismissed",
        script,
        &["shortcut", "--timeout", "20"],
    )
    .await;
    assert_eq!(run.code, Some(4), "{}", run.record);
    assert_eq!(run.record["outcome"], "not-bound");
    assert!(run.record["details"].get("bound").is_none());
    assert!(
        run.elapsed < Duration::from_secs(10),
        "did not wait for the timeout"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn zero_bound_shortcuts_is_not_bound() {
    let script = Script {
        bind: Bind::Zero,
        ..Script::default()
    };
    let run = probe("shortcut-zero", script, &["shortcut", "--timeout", "20"]).await;
    assert_eq!(run.code, Some(4), "{}", run.record);
    assert_eq!(run.record["outcome"], "not-bound");
    assert_eq!(run.record["details"]["bound"], serde_json::json!([]));
    assert!(
        run.elapsed < Duration::from_secs(10),
        "did not wait for the timeout"
    );
}

// --------------------------------------------------- matrix: No press

#[tokio::test(flavor = "multi_thread")]
async fn bound_shortcut_without_a_press_times_out() {
    let script = Script {
        press: false,
        ..Script::default()
    };
    let run = probe(
        "shortcut-no-press",
        script,
        &["shortcut", "--timeout", "1.5"],
    )
    .await;
    assert_eq!(run.code, Some(3), "{}", run.record);
    assert_eq!(run.record["outcome"], "timeout");
    assert_eq!(run.record["details"]["phase"], "wait-for-press");
    assert_eq!(
        run.record["details"]["bound"][0]["trigger_description"],
        TRIGGER
    );
    assert!(run.record["details"].get("bind_to_press_ms").is_none());
    assert!(run.elapsed >= Duration::from_millis(1500));
    assert!(
        run.elapsed < Duration::from_millis(2500),
        "{:?}",
        run.elapsed
    );
}

// ----------------------------------------------------- matrix: Capture

#[tokio::test(flavor = "multi_thread")]
async fn completed_capture_is_captured_with_file_and_size() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("portal-shot-image");
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("Screenshot From 1.png");
    write_png(&image, 640, 48);
    let uri = format!("file://{}", image.display()).replace(' ', "%20");

    let script = Script {
        shot: Shot::Uri(uri.clone()),
        ..Script::default()
    };
    let run = probe(
        "screenshot-captured",
        script,
        &["screenshot", "--timeout", "20"],
    )
    .await;
    assert_eq!(run.code, Some(0), "{}", run.record);
    assert_eq!(run.record["outcome"], "captured");
    assert_eq!(run.record["error"], Json::Null);
    let details = &run.record["details"];
    assert_eq!(details["uri"], uri.as_str());
    assert_eq!(details["file"]["path"], image.to_str().unwrap());
    assert_eq!(details["file"]["exists"], true);
    assert_eq!(details["file"]["width"], 640);
    assert_eq!(details["file"]["height"], 48);
    assert_eq!(details["interface_version"], 2);
    assert_eq!(details["available_targets"]["bits"], 0);

    // Nothing beyond what the backend advertises was sent.
    let seen = run.seen.lock().unwrap();
    assert_eq!(seen.screenshot_option_keys, ["handle_token", "interactive"]);
    assert!(seen.registered.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_of_a_vanished_file_records_that_it_is_missing() {
    let script = Script {
        shot: Shot::Uri("file:///nonexistent/addict-probe/shot.png".to_owned()),
        ..Script::default()
    };
    let run = probe(
        "screenshot-missing-file",
        script,
        &["screenshot", "--timeout", "20"],
    )
    .await;
    assert_eq!(run.record["outcome"], "captured");
    assert_eq!(run.record["details"]["file"]["exists"], false);
    assert!(run.record["details"]["file"].get("width").is_none());
}

// --------------------------------------------- matrix: Capture cancelled

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_capture_is_cancelled_and_references_no_file() {
    let run = probe(
        "screenshot-cancelled",
        Script::default(),
        &["screenshot", "--timeout", "20"],
    )
    .await;
    assert_eq!(run.code, Some(0), "{}", run.record);
    assert_eq!(run.record["outcome"], "cancelled");
    assert!(run.record["details"].get("uri").is_none());
    assert!(run.record["details"].get("file").is_none());
    assert!(
        run.elapsed < Duration::from_secs(10),
        "did not wait for the timeout"
    );
}

// ------------------------------------------------ no portal on the bus

#[tokio::test(flavor = "multi_thread")]
async fn no_portal_owner_is_unavailable_and_states_no_version() {
    for command in ["shortcut", "screenshot"] {
        let script = Script {
            portal: false,
            ..Script::default()
        };
        let run = probe(
            &format!("no-owner-{command}"),
            script,
            &[command, "--timeout", "20"],
        )
        .await;
        assert_eq!(run.code, Some(5), "{}", run.record);
        assert_eq!(run.record["outcome"], "unavailable");
        assert!(run.record["error"].is_string());
        let details = &run.record["details"];
        // The runtime gave no version, so none is stated.
        assert!(details.get("interface_version").is_some(), "{details}");
        assert_eq!(details["interface_version"], Json::Null);
        assert!(details["interface_version_error"].is_string(), "{details}");
        assert!(
            run.elapsed < Duration::from_secs(10),
            "did not wait for the timeout"
        );
    }
}
