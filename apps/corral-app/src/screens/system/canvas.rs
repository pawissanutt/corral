//! The map on a canvas, for desktop and web: measured to its container and laid out by the
//! model at that width with the platform's text engine, so it follows the window and no name is
//! cut. Edges are drawn three ways at the project level (declared and referenced; declared with
//! no reference, dashed; no matching dependency, in the held colour), cards re-coloured by
//! the overlay on the same geometry. A canvas has no parts of its own for the pointer or a
//! screen reader, so every card and every edge also gets a transparent button over it that
//! carries its label: a press picks it, a press on the picked card opens its level. A task
//! page's Impact draws its map here too ([`Host::Impact`]): its marks replace the overlay's
//! figure, ring the touched and dependent cards and dim the rest.

use nsh_core::canvas::CanvasTextAlign;
use nsh_core::prelude::*;

use crate::model::system::layout::{self, Font, Layout, Rect};
use crate::model::system::marks::{self as marked, Mark, Marks};
use crate::model::system::scene::{Card, EdgeKind, Kind, Scene};
use crate::model::system::tree::{matches, share};
use crate::model::system::words::{
    activity, busiest, figure, language_slot, languages, read_shares, slot_hue,
};
use crate::model::system::{Node, Overlay, Pick, State, Window};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::Host;

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
            let room = width.get_untracked();
            Some(untracked(|| drawn(app, layout, scene, room, host)))
        }))
        .into_element()
}

