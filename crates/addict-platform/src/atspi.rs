//! The AT-SPI adapter: the word selected in Android Studio and the sentence
//! around it, read from the accessibility bus.
//!
//! [`read`] takes a [`Locate`], the [`Accessibility`] and the [`Source`] the
//! press found and a deadline and gives one [`Context`] or one
//! [`ContextFailure`]. Accessibility that was read as off ends it at once as
//! [`ContextFailure::AccessibilityOff`], before the source is looked at. A
//! source that is another application ends it at once as
//! [`ContextFailure::NotFromStudio`]: no bus is opened. A source that is not
//! known asks the registry for the names of the applications and nothing
//! else: without Android Studio among them the read ends as
//! [`ContextFailure::AppNotFound`], with it as `NotFromStudio`. No text is
//! read in either case; the decision is the caller's, from what
//! [`crate::foreground`] observed, and `read` takes it as given. Otherwise
//! it opens the accessibility bus, finds the application whose registered name
//! contains `Android Studio`, walks its tree to the one Text object that has a
//! selection, and reads the selected word and a wider text around it. Every
//! offset on the bus is a character offset, never a byte index. The sentence is
//! cut from the wider text by [`addict_core::sentence::of_selection`]: the
//! adapter never asks the application for a sentence, whose answers were wrong
//! in every EXP-2 scenario.
//!
//! The adapter only reads: it changes no setting of the application or of
//! the session. The whole read is bounded by its deadline; a part of the
//! tree that does not answer is skipped, and a bus that is silent ends the
//! read as [`ContextFailure::TimedOut`]. The read is a future for the work
//! runner and never runs on the UI thread.

use std::{
    collections::{HashSet, VecDeque},
    time::Duration,
};

use crate::foreground::{Accessibility, Source};
use addict_core::sentence::{self, Selected};
use futures_util::future::join_all;
use serde::{Serialize, de::DeserializeOwned};
use tokio::time::timeout;
use zbus::{
    Connection,
    zvariant::{DynamicType, OwnedObjectPath, OwnedValue, Type},
};

pub(crate) const A11Y_BUS_NAME: &str = "org.a11y.Bus";
pub(crate) const A11Y_BUS_PATH: &str = "/org/a11y/bus";
pub(crate) const REGISTRY_NAME: &str = "org.a11y.atspi.Registry";
const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const TEXT: &str = "org.a11y.atspi.Text";

/// What the registered name of the application must contain, in lowercase.
/// Android Studio registers as `<project> - Android Studio` (EXP-2).
const APPLICATION: &str = "android studio";

/// Characters read on each side of the selection to find its sentence.
const CONTEXT_CHARS: usize = 600;

/// Nodes of the tree read before the walk is cut.
const MAX_NODES: usize = 5000;

/// Time one node below an application's root gets to answer before it is
/// skipped.
const NODE_LIMIT: Duration = Duration::from_millis(500);

/// Time an application gets to give its name, and an object its selection.
pub(crate) const CALL_LIMIT: Duration = Duration::from_millis(500);

/// Where the accessibility bus is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Locate {
    /// As the session gives it: `AT_SPI_BUS_ADDRESS` if set and not empty,
    /// else the address `org.a11y.Bus` gives on the session bus.
    Session,
    /// A bus at this address.
    Address(String),
}

/// What is selected in the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// The selected text, as the application gave it.
    pub word: String,
    /// The sentence around it with the place of the word in it, when a
    /// sentence could be cut from the text around the selection.
    pub sentence: Option<Selected>,
}

/// Why nothing was read. The detail of `Unavailable` and `Failed` is logged
/// and shown in the window's source line as given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextFailure {
    /// `IsEnabled` of `org.a11y.Status` was read as false: no application
    /// is on the accessibility bus to be read. Nothing was opened.
    AccessibilityOff,
    /// There is no accessibility bus, or it refuses the connection.
    Unavailable(String),
    /// No registered application has `Android Studio` in its name; also
    /// when the application in front was not known.
    AppNotFound,
    /// The application has no Text object with a selection.
    NoSelection,
    /// This many Text objects of the application report a selection.
    Ambiguous(usize),
    /// The bus did not answer within the deadline.
    TimedOut,
    /// The application in front at the press was not Android Studio, or was
    /// not known while Android Studio is registered; no text was read. The
    /// source is what the press found.
    NotFromStudio(Source),
    /// The application answered, but the read could not be completed.
    Failed(String),
}

/// One accessible object: a bus name and an object path on the
/// accessibility bus.
#[derive(Debug, Clone)]
pub(crate) struct Object {
    connection: Connection,
    bus_name: String,
    path: String,
}

