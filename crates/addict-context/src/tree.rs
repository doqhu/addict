//! What one application exposes: a walk bounded by node count and time,
//! with roles and a summary of every Text object.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

use addict_probe::record::{Bounded, Progress, bounded};
use serde::Serialize;

use crate::{
    bus::{ACCESSIBLE, Object, TEXT},
    text,
};

/// Text objects written to a record in full; the rest are only counted.
pub const LISTED_TEXT_OBJECTS: usize = 100;
const LISTED_ERRORS: usize = 10;
const NAME_CHARS: usize = 80;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Nodes read before the walk is cut.
    pub max_nodes: usize,
    /// Moment the walk is cut.
    pub stop_at: Instant,
    /// Time one node below the root gets to answer before it is skipped.
    pub per_node: Duration,
}

/// One object that implements `org.a11y.atspi.Text`.
#[derive(Debug, Clone, Serialize)]
pub struct TextObject {
    pub bus_name: String,
    pub object_path: String,
    /// Child indices from the application root, e.g. `0/2/1`.
    pub index_path: String,
    pub role: String,
    /// Roles from the application root down to this object.
    pub role_path: String,
    pub name: Option<String>,
    pub character_count: Option<i32>,
    pub n_selections: Option<i32>,
    /// Why a value above is `null`.
    pub error: Option<String>,
    #[serde(skip)]
    pub object: Option<Object>,
}

impl TextObject {
    pub fn has_characters(&self) -> bool {
        self.character_count.is_some_and(|count| count > 0)
    }

