//! Dependency graph geometry from PLANGRAPH. The compact fallback has rows, not edges.

use super::{Action, Plan};
use crate::model::board::{Actor, Rail, task};
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
    pub from: &'static str,
    /// Dependent task id.
    pub to: &'static str,
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
    pub id: &'static str,
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
    pub title: &'static str,
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
    pub members: Vec<&'static str>,
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

/// Port PLANGRAPH's fit calculation, card positions, dependency curves and vertical fallback.
/// `phone` changes compact row height from 28 to 40; it does not change the source fit equation.
/// Pass a narrow width for terminal vertical layers, as the screen builder does.
pub fn layout(p: &Plan, selected: &str, width: f32, phone: bool) -> Graph {
    let fallback = [p.members];
    let layers = if p.layers.is_empty() {
        &fallback[..]
    } else {
        p.layers
    };
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
        nodes: Vec::with_capacity(p.members.len()),
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
            members: members.to_vec(),
        });
        if !fits {
            compact_y += 10.5 * if phone { 1.4 } else { 1.45 } + 4.0;
        }
        for (mi, id) in members.iter().copied().enumerate() {
            let t = task(id);
            let needs_you = t.actor == Actor::You;
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
            let title = if fits && t.title.chars().count() > 21 {
                format!("{}…", t.title.chars().take(20).collect::<String>())
            } else {
                t.title.into()
            };
            graph.nodes.push(Node {
                id,
                x: if fits { li as f32 * (cw + gx) } else { 0.0 },
                y,
                w: if fits { cw } else { graph.width },
                h,
                lines: [id.into(), title, t.now.into()],
                title: t.title,
                rail: t.rail,
                card: if fits {
                    Some(CardGeometry {
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
                    })
                } else {
                    None
                },
                hue: t.actor.hue(),
                now_hue: if needs_you {
                    Hue::You
                } else if t.now.contains("stale") {
                    Hue::Held
                } else {
                    Hue::Neutral
                },
                selected: id == selected,
                needs_you,
                action: if needs_you {
                    Action::Question
                } else if fits {
                    Action::SelectMember(id)
                } else {
                    Action::OpenTask(id)
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
    for node in &graph.nodes {
        for (dep, _) in task(node.id).needs {
            if let Some(from) = graph.nodes.iter().find(|from| from.id == *dep) {
                graph.edges.push(Edge {
                    from: from.id,
                    to: node.id,
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
        }
    }
    graph
}

#[cfg(test)]
mod tests {
    use super::layout;
    use crate::model::plans::plan;

    #[test]
    fn insufficient_width_uses_vertical_layers() {
        let p = plan("T-870");
        let wide = layout(p, "T-868", 384.0, false);
        let narrow = layout(p, "T-868", 383.0, false);
        assert!(wide.fits);
        assert!(!narrow.fits);
        assert!(narrow.edges.is_empty());
        assert_eq!(narrow.nodes.len(), p.members.len());
        assert!(narrow.nodes.windows(2).all(|nodes| nodes[0].y < nodes[1].y));
    }
}