impl Object {
    pub(crate) fn new(connection: &Connection, bus_name: &str, path: &str) -> Self {
        Self {
            connection: connection.clone(),
            bus_name: bus_name.to_owned(),
            path: path.to_owned(),
        }
    }

    /// Calls a method and returns its reply, or the error text as the bus
    /// gave it.
    pub(crate) async fn call<B, R>(
        &self,
        interface: &str,
        member: &str,
        body: &B,
    ) -> Result<R, String>
    where
        B: Serialize + DynamicType + Sync,
        R: DeserializeOwned + Type,
    {
        let reply = self
            .connection
            .call_method(
                Some(self.bus_name.as_str()),
                self.path.as_str(),
                Some(interface),
                member,
                body,
            )
            .await
            .map_err(|e| e.to_string())?;
        let body = reply.body();
        body.deserialize::<R>().map_err(|e| e.to_string())
    }

    /// Reads one property.
    pub(crate) async fn property<T>(&self, interface: &str, name: &str) -> Result<T, String>
    where
        T: TryFrom<OwnedValue>,
        T::Error: std::fmt::Display,
    {
        let value: OwnedValue = self
            .call("org.freedesktop.DBus.Properties", "Get", &(interface, name))
            .await?;
        T::try_from(value).map_err(|e| e.to_string())
    }

    /// Children as the object lists them; falls back to asking one index at
    /// a time when `GetChildren` is not answered.
    async fn children(&self) -> Result<Vec<Object>, String> {
        let listed: Result<Vec<(String, OwnedObjectPath)>, String> =
            self.call(ACCESSIBLE, "GetChildren", &()).await;
        let pairs = match listed {
            Ok(pairs) => pairs,
            Err(first) => {
                let count: i32 = self
                    .property(ACCESSIBLE, "ChildCount")
                    .await
                    .map_err(|second| format!("GetChildren: {first}; ChildCount: {second}"))?;
                let mut pairs = Vec::new();
                for index in 0..count.max(0) {
                    let pair: (String, OwnedObjectPath) = self
                        .call(ACCESSIBLE, "GetChildAtIndex", &(index,))
                        .await
                        .map_err(|e| format!("GetChildAtIndex({index}): {e}"))?;
                    pairs.push(pair);
                }
                pairs
            }
        };
        Ok(pairs
            .into_iter()
            .map(|(bus_name, path)| Object::new(&self.connection, &bus_name, path.as_str()))
            .collect())
    }
}

/// Reads the selection of Android Studio from the accessibility bus at
/// `bus`, within `limit` altogether, when `source` says that Android Studio
/// was in front. `accessibility` read as off ends it at once as
/// [`ContextFailure::AccessibilityOff`]; an unknown one does not stop it.
/// Another application in front ends it at once as
/// [`ContextFailure::NotFromStudio`], before anything is opened; an unknown
/// one asks only for the names of the applications, within `limit`: no
/// Android Studio among them is [`ContextFailure::AppNotFound`], one is
/// `NotFromStudio`, and so is an application or a registry that does not
/// answer in time, since nothing says that Android Studio is the silent one;
/// a bus that is not there is [`ContextFailure::Unavailable`].
pub async fn read(
    bus: &Locate,
    accessibility: Accessibility,
    source: Source,
    limit: Duration,
) -> Result<Context, ContextFailure> {
    if accessibility == Accessibility::Off {
        return Err(ContextFailure::AccessibilityOff);
    }
    match source {
        Source::Other => Err(ContextFailure::NotFromStudio(source)),
        Source::Unknown => match timeout(limit, studio_is_registered(bus)).await {
            Ok(Err(ContextFailure::TimedOut)) | Err(_) => {
                Err(ContextFailure::NotFromStudio(Source::Unknown))
            }
            Ok(read) => read,
        },
        Source::AndroidStudio => timeout(limit, read_selection(bus))
            .await
            .unwrap_or(Err(ContextFailure::TimedOut)),
    }
}

/// Why nothing is read when the application in front is not known: Android
/// Studio is registered, or it is not. Only names are asked, never text.
async fn studio_is_registered(bus: &Locate) -> Result<Context, ContextFailure> {
    let connection = open(bus).await?;
    applications(&connection).await?;
    Err(ContextFailure::NotFromStudio(Source::Unknown))
}

