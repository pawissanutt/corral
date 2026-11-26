//! The two controls every task and session shows: its one next step as a button, and More,
//! which opens the menu of every other operation (`ui::kit::more`).

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::next::{NextStep, next_id};
use crate::model::work::Record;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::more::{more_menu, press};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// What the More trigger says: three dots where a row has little room, the word beside them
/// on a page.
const MORE: &str = "More";
const MORE_SHORT: &str = "⋯";

/// The record's next step as its button (test id `next-<record>`): filled when the step is
/// the owner's move, outlined otherwise. `wide` takes the room of its row, as a phone's bar
/// does.
pub fn next_button(app: AppState, record: &str, step: &NextStep, wide: bool) -> AnyElement {
    let kind = if step.yours {
        ButtonKind::Primary
    } else {
        ButtonKind::Ghost
    };
    sent_aware(
        app,
        record,
        step,
        kind,
        ButtonOptions {
            wide,
            ..ButtonOptions::default()
        },
    )
}

/// The step's button, which says the step is under way (`Moving to Ready…`) and takes no press
/// from the moment it is sent until corrald answers. The outcome then shows as the task's
/// status, a receipt or the refusal sheet.
fn sent_aware(
    app: AppState,
    record: &str,
    step: &NextStep,
    kind: ButtonKind,
    options: ButtonOptions,
) -> AnyElement {
    let op = step.op();
    let sent = computed(move || op.as_ref().is_some_and(|op| app.op_pending(op)));
    let (step, id) = (step.clone(), next_id(record));
    dynamic(move || {
        let sent = sent.get();
        let label = sent
            .then(|| step.pending_label())
            .flatten()
            .unwrap_or_else(|| step.label.clone());
        let action = step.press.clone();
        untracked(|| {
            button_with(
                app,
                label,
                kind,
                ButtonOptions {
                    disabled: sent,
                    ..options
                },
                id.clone(),
                move || press(app, action.clone()),
            )
        })
    })
}

/// The record's next step as a list row carries it (test id `next-<record>`): sized to sit
/// inside the row, filled when the step is the owner's move and otherwise only words, so a
/// column of routine steps reads quietly and the one waiting on the owner stands out.
pub fn row_next_button(app: AppState, record: &str, step: &NextStep) -> AnyElement {
    let kind = if step.yours {
        ButtonKind::Primary
    } else {
        ButtonKind::Quiet
    };
    sent_aware(
        app,
        record,
        step,
        kind,
        ButtonOptions {
            compact: true,
            ..ButtonOptions::default()
        },
    )
}

/// The More trigger of `record` (test id `more-<record>`), opening its More menu: the popup
/// on desktop and in a terminal, the action sheet on a phone. `compact` is the three dots a
/// list row carries; a page spells the word.
pub fn more_button(app: AppState, record: Record, compact: bool) -> AnyElement {
    let form = app.form;
    let id = format!("more-{}", record.id());
    let spoken = format!("{MORE}: every operation on {}", record.id());
    let words = text(if compact { MORE_SHORT } else { MORE })
        .number_of_lines(1u32)
        .text_color(color(if form == Form::Terminal {
            Token::Ink
        } else {
            Token::Ink2
        }));
    let trigger = view()
        .test_id(&id)
        .flex_row()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(spoken);
    let trigger = match form {
        Form::Terminal => trigger
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .bg(color(Token::Surface))
            .child(words),
        Form::Desktop => trigger
            .h(px(if compact { 24.0 } else { 30.0 }))
            .px(px(if compact { 6.0 } else { 12.0 }))
            .rounded(px(6.0))
            .when(!compact, |trigger| {
                trigger.border_1().border_color(color(Token::Rule2))
            })
            .hover(|s| s.bg(color(Token::Hover)))
            .child(
                words
                    .font_family(FONT_TEXT)
                    .text_size(px(if compact { 14.0 } else { 12.5 }))
                    .font_weight(FontWeight::SEMIBOLD),
            ),
        Form::Phone => trigger
            .min_h(px(44.0))
            .px(px(if compact { 10.0 } else { 16.0 }))
            .rounded(px(10.0))
            .when(!compact, |trigger| {
                trigger.border_1().border_color(color(Token::Rule2))
            })
            .active(|s| s.bg(color(Token::Hover)))
            .child(
                words
                    .font_family(FONT_TEXT)
                    .text_size(px(if compact { 18.0 } else { 15.0 }))
                    .font_weight(FontWeight::BOLD),
            ),
    };
    more_menu(app, record, trigger.into_element())
}
