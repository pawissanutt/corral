//! Places a scene for the canvas, computed from the data alone: nothing is placed by hand per
//! project. Connected cards are layered by reference rank (a package above what it uses), a
//! rank wider than the room wraps onto more rows, rows are ordered to reduce crossings and
//! each card is pulled toward its neighbours without overlapping another. Unconnected cards
//! are packed in rows under the graph. Text is measured by the caller's text engine, so names
//! keep their whole width.

use std::collections::BTreeMap;

use super::layered::{LEdge, layered};
use super::scene::{Card, EdgeKind, Kind, Level, Scene};
use super::words::figure;
use super::{Overlay, Window};

/// The fonts the map measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    /// A card's name: mono, semibold.
    Name,
    /// A card's tag and a port's count: small mono.
    Tag,
    /// A card's figure line.
    Sub,
    /// An edge's label and a spawn chip: small semibold.
    Label,
}

impl Font {
    /// The CSS size and weight; the caller supplies the family.
    pub const fn size(self) -> f32 {
        match self {
            Font::Name => 12.5,
            Font::Tag => 10.5,
            Font::Sub => 11.5,
            Font::Label => 11.0,
        }
    }
    pub const fn weight(self) -> u16 {
        match self {
            Font::Name | Font::Label => 600,
            Font::Tag | Font::Sub => 400,
        }
    }
    pub const fn mono(self) -> bool {
        matches!(self, Font::Name | Font::Tag)
    }
    /// A width estimate for a measurer that has no text engine (tests).
    pub fn estimate(self, text: &str) -> f32 {
        let advance = if self.mono() { 0.62 } else { 0.56 };
        text.chars().count() as f32 * self.size() * advance
    }
}

/// The text engine's width of `text` in a font.
pub type Measure<'a> = &'a dyn Fn(Font, &str) -> f32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }
}

/// A card's box, by its index in the scene.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub card: usize,
    pub rect: Rect,
}

/// An edge's path, by its index in `Scene.edges`: points the curve passes, ending at the
/// target.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub edge: usize,
    pub points: Vec<(f32, f32)>,
}

/// Words on an edge (`0 refs`), centred at x.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeLabel {
    pub edge: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub text: &'static str,
}

/// A caption over a part of the map, with a dashed rule above it when `rule`.
#[derive(Debug, Clone, PartialEq)]
pub struct Caption {
    pub x: f32,
    pub y: f32,
    pub text: &'static str,
    pub rule: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub placed: Vec<Placed>,
    pub routes: Vec<Route>,
    pub labels: Vec<EdgeLabel>,
    pub captions: Vec<Caption>,
}

impl Layout {
    pub fn rect(&self, card: usize) -> Option<Rect> {
        self.placed.iter().find(|p| p.card == card).map(|p| p.rect)
    }
}

pub const CARD_HEIGHT: f32 = 58.0;
pub const SPAWN_HEIGHT: f32 = 20.0;
pub const PORT_HEIGHT: f32 = 30.0;
const PAD: f32 = 8.0;
const MAX_CARD: f32 = 420.0;

