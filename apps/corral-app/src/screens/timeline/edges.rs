//! Dependency edges on the lanes canvas (desktop and phone): an elbow from the dependency's
//! landing down or up to its dependent's row, then along it to the dependent's first run, with
//! a small arrowhead there. Out of order in the danger colour, others quiet. The model chose
//! which edges show (out-of-order ones and the selected lane's); a terminal draws none.

use nsh_core::prelude::*;

use super::chart::{BAR, DOT, Rect, bar_top, hit};
use crate::model::Hue;
use crate::model::timeline::{Layout, PlacedEdge};
use crate::state::AppState;
use crate::ui::chart::{ink, now};
use crate::ui::kit::marks::hue_ink;
use crate::ui::tokens::Token;

/// The arrowhead's length and half its width.
const HEAD: f32 = 5.0;
const HALF: f32 = 3.5;
/// Below this horizontal run the edge is drawn straight down (or up) into the bar.
const STRAIGHT: f32 = 3.0;

/// The centre line of the bar on the lane starting at `lane_y`.
fn centre(layout: &Layout, phone: bool, lane_y: f32) -> f32 {
    bar_top(layout, phone, lane_y, BAR) + BAR / 2.0
}

/// Where the edge ends, and the direction its head points (`dx`, `dy`).
fn tip(layout: &Layout, phone: bool, edge: &PlacedEdge) -> ((f32, f32), (f32, f32)) {
    let (x1, from) = edge.from;
    let (x2, to) = edge.to;
    let (y1, y2) = (centre(layout, phone, from), centre(layout, phone, to));
    if (x2 - x1).abs() < STRAIGHT && (y2 - y1).abs() > 0.5 {
        let down = if y2 > y1 { 1.0 } else { -1.0 };
        ((x2, y2 - down * BAR / 2.0), (0.0, down))
    } else {
        let right = if x2 >= x1 { 1.0 } else { -1.0 };
        ((x2, y2), (right, 0.0))
    }
}

pub fn paint(mut p: Painter, layout: &Layout, phone: bool) -> Painter {
    for edge in &layout.edges {
        let stroke = if edge.out_of_order {
            now(hue_ink(Hue::Fail))
        } else {
            ink(Token::Ink3)
        };
        let (x1, from) = edge.from;
        let (y1, y2) = (
            centre(layout, phone, from),
            centre(layout, phone, edge.to.1),
        );
        let ((tx, ty), (dx, dy)) = tip(layout, phone, edge);
        // Leave the dependency's bar from its edge, not through it.
        let start = if (y2 - y1).abs() < 0.5 {
            y1
        } else if y2 > y1 {
            y1 + BAR / 2.0
        } else {
            y1 - BAR / 2.0
        };
        p = p
            .stroke_color(stroke)
            .line_width(1.2)
            .begin_path()
            .move_to(x1, start);
        p = if dy == 0.0 {
            p.line_to(x1, ty).line_to(tx - dx * HEAD, ty)
        } else {
            p.line_to(tx, ty - dy * HEAD)
        };
        p = p.stroke().fill_color(stroke).begin_path();
        p = if dy == 0.0 {
            p.move_to(tx, ty)
                .line_to(tx - dx * HEAD, ty - HALF)
                .line_to(tx - dx * HEAD, ty + HALF)
        } else {
            p.move_to(tx, ty)
                .line_to(tx - HALF, ty - dy * HEAD)
                .line_to(tx + HALF, ty - dy * HEAD)
        };
        p = p.close_path().fill();
    }
    p
}

/// A box over each drawn edge's head that names the edge to a screen reader and the hover.
pub fn boxes(app: AppState, layout: &Layout, phone: bool) -> Vec<AnyElement> {
    layout
        .edges
        .iter()
        .map(|edge| {
            let ((x, y), _) = tip(layout, phone, edge);
            hit(
                app.form,
                Rect::new(x - DOT / 2.0, y - DOT / 2.0, DOT, DOT),
                &edge.label,
                None,
            )
        })
        .collect()
}
