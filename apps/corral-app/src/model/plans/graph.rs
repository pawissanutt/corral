//! Dependency graph geometry for feed-backed plan members.

use super::Action;
use crate::model::Id;
use crate::model::board::Rail;
use crate::model::hue::Hue;

/// One coordinate in mockup graph units.
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

/// Internal SVG positions relative to a horizontal card's origin.
#[derive(Debug, Clone, PartialEq)]
pub struct CardGeometry {
    /// Id, title and now-phrase baselines.
    pub text: [Point; 3],
    /// Four rail-mark top-left positions, in stage order.
    pub rail: [Point; 4],
    /// Rail-mark width and height.
    pub rail_size: Point,
}

/// One graph card, or a compact row when `Graph::fits` is false.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Task id, also the first text line.
    pub id: Id,
    /// Card or row origin.
    pub x: f32,
    /// Card or row origin.
    pub y: f32,
    /// Card or row width.
    pub w: f32,
    /// Card or row height.
    pub h: f32,
    /// Id, title and now-phrase; card titles use the mockup's 21-character cutoff.
    pub lines: [String; 3],
    /// Untruncated title for accessibility and compact rows.
    pub title: String,
    /// Four delivery marks.
    pub rail: Rail,
    /// Internal card geometry; absent for compact rows laid out as rows.
    pub card: Option<CardGeometry>,
    /// Actor colour, independent of selection.
    pub hue: Hue,
    /// Colour of the now-phrase, including stale results.
    pub now_hue: Hue,
    /// Neutral selection emphasis.
    pub selected: bool,
    /// Needs-you card treatment and question target.
    pub needs_you: bool,
    /// Press target; compact rows open tasks, cards select members.
    pub action: Action,
}

/// A layer heading and its ordered members.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// Zero-based dependency layer number.
    pub index: usize,
    /// Full rendered heading, including compact dependency wording.
    pub label: String,
    /// Heading origin (baseline for horizontal SVG headings, top for compact headings).
    pub position: Point,
    /// Members in this layer, in source order.
    pub members: Vec<Id>,
}

/// Complete graph geometry. Render compact rows when `fits` is false; no geometry is deferred.
#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    /// True only if every 172-wide card layer plus 40-wide gaps fits.
    pub fits: bool,
    /// Full graph or compact-row width.
    pub width: f32,
    /// Full graph or compact-row height.
    pub height: f32,
    /// Ordered layer headings and membership.
    pub layers: Vec<Layer>,
    /// Ordered cards or compact rows with their press targets.
    pub nodes: Vec<Node>,
    /// Cubic dependency segments; empty for compact vertical layers.
    pub edges: Vec<Edge>,
}

/// One member as the graph draws it, whichever source supplied it.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphMember {
    pub id: Id,
    /// Full task title.
    pub title: String,
    /// The now-phrase under the title.
    pub now: String,
    pub rail: Rail,
    /// Actor colour.
    pub hue: Hue,
    /// Colour of the now-phrase.
    pub now_hue: Hue,
    /// Owner action needed: the needs-you card treatment.
    pub needs_you: bool,
    /// Zero-based dependency layer.
    pub layer: usize,
    /// What a card press does (select the member, or open its question).
    pub select: Action,
    /// What a compact row press does (open the task, or its question).
    pub open: Action,
}

/// Members in source order and their prerequisite → dependent edges.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GraphInput {
    pub members: Vec<GraphMember>,
    pub edges: Vec<(Id, Id)>,
}