/// The card's box size: wide enough for its name with its tag, and for its figure under every
/// overlay, so switching overlays never moves the map; more lines make it a little wider.
pub fn size(card: &Card, scene: &Scene, measure: Measure) -> (f32, f32) {
    if card.kind == Kind::Port {
        let tag = figure(card, Overlay::Structure, Window::Days30);
        let w = measure(Font::Name, &card.name) + measure(Font::Tag, &tag) + 36.0;
        return (w.ceil(), PORT_HEIGHT);
    }
    let plain = matches!(card.kind, Kind::More | Kind::Scripts);
    let t = ((card.lines as f32 + 1.0).ln() / (scene.max_lines as f32 + 1.0).ln()).clamp(0.0, 1.0);
    let package = scene.level == Level::Package;
    let min = if package { 112.0 } else { 128.0 };
    let by_lines = match (plain, package) {
        (true, _) => 0.0,
        (false, true) => 112.0 + 88.0 * t * t,
        (false, false) => 124.0 + 116.0 * t * t,
    };
    let tag = card
        .tag
        .as_deref()
        .map_or(0.0, |tag| measure(Font::Tag, tag) + 14.0);
    let name = measure(Font::Name, &card.name) + tag + 26.0;
    let mut sub: f32 = 0.0;
    for overlay in Overlay::ALL {
        for window in Window::ALL {
            sub = sub.max(measure(Font::Sub, &figure(card, overlay, window)));
        }
    }
    let spawns = card.spawns.first().map_or(0.0, |(process, sites)| {
        measure(Font::Sub, "spawns")
            + 32.0
            + chip_width(measure, process, *sites)
            + if card.spawns.len() > 1 { 40.0 } else { 0.0 }
    });
    let w = by_lines
        .max(name)
        .max(sub + 26.0)
        .max(spawns)
        .clamp(min, MAX_CARD);
    let h = CARD_HEIGHT
        + if card.spawns.is_empty() {
            0.0
        } else {
            SPAWN_HEIGHT
        };
    (w.ceil(), h)
}

/// A spawn chip's width: the process and its sites.
pub fn chip_width(measure: Measure, process: &str, sites: u64) -> f32 {
    (measure(Font::Label, process) + measure(Font::Tag, &sites.to_string()) + 20.0).ceil()
}

/// Lays the scene out within `room` points of width.
pub fn layout(scene: &Scene, room: f32, measure: Measure) -> Layout {
    let sizes: Vec<(f32, f32)> = scene
        .cards
        .iter()
        .map(|c| size(c, scene, measure))
        .collect();
    let inner = (room - 2.0 * PAD).max(320.0);
    let mut out = Layout::default();
    let mut y = PAD;
    let mut width: f32 = 0.0;
    let graph: Vec<usize> = scene
        .top
        .iter()
        .chain(&scene.graph)
        .chain(&scene.bottom)
        .copied()
        .collect();
    if !graph.is_empty() {
        let local: BTreeMap<usize, usize> =
            graph.iter().enumerate().map(|(i, c)| (*c, i)).collect();
        let mut edges = Vec::new();
        for (index, edge) in scene.edges.iter().enumerate() {
            let find = |node| {
                scene
                    .cards
                    .iter()
                    .position(|c| &c.node == node)
                    .and_then(|c| local.get(&c).copied())
            };
            if let (Some(a), Some(b)) = (find(&edge.from), find(&edge.to)) {
                let pri = match edge.kind {
                    EdgeKind::ReferencedOnly => edge.count as f64,
                    _ => 1e7 + edge.count as f64,
                };
                edges.push(LEdge { index, a, b, pri });
            }
        }
        let flags = |list: &[usize]| graph.iter().map(|c| list.contains(c)).collect::<Vec<_>>();
        let (top, bottom) = (flags(&scene.top), flags(&scene.bottom));
        let has_top = top.iter().any(|t| *t);
        let has_bottom = bottom.iter().any(|b| *b);
        let caption_room = if has_top { 26.0 } else { 0.0 };
        let local_sizes: Vec<_> = graph.iter().map(|c| sizes[*c]).collect();
        let g = layered(
            &local_sizes,
            &edges,
            &top,
            &bottom,
            inner,
            (has_top, has_bottom),
        );
        y += caption_room;
        for (i, rect) in g.pos.iter().enumerate() {
            out.placed.push(Placed {
                card: graph[i],
                rect: Rect {
                    x: rect.x + PAD,
                    y: rect.y + y,
                    ..*rect
                },
            });
        }
        for (edge, points) in g.routes {
            out.routes.push(Route {
                edge,
                points: points
                    .into_iter()
                    .map(|(x, py)| (x + PAD, py + y))
                    .collect(),
            });
        }
        if has_top {
            out.captions.push(Caption {
                x: PAD,
                y: y - 10.0,
                text: "Packages that reference this one",
                rule: None,
            });
        }
        if has_bottom && let Some(top) = g.bottom_top {
            out.captions.push(Caption {
                x: PAD,
                y: y + top - 10.0,
                text: "Packages this one references",
                rule: None,
            });
        }
        width = width.max(g.width + 2.0 * PAD);
        y += g.height;
    }
    // The band, then each kind's group: packed rows, each under its caption once anything is
    // above it; a group says its kind even when it is the first thing drawn.
    let parts = std::iter::once((scene.band_title, &scene.band, false)).chain(
        scene
            .groups
            .iter()
            .map(|(title, group)| (Some(*title), group, true)),
    );
    let mut above = !graph.is_empty();
    for (title, part, group) in parts {
        if part.is_empty() {
            continue;
        }
        let titled = title.filter(|_| above || group);
        if above {
            y += 50.0;
        } else if titled.is_some() {
            y += 26.0;
        }
        let part_sizes: Vec<_> = part.iter().map(|c| sizes[*c]).collect();
        let (pos, w, h) = flow_pack(&part_sizes, inner.max(width - 2.0 * PAD), 14.0);
        if let Some(text) = titled {
            out.captions.push(Caption {
                x: PAD,
                y: y - if above { 14.0 } else { 10.0 },
                text,
                rule: above.then_some(y - 34.0),
            });
        }
        for (i, rect) in pos.into_iter().enumerate() {
            out.placed.push(Placed {
                card: part[i],
                rect: Rect {
                    x: rect.x + PAD,
                    y: rect.y + y,
                    ..rect
                },
            });
        }
        width = width.max(w + 2.0 * PAD);
        y += h;
        above = true;
    }
    if above {
        y += PAD;
    }
    out.width = width.ceil();
    out.height = y.ceil();
    out.labels = labels(scene, &out.routes, measure);
    out
}

