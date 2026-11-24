//! The map on a canvas, for desktop and web: measured to its container and laid out by the
//! model at that width with the platform's text engine, so it follows the window and no name is
//! cut. Edges are drawn three ways at the project level (declared and referenced; declared with
//! no reference, dashed; no matching dependency, in the held colour), cards re-coloured by
//! the overlay on the same geometry. A canvas has no parts of its own for the pointer or a
//! screen reader, so every card and every edge also gets a transparent button over it that
//! carries its label: a press picks it, a press on the picked card opens its level. A task
//! page's Impact draws its map here too ([`Host::Impact`]): its marks ring the touched and
//! dependent cards and dim the rest (`card.rs` paints a card); an edge whose reference count
//! the change moved is drawn in the change colour with its before and after, and every other
//! edge is dimmed.

use nsh_core::canvas::CanvasTextAlign;
use nsh_core::prelude::*;

use crate::model::system::layout::{self, Font, Layout, Rect};
use crate::model::system::marks::{self as marked, CHANGED, Marks, change_spoken};
use crate::model::system::scene::{Card, Edge, EdgeKind, Scene};
use crate::model::system::tree::passes;
use crate::model::system::words::{busiest, languages};
use crate::model::system::{Node, Pick, State};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::Host;
use super::card::{Look, paint_card};

/// The CSS font of a map font.
pub(super) fn css(font: Font) -> String {
    let family = if font.mono() { FONT_MONO } else { FONT_TEXT };
    format!("{} {}px {family}", font.weight(), font.size())
}

pub(super) fn measure(font: Font, words: &str) -> f32 {
    measure_text(&css(font), words).0
}

/// The map of the shown level: laid out again when the scene or the room changes, painted
/// again when the overlay, the pick or the filter does.
pub fn map(app: AppState, scene: Memo<Option<Scene>>, host: Host) -> AnyElement {
    let width = signal(0.0_f32);
    let placed = computed(move || {
        let room = width.get();
        let scene = scene.get()?;
        (room > 0.0).then(|| (layout::layout(&scene, room, &measure), scene))
    });
    // The drawing never sizes its container: the room comes from the page, not the map.
    view()
        .w_full()
        .min_w(px(0.0))
        .overflow_hidden()
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(dynamic(move || {
            let (layout, scene) = placed.get()?;
            // The buttons speak each card's and edge's mark, so new marks place them again.
            let marks = host.marks();
            let room = width.get_untracked();
            Some(untracked(|| {
                drawn(app, layout, scene, room, host, marks.as_ref())
            }))
        }))
        .into_element()
}

fn drawn(
    app: AppState,
    layout: Layout,
    scene: Scene,
    room: f32,
    host: Host,
    marks: Option<&Marks>,
) -> AnyElement {
    let (w, h) = (layout.width.max(1.0), layout.height.max(1.0));
    let spoken = format!(
        "Map, {} level: {} cards, {} edges drawn",
        scene.level.label(),
        scene.cards.len(),
        scene.edges.len()
    );
    let (boxes, handles) = hit_boxes(app, &layout, &scene, host, marks);
    reveal_pick(app, host, handles);
    let paint_layout = layout.clone();
    let paint_scene = scene.clone();
    let surface = view()
        .relative()
        .w(px(w))
        .h(px(h))
        .child(
            canvas(move || {
                let state = host.state(app);
                let marks = host.marks();
                paint(&paint_layout, &paint_scene, &state, marks.as_ref())
            })
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .w(px(w))
            .h(px(h))
            .role(AccessibilityRole::Image)
            .label(spoken),
        )
        .children(boxes);
    if w > room + 1.0 {
        scroll().horizontal().w_full().child(surface).into_element()
    } else {
        surface.into_element()
    }
}

/// Brings the picked card into view when the pick changes, as it does when a list item or the
/// inspector picks a card the map's scroll has out of sight.
fn reveal_pick(app: AppState, host: Host, handles: Vec<(Node, ElementRef)>) {
    let picked = computed(move || match host.state(app).picked {
        Some(Pick::Node(node)) => Some(node),
        _ => None,
    });
    effect(move || {
        let Some(node) = picked.get() else {
            return;
        };
        if let Some(&(_, handle)) = handles.iter().find(|(n, _)| *n == node) {
            // A map just placed for a new level has no layout to scroll to until its frame.
            request_animation_frame(move |_| {
                let mut options = RevealOptions::default();
                options.vertical = ScrollAlignment::Nearest;
                handle.reveal(options);
            });
        }
    });
}

