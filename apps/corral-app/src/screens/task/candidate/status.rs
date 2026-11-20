//! The lines that say where an engine operation sent from the Review page stands.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::Hue;
use crate::model::review::StatusLine;
use crate::ui::kit::marks::{dot, hue_ink};
use crate::ui::term;
use crate::ui::tokens::Token;

use super::{Ctx, command, rich_at};

/// One sent operation: a dot in its tone, what it was and where it stands, then Dismiss once
/// it settled. Desktop and phone keep the dot in its own column on the first line, the words
/// wrapping beside it and the actions under them; a terminal sets all of it in one row.
pub(crate) fn status_line(cx: &Ctx, line: &StatusLine) -> AnyElement {
    let app = cx.app;
    let words = rich_at(app, &line.text, 12.0, Token::Ink2, false);
    let words = if line.hue == Hue::Neutral {
        words
    } else {
        words.text_color(hue_ink(line.hue))
    };
    let actions = line.actions.iter().map(|button| command(cx, button));
    if app.form == Form::Terminal {
        return view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .child(dot(app, line.hue))
            .child(term::space(1.0))
            .child(words)
            .children(actions)
            .into_element();
    }
    view()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .child(
            // One line of the 12 pt words high, so the dot sits on the first line.
            view()
                .flex_none()
                .h(px(17.0))
                .justify_center()
                .child(dot(app, line.hue)),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(4.0))
                .child(words)
                .when(!line.actions.is_empty(), |column| {
                    column.child(
                        view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .children(actions),
                    )
                }),
        )
        .into_element()
}
