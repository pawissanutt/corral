//! The position-in-plan mini graph (`.minigraph`): the model's cards and edges in its own units,
//! scaled to the width it is given, as the mockup's SVG at 100 % width.

use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use super::{code, words};
use crate::model::inbox::{MiniEdge, MiniGraph, MiniNode};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::{hue_fill, hue_ink};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The edges' stroke in source units (`.minigraph path`: 1.2).
const STROKE: f32 = 1.2;
/// Where the caption's box starts above the source's bottom edge (its baseline sits 4 up).
const CAPTION_RISE: f32 = 15.0;

/// The graph. Desktop and phone: the edges drawn on a canvas in rule-2, the cards over it
/// (surface on a rule-2 edge; the needs-you card on its hue's fill with a 1.5 edge; the queued
/// card's words faint), the caption under them; one image for screen readers, named by the
/// model's label. Terminal: one line per card (id, then its subtitle) and the caption.
pub(super) fn mini(app: AppState, graph: MiniGraph) -> AnyElement {
    if app.form == Form::Terminal {
        return terminal(graph);
    }
    let (width, height) = graph.size;
    let measured = signal(None::<f32>);
    let scale = computed(move || measured.get().map_or(1.0, |room| room / width));
    let edges = graph.edges;
    let edge_canvas = canvas(move || {
        let s = scale.get();
        let mut painter = Painter::new()
            .stroke_color(for_appearance(color(Token::Rule2)))
            .line_width(STROKE * s)
            .begin_path();
        let at = |(x, y): (f32, f32)| point(px(x * s), px(y * s));
        for edge in &edges {
            painter = match *edge {
                MiniEdge::Line { from, to } => painter
                    .move_to(from.0 * s, from.1 * s)
                    .line_to(to.0 * s, to.1 * s),
                MiniEdge::Cubic {
                    from,
                    control1,
                    control2,
                    to,
                } => painter.move_to(from.0 * s, from.1 * s).bezier_curve_to(
                    at(control1),
                    at(control2),
                    at(to),
                ),
            };
        }
        painter.stroke()
    })
    .absolute()
    .left(px(0.0))
    .top(px(0.0))
    .w(move || px(width * scale.get()))
    .h(move || px(height * scale.get()));
    view()
        .w_full()
        .role(AccessibilityRole::Image)
        .label(graph.label)
        .on_layout(move |frame: &Bounds<Points>| {
            let room = frame.size.width.0;
            if measured.get_untracked() != Some(room) {
                measured.set(Some(room));
            }
        })
        .child(
            view()
                .relative()
                .w_full()
                .h(move || px(height * scale.get()))
                .accessibility_elements_hidden(true)
                .child(edge_canvas)
                .children(graph.nodes.into_iter().map(|node| card(app, node, scale)))
                .child(
                    words(app, graph.caption, 10.0, Token::Ink3)
                        .number_of_lines(1u32)
                        .absolute()
                        .left(px(0.0))
                        .top(move || px((height - CAPTION_RISE) * scale.get())),
                ),
        )
        .into_element()
}

/// One card at its source place: the id in mono, the subtitle under it.
fn card(app: AppState, node: MiniNode, scale: Memo<f32>) -> AnyElement {
    let (x, y) = node.position;
    let (w, h) = node.size;
    let marked = node.hue != Hue::Neutral;
    let (id_ink, sub_ink) = match (marked, node.dim) {
        (_, true) => (Token::Ink4, Token::Ink4),
        (true, false) => (Token::Ink, Token::Ink3),
        (false, false) => (Token::Ink2, Token::Ink3),
    };
    view()
        .absolute()
        .left(move || px(x * scale.get()))
        .top(move || px(y * scale.get()))
        .w(move || px(w * scale.get()))
        .h(move || px(h * scale.get()))
        .justify_center()
        .px(px(8.0))
        .rounded(px(5.0))
        .overflow_hidden()
        .bg(if marked {
            hue_fill(node.hue)
        } else {
            color(Token::Surface)
        })
        .border(px(if marked { 1.5 } else { 1.0 }))
        .border_color(if marked {
            hue_ink(node.hue)
        } else {
            color(Token::Rule2)
        })
        .child(
            code(app, node.id, 10.5, id_ink)
                .number_of_lines(1u32)
                .line_height(px(13.0)),
        )
        .child(
            text(node.subtitle)
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(10.0))
                .line_height(px(12.0))
                .text_color(color(sub_ink)),
        )
        .into_element()
}

/// The graph in Cells: each card's id (bold and in its hue when marked, faint when queued) and
/// subtitle on a line, then the caption.
fn terminal(graph: MiniGraph) -> AnyElement {
    view()
        .role(AccessibilityRole::Image)
        .label(graph.label)
        .children(graph.nodes.into_iter().map(|node| {
            let marked = node.hue != Hue::Neutral;
            let id = text(node.id)
                .flex_shrink_0()
                .text_color(if marked {
                    hue_ink(node.hue)
                } else {
                    color(Token::Ink)
                })
                .when(marked, |t| t.font_weight(FontWeight::BOLD));
            let subtitle = if node.dim {
                term::quiet(node.subtitle)
            } else {
                text(node.subtitle).text_color(color(Token::Ink))
            };
            view()
                .flex_row()
                .accessibility_elements_hidden(true)
                .child(view().w(term::cells(7.0)).flex_shrink_0().child(id))
                .child(subtitle.truncate())
                .into_element()
        }))
        .child(
            term::quiet(graph.caption)
                .truncate()
                .accessibility_elements_hidden(true),
        )
        .into_element()
}
