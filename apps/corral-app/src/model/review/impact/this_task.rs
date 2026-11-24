//! The inspector's "This task" section in Impact (docs/plans/system-map.md §3.3): what the
//! change did under the picked node, before the facts Activity › System's inspector shows. It
//! leads with the lines added and removed and the files they are in, each of which selects
//! that file on the map, then the dependents the node holds, each of which selects its file.
//! A "N more" card explains the files it folds; a definition, its own change and whether it
//! is a dependent.

use corral_types::{ChangeKind, ModuleId, SystemAt, SystemReview, SystemReviewRow as Row};

use super::groups::depth_words;
use super::items::{Target, container_of};
use super::{head_map, request, review_of};
use crate::model::daemon::Feed;
use crate::model::system::scene::Scene;
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

/// What a picked card stands for, so the change's rows can be placed under it.
enum Under<'a> {
    Container(&'a str),
    Module(&'a ModuleId),
    File(&'a str),
    /// The files of `module` the level folds into its "N more" card.
    Folded(&'a ModuleId, &'a Scene),
    /// The definition `name` at `line` of the file `path`, at its file's level.
    Definition {
        path: &'a str,
        name: &'a str,
        line: u64,
    },
}

impl Under<'_> {
    fn holds(&self, path: &str, container: Option<&str>, module: Option<&ModuleId>) -> bool {
        match self {
            Under::Container(c) => container == Some(*c),
            Under::Module(m) => module == Some(*m),
            Under::File(p) => *p == path,
            Under::Folded(m, scene) => {
                module == Some(*m) && scene.card(&Node::File(path.to_owned())).is_none()
            }
            // A definition holds a change or a dependent only by its own name: see `of_review`.
            Under::Definition { .. } => false,
        }
    }
}

/// "This task" for the node picked on task `task`'s map at `at`, the level `scene` draws;
/// none for an edge, whose inspector says what the change did to it.
pub fn this_task(
    feed: &Feed,
    task: &str,
    pick: &Pick,
    scene: &Scene,
    at: Option<&SystemAt>,
) -> Option<ThisTask> {
    let Pick::Node(node) = pick else {
        return None;
    };
    let (review, _) = review_of(feed, &request(task, None))?;
    of_review(review, node, scene, at)
}

pub(super) fn of_review(
    review: &SystemReview,
    node: &Node,
    scene: &Scene,
    at: Option<&SystemAt>,
) -> Option<ThisTask> {
    let under = match (node, at) {
        (Node::Container(c) | Node::Port { path: c, .. }, _) => Under::Container(c),
        (Node::Module(m), _) => Under::Module(m),
        (Node::File(p), _) => Under::File(p),
        (Node::More, Some(SystemAt::Module { module })) => Under::Folded(module, scene),
        (Node::Definition { name, line }, Some(SystemAt::File { path })) => Under::Definition {
            path,
            name,
            line: *line,
        },
        _ => return None,
    };
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
            } if under.holds(path, Some(container), Some(module))
                // A "N more" card folds files the head has; a deleted one is on no level.
                && !(*deleted && matches!(under, Under::Folded(..))) =>
            {
                added += a;
                removed += r;
                files.push(Line {
                    main: path.clone(),
                    right: if *deleted {
                        format!("deleted · −{}", count(*r))
                    } else {
                        format!("+{} −{}", count(*a), count(*r))
                    },
                    // A folded file has no card of its own to select at its module's level.
                    target: (!deleted && !matches!(under, Under::Folded(..)))
                        .then(|| file_target(module, path)),
                });
            }
            Row::Definition {
                change,
                path,
                name,
                line,
            } if *change != ChangeKind::Removed
                && matches!(under, Under::Definition { path: p, name: n, line: l }
                    if p == path && n == name && l == *line) =>
            {
                files.push(Line {
                    main: name.clone(),
                    right: format!(
                        "{} · :{line}",
                        match change {
                            ChangeKind::Added => "added",
                            ChangeKind::Removed => "removed",
                            ChangeKind::Changed => "changed",
                        }
                    ),
                    target: None,
                });
            }
            Row::Dependent { dependent, module } => {
                let held = match under {
                    Under::Definition { path, name, line } => {
                        dependent.path == path && dependent.name == name && dependent.line == line
                    }
                    _ => {
                        let container =
                            module.as_ref().map(|m| m.container.as_str()).or_else(|| {
                                container_of(&containers, &dependent.path).map(String::as_str)
                            });
                        under.holds(&dependent.path, container, module.as_ref())
                    }
                };
                if !held {
                    continue;
                }
                dependents.push(Line {
                    main: dependent.name.clone(),
                    right: format!(
                        "{}:{} · depends on {} · {}",
                        dependent.path,
                        dependent.line,
                        dependent.of,
                        depth_words(dependent.depth)
                    ),
                    target: module.as_ref().map(|m| {
                        let target = file_target(m, &dependent.path);
                        if !matches!(under, Under::Folded(..)) {
                            return target;
                        }
                        // A file folded at its module's level opens at its own, with the
                        // dependent definition selected there.
                        let mut trail = target.trail;
                        trail.push(SystemAt::File {
                            path: dependent.path.clone(),
                        });
                        Target {
                            trail,
                            node: Node::Definition {
                                line: dependent.line,
                                name: dependent.name.clone(),
                            },
                        }
                    }),
                });
            }
            _ => {}
        }
    }
    let lines = match &under {
        _ if files.is_empty() => UNTOUCHED.to_owned(),
        Under::Definition { .. } => "This definition is part of the change.".to_owned(),
        Under::Folded(..) => format!(
            "+{} −{} in {} not listed on the map",
            count(added),
            count(removed),
            plural(files.len() as u64, "file", "files")
        ),
        _ => format!(
            "+{} −{} in {}",
            count(added),
            count(removed),
            plural(files.len() as u64, "file", "files")
        ),
    };
    let held = (!dependents.is_empty()).then(|| match under {
        Under::Definition { .. } => {
            "A dependent: it depends on a changed definition outside it".to_owned()
        }
        _ => format!(
            "{}: code here that depends on a changed definition",
            plural(dependents.len() as u64, "dependent", "dependents")
        ),
    });
    Some(ThisTask {
        lines,
        files,
        held,
        dependents,
    })
}
