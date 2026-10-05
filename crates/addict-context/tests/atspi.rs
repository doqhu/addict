//! The matrix end to end: the built binary runs against a private
//! `dbus-daemon` on which this test owns `org.a11y.atspi.Registry` (and,
//! when wanted, `org.a11y.Bus`) and serves a fake application. The user's
//! session and accessibility buses are never touched.

use std::{
    path::PathBuf,
    process::{Child, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::{Value as Json, json};
use zbus::{Connection, fdo, zvariant::OwnedObjectPath};

const REGISTRY_ROOT: &str = "/org/a11y/atspi/accessible/root";

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
struct FakeText {
    content: String,
    /// Selections in character offsets.
    selections: Vec<(i32, i32)>,
    /// Answer of `GetStringAtOffset` with sentence granularity.
    by_granularity: Option<(String, i32, i32)>,
    /// Answer of `GetTextAtOffset` with the sentence-start boundary.
    by_boundary: Option<(String, i32, i32)>,
    /// `GetText` answers only for exactly the selected range.
    only_selected_range: bool,
    /// `GetSelection` fails (`Some(false)`) or never answers (`Some(true)`).
    broken_selection: Option<bool>,
}

#[derive(Clone, Default)]
struct FakeNode {
    role: &'static str,
    name: &'static str,
    children: Vec<usize>,
    text: Option<FakeText>,
    /// Toolkit name and version: the node is an application root.
    application: Option<(&'static str, &'static str)>,
    /// Never answer `GetRoleName`.
    hang: bool,
    /// Never answer the `Name` property.
    silent_name: bool,
    /// Milliseconds before `GetRoleName` answers.
    slow_ms: u64,
}

type Log = Arc<Mutex<Vec<Instant>>>;

fn node_path(id: usize) -> OwnedObjectPath {
    OwnedObjectPath::try_from(format!("/fake/n{id}")).unwrap()
}

struct Accessible {
    node: FakeNode,
    bus_name: String,
    log: Log,
}

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl Accessible {
    async fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        self.log.lock().unwrap().push(Instant::now());
        self.node
            .children
            .iter()
            .map(|id| (self.bus_name.clone(), node_path(*id)))
            .collect()
    }

    async fn get_role_name(&self) -> String {
        self.log.lock().unwrap().push(Instant::now());
        if self.node.hang {
            std::future::pending::<()>().await;
        }
        if self.node.slow_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.node.slow_ms)).await;
        }
        self.node.role.to_owned()
    }

    async fn get_role(&self) -> u32 {
        0
    }

    async fn get_interfaces(&self) -> Vec<String> {
        let mut interfaces = vec!["org.a11y.atspi.Accessible".to_owned()];
        if self.node.application.is_some() {
            interfaces.push("org.a11y.atspi.Application".to_owned());
        }
        if self.node.text.is_some() {
            interfaces.push("org.a11y.atspi.Text".to_owned());
        }
        interfaces
    }

    #[zbus(property)]
    async fn name(&self) -> String {
        self.log.lock().unwrap().push(Instant::now());
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

struct Application(&'static str, &'static str);

#[zbus::interface(name = "org.a11y.atspi.Application")]
impl Application {
    #[zbus(property)]
    async fn toolkit_name(&self) -> String {
        self.0.to_owned()
    }

    #[zbus(property)]
    async fn version(&self) -> String {
        self.1.to_owned()
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

    async fn get_string_at_offset(
        &self,
        _offset: i32,
        granularity: u32,
    ) -> fdo::Result<(String, i32, i32)> {
        match (&self.0.by_granularity, granularity) {
            (Some(answer), 2) => Ok(answer.clone()),
            _ => Err(fdo::Error::NotSupported(
                "granularity not implemented".to_owned(),
            )),
        }
    }

    async fn get_text_at_offset(&self, _offset: i32, kind: u32) -> fdo::Result<(String, i32, i32)> {
        match (&self.0.by_boundary, kind) {
            (Some(answer), 3) => Ok(answer.clone()),
            _ => Err(fdo::Error::NotSupported(
                "boundary not implemented".to_owned(),
            )),
        }
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

struct Status;

#[zbus::interface(name = "org.a11y.Status")]
impl Status {
    #[zbus(property)]
    async fn is_enabled(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn screen_reader_enabled(&self) -> bool {
        false
    }
}

// --------------------------------------------------------------- the world

#[derive(Clone, Copy, PartialEq)]
enum Address {
    /// The probe finds the bus through `org.a11y.Bus` on the session bus.
    Launcher,
    /// The probe gets `AT_SPI_BUS_ADDRESS`; nothing owns `org.a11y.Bus`.
    Env,
    /// Neither: there is no accessibility bus to find.
    Absent,
}

struct World {
    bus: Bus,
    address: Address,
    log: Log,
    _connection: Option<Connection>,
    _runtime: tokio::runtime::Runtime,
}

struct Run {
    exit: i32,
    record: Json,
    elapsed: Duration,
    started: Instant,
}

/// Serves `nodes`; the registry lists the nodes in `applications`.
fn world(name: &str, address: Address, nodes: Vec<FakeNode>, applications: &[usize]) -> World {
    let bus = start_bus(name);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let log = Log::default();
    let connection = (address != Address::Absent).then(|| {
        runtime.block_on(async {
            let connection = zbus::connection::Builder::address(bus.address.as_str())
                .unwrap()
                .build()
                .await
                .unwrap();
            let bus_name = connection.unique_name().unwrap().to_string();
            let server = connection.object_server();
            let registry = FakeNode {
                role: "desktop frame",
                children: applications.to_vec(),
                hang: nodes.is_empty() && applications.is_empty() && name.contains("hung-registry"),
                ..FakeNode::default()
            };
            if registry.hang {
                // A registry that owns its name and never answers.
                server.at(REGISTRY_ROOT, HungRegistry).await.unwrap();
            } else {
                server
                    .at(
                        REGISTRY_ROOT,
                        Accessible {
                            node: registry,
                            bus_name: bus_name.clone(),
                            log: log.clone(),
                        },
                    )
                    .await
                    .unwrap();
            }
            for (id, node) in nodes.into_iter().enumerate() {
                let path = node_path(id);
                if let Some((toolkit, version)) = node.application {
                    server
                        .at(&path, Application(toolkit, version))
                        .await
                        .unwrap();
                }
                if let Some(text) = node.text.clone() {
                    server.at(&path, Text(text)).await.unwrap();
                }
                server
                    .at(
                        &path,
                        Accessible {
                            node,
                            bus_name: bus_name.clone(),
                            log: log.clone(),
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
                server.at("/org/a11y/bus", Status).await.unwrap();
                connection.request_name("org.a11y.Bus").await.unwrap();
            }
            connection
        })
    });
    World {
        bus,
        address,
        log,
        _connection: connection,
        _runtime: runtime,
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

impl World {
    fn out(&self) -> PathBuf {
        self.bus.dir.join("results.jsonl")
    }

    fn run(&self, args: &[&str]) -> Run {
        self.run_with(args, None)
    }

    fn run_with(&self, args: &[&str], at_spi_address: Option<&str>) -> Run {
        let out = self.out();
        let lines_before = std::fs::read_to_string(&out)
            .map(|text| text.lines().count())
            .unwrap_or(0);
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_addict-context"));
        command
            .args(args)
            .arg("--out")
            .arg(&out)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus.address)
            .env_remove("AT_SPI_BUS_ADDRESS")
            .env_remove("ADDICT_CONTEXT_OUT")
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        match (at_spi_address, self.address) {
            (Some(address), _) => {
                command.env("AT_SPI_BUS_ADDRESS", address);
            }
            (None, Address::Env) => {
                command.env("AT_SPI_BUS_ADDRESS", &self.bus.address);
            }
            _ => {}
        }
        let started = Instant::now();
        let output = command.output().expect("the probe binary runs");
        let elapsed = started.elapsed();
        let text = std::fs::read_to_string(&out).expect("the record file exists");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), lines_before + 1, "exactly one record per run");
        let record: Json = serde_json::from_str(lines[lines.len() - 1]).unwrap();
        let printed: Json =
            serde_json::from_str(String::from_utf8(output.stdout).unwrap().trim()).unwrap();
        assert_eq!(printed, record, "stdout and the file carry the same record");
        for key in [
            "schema",
            "timestamp",
            "tool_version",
            "environment",
            "accessibility",
            "inputs",
            "expected",
            "outcome",
            "details",
        ] {
            assert!(record.get(key).is_some(), "record lacks {key}");
        }
        Run {
            exit: output.status.code().expect("the probe exits by itself"),
            record,
            elapsed,
            started,
        }
    }
}

// ------------------------------------------------------------ the fixtures

/// Character range of the first `needle` in `text`.
fn range(text: &str, needle: &str) -> (i32, i32) {
    let byte = text.find(needle).expect("needle is in the text");
    let start = text[..byte].chars().count() as i32;
    (start, start + needle.chars().count() as i32)
}

fn app(name: &'static str, children: Vec<usize>) -> FakeNode {
    FakeNode {
        role: "application",
        name,
        children,
        application: Some(("FakeKit", "1.2.3")),
        ..FakeNode::default()
    }
}

fn plain(role: &'static str, children: Vec<usize>) -> FakeNode {
    FakeNode {
        role,
        children,
        ..FakeNode::default()
    }
}

fn text_node(text: FakeText) -> FakeNode {
    FakeNode {
        role: "paragraph",
        text: Some(text),
        ..FakeNode::default()
    }
}

/// application → frame → the given text nodes.
fn reader(texts: Vec<FakeText>) -> Vec<FakeNode> {
    let ids: Vec<usize> = (2..2 + texts.len()).collect();
    let mut nodes = vec![app("org.example.Reader", vec![1]), plain("frame", ids)];
    nodes.extend(texts.into_iter().map(text_node));
    nodes
}

const BOOK: &str = "It was late. The bank was closed for the day. We went home.";
const SENTENCE: &str = "The bank was closed for the day.";

fn selected(content: &str, word: &str) -> FakeText {
    FakeText {
        content: content.to_owned(),
        selections: vec![range(content, word)],
        ..FakeText::default()
    }
}

fn with_granularity(content: &str, word: &str, sentence: &str) -> FakeText {
    let (start, end) = range(content, sentence);
    FakeText {
        by_granularity: Some((sentence.to_owned(), start, end)),
        ..selected(content, word)
    }
}

// ----------------------------------------------------------------- status

#[test]
fn status_reports_bus_status_and_applications() {
    let world = world(
        "status",
        Address::Launcher,
        vec![app("org.example.Reader", vec![]), app("Other", vec![])],
        &[0, 1],
    );
    let run = world.run(&["status", "--note", "matrix: status"]);
    assert_eq!(run.exit, 0);
    let r = &run.record;
    assert_eq!(r["outcome"], json!("reported"));
    assert_eq!(r["probe"], json!("status"));
    assert_eq!(r["error"], Json::Null);
    assert_eq!(r["inputs"]["note"], json!("matrix: status"));
    let a = &r["accessibility"];
    assert_eq!(a["bus_address"], json!(world.bus.address));
    assert_eq!(a["bus_address_source"], json!("org.a11y.Bus"));
    assert_eq!(a["is_enabled"], json!(true));
    assert_eq!(a["screen_reader_enabled"], json!(false));
    let apps = r["details"]["applications"].as_array().unwrap();
    assert_eq!(r["details"]["application_count"], json!(2));
    assert_eq!(apps[0]["name"], json!("org.example.Reader"));
    assert_eq!(apps[0]["toolkit"], json!("FakeKit"));
    assert_eq!(apps[0]["toolkit_version"], json!("1.2.3"));
    assert_eq!(apps[0]["pid"], json!(std::process::id()));
    assert_eq!(apps[1]["name"], json!("Other"));
}

#[test]
fn status_takes_the_address_from_the_environment() {
    let world = world("status-env", Address::Env, vec![app("A", vec![])], &[0]);
    let run = world.run(&["status"]);
    assert_eq!(run.exit, 0);
    let a = &run.record["accessibility"];
    assert_eq!(a["bus_address_source"], json!("AT_SPI_BUS_ADDRESS"));
    assert_eq!(a["bus_address"], json!(world.bus.address));
    // Nothing owns org.a11y.Bus here: the status is unknown, not guessed.
    assert_eq!(a["is_enabled"], Json::Null);
    assert!(a["status_error"].as_str().unwrap().contains("IsEnabled"));
}

#[test]
fn status_lists_an_application_that_does_not_answer() {
    let mut hung = app("Hung", vec![]);
    hung.silent_name = true;
    let world = world(
        "status-hung-app",
        Address::Env,
        vec![app("A", vec![]), hung],
        &[0, 1],
    );
    let run = world.run(&["status", "--timeout", "4"]);
    assert_eq!(run.exit, 0);
    assert_eq!(run.record["outcome"], json!("reported"));
    let apps = run.record["details"]["applications"].as_array().unwrap();
    assert_eq!(apps.len(), 2);
    assert_eq!(apps[0]["name"], json!("A"));
    assert_eq!(apps[1]["name"], Json::Null);
    assert!(
        apps[1]["error"]
            .as_str()
            .unwrap()
            .contains("no answer within")
    );
    assert!(run.elapsed < Duration::from_secs(3));
}

// ----------------------------------------------------------------- no bus

#[test]
fn absent_bus_is_unavailable_and_still_recorded() {
    let world = world("no-bus", Address::Absent, vec![], &[]);
    for args in [
        vec!["status"],
        vec!["tree", "--app", "x"],
        vec!["context", "--app", "x"],
    ] {
        let run = world.run(&args);
        assert_eq!(run.exit, 5, "{args:?}");
        assert_eq!(run.record["outcome"], json!("unavailable"));
        let error = run.record["error"].as_str().unwrap();
        assert!(error.contains("org.a11y.Bus"), "{error}");
        assert_eq!(run.record["accessibility"]["bus_address"], Json::Null);
    }
}

#[test]
fn unreachable_address_is_unavailable() {
    let world = world("bad-address", Address::Absent, vec![], &[]);
    let gone = format!(
        "unix:path={}",
        world.bus.dir.join("no-such-socket").display()
    );
    let run = world.run_with(&["status"], Some(&gone));
    assert_eq!(run.exit, 5);
    assert_eq!(run.record["outcome"], json!("unavailable"));
    assert_eq!(run.record["accessibility"]["bus_address"], json!(gone));
    assert!(
        run.record["error"]
            .as_str()
            .unwrap()
            .contains("no-such-socket")
    );
}

// ------------------------------------------------------------- app absent

#[test]
fn absent_application_lists_the_names_seen() {
    let world = world(
        "app-absent",
        Address::Env,
        vec![app("org.example.Reader", vec![]), app("Other", vec![])],
        &[0, 1],
    );
    for command in ["tree", "context"] {
        let run = world.run(&[command, "--app", "Missing"]);
        assert_eq!(run.exit, 4);
        assert_eq!(run.record["outcome"], json!("app-not-found"));
        assert_eq!(
            run.record["details"]["applications_seen"],
            json!(["org.example.Reader", "Other"])
        );
        assert_eq!(run.record["target"], Json::Null);
    }
}

// ------------------------------------------------------------------- tree

#[test]
fn tree_reports_roles_and_text_objects() {
    let mut nodes = reader(vec![
        selected(BOOK, "bank"),
        FakeText {
            content: "Привет 🙂".to_owned(),
            ..FakeText::default()
        },
        FakeText::default(),
    ]);
    nodes[2].name = "first paragraph";
    let world = world("tree", Address::Env, nodes, &[0]);
    // Matching is case-insensitive and accepts a part of the name.
    let run = world.run(&["tree", "--app", "READER"]);
    assert_eq!(run.exit, 0);
    let r = &run.record;
    assert_eq!(r["outcome"], json!("reported"));
    assert_eq!(r["target"][0]["name"], json!("org.example.Reader"));
    assert_eq!(r["target"][0]["toolkit"], json!("FakeKit"));
    assert_eq!(r["target"][0]["toolkit_version"], json!("1.2.3"));
    let walk = &r["details"]["walk"];
    assert_eq!(walk["node_count"], json!(5));
    assert_eq!(walk["cut"], Json::Null);
    assert_eq!(
        walk["roles"],
        json!({ "application": 1, "frame": 1, "paragraph": 3 })
    );
    assert_eq!(walk["text_object_count"], json!(3));
    assert_eq!(walk["text_objects_with_characters"], json!(2));
    assert_eq!(walk["text_objects_with_selection"], json!(1));
    let first = &walk["text_objects_listed"][0];
    assert_eq!(first["index_path"], json!("0/0"));
    assert_eq!(first["role_path"], json!("application / frame / paragraph"));
    assert_eq!(first["name"], json!("first paragraph"));
    assert_eq!(first["character_count"], json!(BOOK.chars().count()));
    assert_eq!(first["n_selections"], json!(1));
    // Characters, not bytes: 6 Cyrillic letters, a space and one emoji.
    assert_eq!(walk["text_objects_listed"][1]["character_count"], json!(8));
    assert_eq!(walk["text_objects_listed"][2]["character_count"], json!(0));
}

#[test]
fn tree_says_when_the_node_limit_cut_it() {
    let world = world(
        "tree-nodes",
        Address::Env,
        reader(vec![selected(BOOK, "bank")]),
        &[0],
    );
    let run = world.run(&["tree", "--app", "org.example.Reader", "--max-nodes", "2"]);
    assert_eq!(run.exit, 0);
    assert_eq!(run.record["outcome"], json!("reported"));
    assert_eq!(run.record["details"]["walk"]["cut"], json!("nodes"));
    assert_eq!(run.record["details"]["walk"]["node_count"], json!(2));
    assert_eq!(run.record["inputs"]["max_nodes"], json!(2));
}

/// application → a chain of nodes that each take 300 ms to answer.
fn slow_chain() -> Vec<FakeNode> {
    let mut nodes = vec![app("Slow", vec![1])];
    for id in 1..=12 {
        nodes.push(FakeNode {
            role: "panel",
            children: if id < 12 { vec![id + 1] } else { vec![] },
            slow_ms: 300,
            ..FakeNode::default()
        });
    }
    nodes
}

#[test]
fn tree_says_when_the_time_limit_cut_it() {
    let world = world("tree-time", Address::Env, slow_chain(), &[0]);
    let run = world.run(&["tree", "--app", "Slow", "--timeout", "2"]);
    assert_eq!(run.exit, 0);
    let walk = &run.record["details"]["walk"];
    assert_eq!(walk["cut"], json!("time"));
    let visited = walk["node_count"].as_u64().unwrap();
    assert!((2..12).contains(&visited), "{visited} nodes");
    assert!(run.elapsed < Duration::from_secs(3));
}

#[test]
fn a_silent_node_below_the_root_is_skipped_not_fatal() {
    let mut nodes = reader(vec![selected(BOOK, "bank")]);
    nodes.push(FakeNode {
        role: "panel",
        hang: true,
        ..FakeNode::default()
    });
    nodes[1].children.insert(0, 3);
    let world = world("tree-silent-node", Address::Env, nodes, &[0]);
    let run = world.run(&["tree", "--app", "org.example.Reader", "--timeout", "4"]);
    assert_eq!(run.exit, 0);
    let walk = &run.record["details"]["walk"];
    assert_eq!(walk["unanswered"], json!(1));
    assert_eq!(walk["text_objects_with_selection"], json!(1));
    assert!(
        walk["errors"][0]
            .as_str()
            .unwrap()
            .contains("no answer within")
    );
}

// ---------------------------------------------------------------- context

#[test]
fn no_text_object_with_characters_is_no_text() {
    for (name, texts) in [
        ("no-text-none", vec![]),
        (
            "no-text-empty",
            vec![FakeText::default(), FakeText::default()],
        ),
    ] {
        let world = world(name, Address::Env, reader(texts), &[0]);
        let run = world.run(&["context", "--app", "org.example.Reader"]);
        assert_eq!(run.exit, 6, "{name}");
        assert_eq!(run.record["outcome"], json!("no-text"));
        assert_eq!(run.record["error"], Json::Null);
        assert_eq!(
            run.record["details"]["walk"]["text_objects_with_characters"],
            json!(0)
        );
    }
}

#[test]
fn text_without_selection_is_no_selection() {
    let unselected = FakeText {
        content: BOOK.to_owned(),
        ..FakeText::default()
    };
    // A collapsed selection (start == end) is a caret, not a selection.
    let caret = FakeText {
        content: BOOK.to_owned(),
        selections: vec![(5, 5)],
        ..FakeText::default()
    };
    let world = world(
        "no-selection",
        Address::Env,
        reader(vec![unselected, caret]),
        &[0],
    );
    let run = world.run(&["context", "--app", "org.example.Reader"]);
    assert_eq!(run.exit, 7);
    assert_eq!(run.record["outcome"], json!("no-selection"));
    assert_eq!(run.record["error"], Json::Null);
}

#[test]
fn word_and_sentence_from_the_application() {
    let world = world(
        "context-app",
        Address::Launcher,
        reader(vec![
            FakeText {
                content: "Another paragraph.".to_owned(),
                ..FakeText::default()
            },
            with_granularity(BOOK, "bank", SENTENCE),
        ]),
        &[0],
    );
    let run = world.run(&[
        "context",
        "--app",
        "org.example.Reader",
        "--expect-word",
        "bank",
        "--expect-sentence",
        "The bank was  closed for the day. ",
        "--note",
        "matrix: word and sentence",
    ]);
    assert_eq!(run.exit, 0);
    let r = &run.record;
    assert_eq!(r["outcome"], json!("context"));
    assert_eq!(r["error"], Json::Null);
    assert_eq!(r["accessibility"]["is_enabled"], json!(true));
    assert_eq!(r["target"][0]["toolkit"], json!("FakeKit"));
    let d = &r["details"];
    let (start, end) = range(BOOK, "bank");
    assert_eq!(d["selection"]["start"], json!(start));
    assert_eq!(d["selection"]["end"], json!(end));
    assert_eq!(d["selection"]["unit"], json!("characters"));
    assert_eq!(d["word"]["text"], json!("bank"));
    assert_eq!(d["word"]["embedded_object_chars"], json!(0));
    assert_eq!(d["object"]["role"], json!("paragraph"));
    assert_eq!(d["object"]["index_path"], json!("0/1"));
    assert_eq!(
        d["object"]["role_path"],
        json!("application / frame / paragraph")
    );
    assert_eq!(d["sentence"]["text"]["text"], json!(SENTENCE));
    assert_eq!(d["sentence"]["source"], json!("application"));
    assert_eq!(d["sentence"]["method"], json!("GetStringAtOffset"));
    assert_eq!(d["sentence_attempts"][0]["ok"], json!(true));
    assert_eq!(d["sentence_attempts"][0]["covers_selection"], json!(true));
    // The older call is still tried and its refusal recorded.
    assert_eq!(
        d["sentence_attempts"][1]["method"],
        json!("GetTextAtOffset")
    );
    assert_eq!(d["sentence_attempts"][1]["ok"], json!(false));
    // The derived sentence is recorded beside the application's.
    assert_eq!(d["derived"]["sentence"]["text"]["text"], json!(SENTENCE));
    assert_eq!(d["derived"]["selection_matches_word"], json!(true));
    assert_eq!(d["word_match"], json!(true));
    assert_eq!(d["sentence_match"], json!(true));
    assert_eq!(d["sentence_match_application"], json!(true));
    assert_eq!(d["sentence_match_derived"], json!(true));
}

#[test]
fn sentence_from_the_boundary_call_when_granularity_is_refused() {
    let (start, end) = range(BOOK, SENTENCE);
    let text = FakeText {
        by_boundary: Some((format!("{SENTENCE} "), start, end + 1)),
        ..selected(BOOK, "bank")
    };
    let world = world("context-boundary", Address::Env, reader(vec![text]), &[0]);
    let run = world.run(&[
        "context",
        "--app",
        "org.example.Reader",
        "--expect-sentence",
        SENTENCE,
    ]);
    assert_eq!(run.exit, 0);
    let d = &run.record["details"];
    assert_eq!(run.record["outcome"], json!("context"));
    assert_eq!(d["sentence"]["source"], json!("application"));
    assert_eq!(d["sentence"]["method"], json!("GetTextAtOffset"));
    assert_eq!(d["sentence_attempts"][0]["ok"], json!(false));
    assert!(
        d["sentence_attempts"][0]["error"]
            .as_str()
            .unwrap()
            .contains("granularity not implemented")
    );
    assert_eq!(d["sentence_match"], json!(true));
}

#[test]
fn sentence_derived_when_the_application_returns_none() {
    let world = world(
        "context-derived",
        Address::Env,
        reader(vec![selected(BOOK, "bank")]),
        &[0],
    );
    let run = world.run(&["context", "--app", "org.example.Reader"]);
    assert_eq!(run.exit, 0);
    let d = &run.record["details"];
    assert_eq!(run.record["outcome"], json!("context"));
    assert_eq!(d["sentence"]["source"], json!("derived"));
    assert_eq!(d["sentence"]["method"], Json::Null);
    assert_eq!(d["sentence"]["text"]["text"], json!(SENTENCE));
    assert_eq!(
        d["derived"]["sentence"]["cut_at_window_start"],
        json!(false)
    );
    assert_eq!(d["derived"]["sentence"]["cut_at_window_end"], json!(false));
    // No expectation given: no match flags invented.
    assert!(d.get("word_match").is_none());
    assert!(d.get("sentence_match").is_none());
}

#[test]
fn a_narrow_window_marks_the_derived_sentence_as_cut() {
    let world = world(
        "context-window",
        Address::Env,
        reader(vec![selected(BOOK, "bank")]),
        &[0],
    );
    let run = world.run(&[
        "context",
        "--app",
        "org.example.Reader",
        "--context-chars",
        "6",
        "--expect-sentence",
        SENTENCE,
    ]);
    assert_eq!(run.exit, 0);
    let d = &run.record["details"];
    assert_eq!(run.record["inputs"]["context_chars"], json!(6));
    assert_eq!(
        d["derived"]["wider_text"]["text"],
        json!(". The bank was c")
    );
    assert_eq!(
        d["derived"]["sentence"]["text"]["text"],
        json!("The bank was c")
    );
    assert_eq!(d["derived"]["sentence"]["cut_at_window_end"], json!(true));
    assert_eq!(d["sentence_match"], json!(false));
}

#[test]
fn word_without_any_sentence_is_word_only() {
    let text = FakeText {
        only_selected_range: true,
        ..selected(BOOK, "bank")
    };
    let world = world("word-only", Address::Env, reader(vec![text]), &[0]);
    let run = world.run(&[
        "context",
        "--app",
        "org.example.Reader",
        "--expect-word",
        "bank",
        "--expect-sentence",
        SENTENCE,
    ]);
    assert_eq!(run.exit, 8);
    let r = &run.record;
    assert_eq!(r["outcome"], json!("word-only"));
    assert_eq!(r["details"]["word"]["text"], json!("bank"));
    assert_eq!(r["details"]["word_match"], json!(true));
    assert_eq!(r["details"]["sentence_match"], json!(false));
    assert!(r["details"].get("sentence").is_none());
    let error = r["error"].as_str().unwrap();
    for part in [
        "GetStringAtOffset(2)",
        "granularity not implemented",
        "GetTextAtOffset(3)",
        "boundary not implemented",
        "GetText(",
        "wider text refused",
    ] {
        assert!(error.contains(part), "{part} missing in {error}");
    }
}

#[test]
fn several_selections_are_ambiguous() {
    let texts: Vec<FakeText> = (0..12)
        .map(|i| {
            if i % 2 == 0 {
                selected(BOOK, "bank")
            } else {
                selected(BOOK, "home")
            }
        })
        .collect();
    let world = world("ambiguous", Address::Env, reader(texts), &[0]);
    let run = world.run(&["context", "--app", "org.example.Reader"]);
    assert_eq!(run.exit, 10);
    let r = &run.record;
    assert_eq!(r["outcome"], json!("ambiguous"));
    assert_eq!(r["details"]["candidate_count"], json!(12));
    let listed = r["details"]["candidates"].as_array().unwrap();
    assert_eq!(listed.len(), 10, "candidates are recorded up to a limit");
    assert_eq!(listed[0]["word"], json!("bank"));
    assert_eq!(listed[1]["word"], json!("home"));
    assert_eq!(listed[1]["object"]["index_path"], json!("0/1"));
    assert_eq!(
        listed[0]["selection"]["start"],
        json!(range(BOOK, "bank").0)
    );
    assert!(r["details"].get("sentence").is_none());
}

#[test]
fn a_mismatched_expectation_is_data_not_an_error() {
    let world = world(
        "expectation",
        Address::Env,
        reader(vec![with_granularity(BOOK, "bank", SENTENCE)]),
        &[0],
    );
    let run = world.run(&[
        "context",
        "--app",
        "org.example.Reader",
        "--expect-word",
        "charge",
        "--expect-sentence",
        "Something else entirely.",
    ]);
    assert_eq!(run.exit, 0);
    let r = &run.record;
    assert_eq!(r["outcome"], json!("context"));
    assert_eq!(r["error"], Json::Null);
    assert_eq!(r["details"]["word_match"], json!(false));
    assert_eq!(r["details"]["sentence_match"], json!(false));
    assert_eq!(r["inputs"]["expect_word"], json!("charge"));
}

#[test]
fn non_ascii_text_is_cut_on_character_boundaries() {
    let content = "Она сказала: «Иди». It’s a “bank” — not a bench 🙂 at all! Потом ушла.";
    let sentence = "It’s a “bank” — not a bench 🙂 at all!";
    for (name, word) in [
        ("non-ascii-latin", "bank"),
        ("non-ascii-emoji", "🙂"),
        ("non-ascii-cyr", "ушла"),
    ] {
        let world = world(
            name,
            Address::Env,
            reader(vec![selected(content, word)]),
            &[0],
        );
        let expected = if word == "ушла" {
            "Потом ушла."
        } else {
            sentence
        };
        let run = world.run(&[
            "context",
            "--app",
            "org.example.Reader",
            "--expect-word",
            word,
            "--expect-sentence",
            expected,
        ]);
        assert_eq!(run.exit, 0, "{name}");
        let d = &run.record["details"];
        assert_eq!(run.record["outcome"], json!("context"));
        assert_eq!(d["word"]["text"], json!(word));
        assert_eq!(d["word"]["chars"], json!(word.chars().count()));
        assert_eq!(d["sentence"]["text"]["text"], json!(expected));
        assert_eq!(d["derived"]["selection_matches_word"], json!(true));
        assert_eq!(d["word_match"], json!(true));
        assert_eq!(d["sentence_match"], json!(true));
    }
}

#[test]
fn non_ascii_with_a_narrow_window_does_not_panic() {
    let content = "«Да» — 🙂🙂 “bank” 👩‍👩‍👧… нет";
    for chars in ["0", "1", "2", "3", "5"] {
        let world = world(
            &format!("non-ascii-window-{chars}"),
            Address::Env,
            reader(vec![selected(content, "bank")]),
            &[0],
        );
        let run = world.run(&[
            "context",
            "--app",
            "org.example.Reader",
            "--context-chars",
            chars,
        ]);
        assert_eq!(run.exit, 0, "context-chars {chars}");
        assert_eq!(run.record["details"]["word"]["text"], json!("bank"));
    }
}

#[test]
fn embedded_object_characters_are_counted() {
    let content = "See the \u{FFFC} bank \u{FFFC} now. Next.";
    let world = world(
        "embedded",
        Address::Env,
        reader(vec![selected(content, "bank")]),
        &[0],
    );
    let run = world.run(&["context", "--app", "org.example.Reader"]);
    assert_eq!(run.exit, 0);
    let d = &run.record["details"];
    assert_eq!(d["word"]["embedded_object_chars"], json!(0));
    assert_eq!(d["sentence"]["text"]["embedded_object_chars"], json!(2));
    assert_eq!(
        d["derived"]["wider_text"]["embedded_object_chars"],
        json!(2)
    );
}

// ---------------------------------------------------------------- timeout

fn assert_timeout(run: &Run, timeout: Duration, phase: &str) {
    assert_eq!(run.exit, 3);
    assert_eq!(run.record["outcome"], json!("timeout"));
    assert_eq!(run.record["details"]["phase"], json!(phase));
    assert!(run.elapsed >= timeout, "ended early: {:?}", run.elapsed);
    assert!(
        run.elapsed < timeout + Duration::from_secs(1),
        "ended {:?} after a {timeout:?} timeout",
        run.elapsed
    );
}

#[test]
fn an_application_that_stops_answering_ends_in_timeout() {
    let mut nodes = reader(vec![selected(BOOK, "bank")]);
    nodes[0].hang = true;
    let world = world("timeout-app", Address::Env, nodes, &[0]);
    for command in ["tree", "context"] {
        let run = world.run(&[command, "--app", "org.example.Reader", "--timeout", "1.5"]);
        assert_timeout(&run, Duration::from_millis(1500), "walk-root");
        // What was known before the wait is still in the record.
        assert_eq!(run.record["target"][0]["name"], json!("org.example.Reader"));
        assert_eq!(
            run.record["accessibility"]["bus_address"],
            json!(world.bus.address)
        );
    }
}

#[test]
fn a_registry_that_never_answers_ends_in_timeout() {
    let world = world("timeout-hung-registry", Address::Env, vec![], &[]);
    for args in [
        vec!["status"],
        vec!["tree", "--app", "x"],
        vec!["context", "--app", "x"],
    ] {
        let mut args = args;
        args.extend(["--timeout", "1"]);
        let run = world.run(&args);
        assert_timeout(&run, Duration::from_secs(1), "registry");
    }
}

#[test]
fn context_cut_by_time_before_a_selection_is_timeout_not_no_text() {
    let world = world("timeout-walk", Address::Env, slow_chain(), &[0]);
    let run = world.run(&["context", "--app", "Slow", "--timeout", "2"]);
    assert_eq!(run.exit, 3);
    assert_eq!(run.record["outcome"], json!("timeout"));
    assert_eq!(run.record["details"]["walk"]["cut"], json!("time"));
    assert!(run.elapsed < Duration::from_secs(3));
}

// ------------------------------------------------------------------ delay

#[test]
fn delay_passes_before_anything_is_read() {
    let world = world(
        "delay",
        Address::Env,
        reader(vec![with_granularity(BOOK, "bank", SENTENCE)]),
        &[0],
    );
    let run = world.run(&[
        "context",
        "--app",
        "org.example.Reader",
        "--delay",
        "1.2",
        "--timeout",
        "5",
    ]);
    assert_eq!(run.exit, 0);
    assert_eq!(run.record["outcome"], json!("context"));
    assert_eq!(run.record["inputs"]["delay_s"], json!(1.2));
    assert!(run.record["inputs"]["delay_waited_ms"].as_u64().unwrap() >= 1200);
    assert!(run.record["elapsed_ms"].as_u64().unwrap() >= 1200);
    let log = world.log.lock().unwrap();
    let first = log.iter().min().expect("the fake application was read");
    assert!(
        first.duration_since(run.started) >= Duration::from_millis(1200),
        "the application was read {:?} after the start",
        first.duration_since(run.started)
    );
}

#[test]
fn timeout_is_counted_after_the_delay() {
    let mut nodes = reader(vec![]);
    nodes[0].hang = true;
    let world = world("delay-timeout", Address::Env, nodes, &[0]);
    let run = world.run(&[
        "context",
        "--app",
        "org.example.Reader",
        "--delay",
        "1",
        "--timeout",
        "1",
    ]);
    assert_timeout(&run, Duration::from_secs(2), "walk-root");
}

// ------------------------------------------------- review: incomplete evidence

#[test]
fn a_target_whose_name_never_answers_is_timeout_not_app_not_found() {
    let mut hung = app("org.example.Reader", vec![]);
    hung.silent_name = true;
    let world = world(
        "find-hung-name",
        Address::Env,
        vec![app("Other", vec![]), hung],
        &[0, 1],
    );
    for command in ["tree", "context"] {
        let run = world.run(&[command, "--app", "org.example.Reader", "--timeout", "2"]);
        assert_eq!(run.exit, 3, "{command}");
        assert_eq!(run.record["outcome"], json!("timeout"));
        let error = run.record["error"].as_str().unwrap();
        assert!(error.contains("1 of 2"), "{error}");
        assert!(error.contains("did not answer"), "{error}");
        assert_eq!(run.record["details"]["applications_unanswered"], json!(1));
        assert!(run.elapsed < Duration::from_secs(3));
    }
}

#[test]
fn a_bus_without_a_registry_is_unavailable() {
    // The bus is reachable, but nothing owns org.a11y.atspi.Registry.
    let world = world("no-registry", Address::Absent, vec![], &[]);
    let address = world.bus.address.clone();
    for args in [
        vec!["status"],
        vec!["tree", "--app", "x"],
        vec!["context", "--app", "x"],
    ] {
        let run = world.run_with(&args, Some(&address));
        assert_eq!(run.exit, 5, "{args:?}");
        assert_eq!(run.record["outcome"], json!("unavailable"));
        assert_eq!(run.record["accessibility"]["bus_address"], json!(address));
        let error = run.record["error"].as_str().unwrap();
        assert!(error.contains("registry"), "{error}");
        assert!(error.contains("org.a11y.atspi.Registry"), "{error}");
    }
}

fn unselected() -> FakeText {
    FakeText {
        content: BOOK.to_owned(),
        ..FakeText::default()
    }
}

#[test]
fn context_cut_by_the_node_limit_is_error_not_a_negative_outcome() {
    let world = world(
        "context-nodes",
        Address::Env,
        reader(vec![unselected(), selected(BOOK, "bank")]),
        &[0],
    );
    let run = world.run(&["context", "--app", "org.example.Reader", "--max-nodes", "3"]);
    assert_eq!(run.exit, 1);
    assert_eq!(run.record["outcome"], json!("error"));
    assert!(
        run.record["error"]
            .as_str()
            .unwrap()
            .contains("--max-nodes")
    );
    assert_eq!(run.record["details"]["walk"]["cut"], json!("nodes"));
}

#[test]
fn context_with_a_silent_node_is_timeout_not_a_negative_outcome() {
    let mut nodes = reader(vec![unselected()]);
    nodes.push(FakeNode {
        role: "panel",
        hang: true,
        ..FakeNode::default()
    });
    nodes[1].children.push(3);
    let world = world("context-silent-node", Address::Env, nodes, &[0]);
    let run = world.run(&["context", "--app", "org.example.Reader", "--timeout", "4"]);
    assert_eq!(run.exit, 3);
    assert_eq!(run.record["outcome"], json!("timeout"));
    let error = run.record["error"].as_str().unwrap();
    assert!(error.contains("1 nodes did not answer"), "{error}");
    assert_eq!(run.record["details"]["walk"]["unanswered"], json!(1));
}

#[test]
fn a_reported_selection_that_cannot_be_read_is_not_no_selection() {
    for (name, hangs, exit, outcome, part) in [
        ("selection-refused", false, 1, "error", "selection refused"),
        ("selection-silent", true, 3, "timeout", "no answer within"),
    ] {
        let text = FakeText {
            broken_selection: Some(hangs),
            ..selected(BOOK, "bank")
        };
        let world = world(name, Address::Env, reader(vec![text]), &[0]);
        let run = world.run(&["context", "--app", "org.example.Reader", "--timeout", "4"]);
        assert_eq!(run.exit, exit, "{name}");
        assert_eq!(run.record["outcome"], json!(outcome));
        let error = run.record["error"].as_str().unwrap();
        assert!(error.contains("GetSelection"), "{error}");
        assert!(error.contains(part), "{error}");
        assert!(
            run.record["details"]["selection_read_errors"][0]
                .as_str()
                .unwrap()
                .contains(part)
        );
    }
}

#[test]
fn an_application_sentence_that_misses_the_selection_is_not_chosen() {
    // The application answers with the first sentence, not the word's.
    let other = "It was late.";
    let (start, end) = range(BOOK, other);
    let text = FakeText {
        by_granularity: Some((other.to_owned(), start, end)),
        ..selected(BOOK, "bank")
    };
    let world = world(
        "context-not-covering",
        Address::Env,
        reader(vec![text]),
        &[0],
    );
    let run = world.run(&[
        "context",
        "--app",
        "org.example.Reader",
        "--expect-sentence",
        SENTENCE,
    ]);
    assert_eq!(run.exit, 0);
    let d = &run.record["details"];
    assert_eq!(run.record["outcome"], json!("context"));
    assert_eq!(d["sentence_attempts"][0]["ok"], json!(true));
    assert_eq!(d["sentence_attempts"][0]["covers_selection"], json!(false));
    assert_eq!(d["sentence_attempts"][0]["text"]["text"], json!(other));
    assert_eq!(d["sentence"]["source"], json!("derived"));
    assert_eq!(d["sentence"]["text"]["text"], json!(SENTENCE));
    assert_eq!(d["sentence_match"], json!(true));
    assert_eq!(d["sentence_match_application"], json!(false));
    assert_eq!(d["sentence_match_derived"], json!(true));
}

#[test]
fn a_non_covering_reply_with_no_derived_sentence_is_word_only() {
    let other = "It was late.";
    let (start, end) = range(BOOK, other);
    let text = FakeText {
        by_granularity: Some((other.to_owned(), start, end)),
        only_selected_range: true,
        ..selected(BOOK, "bank")
    };
    let world = world(
        "word-only-not-covering",
        Address::Env,
        reader(vec![text]),
        &[0],
    );
    let run = world.run(&["context", "--app", "org.example.Reader"]);
    assert_eq!(run.exit, 8);
    assert_eq!(run.record["outcome"], json!("word-only"));
    let error = run.record["error"].as_str().unwrap();
    assert!(error.contains("does not hold the selection"), "{error}");
    assert!(run.record["details"].get("sentence").is_none());
}