/// Port PLANGRAPH's fit calculation, card positions, dependency curves and vertical fallback.
/// `phone` changes compact row height from 28 to 40; it does not change the source fit equation.
/// Pass a narrow width for terminal vertical layers, as the screen builder does.
pub fn layout(input: &GraphInput, selected: Option<&str>, width: f32, phone: bool) -> Graph {
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
    let cw = 172.0;
    let ch = 54.0;
    let gx = 40.0;
    let gy = 12.0;
    let required = layers.len() as f32 * cw + layers.len().saturating_sub(1) as f32 * gx;
    let fits = required <= width;
    let mut graph = Graph {
        fits,
        width: if fits { required } else { width.max(0.0) },
        height: 0.0,
        layers: Vec::with_capacity(layers.len()),
        nodes: Vec::with_capacity(input.members.len()),
        edges: Vec::new(),
    };
    let mut compact_y = 0.0;
    for (li, members) in layers.iter().enumerate() {
        let heading_y = if fits {
            10.0
        } else {
            if li > 0 {
                compact_y += 10.0;
            }
            compact_y
        };
        graph.layers.push(Layer {
            index: li,
            label: if fits {
                format!("Layer {li}")
            } else if li == 0 {
                "Layer 0 · can start now".into()
            } else {
                format!("Layer {li} · after layer {}", li - 1)
            },
            position: Point {
                x: if fits { li as f32 * (cw + gx) } else { 0.0 },
                y: heading_y,
            },
            members: members.iter().map(|member| member.id.clone()).collect(),
        });
        if !fits {
            compact_y += 10.5 * if phone { 1.4 } else { 1.45 } + 4.0;
        }
        for (mi, member) in members.iter().enumerate() {
            let h = if fits {
                ch
            } else if phone {
                40.0
            } else {
                28.0
            };
            let y = if fits {
                18.0 + mi as f32 * (ch + gy)
            } else {
                compact_y
            };
            let title = if fits && member.title.chars().count() > 21 {
                format!("{}…", member.title.chars().take(20).collect::<String>())
            } else {
                member.title.clone()
            };
            graph.nodes.push(Node {
                id: member.id.clone(),
                x: if fits { li as f32 * (cw + gx) } else { 0.0 },
                y,
                w: if fits { cw } else { graph.width },
                h,
                lines: [member.id.to_string(), title, member.now.clone()],
                title: member.title.clone(),
                rail: member.rail,
                card: fits.then(|| CardGeometry {
                    text: [
                        Point { x: 10.0, y: 17.0 },
                        Point { x: 10.0, y: 33.0 },
                        Point { x: 10.0, y: 47.0 },
                    ],
                    rail: [
                        Point {
                            x: cw - 64.0,
                            y: 7.0,
                        },
                        Point {
                            x: cw - 50.0,
                            y: 7.0,
                        },
                        Point {
                            x: cw - 36.0,
                            y: 7.0,
                        },
                        Point {
                            x: cw - 22.0,
                            y: 7.0,
                        },
                    ],
                    rail_size: Point { x: 11.0, y: 11.0 },
                }),
                hue: member.hue,
                now_hue: member.now_hue,
                selected: selected == Some(member.id.as_str()),
                needs_you: member.needs_you,
                action: if fits {
                    member.select.clone()
                } else {
                    member.open.clone()
                },
            });
            if fits {
                graph.height = graph.height.max(18.0 + (mi + 1) as f32 * (ch + gy));
            } else {
                compact_y += h;
            }
        }
    }
    if !fits {
        graph.height = compact_y;
        return graph;
    }
    for (dep, id) in &input.edges {
        let (Some(from), Some(node)) = (
            graph.nodes.iter().find(|node| node.id == *dep),
            graph.nodes.iter().find(|node| node.id == *id),
        ) else {
            continue;
        };
        graph.edges.push(Edge {
            from: from.id.clone(),
            to: node.id.clone(),
            start: Point {
                x: from.x + cw,
                y: from.y + ch / 2.0,
            },
            control1: Point {
                x: from.x + cw + gx / 2.0,
                y: from.y + ch / 2.0,
            },
            control2: Point {
                x: node.x - gx / 2.0,
                y: node.y + ch / 2.0,
            },
            end: Point {
                x: node.x,
                y: node.y + ch / 2.0,
            },
        });
    }
    graph
}

#[cfg(test)]
mod tests {
    use super::{GraphInput, GraphMember, layout};
    use crate::model::Id;
    use crate::model::board::Mark;
    use crate::model::hue::Hue;
    use crate::model::plans::{Action, LiveAction};

    #[test]
    fn insufficient_width_uses_vertical_layers() {
        let member = |id: &str, layer| GraphMember {
            id: Id::from(id),
            title: id.into(),
            now: String::new(),
            rail: [Mark::Next; 4],
            hue: Hue::Neutral,
            now_hue: Hue::Neutral,
            needs_you: false,
            layer,
            select: Action::Live(LiveAction::Task(Id::from(id))),
            open: Action::Live(LiveAction::Task(Id::from(id))),
        };
        let members = GraphInput {
            members: vec![member("T-1", 0), member("T-2", 1)],
            edges: vec![(Id::from("T-1"), Id::from("T-2"))],
        };
        let wide = layout(&members, Some("T-1"), 384.0, false);
        let narrow = layout(&members, Some("T-1"), 383.0, false);
        assert!(wide.fits);
        assert!(!narrow.fits);
        assert!(narrow.edges.is_empty());
        assert_eq!(narrow.nodes.len(), members.members.len());
        assert!(narrow.nodes.windows(2).all(|nodes| nodes[0].y < nodes[1].y));
    }
}
