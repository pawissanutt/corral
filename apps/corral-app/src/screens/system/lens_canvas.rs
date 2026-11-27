//! A lens drawn on a canvas, for desktop and web: laid out by `model::system::lens_layout` at
//! the room's width with the platform's text engine. States are pills and transitions curved
//! arrows with their words (a guard in brackets under them); participants head dashed
//! lifelines and messages run between them top to bottom, numbered, a note under the words.
//! An element with a failed check is drawn in the fail colour with ✗, one the change under
//! review touches or adds in its colour with Δ or +, the picked one ringed. A canvas has no
//! parts for the pointer or a screen reader, so each state, participant and arrow's words get
//! a transparent button carrying its label: a press picks it.
//!
//! Test ids: "lens-diagram", "lens-node-<n>", "lens-edge-<n>".

use std::rc::Rc;

use corral_types::lenses::RecipeLayout;
use nsh_core::canvas::CanvasTextAlign;
use nsh_core::prelude::*;

use super::canvas::{arrow, bounds, css, curve, ink, measure, tip, trim};
use crate::model::system::layout::{Font, Rect};
use crate::model::system::lens::{Diagram, Element, Mark};
use crate::model::system::lens_layout::{self, LINE_H, LensLayout, label_lines};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::tokens::{Token, color};

/// What a press on an element does.
pub(crate) type OnPick = Rc<dyn Fn(String)>;

pub(crate) fn diagram(
    app: AppState,
    shown: Memo<Option<Diagram>>,
    picked: Memo<Option<String>>,
    on_pick: OnPick,
) -> AnyElement {
    let width = signal(0.0_f32);
    let placed = computed(move || {
        let room = width.get();
        let d = shown.get()?;
        (room > 0.0).then(|| (lens_layout::layout(&d, room, &measure), d))
    });
    view()
        .test_id("lens-diagram")
        .w_full()
        .min_w(px(0.0))
        .overflow_hidden()
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(dynamic(move || {
            let (layout, d) = placed.get()?;
            let room = width.get_untracked();
            let on_pick = on_pick.clone();
            Some(untracked(|| drawn(app, layout, d, room, picked, on_pick)))
        }))
        .into_element()
}