fn drawn(app: AppState, layout: Layout, scene: Scene, room: f32, host: Host) -> AnyElement {
    let (w, h) = (layout.width.max(1.0), layout.height.max(1.0));
    let spoken = format!(
        "Map, {} level: {} cards, {} edges drawn",
        scene.level.label(),
        scene.cards.len(),
        scene.edges.len()
    );
    let boxes = hit_boxes(app, &layout, &scene, host);
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

fn now(adaptive: AdaptiveColor) -> Hsla {
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
        if !matches(card, &self.state.filter) || !self.marked(&card.node) {
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

    fn edge(&self, from: &Node, to: &Node) -> bool {
        let matched = |node: &Node| {
            self.scene
                .card(node)
                .is_some_and(|c| matches(c, &self.state.filter))
        };
        if !matched(from) || !matched(to) || !(self.marked(from) || self.marked(to)) {
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
            .fill_text(caption.text, caption.x, caption.y);
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
        let on = lit.edge(&edge.from, &edge.to);
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
        };
        let stroke = if lit.picked_edge(&edge.from, &edge.to) {
            ink(Token::Focus)
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
        p = p
            .global_alpha(if lit.edge(&edge.from, &edge.to) {
                1.0
            } else {
                DIM
            })
            .fill_color(ink(Token::Surface))
            .begin_path()
            .round_rect(bounds(r), px(4.0))
            .fill()
            .stroke_color(ink(Token::Rule2))
            .line_width(1.0)
            .stroke()
            .font(&css(Font::Label))
            .fill_color(ink(Token::Ink3))
            .text_align(CanvasTextAlign::Center)
            .fill_text(label.text, label.x, label.y + 4.0);
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

struct Look {
    overlay: Overlay,
    window: Window,
    languages: Vec<String>,
    busiest: u64,
}

/// `words` cut with an ellipsis to fit `room` points.
fn fit(words: &str, room: f32, font: Font) -> String {
    if measure(font, words) <= room {
        return words.to_owned();
    }
    let mut kept: Vec<char> = words.chars().collect();
    while !kept.is_empty() {
        kept.pop();
        let cut: String = kept.iter().collect::<String>() + "…";
        if measure(font, &cut) <= room {
            return cut;
        }
    }
    String::new()
}

fn paint_card(
    p: Painter,
    card: &Card,
    r: Rect,
    look: &Look,
    scene: &Scene,
    picked: bool,
    // Under a task's marks: this card's, if any.
    mark: Option<Option<Mark>>,
) -> Painter {
    let port = card.kind == Kind::Port;
    let radius = if port { r.h / 2.0 } else { 6.0 };
    let mut p = p;
    if picked {
        p = p
            .stroke_color(ink(Token::Focus))
            .line_width(2.0)
            .begin_path()
            .round_rect(
                bounds(Rect {
                    x: r.x - 4.0,
                    y: r.y - 4.0,
                    w: r.w + 8.0,
                    h: r.h + 8.0,
                }),
                px(radius + 3.0),
            )
            .stroke();
    }
    p = p
        .fill_color(ink(Token::Surface))
        .begin_path()
        .round_rect(bounds(r), px(radius))
        .fill();
    if look.overlay == Overlay::Activity
        && matches!(card.kind, Kind::Package | Kind::Folder | Kind::Module)
    {
        let commits = activity(card, look.window).map_or(0, |a| a.0);
        if commits > 0 {
            let strength = 0.06 + 0.34 * (commits as f32 / look.busiest as f32).sqrt();
            p = p
                .fill_color(now(hue_ink(Hue::Worker)).opacity(strength))
                .begin_path()
                .round_rect(bounds(r), px(radius))
                .fill();
        }
    }
    let dashed = card.excluded()
        || matches!(
            card.kind,
            Kind::Folder | Kind::More | Kind::Owner | Kind::Scripts
        );
    let dash = [px(4.0), px(3.0)];
    p = p
        .stroke_color(ink(if card.excluded() {
            Token::Ink4
        } else {
            Token::Rule2
        }))
        .line_width(1.0)
        .line_dash(if dashed { &dash } else { &[] })
        .begin_path()
        .round_rect(bounds(r), px(radius))
        .stroke()
        .line_dash(&[]);
    if let Some(Some(mark)) = mark {
        // Touched is a solid ring; holding dependents a dashed one, outside it when both.
        let mut rings: Vec<(Hue, &[Points], f32)> = Vec::new();
        if mark.touched {
            rings.push((Hue::Worker, &[], 0.0));
        }
        if mark.dependents > 0 {
            rings.push((Hue::Held, &dash, if mark.touched { 3.0 } else { 0.0 }));
        }
        for (hue, ring, out) in rings {
            p = p
                .stroke_color(now(hue_ink(hue)))
                .line_width(2.0)
                .line_dash(ring)
                .begin_path()
                .round_rect(
                    bounds(Rect {
                        x: r.x - out,
                        y: r.y - out,
                        w: r.w + 2.0 * out,
                        h: r.h + 2.0 * out,
                    }),
                    px(radius + out),
                )
                .stroke()
                .line_dash(&[]);
        }
    }
    let figure = |card: &Card| match mark {
        Some(mark) => mark.map_or_else(|| marked::UNTOUCHED.to_owned(), Mark::words),
        None => figure(card, look.overlay, look.window),
    };
    if port {
        let tag = figure(card);
        let tag_w = measure(Font::Tag, &tag);
        return p
            .font(&css(Font::Name))
            .fill_color(ink(Token::Ink))
            .text_align(CanvasTextAlign::Start)
            .fill_text(
                &fit(&card.name, r.w - tag_w - 36.0, Font::Name),
                r.x + 14.0,
                r.y + r.h / 2.0 + 4.5,
            )
            .font(&css(Font::Tag))
            .fill_color(ink(Token::Ink3))
            .text_align(CanvasTextAlign::End)
            .fill_text(&tag, r.x + r.w - 13.0, r.y + r.h / 2.0 + 4.0);
    }
    let name_font = if matches!(card.kind, Kind::More | Kind::Scripts) {
        Font::Sub
    } else {
        Font::Name
    };
    let name_w = measure(name_font, &card.name);
    // The name wins: a tag that would crowd it stays in the inspector.
    let tag = card
        .tag
        .as_deref()
        .filter(|tag| name_w + measure(Font::Tag, tag) + 38.0 <= r.w);
    let tag_w = tag.map_or(0.0, |t| measure(Font::Tag, t) + 12.0);
    p = p
        .font(&css(name_font))
        .fill_color(ink(Token::Ink))
        .text_align(CanvasTextAlign::Start)
        .fill_text(
            &fit(&card.name, r.w - 24.0 - tag_w, name_font),
            r.x + 12.0,
            r.y + 22.0,
        );
    if let Some(tag) = tag {
        p = p
            .font(&css(Font::Tag))
            .fill_color(ink(Token::Ink3))
            .text_align(CanvasTextAlign::End)
            .fill_text(tag, r.x + r.w - 11.0, r.y + 21.0);
    }
    p = p
        .font(&css(Font::Sub))
        .fill_color(ink(Token::Ink2))
        .text_align(CanvasTextAlign::Start)
        .fill_text(
            &fit(&figure(card), r.w - 24.0, Font::Sub),
            r.x + 12.0,
            r.y + 39.0,
        );
    if matches!(card.kind, Kind::More | Kind::Definition) || card.kind.runtime() {
        return p;
    }
    p = bar(
        p,
        card,
        Rect {
            x: r.x + 12.0,
            y: r.y + 46.0,
            w: r.w - 24.0,
            h: 4.0,
        },
        look,
        scene,
    );
    if !card.spawns.is_empty() {
        p = spawn_chips(p, card, r);
    }
    p
}

/// The overlay's bar under the figure: the language mix, the activity relative to the busiest
/// here, or the files read, not read and failed.
fn bar(p: Painter, card: &Card, area: Rect, look: &Look, scene: &Scene) -> Painter {
    let mut p = p
        .fill_color(ink(Token::Rule))
        .fill_rect(area.x, area.y, area.w, area.h);
    match look.overlay {
        Overlay::Structure => {
            let mut parts: Vec<(Option<usize>, u64)> = Vec::new();
            if let Some(stats) = &card.stats {
                for (language, lines) in &stats.languages {
                    parts.push((language_slot(&look.languages, language), *lines));
                }
            } else if let Some((language, _, _)) = &card.file {
                parts.push((language_slot(&look.languages, language), card.lines));
            }
            parts.sort_by_key(|(slot, _)| slot.unwrap_or(usize::MAX));
            let total: u64 = parts.iter().map(|p| p.1).sum();
            if total == 0 {
                return p;
            }
            let mut x = area.x;
            for (slot, lines) in parts {
                let w = area.w * lines as f32 / total as f32;
                let fill = slot.map_or(ink(Token::Ink4), |s| now(hue_ink(slot_hue(s))));
                p = p.fill_color(fill).fill_rect(x, area.y, w, area.h);
                x += w;
            }
            p
        }
        Overlay::Activity => {
            let s = share(scene, card, Overlay::Activity, look.window);
            if s > 0.0 {
                p = p.fill_color(now(hue_ink(Hue::Worker))).fill_rect(
                    area.x,
                    area.y,
                    (area.w * s).max(2.0),
                    area.h,
                );
            }
            p
        }
        Overlay::Coverage => {
            let Some((read, unsupported, failed)) = read_shares(card) else {
                return p;
            };
            let mut x = area.x;
            for (share, fill) in [
                (read, ink(Token::Ink2)),
                (unsupported, ink(Token::Ink4).opacity(0.6)),
                (failed, now(hue_ink(Hue::Fail))),
            ] {
                let w = area.w * share;
                p = p.fill_color(fill).fill_rect(x, area.y, w, area.h);
                x += w;
            }
            p
        }
    }
}

/// The processes a package spawns, one chip each, then `+n` for those that do not fit.
fn spawn_chips(p: Painter, card: &Card, r: Rect) -> Painter {
    let y = r.y + 56.0;
    let mut p = p
        .font(&css(Font::Sub))
        .fill_color(ink(Token::Ink3))
        .text_align(CanvasTextAlign::Start)
        .fill_text("spawns", r.x + 12.0, y + 12.5);
    let mut x = r.x + 12.0 + measure(Font::Sub, "spawns") + 8.0;
    let end = r.x + r.w - 10.0;
    for (i, (process, sites)) in card.spawns.iter().enumerate() {
        let w = layout::chip_width(&measure, process, *sites);
        let rest = card.spawns.len() - i - 1;
        if i > 0 && x + w + if rest > 0 { 30.0 } else { 0.0 } > end {
            let more = format!("+{}", card.spawns.len() - i);
            let mw = measure(Font::Label, &more) + 14.0;
            return chip(
                p,
                Rect {
                    x,
                    y,
                    w: mw,
                    h: 17.0,
                },
            )
            .fill_color(ink(Token::Ink2))
            .text_align(CanvasTextAlign::Center)
            .fill_text(&more, x + mw / 2.0, y + 12.5);
        }
        p = chip(p, Rect { x, y, w, h: 17.0 })
            .font(&css(Font::Label))
            .fill_color(ink(Token::Ink2))
            .text_align(CanvasTextAlign::Start)
            .fill_text(process, x + 8.0, y + 12.5)
            .font(&css(Font::Tag))
            .fill_color(ink(Token::Ink3))
            .text_align(CanvasTextAlign::End)
            .fill_text(&sites.to_string(), x + w - 8.0, y + 12.5);
        x += w + 4.0;
    }
    p
}

fn chip(p: Painter, r: Rect) -> Painter {
    p.fill_color(ink(Token::BgSunk))
        .begin_path()
        .round_rect(bounds(r), px(8.5))
        .fill()
        .stroke_color(ink(Token::Rule2))
        .line_width(1.0)
        .stroke()
        .font(&css(Font::Label))
}

/// The buttons over the canvas: one per card and one at the start of each drawn edge.
fn hit_boxes(app: AppState, layout: &Layout, scene: &Scene, host: Host) -> Vec<AnyElement> {
    let mut boxes = Vec::new();
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
        let spoken = format!("{}, {}{tag}", card.name, card.kind.word());
        let node = card.node.clone();
        boxes.push(hit(
            app,
            placed.rect,
            card.node.test_id(),
            spoken,
            Pick::Node(node),
            6.0,
            host,
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
        let spoken = format!(
            "{} → {}: {}, {} references",
            scene.name(&edge.from),
            scene.name(&edge.to),
            edge.kind.word(),
            edge.count
        );
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
        ));
    }
    boxes
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
) -> AnyElement {
    let desktop = app.form == Form::Desktop;
    view()
        .absolute()
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
