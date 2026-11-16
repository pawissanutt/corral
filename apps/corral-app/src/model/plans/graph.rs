//! Dependency graph geometry for feed-backed plan members: member cards in dependency layers,
//! left to right, joined by curves, or, where the layers do not fit, the layers stacked.

use super::Action;
use crate::model::Id;
use crate::model::glance::StageRail;
use crate::model::hue::Hue;

/// A member card (`.mcard`): 236 wide, its id and rail over its title.
const CARD_WIDTH: f32 = 236.0;
const CARD_HEIGHT: f32 = 60.0;
/// Between layers (`.graph` gap) and between the cards of a layer (`.layer` gap).
const LAYER_GAP: f32 = 48.0;
const CARD_GAP: f32 = 12.0;

/// One coordinate in graph points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f32,
    /// Vertical coordinate.
    pub y: f32,
}

/// A dependency path, drawn behind cards as one cubic Bézier segment.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// Prerequisite task id.
    pub from: Id,
    /// Dependent task id.
    pub to: Id,
    /// Start at the prerequisite card's right centre.
    pub start: Point,
    /// First cubic control point.
    pub control1: Point,
    /// Second cubic control point.
    pub control2: Point,
    /// End at the dependent card's left centre.
    pub end: Point,
}

/// One member card, placed in its layer when the graph fits, stacked otherwise.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Task id, the card's first line.
    pub id: Id,
    /// Card origin.
    pub x: f32,
    /// Card origin.
    pub y: f32,
    /// Card width.
    pub w: f32,
    /// Card height.
    pub h: f32,
    /// Full title.
    pub title: String,
    /// The rail with its current stage named; its hint is the now-phrase.
    pub stage: StageRail,
    /// Actor colour, independent of selection.
    pub hue: Hue,
    /// Neutral selection emphasis.
    pub selected: bool,
    /// Needs-you card treatment and question target.
    pub needs_you: bool,
    /// Press target; stacked cards open tasks, placed cards select members.
    pub action: Action,
}

/// A layer and its ordered members.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// Zero-based dependency layer number.
    pub index: usize,
    /// The stacked layout's heading, saying what the layer waits for.
    pub label: String,
    /// Members in this layer, in source order.
    pub members: Vec<Id>,
}

/// Complete graph geometry; stacked layers when `fits` is false.
#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    /// True only if every layer of cards and the gaps between them fit.
    pub fits: bool,
    /// Full graph or stacked width.
    pub width: f32,
    /// Full graph or stacked height.
    pub height: f32,
    /// Ordered layers and membership.
    pub layers: Vec<Layer>,
    /// Ordered cards with their press targets.
    pub nodes: Vec<Node>,
    /// Cubic dependency segments; empty for stacked layers.
    pub edges: Vec<Edge>,
}

/// One member as the graph draws it, whichever source supplied it.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphMember {
    pub id: Id,
    /// Full task title.
    pub title: String,
    /// The rail with its current stage named.
    pub stage: StageRail,
    /// Actor colour.
    pub hue: Hue,
    /// Owner action needed: the needs-you card treatment.
    pub needs_you: bool,
    /// Zero-based dependency layer.
    pub layer: usize,
    /// What a placed card press does (select the member, or open its question).
    pub select: Action,
    /// What a stacked card press does (open the task, or its question).
    pub open: Action,
}

/// Members in source order and their prerequisite → dependent edges.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GraphInput {
    pub members: Vec<GraphMember>,
    pub edges: Vec<(Id, Id)>,
}

