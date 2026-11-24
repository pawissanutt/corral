//! The inspector's "This task" section in Impact (docs/plans/system-map.md §3.3): what the
//! change did under the picked node, before the facts Activity › System's inspector shows. It
//! leads with the lines added and removed and the files they are in, each of which selects
//! that file on the map, then the dependents the node holds, each of which selects its file.

use corral_types::{ModuleId, SystemAt, SystemReviewRow as Row};

use super::items::{Target, container_of};
use super::{head_map, request, review_of};
use crate::model::daemon::Feed;
use crate::model::system::{Node, Pick, count, plural};

/// One file or dependent under the node.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub main: String,
    pub right: String,
    /// Where selecting it takes the map; none for a file the head no longer has.
    pub target: Option<Target>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThisTask {
    /// "+316 −69 in 12 files", or that the change did not touch the node.
    pub lines: String,
    pub files: Vec<Line>,
    /// Said over the dependents when there are any.
    pub held: Option<String>,
    pub dependents: Vec<Line>,
}

pub const TITLE: &str = "This task";
/// Said while the pick names nothing this level shows, until it is read or let go.
pub const NOTHING: &str = "Nothing on this level matches the selection.";
pub const UNTOUCHED: &str = "No line of this change is here.";

/// The file's place on the map: its module's level with the file selected.
fn file_target(module: &ModuleId, path: &str) -> Target {
    Target {
        trail: vec![
            SystemAt::Container {
                path: module.container.clone(),
            },
            SystemAt::Module {
                module: module.clone(),
            },
        ],
        node: Node::File(path.to_owned()),
    }
}

/// Whether a file of `container` and `module` lies under `node`.
fn under(node: &Node, path: &str, container: Option<&str>, module: Option<&ModuleId>) -> bool {
    match node {
        Node::Container(c) | Node::Port { path: c, .. } => container == Some(c.as_str()),
        Node::Module(m) => module == Some(m),
        Node::File(p) => p == path,
        _ => false,
    }
}

/// "This task" for the node picked on task `task`'s map; none for an edge, a "N more" card or
/// a definition, which the change's rows do not place.
pub fn this_task(feed: &Feed, task: &str, pick: &Pick) -> Option<ThisTask> {
    let Pick::Node(node) = pick else {
        return None;
    };
    if !matches!(
        node,
        Node::Container(_) | Node::Port { .. } | Node::Module(_) | Node::File(_)
    ) {
        return None;
    }
    let (review, _) = review_of(feed, &request(task, None))?;
    let containers: Vec<String> = head_map(review)
        .rows
        .iter()
        .filter_map(|row| match row {
            corral_types::SystemMapRow::Container { container, .. } => Some(container.path.clone()),
            _ => None,
        })
        .collect();
    let (mut added, mut removed) = (0u64, 0u64);
    let mut files = Vec::new();
    let mut dependents = Vec::new();
    for row in &review.rows {
        match row {
            Row::File {
                path,
                container,
                module,
                added: a,
                removed: r,
                deleted,
                ..
            } if under(node, path, Some(container), Some(module)) => {
                added += a;
                removed += r;
                files.push(Line {
                    main: path.clone(),
                    right: format!("+{} −{}", count(*a), count(*r)),
                    target: (!deleted).then(|| file_target(module, path)),
                });
            }
            Row::Dependent { dependent, module } => {
                let container = module
                    .as_ref()
                    .map(|m| m.container.as_str())
                    .or_else(|| container_of(&containers, &dependent.path).map(String::as_str));
                if !under(node, &dependent.path, container, module.as_ref()) {
                    continue;
                }
                dependents.push(Line {
                    main: dependent.name.clone(),
                    right: format!(
                        "{}:{} · uses {}",
                        dependent.path, dependent.line, dependent.of
                    ),
                    target: module.as_ref().map(|m| file_target(m, &dependent.path)),
                });
            }
            _ => {}
        }
    }
    let lines = if files.is_empty() {
        UNTOUCHED.to_owned()
    } else {
        format!(
            "+{} −{} in {}",
            count(added),
            count(removed),
            plural(files.len() as u64, "file", "files")
        )
    };
    let held = (!dependents.is_empty()).then(|| {
        format!(
            "{}: code here that references a changed definition",
            plural(dependents.len() as u64, "dependent", "dependents")
        )
    });
    Some(ThisTask {
        lines,
        files,
        held,
        dependents,
    })
}
