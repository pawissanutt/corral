//! How the app's graphical charts paint (desktop, web and phone canvases; a terminal draws its
//! charts in Cells and takes none of this). `model::chart` holds the numbers.
//!
//! The conventions every full chart follows:
//!
//! - **Axes.** A value scale from zero: one gridline per round step in `rule`, the zero
//!   baseline stronger in `rule-2`, and each step's compact label in 10.5 pt mono `ink-3`,
//!   right-aligned in a gutter left of the plot. Time runs left to right with its labels under
//!   the baseline, thinned from the newest end so they never collide.
//! - **Colour.** One measured series in `worker`; the point read now in `foreground`; a second
//!   series (the burn-up's scope) dashed in `ink-3`. Every colour is a theme role read through
//!   [`now`], so a chart redraws when the appearance changes.
//! - **Values.** A chart says its exact figures in words: hovered, touched or reached with the
//!   arrow keys, the point's sentence shows in a readout line above the plot (a full chart) or
//!   a hover bubble ([`tip`], an inline mark). Screen readers hear the chart's summary.
//!
//! An inline sparkline keeps only the baseline and the series colour: no axes, no labels.

use nsh_core::canvas::CanvasTextAlign;
use nsh_core::prelude::*;

use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// Axis labels: 10.5 pt mono, and its estimated advance (a canvas cannot measure).
pub const AXIS_SIZE: f32 = 10.5;
pub const AXIS_ADVANCE: f32 = 6.4;
/// Room between an axis label and the plot.
pub const AXIS_GAP: f32 = 6.0;
/// The series colour, and the colour of the point read now.
pub const SERIES: Token = Token::Worker;
pub const PICKED: Token = Token::Ink;

/// The colour of a theme colour in the appearance shown now. Reading it inside a canvas's
/// drawing redraws the canvas when the appearance changes.
pub fn now(adaptive: AdaptiveColor) -> Hsla {
    if dark() {
        adaptive.dark
    } else {
        adaptive.light
    }
}

pub fn ink(token: Token) -> Hsla {
    now(color(token))
}

pub fn mono_font(px_size: f32) -> String {
    format!("{px_size}px {FONT_MONO}")
}

pub fn text_font(weight: u16, px_size: f32) -> String {
    format!("{weight} {px_size}px {FONT_TEXT}")
}

/// The gutter a value scale's labels need: the widest label and the gap after it.
pub fn gutter<'a>(labels: impl IntoIterator<Item = &'a str>) -> f32 {
    let widest = labels
        .into_iter()
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(1);
    widest as f32 * AXIS_ADVANCE + AXIS_GAP
}

/// The value scale's gridlines from `left` to `right`, each `(label, y)` with its label
/// right-aligned just left of `left`; the first is the zero baseline, drawn stronger.
pub fn grid(mut p: Painter, left: f32, right: f32, lines: &[(String, f32)]) -> Painter {
    p = p
        .font(&mono_font(AXIS_SIZE))
        .fill_color(ink(Token::Ink3))
        .text_align(CanvasTextAlign::End)
        .line_width(1.0);
    for (at, (label, y)) in lines.iter().enumerate() {
        let rule = if at == 0 { Token::Rule2 } else { Token::Rule };
        // Half a point puts a one-point line on whole device pixels.
        let y = y.round() + 0.5;
        p = p
            .stroke_color(ink(rule))
            .begin_path()
            .move_to(left, y)
            .line_to(right, y)
            .stroke()
            .fill_text(label, left - AXIS_GAP, y + AXIS_SIZE * 0.35);
    }
    p
}

/// The hover bubble: the words on the raised surface in 11.5 pt ink.
pub fn tip(words: SharedString) -> AnyElement {
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
