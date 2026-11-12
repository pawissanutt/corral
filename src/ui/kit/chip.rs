//! Small labels that carry a state: the chip, the verdict stamp, the "illustrative" tag and the
//! count pill.
//!
//! In a terminal a fill is a band (the standard grey on 16 colours), so a chip or stamp keeps
//! its words in the state's colour on that band; a stamp is also bold, so it reads as a verdict
//! without colour.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::kit::marks::{hue_fill, hue_ink};
use crate::ui::term::cells;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The ink of a chip's words: the hue's colour, ink-2 for a neutral chip (ink in a terminal,
/// where ink-2 fades and would read as disabled).
fn chip_ink(form: Form, hue: Hue) -> AdaptiveColor {
    match (hue, form) {
        (Hue::Neutral, Form::Terminal) => color(Token::Ink),
        (Hue::Neutral, _) => color(Token::Ink2),
        _ => hue_ink(hue),
    }
}

/// A chip: the label on the hue's quiet fill in the hue's colour (`.chip.you`, `.chip.held` ...);
/// [`Hue::Neutral`] is ink-2 on surface-2. Desktop 18 pt high, 11 pt text, 4 pt corners; phone
/// 20 pt, 12 pt text; terminal the label on a band with a Cell either side. One line, never cut.
pub fn chip(app: AppState, label: impl Into<String>, hue: Hue) -> AnyElement {
    let form = app.form;
    let label: String = label.into();
    let words = text(label)
        .number_of_lines(1u32)
        .text_color(chip_ink(form, hue));
    let shell = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .bg(hue_fill(hue));
    match form {
        Form::Terminal => shell.px(cells(1.0)).child(words).into_element(),
        Form::Desktop | Form::Phone => {
            let (height, size) = if form == Form::Phone {
                (20.0, 12.0)
            } else {
                (18.0, 11.0)
            };
            shell
                .h(px(height))
                .px(px(6.0))
                .rounded(px(4.0))
                .child(words.font_family(FONT_TEXT).text_size(px(size)))
                .into_element()
        }
    }
}

/// A verdict stamped in place (`.stamp`): ANSWERED, ANSWERED ELSEWHERE, QUEUED, EXECUTED ...,
/// bold capitals inside a 1.5 pt ring of the hue. [`Hue::Ok`] is the mock's plain stamp,
/// [`Hue::Held`] its `void` (queued, waiting, superseded), [`Hue::Fail`] its `bad` (refused).
/// Desktop 12.5 pt text, 6 by 12 pt padding; phone 14 pt; terminal bold words in the hue on a
/// band. The label is shown as given; pass it in capitals as the mock writes it.
pub fn stamp(app: AppState, label: impl Into<String>, hue: Hue) -> AnyElement {
    stamped(app.form, label.into(), hue, false)
}

/// The small stamp of a receipt line (`.rcpt .stamp`): 11 pt text, 2 by 8 pt padding. Terminal
/// as [`stamp`].
pub fn stamp_small(app: AppState, label: impl Into<String>, hue: Hue) -> AnyElement {
    stamped(app.form, label.into(), hue, true)
}

fn stamped(form: Form, label: String, hue: Hue, small: bool) -> AnyElement {
    let ink = hue_ink(hue);
    let words = text(label)
        .number_of_lines(1u32)
        .font_weight(FontWeight::BOLD)
        .text_color(ink);
    if form == Form::Terminal {
        return view()
            .flex_row()
            .flex_shrink_0()
            .px(cells(1.0))
            .bg(hue_fill(hue))
            .child(words)
            .into_element();
    }
    let (size, pad_y, pad_x) = match (small, form) {
        (true, _) => (11.0, 2.0, 8.0),
        (false, Form::Phone) => (14.0, 7.0, 12.0),
        (false, _) => (12.5, 6.0, 12.0),
    };
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .self_start()
        .py(px(pad_y))
        .px(px(pad_x))
        .rounded(px(6.0))
        .border(px(1.5))
        .border_color(ink)
        .child(
            words
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .letter_spacing(px(size * 0.02)),
        )
        .into_element()
}

/// The tag that marks invented content (`.illus`): "illustrative", "tail illustrative",
/// "machine name illustrative". Italic, faint ink-4, desktop 10.5 pt, phone 12 pt; terminal
/// italic ink-3, as ink-4 would vanish there. A long note wraps inside its parent instead of
/// running past it; a short tag keeps its natural width.
pub fn illus(app: AppState, label: impl Into<String>) -> AnyElement {
    let form = app.form;
    let label: String = label.into();
    let words = text(label).italic().flex_shrink(1.0).min_w(px(0.0));
    match form {
        Form::Terminal => words.text_color(color(Token::Ink3)).into_element(),
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 12.0 } else { 10.5 }))
            .letter_spacing(px(0.1))
            .text_color(color(Token::Ink4))
            .into_element(),
    }
}

/// A count in a pill of its own fill, for badges and tab counts outside the navigation
/// (`atoms::count` keeps the destinations' own). A coloured hue is a solid pill of that colour
/// with bold figures in the button ink (the needs-you badge in [`Hue::You`]); [`Hue::Neutral`] is
/// quiet ink-2 on surface-2. Desktop 16 pt high, 11 pt mono figures; phone 22 pt, 13 pt. A
/// terminal shows the figures alone, bold in the hue: a column of small fills reads as one band
/// there.
pub fn pill(app: AppState, n: u32, hue: Hue) -> AnyElement {
    let form = app.form;
    let figures = text(n.to_string()).number_of_lines(1u32);
    if form == Form::Terminal {
        return figures
            .flex_shrink_0()
            .text_color(chip_ink(form, hue))
            .when(hue != Hue::Neutral, |t| t.font_weight(FontWeight::BOLD))
            .into_element();
    }
    let (fill, ink, weight) = match hue {
        Hue::Neutral => (
            color(Token::Surface2),
            color(Token::Ink2),
            FontWeight::MEDIUM,
        ),
        _ => (hue_ink(hue), color(Token::BtnInk), FontWeight::BOLD),
    };
    let (size, height, pad) = if form == Form::Phone {
        (13.0, 22.0, 7.0)
    } else {
        (11.0, 16.0, 5.0)
    };
    view()
        .flex_row()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .h(px(height))
        .min_w(px(height))
        .px(px(pad))
        .rounded(px(height / 2.0))
        .bg(fill)
        .child(
            figures
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(size))
                .font_weight(weight)
                .text_color(ink),
        )
        .into_element()
}
