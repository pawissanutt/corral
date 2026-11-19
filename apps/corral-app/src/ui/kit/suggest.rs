//! Values a free-text field offers beside what is typed, such as a landing target's local
//! branches: a quiet caption, then one Ghost button per value in a row that wraps within its
//! pane. A press puts the value in the field; the field still takes anything typed. The model
//! picks and orders the values; the row is absent while it offers none. Tab reaches each
//! button, so the keyboard and the pointer reach the same values. test_id `seg-<group>-<value>`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The row for `values`, redrawn as they change; `pick` receives the pressed value.
pub fn suggestions(
    app: AppState,
    caption: &'static str,
    group: impl Into<String>,
    values: Memo<Vec<String>>,
    pick: impl Fn(String) + Clone + 'static,
) -> AnyElement {
    let form = app.form;
    let group: String = group.into();
    dynamic(move || {
        let values = values.get();
        if values.is_empty() {
            return None;
        }
        let buttons: Vec<AnyElement> = values
            .into_iter()
            .map(|value| {
                let pick = pick.clone();
                let id = format!("seg-{group}-{value}");
                button(app, value.clone(), ButtonKind::Ghost, id, move || {
                    pick(value.clone());
                })
            })
            .collect();
        let words = text(caption)
            .flex_shrink_0()
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3));
        let words = match form {
            Form::Terminal => words,
            Form::Desktop => words.font_family(FONT_TEXT).text_size(px(12.0)),
            Form::Phone => words.font_family(FONT_TEXT).text_size(px(13.0)),
        };
        Some(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .min_w(px(0.0))
                .gap(px(match form {
                    Form::Terminal => 8.0,
                    Form::Desktop => 6.0,
                    Form::Phone => 8.0,
                }))
                .role(AccessibilityRole::Group)
                .label(caption)
                .child(words)
                .children(buttons),
        )
    })
    .into_element()
}
