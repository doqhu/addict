//! A private `dbus-daemon` on which a test owns `org.a11y.atspi.Registry`
//! (and, when wanted, `org.a11y.Bus`) and serves fake applications, for the
//! tests that call the AT-SPI adapter of the library. The user's session and
//! accessibility buses are never touched.
//!
//! The nodes of a fake application are [`FakeNode`]s: build the tree with
//! [`app`], [`plain`] and [`text_node`], serve it with [`world`] and read it
//! with [`World::read`]. An application that only announces its activation
//! is a [`FakeApp`]; the registry keeps the events it was asked for and the
//! status of accessibility can be flipped.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    path::PathBuf,
    process::{Child, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use addict_platform::{
    atspi::{Context, ContextFailure, Locate, read},
    foreground::{Accessibility, Source},
};
use zbus::{
    Connection, fdo,
    zvariant::{OwnedObjectPath, Value},
};

const REGISTRY_ROOT: &str = "/org/a11y/atspi/accessible/root";

/// A private bus that dies with the test.
pub struct Bus {
    daemon: Child,
    pub address: String,
    pub dir: PathBuf,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

pub fn start_bus(name: &str) -> Bus {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("atspi-{name}"));
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
        .expect("dbus-daemon is required for these tests and could not be started");
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

// ------------------------------------------------- the fake application

#[derive(Clone, Default)]
pub struct FakeText {
    pub content: String,
    /// Selections in character offsets.
    pub selections: Vec<(i32, i32)>,
    /// `GetText` answers only for exactly the selected range.
    pub only_selected_range: bool,
    /// `GetSelection` fails (`Some(false)`) or never answers (`Some(true)`).
    pub broken_selection: Option<bool>,
}

#[derive(Clone, Default)]
pub struct FakeNode {
    pub name: &'static str,
    pub children: Vec<usize>,
    pub text: Option<FakeText>,
    /// The node is an application root.
    pub application: bool,
    /// Never answer `GetInterfaces`.
    pub hang: bool,
    /// Never answer the `Name` property.
    pub silent_name: bool,
    /// `GetState` says `ACTIVE`; [`World::set_active`] changes it.
    pub active: bool,
    /// Never answer `GetState`.
    pub silent_state: bool,
}

fn node_path(id: usize) -> OwnedObjectPath {
    OwnedObjectPath::try_from(format!("/fake/n{id}")).unwrap()
}

const STATUS_PATH: &str = "/org/a11y/bus";

/// The role `frame` and the state `ACTIVE`, as AT-SPI numbers them.
const ROLE_FRAME: u32 = 23;
const STATE_ACTIVE: u32 = 1;
const REGISTRY_PATH: &str = "/org/a11y/atspi/registry";

struct Accessible {
    node: FakeNode,
    bus_name: String,
}

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl Accessible {
    async fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        self.node
            .children
            .iter()
            .map(|id| (self.bus_name.clone(), node_path(*id)))
            .collect()
    }

    async fn get_interfaces(&self) -> Vec<String> {
        if self.node.hang {
            std::future::pending::<()>().await;
        }
        let mut interfaces = vec!["org.a11y.atspi.Accessible".to_owned()];
        if self.node.application {
            interfaces.push("org.a11y.atspi.Application".to_owned());
        }
        if self.node.text.is_some() {
            interfaces.push("org.a11y.atspi.Text".to_owned());
        }
        interfaces
    }

    #[zbus(property)]
    async fn name(&self) -> String {
        if self.node.silent_name {
            std::future::pending::<()>().await;
        }
        self.node.name.to_owned()
    }

    #[zbus(property)]
    async fn child_count(&self) -> i32 {
        self.node.children.len() as i32
    }

    async fn get_role(&self) -> u32 {
        ROLE_FRAME
    }

    /// The two words of the state set: `ACTIVE` is bit 1 of the first.
    async fn get_state(&self) -> Vec<u32> {
        if self.node.silent_state {
            std::future::pending::<()>().await;
        }
        let first = if self.node.active {
            1 << STATE_ACTIVE
        } else {
            0
        };
        vec![first, 0]
    }
}

/// A text object and the count of the calls it answered.
struct Text(FakeText, Arc<AtomicUsize>);

fn chars(text: &str, start: i32, end: i32) -> String {
    let (start, end) = (start.max(0) as usize, end.max(0) as usize);
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

#[zbus::interface(name = "org.a11y.atspi.Text")]
impl Text {
    #[zbus(property)]
    async fn character_count(&self) -> i32 {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.content.chars().count() as i32
    }

    #[zbus(name = "GetNSelections")]
    async fn get_n_selections(&self) -> i32 {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.selections.len() as i32
    }

    async fn get_selection(&self, index: i32) -> fdo::Result<(i32, i32)> {
        match self.0.broken_selection {
            Some(true) => std::future::pending::<()>().await,
            Some(false) => return Err(fdo::Error::Failed("selection refused".to_owned())),
            None => {}
        }
        self.0
            .selections
            .get(index as usize)
            .copied()
            .ok_or_else(|| fdo::Error::InvalidArgs("no such selection".to_owned()))
    }

    async fn get_text(&self, start: i32, end: i32) -> fdo::Result<String> {
        self.1.fetch_add(1, Ordering::SeqCst);
        if self.0.only_selected_range && !self.0.selections.contains(&(start, end)) {
            return Err(fdo::Error::Failed("wider text refused".to_owned()));
        }
        Ok(chars(&self.0.content, start, end))
    }

    /// The sentence the application would give, wrongly, as in EXP-2: the
    /// adapter must never ask for it.
    async fn get_string_at_offset(
        &self,
        _offset: i32,
        _granularity: u32,
    ) -> fdo::Result<(String, i32, i32)> {
        Ok(("THE APPLICATION'S SENTENCE".to_owned(), 0, 1))
    }
}

/// `org.a11y.Bus` as the session bus has it.
struct Launcher {
    address: String,
}

#[zbus::interface(name = "org.a11y.Bus")]
impl Launcher {
    async fn get_address(&self) -> String {
        self.address.clone()
    }
}

/// `org.a11y.Status` as the session bus has it.
struct Status {
    enabled: bool,
}

#[zbus::interface(name = "org.a11y.Status")]
impl Status {
    #[zbus(property)]
    async fn is_enabled(&self) -> bool {
        self.enabled
    }
}

/// `org.a11y.atspi.Registry` at its own path: it keeps what it was asked to
/// forward.
struct Registry {
    events: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.a11y.atspi.Registry")]
impl Registry {
    async fn register_event(&self, event: String, _properties: Vec<String>, _app: String) {
        self.events.lock().unwrap().push(event);
    }
}

struct HungRegistry;

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl HungRegistry {
    async fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        std::future::pending::<()>().await;
        Vec::new()
    }

    #[zbus(property)]
    async fn child_count(&self) -> i32 {
        std::future::pending::<()>().await;
        0
    }
}

// --------------------------------------------------------------- the world

#[derive(Clone, Copy, PartialEq)]
pub enum Address {
    /// `org.a11y.Bus` on this bus gives the address of this bus.
    Launcher,
    /// Nothing owns `org.a11y.Bus`; the address is given to the reader.
    Plain,
}

pub struct World {
    pub bus: Bus,
    connection: Connection,
    runtime: tokio::runtime::Runtime,
    registered: Arc<Mutex<Vec<String>>>,
    text_calls: Arc<AtomicUsize>,
}

/// Serves `nodes`; the registry lists the nodes in `applications`.
pub fn world(name: &str, address: Address, nodes: Vec<FakeNode>, applications: &[usize]) -> World {
    build(name, address, nodes, applications, false, true)
}

/// A bus on which `org.a11y.Bus` answers its address and `IsEnabled` is
/// `enabled`; no application is on it.
pub fn status_world(name: &str, enabled: bool) -> World {
    build(name, Address::Launcher, Vec::new(), &[], false, enabled)
}

/// A registry that owns its name and never answers.
pub fn hung_registry(name: &str) -> World {
    build(name, Address::Plain, Vec::new(), &[], true, true)
}

fn build(
    name: &str,
    address: Address,
    nodes: Vec<FakeNode>,
    applications: &[usize],
    hung_registry: bool,
    enabled: bool,
) -> World {
    let bus = start_bus(name);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let registered = Arc::new(Mutex::new(Vec::new()));
    let text_calls = Arc::new(AtomicUsize::new(0));
    let connection = runtime.block_on(async {
        let connection = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let bus_name = connection.unique_name().unwrap().to_string();
        let server = connection.object_server();
        server
            .at(
                REGISTRY_PATH,
                Registry {
                    events: registered.clone(),
                },
            )
            .await
            .unwrap();
        if hung_registry {
            server.at(REGISTRY_ROOT, HungRegistry).await.unwrap();
        } else {
            let registry = FakeNode {
                children: applications.to_vec(),
                ..FakeNode::default()
            };
            server
                .at(
                    REGISTRY_ROOT,
                    Accessible {
                        node: registry,
                        bus_name: bus_name.clone(),
                    },
                )
                .await
                .unwrap();
        }
        for (id, node) in nodes.into_iter().enumerate() {
            let path = node_path(id);
            if let Some(text) = node.text.clone() {
                server
                    .at(&path, Text(text, text_calls.clone()))
                    .await
                    .unwrap();
            }
            server
                .at(
                    &path,
                    Accessible {
                        node,
                        bus_name: bus_name.clone(),
                    },
                )
                .await
                .unwrap();
        }
        connection
            .request_name("org.a11y.atspi.Registry")
            .await
            .unwrap();
        if address == Address::Launcher {
            server
                .at(
                    STATUS_PATH,
                    Launcher {
                        address: bus.address.clone(),
                    },
                )
                .await
                .unwrap();
            server.at(STATUS_PATH, Status { enabled }).await.unwrap();
            connection.request_name("org.a11y.Bus").await.unwrap();
        }
        connection
    });
    World {
        bus,
        connection,
        runtime,
        registered,
        text_calls,
    }
}

impl World {
    /// Reads this world's bus by its address, accessibility on and Android
    /// Studio in front.
    pub fn read(&self, limit: Duration) -> Result<Context, ContextFailure> {
        self.read_from(Accessibility::On, Source::AndroidStudio, limit)
    }

    /// Reads this world's bus by its address with `accessibility` and
    /// `source` in front.
    pub fn read_from(
        &self,
        accessibility: Accessibility,
        source: Source,
        limit: Duration,
    ) -> Result<Context, ContextFailure> {
        self.read_with(
            &Locate::Address(self.bus.address.clone()),
            accessibility,
            source,
            limit,
        )
    }

    /// Reads with `locate`, accessibility on and Android Studio in front, on
    /// the runtime that serves the fake.
    pub fn read_at(&self, locate: &Locate, limit: Duration) -> Result<Context, ContextFailure> {
        self.read_with(locate, Accessibility::On, Source::AndroidStudio, limit)
    }

    /// Reads with `locate`, `accessibility` and `source`.
    pub fn read_with(
        &self,
        locate: &Locate,
        accessibility: Accessibility,
        source: Source,
        limit: Duration,
    ) -> Result<Context, ContextFailure> {
        self.runtime
            .block_on(read(locate, accessibility, source, limit))
    }

    /// The text objects' calls answered so far.
    pub fn text_calls(&self) -> usize {
        self.text_calls.load(Ordering::SeqCst)
    }

    /// The events the registry was asked to forward, in order.
    pub fn registered_events(&self) -> Vec<String> {
        self.registered.lock().unwrap().clone()
    }

    /// Flips `IsEnabled` of a status world and announces the change.
    pub fn set_enabled(&self, enabled: bool) {
        self.runtime.block_on(async {
            let status = self
                .connection
                .object_server()
                .interface::<_, Status>(STATUS_PATH)
                .await
                .expect("a status world");
            let mut guard = status.get_mut().await;
            guard.enabled = enabled;
            guard
                .is_enabled_changed(status.signal_emitter())
                .await
                .unwrap();
        });
    }

    /// Gives the node with `id` another `Name`, as an application does once
    /// it has started; no signal is sent.
    pub fn set_name(&self, id: usize, name: &'static str) {
        self.runtime.block_on(async {
            let node = self
                .connection
                .object_server()
                .interface::<_, Accessible>(node_path(id))
                .await
                .expect("a node of this world");
            node.get_mut().await.node.name = name;
        });
    }

    /// Sets whether the node with `id` says `ACTIVE`; no signal is sent, as
    /// Android Studio sends none.
    pub fn set_active(&self, id: usize, active: bool) {
        self.runtime.block_on(async {
            let node = self
                .connection
                .object_server()
                .interface::<_, Accessible>(node_path(id))
                .await
                .expect("a node of this world");
            node.get_mut().await.node.active = active;
        });
    }

    /// How many connections the bus has, the daemon's own not counted.
    pub fn client_count(&self) -> usize {
        self.runtime.block_on(async {
            let names: Vec<String> = Connection::call_method(
                &self.connection,
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "ListNames",
                &(),
            )
            .await
            .unwrap()
            .body()
            .deserialize()
            .unwrap();
            names.iter().filter(|name| name.starts_with(':')).count()
        })
    }

    /// An application on this bus that has this registered name.
    pub fn open_app(&self, name: &'static str) -> FakeApp {
        let address = self.bus.address.clone();
        let handle = self.runtime.handle().clone();
        let connection = handle.block_on(async {
            let connection = zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .build()
                .await
                .unwrap();
            let node = FakeNode {
                name,
                application: true,
                ..FakeNode::default()
            };
            let bus_name = connection.unique_name().unwrap().to_string();
            connection
                .object_server()
                .at(REGISTRY_ROOT, Accessible { node, bus_name })
                .await
                .unwrap();
            connection
        });
        FakeApp { connection, handle }
    }

    /// A client on this bus that serves no object: it announces windows
    /// but cannot give its name.
    pub fn open_mute_client(&self) -> FakeApp {
        let address = self.bus.address.clone();
        let handle = self.runtime.handle().clone();
        let connection = handle.block_on(async {
            zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .build()
                .await
                .unwrap()
        });
        FakeApp { connection, handle }
    }
}

/// An application that announces its activation, as one connection of its
/// own on the bus.
pub struct FakeApp {
    connection: Connection,
    handle: tokio::runtime::Handle,
}

impl FakeApp {
    /// Its bus name.
    pub fn bus_name(&self) -> String {
        self.connection.unique_name().unwrap().to_string()
    }

    /// `Window:Activate`.
    pub fn window_activated(&self) {
        self.emit("org.a11y.atspi.Event.Window", "Activate", "", 0);
    }

    /// `Window:Deactivate`.
    pub fn window_deactivated(&self) {
        self.emit("org.a11y.atspi.Event.Window", "Deactivate", "", 0);
    }

    /// `Object:StateChanged` of `state` set to `value`.
    pub fn state_changed(&self, state: &str, value: bool) {
        self.emit(
            "org.a11y.atspi.Event.Object",
            "StateChanged",
            state,
            i32::from(value),
        );
    }

    fn emit(&self, interface: &str, member: &str, kind: &str, detail: i32) {
        self.handle.block_on(async {
            self.connection
                .emit_signal(
                    None::<&str>,
                    "/org/a11y/atspi/accessible/window",
                    interface,
                    member,
                    &(
                        kind,
                        detail,
                        0i32,
                        Value::from(0i32),
                        HashMap::<String, Value>::new(),
                    ),
                )
                .await
                .unwrap();
        });
    }
}

// ------------------------------------------------------------ the fixtures

/// Character range of the `nth` `needle` in `text`.
pub fn range(text: &str, needle: &str, nth: usize) -> (i32, i32) {
    let byte = text
        .match_indices(needle)
        .nth(nth)
        .expect("needle is in the text")
        .0;
    let start = text[..byte].chars().count() as i32;
    (start, start + needle.chars().count() as i32)
}

/// An application of this name with the given children.
pub fn app(name: &'static str, children: Vec<usize>) -> FakeNode {
    FakeNode {
        name,
        children,
        application: true,
        ..FakeNode::default()
    }
}

pub fn plain(children: Vec<usize>) -> FakeNode {
    FakeNode {
        children,
        ..FakeNode::default()
    }
}

pub fn text_node(text: FakeText) -> FakeNode {
    FakeNode {
        text: Some(text),
        ..FakeNode::default()
    }
}

/// application → frame → the given text nodes, as ids 0, 1, 2…
pub fn editor(name: &'static str, texts: Vec<FakeText>) -> Vec<FakeNode> {
    let ids: Vec<usize> = (2..2 + texts.len()).collect();
    let mut nodes = vec![app(name, vec![1]), plain(ids)];
    nodes.extend(texts.into_iter().map(text_node));
    nodes
}

/// `content` with the `nth` `needle` selected.
pub fn selected(content: &str, needle: &str, nth: usize) -> FakeText {
    FakeText {
        content: content.to_owned(),
        selections: vec![range(content, needle, nth)],
        ..FakeText::default()
    }
}

/// `content` with nothing selected.
pub fn unselected(content: &str) -> FakeText {
    FakeText {
        content: content.to_owned(),
        ..FakeText::default()
    }
}