pub(super) fn now(adaptive: AdaptiveColor) -> Hsla {
    if dark() {
        adaptive.dark
    } else {
        adaptive.light
    }
}

pub(super) fn ink(token: Token) -> Hsla {
    now(color(token))
}

pub(super) fn bounds(r: Rect) -> Bounds<Points> {
    nsh_core::prelude::bounds(point(px(r.x), px(r.y)), size(px(r.w), px(r.h)))
}

/// What the pick and the filter light: every card and edge, unless something is picked (its
/// neighbourhood) or a filter is typed (its matches).
struct Lit<'a> {
    scene: &'a Scene,
    state: &'a State,
    /// A task's marks: what they leave unmarked is dimmed.
    marks: Option<&'a Marks>,
}

impl Lit<'_> {
    fn marked(&self, node: &Node) -> bool {
        self.marks
            .is_none_or(|marks| marked::of(marks, node).is_some())
    }

    fn card(&self, card: &Card) -> bool {
        if !passes(self.scene, card, &self.state.filter) || !self.marked(&card.node) {
            return false;
        }
        match &self.state.picked {
            Some(Pick::Node(node)) => {
                node == &card.node
                    || self.scene.edges.iter().any(|e| {
                        (&e.from == node && e.to == card.node)
                            || (&e.to == node && e.from == card.node)
                    })
            }
            Some(Pick::Edge(a, b)) => a == &card.node || b == &card.node,
            None => true,
        }
    }

    /// Under a task's marks only an edge whose reference count the change moved is lit,
    /// whatever its ends.
    fn edge(&self, edge: &Edge) -> bool {
        let (from, to) = (&edge.from, &edge.to);
        let matched = |node: &Node| {
            self.scene
                .card(node)
                .is_some_and(|c| passes(self.scene, c, &self.state.filter))
        };
        if !matched(from) || !matched(to) || (self.marks.is_some() && edge.change.is_empty()) {
            return false;
        }
        match &self.state.picked {
            Some(Pick::Node(node)) => node == from || node == to,
            Some(Pick::Edge(a, b)) => a == from && b == to,
            None => true,
        }
    }

    fn picked_edge(&self, from: &Node, to: &Node) -> bool {
        matches!(&self.state.picked, Some(Pick::Edge(a, b)) if a == from && b == to)
    }
}

const DIM: f32 = 0.28;

