//! The phone's and the terminal's form of the map: the same nodes as a list in the map's order
//! (the connected ones, then the band), each with its overlay figure and a bar, and under the
//! picked node its edges, each of which picks that edge. A list keeps every name whole and
//! every edge readable where a drawing of thirty boxes could not (docs/plans/system-map.md §3.1).

use super::scene::{Card, EdgeKind, Scene};
use super::words::{activity, busiest, figure, read_shares};
use super::{Node, Overlay, Pick, State, Window};

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// A part of the level: "Packages that reference this one", "No package reference found".
    Section(&'static str),
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
        Node::More => String::new(),
    };
    let hay = format!("{} {}", card.name, path).to_lowercase();
    filter.split_whitespace().all(|word| hay.contains(word))
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

pub fn lines(scene: &Scene, state: &State) -> Vec<Line> {
    let mut out = Vec::new();
    let picked_node = match &state.picked {
        Some(Pick::Node(node)) => Some(node),
        _ => None,
    };
    let picked_edge = match &state.picked {
        Some(Pick::Edge(a, b)) => Some((a, b)),
        _ => None,
    };
    let parts: [(Option<&'static str>, &[usize]); 4] = [
        (
            (!scene.top.is_empty()).then_some("Packages that reference this one"),
            &scene.top,
        ),
        (
            (!scene.top.is_empty() || !scene.bottom.is_empty()).then_some("Modules"),
            &scene.graph,
        ),
        (
            (!scene.bottom.is_empty()).then_some("Packages this one references"),
            &scene.bottom,
        ),
        (
            (!scene.graph.is_empty())
                .then_some(scene.band_title)
                .flatten(),
            &scene.band,
        ),
    ];
    for (title, part) in parts {
        let cards: Vec<&Card> = part
            .iter()
            .filter_map(|i| scene.cards.get(*i))
            .filter(|c| matches(c, &state.filter))
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
                figure: figure(card, state.overlay, state.window),
                share: share(scene, card, state.overlay, state.window).clamp(0.0, 1.0),
                picked: picked_node == Some(&card.node),
                opens: card.node.opens().is_some(),
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
                EdgeKind::ReferencedOnly => format!("not declared · {refs}"),
                EdgeKind::Module | EdgeKind::Boundary => refs,
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