fn labels(scene: &Scene, routes: &[Route], measure: Measure) -> Vec<EdgeLabel> {
    let mut out: Vec<EdgeLabel> = Vec::new();
    for route in routes {
        let text = match scene.edges.get(route.edge).map(|e| e.kind) {
            Some(EdgeKind::DeclaredOnly) => "0 refs",
            _ => continue,
        };
        let (Some(a), Some(b)) = (route.points.first(), route.points.get(1)) else {
            continue;
        };
        out.push(EdgeLabel {
            edge: route.edge,
            x: (a.0 + b.0) / 2.0,
            y: (a.1 + b.1) / 2.0,
            w: (measure(Font::Label, text) + 12.0).ceil(),
            text,
        });
    }
    out.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    for i in 0..out.len() {
        for j in 0..i {
            let (c, a) = (out[j].clone(), &mut out[i]);
            if (a.y - c.y).abs() < 20.0 && (a.x - c.x).abs() < (a.w + c.w) / 2.0 + 4.0 {
                a.x = c.x + (a.w + c.w) / 2.0 + 4.0;
            }
        }
    }
    out
}

/// Boxes left to right, wrapping at `max_w`.
fn flow_pack(sizes: &[(f32, f32)], max_w: f32, gap: f32) -> (Vec<Rect>, f32, f32) {
    let (mut x, mut y, mut row_h, mut w) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
    let mut out = Vec::with_capacity(sizes.len());
    for &(bw, bh) in sizes {
        if x > 0.0 && x + bw > max_w {
            x = 0.0;
            y += row_h + gap;
            row_h = 0.0;
        }
        out.push(Rect { x, y, w: bw, h: bh });
        x += bw + gap;
        row_h = row_h.max(bh);
        w = w.max(x - gap);
    }
    let h = if sizes.is_empty() { 0.0 } else { y + row_h };
    (out, w, h)
}
