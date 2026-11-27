//! The commit graph beside the rows: on desktop and web a canvas per row, as tall as the row,
//! drawing the lanes `model::history::graph` laid out (a lane bending into another as a curve,
//! the commit a dot, a root a ring); a terminal and the phone draw the same row as text glyphs.
//! The line under the last row marks each lane that continues past the loaded commits.

use nsh_core::prelude::*;

use crate::model::history::graph::{GraphRow, Line};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::chart::{ink, now};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::mono;
use crate::ui::tokens::Token;

/// A lane's width and a desktop row's height (the list row's).
const LANE: f32 = 14.0;
const ROW: f32 = 30.0;
const DOT: f32 = 4.0;
/// Lanes a desktop row draws at most; wider graphs are rare and still list every commit.
const MOST: usize = 16;
/// The hues lanes take in turn, so neighbouring lines tell apart.
const HUES: [Hue; 5] = [Hue::Worker, Hue::Corral, Hue::Held, Hue::Ok, Hue::You];

fn lane_x(lane: usize) -> f32 {
    LANE * (lane as f32 + 0.5)
}

fn lane_ink(lane: usize) -> Hsla {
    now(hue_ink(HUES[lane % HUES.len()]))
}

fn width(lanes: usize) -> f32 {
    LANE * lanes.clamp(1, MOST) as f32
}

/// One half of a row: each line from its column at `top` to its column at `top + ROW / 2`,
/// coloured by the lane it reaches.
fn half(mut p: Painter, lines: &[Line], top: f32) -> Painter {
    let mid = top + ROW / 4.0;
    let bottom = top + ROW / 2.0;
    for line in lines {
        if line.from >= MOST || line.to >= MOST {
            continue;
        }
        let (x1, x2) = (lane_x(line.from), lane_x(line.to));
        let colour = lane_ink(line.from.max(line.to));
        p = p
            .stroke_color(colour)
            .line_width(1.6)
            .begin_path()
            .move_to(x1, top);
        p = if line.from == line.to {
            p.line_to(x2, bottom)
        } else {
            p.bezier_curve_to(
                point(px(x1), px(mid)),
                point(px(x2), px(mid)),
                point(px(x2), px(bottom)),
            )
        };
        p = p.stroke();
    }
    p
}

fn paint(row: &GraphRow) -> Painter {
    let mut p = half(Painter::new(), &row.upper, 0.0);
    p = half(p, &row.lower, ROW / 2.0);
    if row.column >= MOST {
        return p;
    }
    let (x, y) = (lane_x(row.column), ROW / 2.0);
    let colour = lane_ink(row.column);
    p = p.begin_path().arc(x, y, DOT, 0.0, std::f32::consts::TAU);
    if row.root {
        p.fill_color(ink(Token::Surface))
            .fill()
            .stroke_color(colour)
            .line_width(1.6)
            .stroke()
    } else {
        p.fill_color(colour).fill()
    }
}

/// The graph cell of a row: a canvas on desktop and web, the row's glyphs elsewhere.
pub fn cell(app: AppState, row: &GraphRow, glyphs: &str, lanes: usize) -> AnyElement {
    if app.form != Form::Desktop {
        return mono(app, glyphs.to_owned())
            .flex_shrink_0()
            .number_of_lines(1u32)
            .accessibility_elements_hidden(true)
            .into_element();
    }
    let row = row.clone();
    canvas(move || paint(&row))
        .flex_shrink_0()
        .w(px(width(lanes)))
        .h(px(ROW))
        .accessibility_elements_hidden(true)
        .into_element()
}

/// Under the last row: the lanes that continue past the loaded commits, dashed on a canvas, `┊`
/// as text.
pub fn continuing(app: AppState, glyphs: &str, lanes: usize, open: Vec<usize>) -> AnyElement {
    if app.form != Form::Desktop {
        return mono(app, glyphs.to_owned())
            .flex_shrink_0()
            .text_color(color_quiet())
            .into_element();
    }
    canvas(move || {
        let mut p = Painter::new()
            .line_width(1.6)
            .line_dash(&[px(3.0), px(3.0)]);
        for &lane in open.iter().filter(|&&lane| lane < MOST) {
            p = p
                .stroke_color(lane_ink(lane))
                .begin_path()
                .move_to(lane_x(lane), 0.0)
                .line_to(lane_x(lane), ROW / 2.0)
                .stroke();
        }
        p
    })
    .flex_shrink_0()
    .w(px(width(lanes)))
    .h(px(ROW / 2.0))
    .accessibility_elements_hidden(true)
    .into_element()
}

fn color_quiet() -> AdaptiveColor {
    crate::ui::tokens::color(Token::Ink3)
}
