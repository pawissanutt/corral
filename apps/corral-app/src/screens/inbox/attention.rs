//! An item's attention move as corrald records it: whose move it is, why the item waits, the
//! next step, and the command a terminal would run. The command is a hint to read and copy,
//! never something the app runs.

use nsh_core::prelude::*;

use super::{code, words};
use crate::model::inbox::AttentionLine;
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::label_value;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// The whole move in a detail pane: the whose-move chip beside the reason, then the next step
/// and the command under their labels. Terminal: the chip and the reason on one line, then a
/// label-value line each.
pub(super) fn block(app: AppState, line: AttentionLine) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone { 14.5 } else { 13.0 };
    let reason = words(app, line.reason, size, Token::Ink)
        .flex_1()
        .min_w(px(0.0))
        .line_height(px((size * 1.5).round()));
    let next = words(app, line.next_step, size, Token::Ink)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .line_height(px((size * 1.5).round()));
    let command = line.command.map(|command| {
        code(app, command, size - 1.0, Token::Ink2)
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .line_height(px((size * 1.5).round()))
    });
    if form == Form::Terminal {
        return view()
            .test_id("attention")
            .child(
                view()
                    .flex_row()
                    .child(chip(app, line.whose, line.hue))
                    .child(term::space(1.0))
                    .child(reason),
            )
            .child(label_value(app, line.next_label, next.into_element()))
            .children(
                command.map(|command| label_value(app, line.command_label, command.into_element())),
            )
            .into_element();
    }
    view()
        .test_id("attention")
        .gap(px(10.0))
        .max_w(px(640.0))
        .child(
            view()
                .flex_row()
                .items_start()
                .gap(px(10.0))
                .child(
                    view()
                        .flex_row()
                        .flex_shrink_0()
                        .pt(px(1.0))
                        .child(chip(app, line.whose, line.hue)),
                )
                .child(reason),
        )
        .child(pair(app, line.next_label, next))
        .children(command.map(|command| pair(app, line.command_label, command)))
        .into_element()
}

/// A label in spaced capitals over its value, as the options' consequences read.
fn pair(app: AppState, label: &'static str, value: Text) -> AnyElement {
    view()
        .gap(px(2.0))
        .child(
            words(app, label, 11.0, Token::Ink3)
                .uppercase()
                .letter_spacing(px(0.66))
                .number_of_lines(1u32),
        )
        .child(value)
        .into_element()
}

/// The move on one line of a list row or card: whose move in its colour, then the reason.
/// `lines` caps the wrap (one in a row, two on a phone card).
pub(super) fn summary(app: AppState, line: &AttentionLine, size: f32, lines: u32) -> Text {
    let whose_ink = match line.hue {
        Hue::Neutral => color(Token::Ink2),
        hue => hue_ink(hue),
    };
    words(app, "", size, Token::Ink2)
        .min_w(px(0.0))
        .number_of_lines(lines)
        .child(
            text(line.whose.clone())
                .text_color(whose_ink)
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(text(" · "))
        .child(text(line.reason.clone()))
}