fn paint(layout: &Layout, scene: &Scene, state: &State, marks: Option<&Marks>) -> Painter {
    let lit = Lit {
        scene,
        state,
        marks,
    };
    let mut p = Painter::new();
    for caption in &layout.captions {
        if let Some(y) = caption.rule {
            p = p
                .stroke_color(ink(Token::Rule2))
                .line_width(1.0)
                .line_dash(&[px(2.0), px(4.0)])
                .begin_path()
                .move_to(caption.x, y)
                .line_to(layout.width - caption.x, y)
                .stroke()
                .line_dash(&[]);
        }
        p = p
            .font(&css(Font::Sub))
            .fill_color(ink(Token::Ink3))
            .text_align(CanvasTextAlign::Start)
            .fill_text(&caption.text, caption.x, caption.y);
    }
    let most = scene
        .edges
        .iter()
        .map(|e| e.count)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    for route in &layout.routes {
        let Some(edge) = scene.edges.get(route.edge) else {
            continue;
        };
        let picked = lit.picked_edge(&edge.from, &edge.to);
        let changed = marks.is_some() && !edge.change.is_empty();
        // In a task's Impact the stroke says what the change did, picked or not: an unchanged
        // edge stays dimmed and a changed one keeps the change colour; a halo marks the pick.
        let on = lit.edge(edge);
        let strong = state.picked.is_some() && on;
        let base = 1.1 + 2.4 * (1.0 + edge.count as f32).log10() / (1.0 + most).log10();
        let (stroke, width, dash): (Hsla, f32, &[Points]) = match edge.kind {
            EdgeKind::Declared | EdgeKind::Module => (ink(Token::Ink3), base, &[]),
            EdgeKind::DeclaredOnly => (ink(Token::Ink4), 1.3, &[px(4.0), px(3.0)]),
            EdgeKind::ReferencedOnly => (now(hue_ink(Hue::Held)), base, &[]),
            EdgeKind::Boundary => (ink(Token::Ink3), base, &[px(5.0), px(3.0)]),
            EdgeKind::Starts => (now(hue_ink(Hue::Worker)), base, &[]),
            EdgeKind::Runs => (ink(Token::Ink3), base, &[]),
            EdgeKind::DependsOn => (ink(Token::Ink3), base, &[px(5.0), px(3.0)]),
            EdgeKind::Builds => (now(hue_ink(Hue::Held)), base, &[px(4.0), px(3.0)]),
            EdgeKind::Has => (now(hue_ink(Hue::Worker)), base, &[]),
        };
        let (stroke, width) = if changed {
            (now(hue_ink(CHANGED)), base.max(1.6) + 0.6)
        } else {
            (stroke, width)
        };
        if picked && marks.is_some() {
            p = curve(
                p.global_alpha(0.55)
                    .stroke_color(ink(Token::Focus))
                    .line_width(width + 6.0),
                &trim(&route.points, 7.0),
            )
            .stroke();
        }
        let stroke = if picked && marks.is_none() {
            ink(Token::Focus)
        } else if changed {
            stroke
        } else if strong {
            ink(Token::Ink)
        } else {
            stroke
        };
        p = p.global_alpha(if on { 1.0 } else { DIM });
        p = curve(
            p.stroke_color(stroke)
                .line_width(width + if strong { 0.6 } else { 0.0 })
                .line_dash(dash),
            &trim(&route.points, 7.0),
        )
        .stroke()
        .line_dash(&[]);
        p = arrow(p.fill_color(stroke), &route.points);
    }
    p = p.global_alpha(1.0);
    for label in &layout.labels {
        let Some(edge) = scene.edges.get(label.edge) else {
            continue;
        };
        let r = Rect {
            x: label.x - label.w / 2.0,
            y: label.y - 9.0,
            w: label.w,
            h: 18.0,
        };
        let (rule, words) = if label.changed {
            (now(hue_ink(CHANGED)), now(hue_ink(CHANGED)))
        } else {
            (ink(Token::Rule2), ink(Token::Ink3))
        };
        p = p
            .global_alpha(if lit.edge(edge) { 1.0 } else { DIM })
            .fill_color(ink(Token::Surface))
            .begin_path()
            .round_rect(bounds(r), px(4.0))
            .fill()
            .stroke_color(rule)
            .line_width(1.0)
            .stroke()
            .font(&css(Font::Label))
            .fill_color(words)
            .text_align(CanvasTextAlign::Center)
            .fill_text(&label.text, label.x, label.y + 4.0);
    }
    let view = Look {
        overlay: state.overlay,
        window: state.window,
        languages: languages(scene),
        busiest: busiest(scene, state.window),
    };
    for placed in &layout.placed {
        let Some(card) = scene.cards.get(placed.card) else {
            continue;
        };
        let picked = state.picked == Some(Pick::Node(card.node.clone()));
        // A task's pick may be an untouched card; it is shown whole all the same.
        let shown = lit.card(card) || (picked && marks.is_some());
        p = p.global_alpha(if shown { 1.0 } else { DIM });
        let mark = marks.map(|marks| marked::of(marks, &card.node).copied());
        p = paint_card(p, card, placed.rect, &view, scene, picked, mark);
    }
    p.global_alpha(1.0)
}

/// Points along a route, the end pulled back so the arrowhead does not sink into the card.
pub(super) fn trim(points: &[(f32, f32)], by: f32) -> Vec<(f32, f32)> {
    let mut out = points.to_vec();
    let n = out.len();
    if n < 2 {
        return out;
    }
    let ((x0, y0), (x1, y1)) = (out[n - 2], out[n - 1]);
    let length = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt().max(1.0);
    let k = by.min(length - 0.5);
    out[n - 1] = if (x1 - x0).abs() > 0.6 && (y1 - y0).abs() > 0.6 {
        (x1, y1 - (y1 - y0).signum() * k)
    } else {
        (x1 - (x1 - x0) / length * k, y1 - (y1 - y0) / length * k)
    };
    out
}

/// Straight where a step is level or upright, an S-curve between rows otherwise.
pub(super) fn curve(p: Painter, points: &[(f32, f32)]) -> Painter {
    let Some(&(x, y)) = points.first() else {
        return p;
    };
    let mut p = p.begin_path().move_to(x, y);
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        p = if (x1 - x0).abs() < 0.6 || (y1 - y0).abs() < 0.6 {
            p.line_to(x1, y1)
        } else {
            let my = (y0 + y1) / 2.0;
            p.bezier_curve_to(
                point(px(x0), px(my)),
                point(px(x1), px(my)),
                point(px(x1), px(y1)),
            )
        };
    }
    p
}