fn drawn(
    app: AppState,
    layout: LensLayout,
    d: Diagram,
    room: f32,
    picked: Memo<Option<String>>,
    on_pick: OnPick,
) -> AnyElement {
    let (w, h) = (layout.width.max(1.0), layout.height.max(1.0));
    let spoken = format!("{}: {}", d.title, d.figures());
    let boxes = hit_boxes(app, &layout, &d, &on_pick);
    let (pl, pd) = (layout.clone(), d.clone());
    let surface = view()
        .relative()
        .w(px(w))
        .h(px(h))
        .child(
            canvas(move || {
                let picked = picked.get();
                paint(&pl, &pd, picked.as_deref())
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

/// The ink an element is drawn in: its strongest mark's, else `plain`.
fn element_ink(e: &Element, plain: Token) -> Hsla {
    match e.marked() {
        Some(mark) => mark_ink(mark),
        None => ink(plain),
    }
}

fn mark_ink(mark: Mark) -> Hsla {
    let hue: Hue = mark.hue();
    let adaptive = hue_ink(hue);
    if dark() {
        adaptive.dark
    } else {
        adaptive.light
    }
}

fn paint(layout: &LensLayout, d: &Diagram, picked: Option<&str>) -> Painter {
    let mut p = Painter::new();
    for (x, y0, y1) in &layout.lifelines {
        p = p
            .stroke_color(ink(Token::Rule2))
            .line_width(1.0)
            .line_dash(&[px(4.0), px(4.0)])
            .begin_path()
            .move_to(*x, *y0)
            .line_to(*x, *y1)
            .stroke()
            .line_dash(&[]);
    }
    for a in &layout.arrows {
        let Some(e) = d.edges.get(a.edge) else {
            continue;
        };
        let is_picked = picked == Some(e.name.as_str());
        let stroke = if is_picked {
            ink(Token::Focus)
        } else {
            element_ink(e, Token::Ink3)
        };
        let points = trim(&a.points, 4.0);
        p = curve(
            p.stroke_color(stroke)
                .line_width(if is_picked || e.marked().is_some() {
                    2.0
                } else {
                    1.3
                }),
            &points,
        )
        .stroke()
        .fill_color(stroke);
        p = arrow(p, &points);
        p = label(p, a.label, d, a.edge, e, is_picked);
    }
    for n in &layout.nodes {
        let Some(e) = d.nodes.get(n.node) else {
            continue;
        };
        p = node(p, n.rect, d.kind, e, picked == Some(e.name.as_str()));
    }
    p
}

fn label(p: Painter, r: Rect, d: &Diagram, edge: usize, e: &Element, picked: bool) -> Painter {
    let (lead, detail) = label_lines(d, edge);
    let mut p = p
        .fill_color(ink(Token::Bg))
        .begin_path()
        .round_rect(bounds(r), px(4.0))
        .fill();
    if picked {
        p = p.stroke_color(ink(Token::Focus)).line_width(1.5).stroke();
    }
    p = p
        .font(&css(Font::Label))
        .fill_color(element_ink(e, Token::Ink2))
        .text_align(CanvasTextAlign::Center)
        .fill_text(&lead, r.x + r.w / 2.0, r.y + 13.0);
    if let Some(detail) = detail {
        p = p
            .font(&css(Font::Sub))
            .fill_color(ink(Token::Ink3))
            .fill_text(&detail, r.x + r.w / 2.0, r.y + 13.0 + LINE_H);
    }
    p
}

fn node(p: Painter, r: Rect, kind: RecipeLayout, e: &Element, picked: bool) -> Painter {
    let radius = match kind {
        RecipeLayout::Graph => r.h / 2.0,
        RecipeLayout::OrderedLanes => 6.0,
    };
    let edge = if picked {
        ink(Token::Focus)
    } else {
        element_ink(e, Token::Rule2)
    };
    let words = lens_layout::node_words(e);
    p.fill_color(ink(Token::Surface))
        .begin_path()
        .round_rect(bounds(r), px(radius))
        .fill()
        .stroke_color(edge)
        .line_width(if picked || e.marked().is_some() {
            2.0
        } else {
            1.0
        })
        .stroke()
        .font(&css(Font::Name))
        .fill_color(element_ink(e, Token::Ink))
        .text_align(CanvasTextAlign::Center)
        .fill_text(&words, r.x + r.w / 2.0, r.y + r.h / 2.0 + 4.5)
}

fn hit_boxes(app: AppState, layout: &LensLayout, d: &Diagram, on_pick: &OnPick) -> Vec<AnyElement> {
    let mut out = Vec::new();
    for n in &layout.nodes {
        let Some(e) = d.nodes.get(n.node) else {
            continue;
        };
        out.push(hit(
            app,
            n.rect,
            format!("lens-node-{}", n.node),
            spoken(e),
            e,
            on_pick,
        ));
    }
    for a in &layout.arrows {
        let Some(e) = d.edges.get(a.edge) else {
            continue;
        };
        let ends = e
            .ends
            .map(|(x, y)| format!("{} → {}: ", d.node_label(x), d.node_label(y)))
            .unwrap_or_default();
        let words = format!(
            "{}, {ends}{}",
            spoken(e),
            e.detail.clone().unwrap_or_default()
        );
        out.push(hit(
            app,
            a.label,
            format!("lens-edge-{}", a.edge),
            words,
            e,
            on_pick,
        ));
    }
    out
}

fn spoken(e: &Element) -> String {
    let marks: Vec<&str> = e.marks.iter().map(|m| m.word()).collect();
    if marks.is_empty() {
        format!("{} {}", e.what, e.label)
    } else {
        format!("{} {}, {}", e.what, e.label, marks.join(", "))
    }
}

fn hit(
    app: AppState,
    area: Rect,
    test_id: String,
    spoken: String,
    e: &Element,
    on_pick: &OnPick,
) -> AnyElement {
    let (name, on_pick) = (e.name.clone(), on_pick.clone());
    view()
        .absolute()
        .left(px(area.x))
        .top(px(area.y))
        .w(px(area.w))
        .h(px(area.h))
        .test_id(&test_id)
        .cursor_pointer()
        .rounded(px(6.0))
        .role(AccessibilityRole::Button)
        .label(spoken.clone())
        .on_press(move || on_pick(name.clone()))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(2.0))
        })
        .when(app.form == Form::Desktop, |v| v.tooltip(tip(spoken.into())))
        .into_element()
}
