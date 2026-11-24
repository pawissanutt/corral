//! The phone's and the terminal's form of the map: the same nodes as a list in the map's order
//! (the connected ones, then the band), each with its overlay figure and a bar, and under the
//! picked node its edges, each of which picks that edge. A list keeps every name whole and
//! every edge readable where a drawing of thirty boxes could not (docs/plans/system-map.md §3.1).

use super::marks::{self, Marks};
use super::runtime::{at, edge_words, entry_word, group};
use super::scene::{Card, EdgeKind, Kind, Level, Scene};
use super::words::{activity, busiest, figure, read_shares};
use super::{Node, Overlay, Pick, State, Window};

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// A part of the level: "Packages that reference this one", "No package reference found",
    /// a container on the Data view.
    Section(String),
    Node(NodeLine),
    /// One of the picked node's edges.
    Edge(EdgeLine),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeLine {
    pub node: Node,
    pub name: String,
    pub tag: Option<String>,
    pub figure: String,
    /// The overlay's share, 0 to 1, drawn as a bar.
    pub share: f32,
    pub picked: bool,
    pub opens: bool,
    /// Whether the row draws the overlay's bar; the Runtime view has none.
    pub bar: bool,
    /// Untouched by the change a task page's Impact shows.
    pub dim: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeLine {
    /// `→` for an edge out of the picked node, `←` for one into it.
    pub arrow: &'static str,
    pub other: Node,
    pub name: String,
    /// None for a process the node spawns.
    pub kind: Option<EdgeKind>,
    pub words: String,
    pub pick: Option<Pick>,
    pub picked: bool,
}

/// Whether a card matches the filter: every word in its name or path, ignoring case.
pub fn matches(card: &Card, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    if filter.is_empty() {
        return true;
    }
    let path = match &card.node {
        Node::Container(p) | Node::Port { path: p, .. } | Node::File(p) => p.clone(),
        Node::Module(m) => format!("{}/{}", m.container, m.path),
        Node::Definition { name, .. } => name.clone(),
        Node::Unit(id) => id.path.clone(),
        Node::Type(id) => id.path.clone(),
        Node::More | Node::Scripts => String::new(),
    };
    let hay = format!("{} {}", card.name, path).to_lowercase();
    filter.split_whitespace().all(|word| hay.contains(word))
}

/// Whether the filter leaves a card of this scene shown. The Data view's scene is built from the
/// filter already, its counted rest included, so it is not filtered again.
pub fn passes(scene: &Scene, card: &Card, filter: &str) -> bool {
    scene.level == Level::Data || matches(card, filter)
}

/// How many of the level's cards the filter matches, of how many.
pub fn matched(scene: &Scene, filter: &str) -> (usize, usize) {
    let total = scene.cards.len();
    (
        scene.cards.iter().filter(|c| matches(c, filter)).count(),
        total,
    )
}

/// The overlay's share of a card, for its bar.
pub fn share(scene: &Scene, card: &Card, overlay: Overlay, window: Window) -> f32 {
    match overlay {
        Overlay::Structure => card.lines as f32 / scene.max_lines as f32,
        Overlay::Activity => activity(card, window)
            .map_or(0.0, |a| (a.0 as f32 / busiest(scene, window) as f32).sqrt()),
        Overlay::Coverage => read_shares(card).map_or(0.0, |s| s.0),
    }
}

/// The level's rows. Under `marks` (a task's Impact) each node's figure is what the change did
/// to it, its bar its share of the most churned, and an unmarked node is dimmed.
pub fn lines(scene: &Scene, state: &State, marks: Option<&Marks>) -> Vec<Line> {
    let churn = |node: &Node| {
        marks
            .and_then(|marks| marks::of(marks, node))
            .map_or(0, |mark| mark.churn())
    };
    let most = scene
        .cards
        .iter()
        .map(|card| churn(&card.node))
        .max()
        .unwrap_or(0)
        .max(1);
    let mut out = Vec::new();
    let picked_node = match &state.picked {
        Some(Pick::Node(node)) => Some(node),
        _ => None,
    };
    let picked_edge = match &state.picked {
        Some(Pick::Edge(a, b)) => Some((a, b)),
        _ => None,
    };
    let parts: Vec<(Option<String>, Vec<usize>)> = if scene.level == Level::Data {
        // The scene keeps the types in container order; each band's caption heads its part.
        let mut parts: Vec<(Option<String>, Vec<usize>)> = Vec::new();
        let ranks = scene.ranks.clone().unwrap_or_default();
        let mut last: Option<Option<i32>> = None;
        for (i, card) in scene.cards.iter().enumerate() {
            let rank = ranks.get(i).copied();
            if card.kind == Kind::More {
                parts.push((None, vec![i]));
            } else if last == Some(rank) {
                if let Some((_, list)) = parts.last_mut() {
                    list.push(i);
                }
            } else {
                let title = rank.and_then(|r| scene.rank_titles.get(&r).cloned());
                parts.push((title, vec![i]));
            }
            last = Some(rank);
        }
        parts
    } else if scene.level == Level::Runtime {
        // Every card under its kind, connected or not; the edges show under the pick.
        let mut all: Vec<usize> = (0..scene.cards.len()).collect();
        all.sort_by(|a, b| {
            let key = |i: usize| (group(scene.cards[i].kind).0, &scene.cards[i].name);
            key(*a).cmp(&key(*b))
        });
        let mut parts: Vec<(Option<String>, Vec<usize>)> = Vec::new();
        for i in all {
            let title = group(scene.cards[i].kind).1;
            match parts.last_mut() {
                Some((Some(last), list)) if last == title => list.push(i),
                _ => parts.push((Some(title.to_owned()), vec![i])),
            }
        }
        parts
    } else {
        vec![
            (
                (!scene.top.is_empty()).then(|| "Packages that reference this one".into()),
                scene.top.clone(),
            ),
            (
                (!scene.top.is_empty() || !scene.bottom.is_empty()).then(|| "Modules".into()),
                scene.graph.clone(),
            ),
            (
                (!scene.bottom.is_empty()).then(|| "Packages this one references".into()),
                scene.bottom.clone(),
            ),
            (
                (!scene.graph.is_empty())
                    .then_some(scene.band_title)
                    .flatten()
                    .map(str::to_owned),
                scene.band.clone(),
            ),
        ]
    };
    for (title, part) in parts {
        let cards: Vec<&Card> = part
            .iter()
            .filter_map(|i| scene.cards.get(*i))
            .filter(|c| passes(scene, c, &state.filter))
            .collect();
        if cards.is_empty() {
            continue;
        }
        if let Some(title) = title {
            out.push(Line::Section(title));
        }
        for card in cards {
            let picked = picked_node == Some(&card.node)
                || picked_edge.is_some_and(|(a, b)| a == &card.node || b == &card.node);
            out.push(Line::Node(NodeLine {
                node: card.node.clone(),
                name: card.name.clone(),
                tag: card.tag.clone(),
                figure: match marks {
                    Some(marks) => marks::figure(marks, &card.node),
                    None => figure(card, state.overlay, state.window),
                },
                share: match marks {
                    Some(_) => churn(&card.node) as f32 / most as f32,
                    None => share(scene, card, state.overlay, state.window),
                }
                .clamp(0.0, 1.0),
                picked: picked_node == Some(&card.node),
                bar: !card.kind.barless(),
                opens: if scene.level == Level::Runtime {
                    card.kind == Kind::Scripts
                } else if scene.level == Level::Data {
                    false
                } else {
                    card.node.opens().is_some()
                },
                dim: marks.is_some_and(|marks| marks::of(marks, &card.node).is_none()),
            }));
            if picked {
                out.extend(edges(scene, &card.node, picked_edge));
                out.extend(card.spawns.iter().map(|(process, sites)| {
                    Line::Edge(EdgeLine {
                        arrow: "→",
                        other: Node::More,
                        name: process.clone(),
                        kind: None,
                        words: format!("spawns · {}", super::plural(*sites, "site", "sites")),
                        pick: None,
                        picked: false,
                    })
                }));
                out.extend(card.entries.iter().map(|entry| {
                    Line::Edge(EdgeLine {
                        arrow: "▸",
                        other: Node::More,
                        name: entry.name.clone(),
                        kind: None,
                        words: format!(
                            "{} · {}",
                            entry_word(entry.kind),
                            at(&entry.path, entry.line)
                        ),
                        pick: None,
                        picked: false,
                    })
                }));
            }
        }
    }
    out
}

fn edges(scene: &Scene, node: &Node, picked: Option<(&Node, &Node)>) -> Vec<Line> {
    let mut list: Vec<(&'static str, &super::scene::Edge)> = scene
        .all_edges()
        .filter_map(|e| {
            if &e.from == node {
                Some(("→", e))
            } else if &e.to == node {
                Some(("←", e))
            } else {
                None
            }
        })
        .collect();
    list.sort_by(|a, b| a.0.cmp(b.0).then(b.1.count.cmp(&a.1.count)));
    list.into_iter()
        .map(|(arrow, e)| {
            let other = if arrow == "→" { &e.to } else { &e.from };
            let refs = super::plural(e.count, "ref", "refs");
            let words = match e.kind {
                EdgeKind::Declared => format!("declared · {refs}"),
                EdgeKind::DeclaredOnly => "declared · 0 refs".into(),
                EdgeKind::ReferencedOnly => format!("{refs} · no dependency match"),
                EdgeKind::Module | EdgeKind::Boundary => refs,
                EdgeKind::Has => has_words(scene, e),
                _ => edge_words(e),
            };
            Line::Edge(EdgeLine {
                arrow,
                other: other.clone(),
                name: scene.name(other),
                kind: Some(e.kind),
                words,
                pick: Some(Pick::Edge(e.from.clone(), e.to.clone())),
                picked: picked == Some((&e.from, &e.to)),
            })
        })
        .collect()
}

/// A has edge's fields: `has · field runs`, or how many.
fn has_words(scene: &Scene, edge: &super::scene::Edge) -> String {
    let Node::Type(to) = &edge.to else {
        return "has".into();
    };
    let fields: Vec<&str> = scene
        .card(&edge.from)
        .and_then(|card| card.data.as_ref())
        .map(|data| {
            data.holds
                .iter()
                .filter(|(other, _, _)| other == to)
                .map(|(_, field, _)| field.as_str())
                .collect()
        })
        .unwrap_or_default();
    match fields.as_slice() {
        [one] => format!("has · field {one}"),
        many => format!(
            "has · {}",
            super::plural(many.len() as u64, "field", "fields")
        ),
    }
}
