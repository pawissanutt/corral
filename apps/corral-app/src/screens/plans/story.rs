//! The selected member's story, under the path to done: its id and plan, title, who works it,
//! the run/gate/review/land stepper, its facts and its commands.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::Mark;
use crate::model::plans::{Story, StoryField, StoryStage};
use crate::state::AppState;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::table::kv;
use crate::ui::kit::text::{mono, rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::command;

/// The story of one member. test id "plan-story".
pub(super) fn story(app: AppState, story: &Story) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    view()
        .test_id("plan-story")
        .min_w(px(0.0))
        .child(identity(app, story))
        .child(title(app, &story.title))
        .child(
            view()
                .when(!terminal, |v| v.mb(px(14.0)))
                .when(!story.context.is_empty(), |v| {
                    v.child(rich_small(app, &story.context))
                }),
        )
        .when(!story.stages.is_empty(), |v| {
            v.child(stepper(app, &story.stages))
        })
        .child(kv(
            app,
            story
                .fields
                .iter()
                .map(|field| (field.label.to_owned(), fact(app, field)))
                .collect(),
        ))
        .when(!story.buttons.is_empty(), |v| v.child(commands(app, story)))
        .into_element()
}

/// The id in mono, then the parent plan.
fn identity(app: AppState, story: &Story) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let plan = story.plan.clone().map(|plan| {
        let words = text(plan)
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3));
        if terminal {
            words.into_element()
        } else {
            words
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .into_element()
        }
    });
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(if terminal { term::cells(1.0) } else { px(8.0) })
        .child(mono(app, story.id.to_string()).text_color(color(Token::Ink3)))
        .children(plan)
        .into_element()
}

/// The title (`.phd h2`): 16 pt bold, 4 pt above and 3 below.
fn title(app: AppState, words: &str) -> AnyElement {
    let heading = text(words.to_owned())
        .role(AccessibilityRole::Header)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    if app.form == Form::Terminal {
        return heading.into_element();
    }
    heading
        .mt(px(4.0))
        .mb(px(3.0))
        .font_family(FONT_TEXT)
        .text_size(px(16.0))
        .line_height(px(21.0))
        .into_element()
}

/// The stepper (`.stages`): run, gate, review and land, two to a row in the side column, a
/// 2 pt line of the active stage's colour over the running one. A terminal gives each stage
/// a line: label, mark and state, then its detail.
fn stepper(app: AppState, stages: &[StoryStage]) -> AnyElement {
    let cells = stages
        .iter()
        .map(|stage| stage_cell(app, stage))
        .collect::<Vec<_>>();
    if app.form == Form::Terminal {
        return view()
            .child(view().children(cells))
            .child(term::blank())
            .into_element();
    }
    view()
        .flex_row()
        .flex_wrap()
        .mb(px(14.0))
        .children(cells)
        .into_element()
}

fn stage_cell(app: AppState, stage: &StoryStage) -> AnyElement {
    let active = stage.mark == Mark::Active;
    let status = text(stage.status)
        .number_of_lines(1u32)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink));
    let label = text(stage.label)
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let sub = text(stage.sub.clone())
        .number_of_lines(1u32)
        .truncate()
        .text_color(color(Token::Ink3));
    let spoken = format!("{}: {}, {}", stage.label, stage.status, stage.sub);
    if app.form == Form::Terminal {
        return view()
            .flex_row()
            .items_center()
            .accessible(true)
            .label(spoken)
            .child(view().w(term::cells(8.0)).flex_shrink_0().child(label))
            .child(mark(app, stage.mark, stage.hue))
            .child(term::space(1.0))
            .child(status)
            .child(term::space(2.0))
            .child(sub.flex_1().min_w(px(0.0)))
            .into_element();
    }
    view()
        .w(relative(0.5))
        .pt(px(if active { 7.0 } else { 8.0 }))
        .pr(px(10.0))
        .pb(px(8.0))
        .border_t(px(if active { 2.0 } else { 1.0 }))
        .border_color(if active {
            hue_ink(stage.hue)
        } else {
            color(Token::Rule)
        })
        .accessible(true)
        .label(spoken)
        .child(label.font_family(FONT_TEXT).text_size(px(11.5)))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .mt(px(2.0))
                .child(mark(app, stage.mark, stage.hue))
                .child(status.font_family(FONT_TEXT).text_size(px(13.0))),
        )
        .child(sub.font_family(FONT_MONO).text_size(px(11.5)))
        .into_element()
}

/// A fact's value: its command, or its sentence.
fn fact(app: AppState, field: &StoryField) -> AnyElement {
    match &field.button {
        Some(button) => view()
            .flex_row()
            .child(command(app, button, false))
            .into_element(),
        None => rich(app, &field.value),
    }
}

/// The member's commands (`.acts`): wrapping, 8 pt apart, 14 pt under the facts; a terminal
/// two Cells apart after a blank row.
fn commands(app: AppState, story: &Story) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    view()
        .flex_row()
        .flex_wrap()
        .when(terminal, |row| {
            row.mt(term::rows(1.0)).gap_x(term::cells(2.0))
        })
        .when(!terminal, |row| row.gap(px(8.0)).mt(px(14.0)))
        .children(
            story
                .buttons
                .iter()
                .map(|button| command(app, button, false))
                .collect::<Vec<_>>(),
        )
        .into_element()
}
