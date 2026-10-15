//! A private `dbus-daemon` on which a test owns `org.a11y.atspi.Registry`
//! (and, when wanted, `org.a11y.Bus`) and serves fake applications, for the
//! tests that call the AT-SPI adapter of the library. The user's session and
//! accessibility buses are never touched.
//!
//! The nodes of a fake application are [`FakeNode`]s: build the tree with
//! [`app`], [`plain`] and [`text_node`], serve it with [`world`] and read it
//! with [`World::read`].
#![allow(dead_code)]

use std::{
    path::PathBuf,
    process::{Child, Stdio},
    time::{Duration, Instant},
};

use addict_platform::atspi::{Context, ContextFailure, Locate, read};
use zbus::{Connection, fdo, zvariant::OwnedObjectPath};

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
}

fn node_path(id: usize) -> OwnedObjectPath {
    OwnedObjectPath::try_from(format!("/fake/n{id}")).unwrap()
}

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
}

struct Text(FakeText);

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
        self.0.content.chars().count() as i32
    }

    #[zbus(name = "GetNSelections")]
    async fn get_n_selections(&self) -> i32 {
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
    _connection: Connection,
    runtime: tokio::runtime::Runtime,
}

/// Serves `nodes`; the registry lists the nodes in `applications`.
pub fn world(name: &str, address: Address, nodes: Vec<FakeNode>, applications: &[usize]) -> World {
    build(name, address, nodes, applications, false)
}

/// A registry that owns its name and never answers.
pub fn hung_registry(name: &str) -> World {
    build(name, Address::Plain, Vec::new(), &[], true)
}

fn build(
    name: &str,
    address: Address,
    nodes: Vec<FakeNode>,
    applications: &[usize],
    hung_registry: bool,
) -> World {
    let bus = start_bus(name);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let connection = runtime.block_on(async {
        let connection = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let bus_name = connection.unique_name().unwrap().to_string();
        let server = connection.object_server();
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
                server.at(&path, Text(text)).await.unwrap();
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
                    "/org/a11y/bus",
                    Launcher {
                        address: bus.address.clone(),
                    },
                )
                .await
                .unwrap();
            connection.request_name("org.a11y.Bus").await.unwrap();
        }
        connection
    });
    World {
        bus,
        _connection: connection,
        runtime,
    }
}

impl World {
    /// Reads this world's bus by its address.
    pub fn read(&self, limit: Duration) -> Result<Context, ContextFailure> {
        self.read_at(&Locate::Address(self.bus.address.clone()), limit)
    }

    /// Reads with `locate`, on the runtime that serves the fake.
    pub fn read_at(&self, locate: &Locate, limit: Duration) -> Result<Context, ContextFailure> {
        self.runtime.block_on(read(locate, limit))
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
