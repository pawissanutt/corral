//! What the change did to the level shown beyond its nodes: the reference counts it moved on
//! the level's edges, and what the level's "N more" card hides.

use std::collections::BTreeMap;

use corral_types::{
    ChangeKind, ModuleId, PackageEdge, SystemAt, SystemReview, SystemReviewRow as Row,
};

use super::items::module_words;
use crate::model::system::Node;
use crate::model::system::marks::{Delta, Mark, Marks, figure};
use crate::model::system::scene::{EdgeKind, Scene};

/// Gives each edge of `scene` the module pairs whose reference count the change moved: a
/// module reference its own pair, a boundary edge the pairs it sums, a package edge the pairs
/// across it or, where none is listed, the package edge's own count.
pub(super) fn edges(scene: &mut Scene, review: &SystemReview) {
    let mut pairs: BTreeMap<(&ModuleId, &ModuleId), (u64, u64)> = BTreeMap::new();
    let mut packages: BTreeMap<(&str, &str), (u64, u64)> = BTreeMap::new();
    let refs = |edge: &Option<PackageEdge>| edge.as_ref().map_or(0, |e| e.references);
    for row in &review.rows {
        match row {
            Row::ModuleReference {
                from,
                to,
                before,
                after,
            } if before != after => {
                pairs.insert((from, to), (*before, *after));
            }
            Row::PackageEdge { before, after } if refs(before) != refs(after) => {
                if let Some(edge) = after.as_ref().or(before.as_ref()) {
                    packages.insert(
                        (edge.from.as_str(), edge.to.as_str()),
                        (refs(before), refs(after)),
                    );
                }
            }
            _ => {}
        }
    }
    let delta = |from: &ModuleId, to: &ModuleId, (before, after): (u64, u64)| Delta {
        from: module_words(from),
        to: module_words(to),
        before,
        after,
    };
    for edge in scene.edges.iter_mut().chain(scene.undrawn.iter_mut()) {
        edge.change = match (&edge.from, &edge.to, edge.kind) {
            (Node::Module(a), Node::Module(b), _) => pairs
                .get(&(a, b))
                .map(|counts| delta(a, b, *counts))
                .into_iter()
                .collect(),
            (_, _, EdgeKind::Boundary) => edge
                .pairs
                .iter()
                .filter_map(|(a, b, _)| pairs.get(&(a, b)).map(|counts| delta(a, b, *counts)))
                .collect(),
            (Node::Container(a), Node::Container(b), _) => {
                let across: Vec<Delta> = pairs
                    .iter()
                    .filter(|((from, to), _)| &from.container == a && &to.container == b)
                    .map(|((from, to), counts)| delta(from, to, *counts))
                    .collect();
                if across.is_empty() {
                    packages
                        .get(&(a.as_str(), b.as_str()))
                        .map(|(before, after)| Delta {
                            from: a.clone(),
                            to: b.clone(),
                            before: *before,
                            after: *after,
                        })
                        .into_iter()
                        .collect()
                } else {
                    across
                }
            }
            _ => Vec::new(),
        };
    }
}

/// Gives each card the figure its mark shows, so the level is laid out with room for it.
pub(super) fn figures(scene: &mut Scene, marks: &Marks) {
    for card in &mut scene.cards {
        card.marked = Some(figure(marks, &card.node));
    }
}

/// Gives the level's "N more" card the total of what it hides: at a module level the changed
/// files and the dependents of files it does not list, at a file level the changed definitions
/// and dependents it does not list.
pub(super) fn more(marks: &mut Marks, scene: &Scene, review: &SystemReview, at: &SystemAt) {
    marks.remove(&Node::More);
    if scene.card(&Node::More).is_none() {
        return;
    }
    let listed = |node: Node| scene.card(&node).is_some();
    let mut mark = Mark::default();
    for row in &review.rows {
        match (at, row) {
            (
                SystemAt::Module { module },
                Row::File {
                    path,
                    module: holder,
                    added,
                    removed,
                    deleted: false,
                    ..
                },
            ) if holder == module && !listed(Node::File(path.clone())) => {
                mark.touched = true;
                mark.added += added;
                mark.removed += removed;
                mark.files += 1;
            }
            (
                SystemAt::Module { module },
                Row::Dependent {
                    dependent,
                    module: Some(holder),
                },
            ) if holder == module && !listed(Node::File(dependent.path.clone())) => {
                mark.dependents += 1;
            }
            (
                SystemAt::File { path },
                Row::Definition {
                    change,
                    path: holder,
                    name,
                    line,
                },
            ) if holder == path
                && *change != ChangeKind::Removed
                && !listed(Node::Definition {
                    line: *line,
                    name: name.clone(),
                }) =>
            {
                mark.touched = true;
            }
            (SystemAt::File { path }, Row::Dependent { dependent, .. })
                if &dependent.path == path
                    && !listed(Node::Definition {
                        line: dependent.line,
                        name: dependent.name.clone(),
                    }) =>
            {
                mark.dependents += 1;
            }
            _ => {}
        }
    }
    if mark != Mark::default() {
        marks.insert(Node::More, mark);
    }
}
