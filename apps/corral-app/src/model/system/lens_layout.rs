//! Places a lens for the canvas, computed from the lens alone; nothing is placed by hand. A
//! state lens goes through the map's layered layout (states ranked along their transitions,
//! rows ordered to reduce crossings), each transition's words set at its route's middle and
//! pushed clear of the cards and of each other; a transition from a state to itself loops on
//! the card's right. A sequence lens puts participants in columns as wide as their names and
//! far enough apart for the widest message between them, and each message on its own row in
//! file order, a note under its words. Text is measured by the caller's text engine.

use std::collections::BTreeMap;

use super::layered::{LEdge, layered};
use super::layout::{Font, Measure, Rect};
use super::lens::{Diagram, Element};
use corral_types::lenses::LensKind;

#[derive(Debug, Clone, PartialEq)]
pub struct NodeBox {
    /// Index in `Diagram.nodes`.
    pub node: usize,
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arrow {
    /// Index in `Diagram.edges`.
    pub edge: usize,
    /// The path, ending at the target.
    pub points: Vec<(f32, f32)>,
    /// The box of its words (and a guard's or note's line under them).
    pub label: Rect,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LensLayout {
    pub width: f32,
    pub height: f32,
    pub nodes: Vec<NodeBox>,
    pub arrows: Vec<Arrow>,
    /// A sequence's lifelines: x, from y, to y.
    pub lifelines: Vec<(f32, f32, f32)>,
}

const PAD: f32 = 12.0;
pub const NODE_H: f32 = 34.0;
/// One line of an edge's words.
pub const LINE_H: f32 = 15.0;
const LABEL_PAD: f32 = 8.0;
const LOOP: f32 = 26.0;

/// An edge's words: its label, then its guard or note.
pub fn label_lines(diagram: &Diagram, edge: usize) -> (String, Option<String>) {
    let Some(e) = diagram.edges.get(edge) else {
        return (String::new(), None);
    };
    let detail = e.detail.as_ref().map(|d| match diagram.kind {
        LensKind::State => format!("[{d}]"),
        LensKind::Sequence => d.clone(),
    });
    let lead = match diagram.kind {
        LensKind::Sequence => format!("{}. {}", edge + 1, e.label),
        LensKind::State => e.label.clone(),
    };
    let lead = match e.glyphs() {
        Some(glyphs) => format!("{glyphs} {lead}"),
        None => lead,
    };
    (lead, detail)
}

/// A state's or participant's words: its marks' glyphs, then its label.
pub fn node_words(e: &Element) -> String {
    match e.glyphs() {
        Some(glyphs) => format!("{glyphs} {}", e.label),
        None => e.label.clone(),
    }
}

fn label_size(diagram: &Diagram, edge: usize, measure: Measure) -> (f32, f32) {
    let (lead, detail) = label_lines(diagram, edge);
    let w =
        measure(Font::Label, &lead).max(detail.as_deref().map_or(0.0, |d| measure(Font::Sub, d)));
    let h = LINE_H * if detail.is_some() { 2.0 } else { 1.0 };
    ((w + 2.0 * LABEL_PAD).ceil(), h + 4.0)
}

pub fn layout(diagram: &Diagram, room: f32, measure: Measure) -> LensLayout {
    match diagram.kind {
        LensKind::State => state(diagram, room, measure),
        LensKind::Sequence => sequence(diagram, room, measure),
    }
}

fn node_width(label: &str, measure: Measure) -> f32 {
    (measure(Font::Name, label) + 32.0).max(84.0).ceil()
}

fn state(diagram: &Diagram, room: f32, measure: Measure) -> LensLayout {
    let sizes: Vec<(f32, f32)> = diagram
        .nodes
        .iter()
        .map(|n| (node_width(&node_words(n), measure), NODE_H))
        .collect();
    let edges: Vec<LEdge> = diagram
        .edges
        .iter()
        .enumerate()
        .filter_map(|(index, e)| {
            let (a, b) = e.ends?;
            Some(LEdge {
                index,
                a,
                b,
                pri: 1.0,
            })
        })
        .collect();
    let none = vec![false; sizes.len()];
    let g = layered(
        &sizes,
        &edges,
        &none,
        &none,
        (room - 2.0 * PAD).max(320.0),
        (false, false),
        None,
    );
    let mut out = LensLayout::default();
    for (node, rect) in g.pos.iter().enumerate() {
        out.nodes.push(NodeBox {
            node,
            rect: Rect {
                x: rect.x + PAD,
                y: rect.y + PAD,
                ..*rect
            },
        });
    }
    let mut width = g.width + 2.0 * PAD;
    let mut height = g.height + 2.0 * PAD;
    let mut taken: Vec<Rect> = out.nodes.iter().map(|n| n.rect).collect();
    // Transitions between the same two states (either way) share a route, and repeats on one
    // state share a loop: each takes its own lane so no stroke or mark hides another.
    let mut lanes: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    // Routes arrive by edge index; one from a state to itself has none.
    for (edge, e) in diagram.edges.iter().enumerate() {
        let Some((a, b)) = e.ends else { continue };
        let (w, h) = label_size(diagram, edge, measure);
        let lane = lanes.entry((a.min(b), a.max(b))).or_default();
        let k = *lane;
        *lane += 1;
        let (points, at) = if a == b {
            let Some(r) = out.nodes.get(a).map(|n| n.rect) else {
                continue;
            };
            let (x, cy) = (r.x + r.w, r.y + r.h / 2.0);
            let reach = LOOP + 14.0 * k as f32;
            let rise = 16.0 + 4.0 * k as f32;
            let points = vec![
                (x, cy - 8.0),
                (x + reach, cy - rise),
                (x + reach, cy + rise),
                (x, cy + 8.0),
            ];
            (points, (x + reach + 6.0 + w / 2.0, cy))
        } else {
            let Some((_, points)) = g.routes.iter().find(|(i, _)| *i == edge) else {
                continue;
            };
            let dx = lane_offset(k);
            let points: Vec<(f32, f32)> = points
                .iter()
                .map(|(x, y)| (x + PAD + dx, y + PAD))
                .collect();
            let at = middle(&points);
            (points, at)
        };
        let label = clear(
            Rect {
                x: at.0 - w / 2.0,
                y: at.1 - h / 2.0,
                w,
                h,
            },
            &taken,
        );
        taken.push(label);
        width = width.max(label.x + label.w + PAD);
        height = height.max(label.y + label.h + PAD);
        out.arrows.push(Arrow {
            edge,
            points,
            label,
        });
    }
    out.width = width.ceil();
    out.height = height.ceil();
    out
}

/// The sideways shift of the k-th transition between one pair of states: none, then
/// alternately right and left in widening steps.
fn lane_offset(k: usize) -> f32 {
    let step = 14.0 * k.div_ceil(2) as f32;
    if k % 2 == 1 { step } else { -step }
}

/// The middle of a path: halfway along its middle segment.
fn middle(points: &[(f32, f32)]) -> (f32, f32) {
    match points.len() {
        0 => (0.0, 0.0),
        1 => points[0],
        n => {
            let i = (n - 1) / 2;
            let (a, b) = (points[i], points[i + 1]);
            ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
        }
    }
}

/// The nearest place for `label` that overlaps nothing taken: tried right, left, down and up
/// in growing steps.
fn clear(label: Rect, taken: &[Rect]) -> Rect {
    let free = |r: &Rect| r.x >= 0.0 && r.y >= 0.0 && !taken.iter().any(|t| t.overlaps(r));
    if free(&label) {
        return label;
    }
    for step in 1..40 {
        let d = step as f32 * 8.0;
        for (dx, dy) in [(d, 0.0), (-d, 0.0), (0.0, d), (0.0, -d)] {
            let r = Rect {
                x: label.x + dx,
                y: label.y + dy,
                ..label
            };
            if free(&r) {
                return r;
            }
        }
    }
    label
}

const HEAD_GAP: f32 = 18.0;
const ROW: f32 = 30.0;
const COLUMN_GAP: f32 = 40.0;

fn sequence(diagram: &Diagram, _room: f32, measure: Measure) -> LensLayout {
    let n = diagram.nodes.len();
    let widths: Vec<f32> = diagram
        .nodes
        .iter()
        .map(|p| node_width(&node_words(p), measure))
        .collect();
    // Centres: each next column clears the boxes, then every message's words fit between its
    // ends; the nearest spans first so a wide one widens only the gap it crosses last.
    let mut centre = vec![0.0_f32; n];
    let mut x = PAD;
    for (i, w) in widths.iter().enumerate() {
        centre[i] = x + w / 2.0;
        x += w + COLUMN_GAP;
    }
    let mut spans: Vec<(usize, usize, f32)> = diagram
        .edges
        .iter()
        .enumerate()
        .filter_map(|(edge, e)| {
            let (a, b) = e.ends?;
            (a != b).then(|| {
                (
                    a.min(b),
                    a.max(b),
                    label_size(diagram, edge, measure).0 + 16.0,
                )
            })
        })
        .collect();
    spans.sort_by_key(|(a, b, _)| b - a);
    for (a, b, need) in spans {
        let short = need - (centre[b] - centre[a]);
        if short > 0.0 {
            for c in centre.iter_mut().skip(b) {
                *c += short;
            }
        }
    }
    let mut out = LensLayout::default();
    let mut width: f32 = 0.0;
    for (node, (c, w)) in centre.iter().zip(&widths).enumerate() {
        let rect = Rect {
            x: c - w / 2.0,
            y: PAD,
            w: *w,
            h: NODE_H,
        };
        width = width.max(rect.x + rect.w + PAD);
        out.nodes.push(NodeBox { node, rect });
    }
    let mut y = PAD + NODE_H + HEAD_GAP;
    for (edge, e) in diagram.edges.iter().enumerate() {
        let Some((a, b)) = e.ends else { continue };
        let (w, h) = label_size(diagram, edge, measure);
        let top = y;
        let (points, label) = if a == b {
            let x = centre[a];
            let line = top + h + 2.0;
            let points = vec![
                (x, line),
                (x + LOOP, line),
                (x + LOOP, line + 14.0),
                (x, line + 14.0),
            ];
            let label = Rect {
                x: x + 6.0,
                y: top,
                w,
                h,
            };
            y = line + 14.0 + ROW - 10.0;
            (points, label)
        } else {
            let (x0, x1) = (centre[a], centre[b]);
            let line = top + h + 2.0;
            let label = Rect {
                x: (x0 + x1) / 2.0 - w / 2.0,
                y: top,
                w,
                h,
            };
            y = line + ROW - 6.0;
            (vec![(x0, line), (x1, line)], label)
        };
        width = width.max(label.x + label.w + PAD);
        width = width.max(points.iter().map(|p| p.0).fold(0.0, f32::max) + PAD);
        out.arrows.push(Arrow {
            edge,
            points,
            label,
        });
    }
    let bottom = y;
    out.lifelines = centre.iter().map(|c| (*c, PAD + NODE_H, bottom)).collect();
    out.width = width.ceil();
    out.height = (bottom + PAD).ceil();
    out
}