async fn read_selection(bus: &Locate) -> Result<Context, ContextFailure> {
    let connection = open(bus).await?;
    let roots = applications(&connection).await?;
    let walk = walk(&roots).await;
    let candidate = select(&walk).await?;
    read_candidate(&candidate).await
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Finds and opens the accessibility bus. A bus that is absent or refuses is
/// [`ContextFailure::Unavailable`].
pub(crate) async fn open(bus: &Locate) -> Result<Connection, ContextFailure> {
    let address = match bus {
        Locate::Address(address) => address.clone(),
        Locate::Session => session_address().await?,
    };
    let unavailable = |e: zbus::Error| {
        ContextFailure::Unavailable(format!("accessibility bus at {address}: {e}"))
    };
    zbus::connection::Builder::address(address.as_str())
        .map_err(unavailable)?
        .build()
        .await
        .map_err(unavailable)
}

/// `AT_SPI_BUS_ADDRESS` if set, else what `org.a11y.Bus` says on the
/// session bus.
async fn session_address() -> Result<String, ContextFailure> {
    if let Some(address) = env_var("AT_SPI_BUS_ADDRESS") {
        return Ok(address);
    }
    let session = Connection::session().await.map_err(|e| {
        ContextFailure::Unavailable(format!(
            "AT_SPI_BUS_ADDRESS is not set and the session bus is unreachable: {e}"
        ))
    })?;
    let launcher = Object::new(&session, A11Y_BUS_NAME, A11Y_BUS_PATH);
    match launcher
        .call::<_, String>(A11Y_BUS_NAME, "GetAddress", &())
        .await
    {
        Ok(address) if !address.is_empty() => Ok(address),
        Ok(_) => Err(ContextFailure::Unavailable(
            "org.a11y.Bus GetAddress returned an empty address".to_owned(),
        )),
        Err(e) => Err(ContextFailure::Unavailable(format!(
            "org.a11y.Bus GetAddress: {e}"
        ))),
    }
}

/// The roots of the registered applications whose name contains
/// `Android Studio`, compared without case. No such application is
/// [`ContextFailure::AppNotFound`], or [`ContextFailure::TimedOut`] when an
/// application did not give its name: its silence is not proof of absence.
async fn applications(connection: &Connection) -> Result<Vec<Object>, ContextFailure> {
    let registry = Object::new(connection, REGISTRY_NAME, ROOT_PATH);
    let roots = registry
        .children()
        .await
        .map_err(|e| ContextFailure::Unavailable(format!("registry: {e}")))?;
    let named = join_all(roots.into_iter().map(|root| async move {
        let name = timeout(CALL_LIMIT, root.property::<String>(ACCESSIBLE, "Name")).await;
        (root, name)
    }))
    .await;
    let mut silent = 0;
    let mut matched = Vec::new();
    for (root, name) in named {
        match name {
            Ok(Ok(name)) if is_android_studio(&name) => matched.push(root),
            Ok(_) => {}
            Err(_) => silent += 1,
        }
    }
    if !matched.is_empty() {
        Ok(matched)
    } else if silent > 0 {
        Err(ContextFailure::TimedOut)
    } else {
        Err(ContextFailure::AppNotFound)
    }
}

/// Whether an application of this registered name is Android Studio:
/// the name contains `Android Studio`, compared without case.
pub(crate) fn is_android_studio(name: &str) -> bool {
    name.to_lowercase().contains(APPLICATION)
}

/// The registered name of the application at `bus_name`, from its root; none
/// when it does not give it within [`CALL_LIMIT`].
pub(crate) async fn application_name(connection: &Connection, bus_name: &str) -> Option<String> {
    let root = Object::new(connection, bus_name, ROOT_PATH);
    timeout(CALL_LIMIT, root.property::<String>(ACCESSIBLE, "Name"))
        .await
        .ok()?
        .ok()
}

/// A Text object that reports a selection.
struct Selecting {
    object: Object,
    character_count: Option<usize>,
}

/// What a walk of the applications' trees found.
#[derive(Default)]
struct Walk {
    selecting: Vec<Selecting>,
    /// The walk stopped at [`MAX_NODES`] before the trees ended.
    cut: bool,
    /// Nodes that did not answer within [`NODE_LIMIT`].
    silent: usize,
}

/// What one node says: its children, and for a Text object its size and
/// number of selections.
struct Node {
    children: Result<Vec<Object>, String>,
    text: Option<(Option<usize>, i32)>,
}

async fn read_node(object: &Object) -> Node {
    let (interfaces, children) = tokio::join!(
        object.call::<_, Vec<String>>(ACCESSIBLE, "GetInterfaces", &()),
        object.children(),
    );
    let is_text = interfaces.is_ok_and(|list| list.iter().any(|name| name == TEXT));
    let text = if is_text {
        let (count, selections) = tokio::join!(
            object.property::<i32>(TEXT, "CharacterCount"),
            object.call::<_, i32>(TEXT, "GetNSelections", &()),
        );
        Some((
            count.ok().and_then(|count| usize::try_from(count).ok()),
            selections.unwrap_or(0),
        ))
    } else {
        None
    };
    Node { children, text }
}

/// Walks the trees under `roots` breadth-first. A root is read without a
/// limit of its own: an application that never answers ends in the deadline
/// of the read, not in an empty tree.
async fn walk(roots: &[Object]) -> Walk {
    let mut walk = Walk::default();
    let mut seen = HashSet::new();
    let mut nodes = 0;
    let mut queue: VecDeque<(Object, bool)> =
        roots.iter().map(|root| (root.clone(), true)).collect();
    while let Some((object, is_root)) = queue.pop_front() {
        if !seen.insert((object.bus_name.clone(), object.path.clone())) {
            continue;
        }
        if nodes >= MAX_NODES {
            walk.cut = true;
            break;
        }
        nodes += 1;
        let node = if is_root {
            read_node(&object).await
        } else {
            match timeout(NODE_LIMIT, read_node(&object)).await {
                Ok(node) => node,
                Err(_) => {
                    walk.silent += 1;
                    continue;
                }
            }
        };
        if let Some((character_count, selections)) = node.text
            && selections > 0
        {
            walk.selecting.push(Selecting {
                object: object.clone(),
                character_count,
            });
        }
        if let Ok(children) = node.children {
            queue.extend(children.into_iter().map(|child| (child, false)));
        }
    }
    walk
}

/// The one Text object with a non-empty selection, and where it is.
struct Candidate {
    object: Object,
    character_count: Option<usize>,
    start: usize,
    end: usize,
}

fn offset(value: i32) -> usize {
    usize::try_from(value).unwrap_or(0)
}

fn wire(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// The one candidate of `walk`. Several are [`ContextFailure::Ambiguous`];
/// none is [`ContextFailure::NoSelection`] unless the evidence is
/// incomplete: a node or a selection that did not answer, or a walk that was
/// cut, has not shown that nothing is selected.
async fn select(walk: &Walk) -> Result<Candidate, ContextFailure> {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    let mut silent = walk.silent;
    for selecting in &walk.selecting {
        let selection = timeout(
            CALL_LIMIT,
            selecting
                .object
                .call::<_, (i32, i32)>(TEXT, "GetSelection", &(0i32,)),
        )
        .await;
        match selection {
            Ok(Ok((a, b))) => {
                let (start, end) = (offset(a.min(b)), offset(a.max(b)));
                if end > start {
                    found.push(Candidate {
                        object: selecting.object.clone(),
                        character_count: selecting.character_count,
                        start,
                        end,
                    });
                }
            }
            Ok(Err(e)) => errors.push(format!(
                "{} {}: GetSelection: {e}",
                selecting.object.bus_name, selecting.object.path
            )),
            Err(_) => silent += 1,
        }
    }
    match found.len() {
        0 if silent > 0 => Err(ContextFailure::TimedOut),
        0 if !errors.is_empty() => Err(ContextFailure::Failed(format!(
            "a selection was reported but could not be read: {}",
            errors.join("; ")
        ))),
        0 if walk.cut => Err(ContextFailure::Failed(format!(
            "the walk was cut at {MAX_NODES} nodes before a selection was found"
        ))),
        0 => Err(ContextFailure::NoSelection),
        1 => Ok(found.remove(0)),
        count => Err(ContextFailure::Ambiguous(count)),
    }
}

/// The characters `start..end` of `text`.
fn char_slice(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// Reads the word and the text around it. The sentence is `None` when the
/// text around could not be read, when its offsets do not agree with the
/// word (the application counts in other units than characters), or when
/// no sentence is cut from it; the word alone is then the result.
async fn read_candidate(candidate: &Candidate) -> Result<Context, ContextFailure> {
    let (start, end) = (candidate.start, candidate.end);
    let object = &candidate.object;
    let word: String = object
        .call(TEXT, "GetText", &(wire(start), wire(end)))
        .await
        .map_err(|e| {
            ContextFailure::Failed(format!(
                "selection {start}..{end} was reported but GetText failed: {e}"
            ))
        })?;
    if word.trim().is_empty() {
        return Err(ContextFailure::NoSelection);
    }
    let wide_start = start.saturating_sub(CONTEXT_CHARS);
    let wide_end = {
        let wanted = end.saturating_add(CONTEXT_CHARS);
        match candidate.character_count.filter(|count| *count >= end) {
            Some(count) => count.min(wanted),
            None => wanted,
        }
    };
    let wider: Option<String> = object
        .call(TEXT, "GetText", &(wire(wide_start), wire(wide_end)))
        .await
        .ok();
    let sentence = wider.and_then(|wider| {
        let (from, to) = (start - wide_start, end - wide_start);
        // False when the offsets are not character offsets of this text.
        (char_slice(&wider, from, to) == word)
            .then(|| sentence::of_selection(&wider, from, to))
            .flatten()
    });
    Ok(Context { word, sentence })
}