    pub fn reports_selection(&self) -> bool {
        self.n_selections.is_some_and(|count| count > 0)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Walk {
    pub node_count: usize,
    /// `"nodes"` or `"time"` when the walk stopped before the tree ended.
    pub cut: Option<&'static str>,
    pub roles: BTreeMap<String, usize>,
    pub text_object_count: usize,
    pub text_objects_with_characters: usize,
    pub text_objects_with_selection: usize,
    /// Nodes that did not answer within the per-node limit.
    pub unanswered: usize,
    /// Failed reads on nodes that did answer.
    pub read_errors: usize,
    /// The first few of both, as the bus gave them.
    pub errors: Vec<String>,
    /// The first `LISTED_TEXT_OBJECTS` Text objects.
    pub text_objects_listed: Vec<TextObject>,
    #[serde(skip)]
    pub text_objects: Vec<TextObject>,
}

struct Node {
    role: Result<String, String>,
    children: Result<Vec<Object>, String>,
    text: Option<TextSummary>,
}

struct TextSummary {
    name: Option<String>,
    character_count: Result<i32, String>,
    n_selections: Result<i32, String>,
}

async fn read(object: &Object) -> Node {
    let (role, role_number, interfaces, children) = tokio::join!(
        object.call::<_, String>(ACCESSIBLE, "GetRoleName", &()),
        object.call::<_, u32>(ACCESSIBLE, "GetRole", &()),
        object.call::<_, Vec<String>>(ACCESSIBLE, "GetInterfaces", &()),
        object.children(),
    );
    let is_text = interfaces.is_ok_and(|list| list.iter().any(|name| name == TEXT));
    let text = if is_text {
        let (name, character_count, n_selections) = tokio::join!(
            object.property::<String>(ACCESSIBLE, "Name"),
            object.property::<i32>(TEXT, "CharacterCount"),
            object.call::<_, i32>(TEXT, "GetNSelections", &()),
        );
        Some(TextSummary {
            name: name.ok().filter(|name| !name.is_empty()),
            character_count,
            n_selections,
        })
    } else {
        None
    };
    // Some toolkits (WebKitGTK content) answer GetRoleName with an empty
    // string; the numeric AT-SPI role then stands in, as `role <n>`.
    let role = match (role, role_number) {
        (Ok(name), Ok(number)) if name.is_empty() => Ok(format!("role {number}")),
        (Err(_), Ok(number)) => Ok(format!("role {number}")),
        (role, _) => role,
    };
    Node {
        role,
        children,
        text,
    }
}

struct Pending {
    object: Object,
    index_path: String,
    parent_roles: String,
    is_root: bool,
}

/// Walks the trees under `roots` breadth-first. A root is read without a
/// per-node limit: a target that never answers ends in the run's timeout,
/// not in an empty tree.
pub async fn walk(roots: &[Object], limits: Limits, progress: &Progress) -> Walk {
    let mut walk = Walk::default();
    let mut seen = HashSet::new();
    let mut queue: VecDeque<Pending> = roots
        .iter()
        .enumerate()
        .map(|(index, object)| Pending {
            object: object.clone(),
            index_path: if roots.len() > 1 {
                format!("app{index}")
            } else {
                String::new()
            },
            parent_roles: String::new(),
            is_root: true,
        })
        .collect();

    while let Some(pending) = queue.pop_front() {
        if !seen.insert((pending.object.bus_name.clone(), pending.object.path.clone())) {
            continue;
        }
        if walk.node_count >= limits.max_nodes {
            walk.cut = Some("nodes");
            break;
        }
        if Instant::now() >= limits.stop_at {
            walk.cut = Some("time");
            break;
        }
        progress.phase(if pending.is_root { "walk-root" } else { "walk" });
        let node = if pending.is_root {
            read(&pending.object).await
        } else {
            match bounded(limits.per_node, read(&pending.object)).await {
                Bounded::Done(node) => node,
                Bounded::TimedOut => {
                    walk.node_count += 1;
                    walk.unanswered += 1;
                    note_error(
                        &mut walk,
                        &pending,
                        &format!("no answer within {} s", limits.per_node.as_secs_f64()),
                    );
                    walk.read_errors -= 1;
                    continue;
                }
            }
        };
        walk.node_count += 1;

        let role = match &node.role {
            Ok(role) => role.clone(),
            Err(e) => {
                note_error(&mut walk, &pending, &format!("GetRoleName: {e}"));
                "(unknown)".to_owned()
            }
        };
        *walk.roles.entry(role.clone()).or_default() += 1;
        let role_path = if pending.parent_roles.is_empty() {
            role.clone()
        } else {
            format!("{} / {role}", pending.parent_roles)
        };

        if let Some(summary) = node.text {
            let mut errors = Vec::new();
            if let Err(e) = &summary.character_count {
                errors.push(format!("CharacterCount: {e}"));
            }
            if let Err(e) = &summary.n_selections {
                errors.push(format!("GetNSelections: {e}"));
            }
            let text_object = TextObject {
                bus_name: pending.object.bus_name.clone(),
                object_path: pending.object.path.clone(),
                index_path: pending.index_path.clone(),
                role,
                role_path: role_path.clone(),
                name: summary.name.map(|name| text::clip(&name, NAME_CHARS).0),
                character_count: summary.character_count.ok(),
                n_selections: summary.n_selections.ok(),
                error: (!errors.is_empty()).then(|| errors.join("; ")),
                object: Some(pending.object.clone()),
            };
            walk.text_object_count += 1;
            walk.text_objects_with_characters += usize::from(text_object.has_characters());
            walk.text_objects_with_selection += usize::from(text_object.reports_selection());
            if walk.text_objects_listed.len() < LISTED_TEXT_OBJECTS {
                walk.text_objects_listed.push(text_object.clone());
            }
            walk.text_objects.push(text_object);
        }

        match node.children {
            Ok(children) => {
                for (index, child) in children.into_iter().enumerate() {
                    queue.push_back(Pending {
                        object: child,
                        index_path: if pending.index_path.is_empty() {
                            index.to_string()
                        } else {
                            format!("{}/{index}", pending.index_path)
                        },
                        parent_roles: role_path.clone(),
                        is_root: false,
                    });
                }
            }
            Err(e) => note_error(&mut walk, &pending, &format!("children: {e}")),
        }
        if walk.node_count % 25 == 0 {
            progress.put("walk", &walk);
        }
    }
    progress.put("walk", &walk);
    walk
}

fn note_error(walk: &mut Walk, pending: &Pending, error: &str) {
    walk.read_errors += 1;
    if walk.errors.len() < LISTED_ERRORS {
        walk.errors.push(format!(
            "{} {} [{}]: {error}",
            pending.object.bus_name, pending.object.path, pending.index_path
        ));
    }
}