/// Places the layers left to right when they fit `width`, each layer's cards centred on the
/// tallest layer, with a curve from each prerequisite's right edge to its dependent's left;
/// otherwise stacks the layers top to bottom under their headings. A terminal passes no width.
pub fn layout(input: &GraphInput, selected: Option<&str>, width: f32) -> Graph {
    let count = input
        .members
        .iter()
        .map(|member| member.layer + 1)
        .max()
        .unwrap_or(0);
    let layers: Vec<Vec<&GraphMember>> = (0..count)
        .map(|li| {
            input
                .members
                .iter()
                .filter(|member| member.layer == li)
                .collect()
        })
        .collect();
    let required =
        layers.len() as f32 * CARD_WIDTH + layers.len().saturating_sub(1) as f32 * LAYER_GAP;
    let fits = required <= width;
    let column =
        |cards: usize| cards as f32 * CARD_HEIGHT + cards.saturating_sub(1) as f32 * CARD_GAP;
    let tallest = layers
        .iter()
        .map(|members| column(members.len()))
        .fold(0.0, f32::max);
    let mut graph = Graph {
        fits,
        width: if fits { required } else { width.max(0.0) },
        height: if fits { tallest } else { 0.0 },
        layers: Vec::with_capacity(layers.len()),
        nodes: Vec::with_capacity(input.members.len()),
        edges: Vec::new(),
    };
    let mut stacked_y = 0.0;
    for (li, members) in layers.iter().enumerate() {
        graph.layers.push(Layer {
            index: li,
            label: if li == 0 {
                "Layer 0 · can start now".into()
            } else {
                format!("Layer {li} · after layer {}", li - 1)
            },
            members: members.iter().map(|member| member.id.clone()).collect(),
        });
        let top = (tallest - column(members.len())) / 2.0;
        for (mi, member) in members.iter().enumerate() {
            let y = if fits {
                top + mi as f32 * (CARD_HEIGHT + CARD_GAP)
            } else {
                stacked_y
            };
            graph.nodes.push(Node {
                id: member.id.clone(),
                x: if fits {
                    li as f32 * (CARD_WIDTH + LAYER_GAP)
                } else {
                    0.0
                },
                y,
                w: if fits { CARD_WIDTH } else { graph.width },
                h: CARD_HEIGHT,
                title: member.title.clone(),
                stage: member.stage.clone(),
                hue: member.hue,
                selected: selected == Some(member.id.as_str()),
                needs_you: member.needs_you,
                action: if fits {
                    member.select.clone()
                } else {
                    member.open.clone()
                },
            });
            if !fits {
                stacked_y += CARD_HEIGHT + CARD_GAP;
            }
        }
    }
    if !fits {
        graph.height = stacked_y;
        return graph;
    }
    for (dep, id) in &input.edges {
        let (Some(from), Some(node)) = (
            graph.nodes.iter().find(|node| node.id == *dep),
            graph.nodes.iter().find(|node| node.id == *id),
        ) else {
            continue;
        };
        let (from_y, to_y) = (from.y + CARD_HEIGHT / 2.0, node.y + CARD_HEIGHT / 2.0);
        let middle = (from.x + CARD_WIDTH + node.x) / 2.0;
        graph.edges.push(Edge {
            from: from.id.clone(),
            to: node.id.clone(),
            start: Point {
                x: from.x + CARD_WIDTH,
                y: from_y,
            },
            control1: Point {
                x: middle,
                y: from_y,
            },
            control2: Point { x: middle, y: to_y },
            end: Point { x: node.x, y: to_y },
        });
    }
    graph
}

#[cfg(test)]
mod tests {
    use super::{GraphInput, GraphMember, layout};
    use crate::model::Id;
    use crate::model::board::{Actor, Mark};
    use crate::model::glance::StageRail;
    use crate::model::hue::Hue;
    use crate::model::plans::{Action, LiveAction};

    #[test]
    fn insufficient_width_uses_vertical_layers() {
        let member = |id: &str, layer| GraphMember {
            id: Id::from(id),
            title: id.into(),
            stage: StageRail::of([Mark::Next; 4], Actor::Queued, ""),
            hue: Hue::Neutral,
            needs_you: false,
            layer,
            select: Action::Live(LiveAction::Task(Id::from(id))),
            open: Action::Live(LiveAction::Task(Id::from(id))),
        };
        let members = GraphInput {
            members: vec![member("T-1", 0), member("T-2", 1)],
            edges: vec![(Id::from("T-1"), Id::from("T-2"))],
        };
        let wide = layout(&members, Some("T-1"), 520.0);
        let narrow = layout(&members, Some("T-1"), 519.0);
        assert!(wide.fits);
        assert!(!narrow.fits);
        assert!(narrow.edges.is_empty());
        assert_eq!(narrow.nodes.len(), members.members.len());
        assert!(narrow.nodes.windows(2).all(|nodes| nodes[0].y < nodes[1].y));
    }
}
