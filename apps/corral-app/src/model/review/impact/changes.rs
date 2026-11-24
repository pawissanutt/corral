//! What the change did to the level shown beyond its nodes: the reference counts it moved on
//! the level's edges, and what the level's "N more" card hides.

use std::collections::BTreeMap;

use corral_types::{
    ChangeKind, ContainerFact, ModuleId, PackageEdge, PackageEdgeKind, SystemAt, SystemMapRow,
    SystemReview, SystemReviewRow as Row,
};

use super::items::module_words;
use crate::model::system::Node;
use crate::model::system::marks::{Delta, Mark, Marks, figure};
use crate::model::system::scene::{Edge, EdgeKind, Scene, port_card};

/// Gives each edge of the level `at` shows the module pairs whose reference count the change
/// moved: a module reference its own pair, a boundary edge the pairs it sums, a package edge
/// the pairs across it or, where none is listed, the package edge's own count. A pair whose
/// edge the head no longer has (its count fell to none) gets an edge of its own between the
/// level's cards, and a changed edge the level left undrawn to stay legible is drawn, so no
/// change is lost to either.
pub(super) fn edges(scene: &mut Scene, review: &SystemReview, at: Option<&SystemAt>) {
    let shown = match at {
        None => None,
        Some(SystemAt::Container { path }) => Some(path.as_str()),
        Some(_) => return,
    };
    let refs = |edge: &Option<PackageEdge>| edge.as_ref().map_or(0, |e| e.references);
    // Each change by the two cards it joins, with the kind a new edge between them takes.
    let mut moved: Vec<((Node, Node), EdgeKind, Delta)> = Vec::new();
    let mut packages: BTreeMap<(Node, Node), (EdgeKind, Delta)> = BTreeMap::new();
    let mut facts: BTreeMap<&str, &ContainerFact> = BTreeMap::new();
    for row in &review.rows {
        match row {
            Row::Map {
                row: SystemMapRow::Container { container, .. },
            } => {
                facts.insert(container.path.as_str(), container);
            }
            Row::ModuleReference {
                from,
                to,
                before,
                after,
            } if before != after => {
                let ends = match shown {
                    None => (
                        Node::Container(from.container.clone()),
                        Node::Container(to.container.clone()),
                    ),
                    Some(path) => (end(from, path, true), end(to, path, false)),
                };
                if ends.0 == ends.1 {
                    continue;
                }
                let kind = match &ends {
                    (Node::Module(_), Node::Module(_)) => EdgeKind::Module,
                    (Node::Container(_), _) => EdgeKind::Declared,
                    _ => EdgeKind::Boundary,
                };
                let delta = Delta {
                    from: module_words(from),
                    to: module_words(to),
                    before: *before,
                    after: *after,
                };
                moved.push((ends, kind, delta));
            }
            Row::PackageEdge { before, after }
                if shown.is_none() && refs(before) != refs(after) =>
            {
                let Some(edge) = after.as_ref().or(before.as_ref()) else {
                    continue;
                };
                let ends = (
                    Node::Container(edge.from.clone()),
                    Node::Container(edge.to.clone()),
                );
                let kind = match edge.kind {
                    PackageEdgeKind::DeclaredAndReferenced => EdgeKind::Declared,
                    PackageEdgeKind::DeclaredOnly => EdgeKind::DeclaredOnly,
                    PackageEdgeKind::ReferencedOnly => EdgeKind::ReferencedOnly,
                };
                let delta = Delta {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    before: refs(before),
                    after: refs(after),
                };
                packages.insert(ends, (kind, delta));
            }
            _ => {}
        }
    }
    // The head's package level lists a neighbour only while a reference or a declaration
    // joins them, so one whose last reference the change removed has no port: it gets one.
    if shown.is_some() {
        for ((from, to), ..) in &moved {
            for node in [from, to] {
                let Node::Port { path, into } = node else {
                    continue;
                };
                let inside = if *into { to } else { from };
                if scene.card(node).is_some() || scene.card(inside).is_none() {
                    continue;
                }
                if let Some(fact) = facts.get(path.as_str()) {
                    let side = if *into {
                        &mut scene.top
                    } else {
                        &mut scene.bottom
                    };
                    side.push(scene.cards.len());
                    scene.cards.push(port_card(fact, *into));
                }
            }
        }
    }
    let listed = |node: &Node| scene.card(node).is_some();
    let mut joined: BTreeMap<(Node, Node), (EdgeKind, Vec<Delta>)> = BTreeMap::new();
    for (ends, kind, delta) in moved {
        if listed(&ends.0) && listed(&ends.1) {
            joined
                .entry(ends)
                .or_insert((kind, Vec::new()))
                .1
                .push(delta);
        }
    }
    // A package edge whose module pairs are not listed says its own count.
    for (ends, (kind, delta)) in packages {
        if listed(&ends.0) && listed(&ends.1) {
            joined.entry(ends).or_insert_with(|| (kind, vec![delta]));
        }
    }
    for edge in scene.edges.iter_mut().chain(scene.undrawn.iter_mut()) {
        let key = (edge.from.clone(), edge.to.clone());
        edge.change = joined.remove(&key).map(|(_, d)| d).unwrap_or_default();
    }
    for ((from, to), (kind, change)) in joined {
        scene.edges.push(Edge {
            from,
            to,
            kind,
            count: change.iter().map(|d| d.after).sum(),
            basis: None,
            pairs: Vec::new(),
            sites: Vec::new(),
            change,
        });
    }
    let (changed, quiet): (Vec<Edge>, Vec<Edge>) = std::mem::take(&mut scene.undrawn)
        .into_iter()
        .partition(|e| !e.change.is_empty());
    scene.undrawn = quiet;
    scene.edges.extend(changed);
    // A card an edge now joins leaves the band of unconnected cards for the graph.
    let ends: Vec<&Node> = scene.edges.iter().flat_map(|e| [&e.from, &e.to]).collect();
    let (joined, band): (Vec<usize>, Vec<usize>) = scene.band.iter().partition(|i| {
        scene
            .cards
            .get(**i)
            .is_some_and(|c| ends.contains(&&c.node))
    });
    for i in joined {
        match scene.cards[i].node {
            Node::Port { into: true, .. } => scene.top.push(i),
            Node::Port { into: false, .. } => scene.bottom.push(i),
            _ => scene.graph.push(i),
        }
    }
    scene.band = band;
}

/// A module's card on the level of package `path`: the module itself inside it, else the
/// neighbouring package's port on the side the reference crosses.
fn end(module: &ModuleId, path: &str, into: bool) -> Node {
    if module.container == path {
        Node::Module(module.clone())
    } else {
        Node::Port {
            path: module.container.clone(),
            into,
        }
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
                mark.file_added += added;
                mark.file_removed += removed;
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