pub(super) fn arrow(p: Painter, points: &[(f32, f32)]) -> Painter {
    let n = points.len();
    if n < 2 {
        return p;
    }
    let ((x0, y0), (x1, y1)) = (points[n - 2], points[n - 1]);
    let (ux, uy) = if (x1 - x0).abs() > 0.6 && (y1 - y0).abs() > 0.6 {
        (0.0, (y1 - y0).signum())
    } else {
        let l = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt().max(1.0);
        ((x1 - x0) / l, (y1 - y0) / l)
    };
    let (w, len) = (4.5, 8.0);
    let (bx, by) = (x1 - ux * len, y1 - uy * len);
    p.begin_path()
        .move_to(x1, y1)
        .line_to(bx - uy * w, by + ux * w)
        .line_to(bx + uy * w, by - ux * w)
        .close_path()
        .fill()
}

/// The buttons over the canvas: one per card and one at the start of each drawn edge.
fn hit_boxes(
    app: AppState,
    layout: &Layout,
    scene: &Scene,
    host: Host,
    marks: Option<&Marks>,
) -> (Vec<AnyElement>, Vec<(Node, ElementRef)>) {
    let mut boxes = Vec::new();
    let mut handles = Vec::new();
    for placed in &layout.placed {
        let Some(card) = scene.cards.get(placed.card) else {
            continue;
        };
        // The overlay re-paints the canvas without placing these again, so their words
        // keep to what does not change with it.
        let tag = card
            .tag
            .as_ref()
            .map(|tag| format!(", {tag}"))
            .unwrap_or_default();
        let mark = marks
            .map(|marks| format!(", {}", marked::spoken(marks, &card.node)))
            .unwrap_or_default();
        let spoken = format!("{}, {}{tag}{mark}", card.name, card.kind.word());
        let node = card.node.clone();
        let handle = element_ref();
        handles.push((node.clone(), handle));
        boxes.push(hit(
            app,
            placed.rect,
            card.node.test_id(),
            spoken,
            Pick::Node(node),
            6.0,
            host,
            Some(handle),
        ));
    }
    for route in &layout.routes {
        let Some(edge) = scene.edges.get(route.edge) else {
            continue;
        };
        let (Some(a), Some(b)) = (route.points.first(), route.points.get(1)) else {
            continue;
        };
        let (x, y) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let mut spoken = format!(
            "{} → {}: {}, {} references",
            scene.name(&edge.from),
            scene.name(&edge.to),
            edge.kind.word(),
            edge.count
        );
        if marks.is_some() {
            spoken.push_str(", ");
            spoken.push_str(&change_spoken(&edge.change).unwrap_or_else(|| "unchanged".into()));
        }
        let pick = Pick::Edge(edge.from.clone(), edge.to.clone());
        let area = Rect {
            x: x - 9.0,
            y: y - 9.0,
            w: 18.0,
            h: 18.0,
        };
        boxes.push(hit(
            app,
            area,
            format!("edge-{}", route.edge),
            spoken,
            pick,
            9.0,
            host,
            None,
        ));
    }
    (boxes, handles)
}

fn hit(
    app: AppState,
    area: Rect,
    test_id: String,
    // Also the desktop's tooltip.
    spoken: String,
    pick: Pick,
    radius: f32,
    host: Host,
    // A card's, so a pick made elsewhere can bring it into view.
    handle: Option<ElementRef>,
) -> AnyElement {
    let desktop = app.form == Form::Desktop;
    let v = view();
    let v = match handle {
        Some(handle) => v.element_ref(&handle),
        None => v,
    };
    v.absolute()
        .left(px(area.x))
        .top(px(area.y))
        .w(px(area.w))
        .h(px(area.h))
        .test_id(&test_id)
        .cursor_pointer()
        .rounded(px(radius))
        .role(AccessibilityRole::Button)
        .label(spoken.clone())
        .on_press(move || host.pick(app, pick.clone()))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(2.0))
        })
        .when(desktop, |v| v.tooltip(tip(spoken.into())))
        .into_element()
}

pub(super) fn tip(words: SharedString) -> AnyElement {
    view()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(6.0))
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule2))
        .child(
            text(words)
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .text_color(color(Token::Ink)),
        )
        .into_element()
}
