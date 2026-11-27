//! The branch field (`field-history-branch`): its words narrow the branches offered under it,
//! and Enter reads the branch it names, so a branch past those listed is reached by typing.
//! corrald decides whether the name exists. Built once per shown page; typing re-reads the
//! model without rebuilding the field.

use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::history::{Action, BRANCH_FIELD, BRANCH_PLACEHOLDER};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The field's width: room for `release/2026.11` and a little more.
const FIELD: f32 = 220.0;
const FIELD_CELLS: f32 = 28.0;
const TARGET: f32 = 44.0;

pub(super) fn field(app: AppState) -> AnyElement {
    let form = app.form;
    let input = copyable_input(app, None, move || {
        app.history.with(|state| state.typed.clone())
    })
    .test_id("field-history-branch")
    .label(BRANCH_FIELD)
    .placeholder(BRANCH_PLACEHOLDER)
    .on_change(move |words: &str| app.history_act(Action::Type(words.to_owned())))
    // Enter keeps the field focused, so a mistyped name is corrected in place.
    .submit_behavior(SubmitBehavior::Submit)
    .on_submit(move |_: &str| app.history_act(Action::Submit))
    .min_w(px(0.0))
    .text_color(color(Token::Ink))
    .placeholder_color(move || for_appearance(color(Token::Ink4)))
    .caret_color(move || for_appearance(color(Token::Ink)))
    .selection_color(move || for_appearance(color(Token::Sel)));
    let input = match form {
        Form::Terminal => input
            .h(term::rows(1.0))
            .w(term::cells(FIELD_CELLS))
            .flex_shrink_0()
            .px(term::cells(1.0))
            .bg(color(Token::Surface))
            .focus(|s| s.bg(color(Token::Sel))),
        Form::Desktop => input
            .w(px(FIELD))
            .flex_shrink_0()
            .h(px(28.0))
            .px(px(10.0))
            .font_family(FONT_TEXT)
            .text_size(px(13.0))
            .rounded(px(6.0))
            .bg(AdaptiveColor::TRANSPARENT)
            .border_1()
            .border_color(color(Token::Rule2))
            .focus(|s| s.border_color(color(Token::Focus))),
        Form::Phone => input
            .flex_1()
            .min_h(px(TARGET))
            .px(px(12.0))
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .rounded(px(10.0))
            .bg(AdaptiveColor::TRANSPARENT)
            .border_1()
            .border_color(color(Token::Rule2))
            .focus(|s| s.border_color(color(Token::Focus))),
    };
    let caption = text(BRANCH_FIELD)
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let caption = match form {
        Form::Terminal => caption,
        _ => caption.font_family(FONT_TEXT).text_size(px(12.0)),
    };
    view()
        .flex_row()
        .items_center()
        .min_w(px(0.0))
        .gap(px(if form == Form::Terminal { 8.0 } else { 10.0 }))
        .child(caption)
        .child(input)
        .into_element()
}
