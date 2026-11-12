//! The selected member's story (`.insp` in the tracker's side column): id and plan, title,
//! context, the run/gate/review/land stepper, progress steps, the facts and the commands.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::plans::{Story, StoryField, StoryStage};
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::table::{kv, steps};
use crate::ui::kit::text::{h3, mono, plain, rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::command;

/// The story of one member. A queued member has no stepper, steps or commands: its id, title,
/// context and facts only.
pub(super) fn story(app: AppState, inspector: &Story) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    view()
        .test_id("plan-story")
        .min_w(px(0.0))
        .child(identity(app, inspector))
        .child(title(app, inspector.title))
        .child(
            view()
                .when(!terminal, |v| v.mb(px(14.0)))
                .when(!inspector.context.is_empty(), |v| {
                    v.child(rich_small(app, &inspector.context))
                }),
        )
        .when(!inspector.stages.is_empty(), |v| {
            v.child(stepper(app, &inspector.stages))
        })
        .children(
            inspector
                .progress
                .map(|label| progress(app, label, inspector.progress_count.clone())),
        )
        .when(!inspector.steps.is_empty(), |v| {
            v.child(
                view().when(!terminal, |s| s.mb(px(16.0))).child(steps(
                    app,
                    inspector
                        .steps
                        .iter()
                        .map(|&(state, words)| {
                            let hue = if state == Mark::Active {
                                Hue::Worker
                            } else {
                                Hue::Neutral
                            };
                            let line =
                                plain(app, words).text_color(color(if state == Mark::Next {
                                    Token::Ink3
                                } else {
                                    Token::Ink
                                }));
                            (state, hue, line.into_element())
                        })
                        .collect(),
                )),
            )
        })
        .when(terminal, |v| v.child(term::blank()))
        .child(kv(
            app,
            inspector
                .fields
                .iter()
                .map(|field| (field.label.to_owned(), fact(app, field)))
                .collect(),
        ))
        .when(!inspector.buttons.is_empty(), |v| {
            v.child(commands(app, inspector))
        })
        .into_element()
}

/// The id in mono, the parent plan and the changed-since chip.
fn identity(app: AppState, inspector: &Story) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let plan = inspector.plan.clone().map(|plan| {
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
    let parts: Vec<AnyElement> = [
        Some(
            mono(app, inspector.id)
                .text_color(color(Token::Ink3))
                .into_element(),
        ),
        plan,
        inspector
            .changed
            .as_ref()
            .map(|changed| chip(app, changed.label.clone(), changed.hue)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let row = view().flex_row().flex_wrap().items_center();
    if terminal {
        let mut spaced = Vec::with_capacity(parts.len() * 2);
        for (at, part) in parts.into_iter().enumerate() {
            if at > 0 {
                spaced.push(term::space(1.0));
            }
            spaced.push(part);
        }
        return row.children(spaced).into_element();
    }
    row.gap(px(8.0)).children(parts).into_element()
}

/// The title (`.insp h2`): 16 pt bold, 2 pt above and below.
fn title(app: AppState, words: &'static str) -> AnyElement {
    let heading = text(words)
        .role(AccessibilityRole::Header)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    if app.form == Form::Terminal {
        return heading.into_element();
    }
    heading
        .my(px(2.0))
        .font_family(FONT_TEXT)
        .text_size(px(16.0))
        .line_height(px(21.0))
        .letter_spacing(px(-0.16))
        .into_element()
}

/// The stepper (`.stepper`): run, gate, review and land, two to a row in the side column, a
/// 2 pt line of the active stage's colour over the running one. A terminal gives each stage
/// a line: label, mark and state, then its detail.
fn stepper(app: AppState, stages: &[StoryStage]) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let cells = stages
        .iter()
        .map(|stage| stage_cell(app, stage))
        .collect::<Vec<_>>();
    if terminal {
        return view()
            .child(view().children(cells))
            .child(term::blank())
            .into_element();
    }
    view()
        .flex_row()
        .flex_wrap()
        .mt(px(4.0))
        .mb(px(16.0))
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
        .uppercase()
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
        .child(
            label
                .font_family(FONT_TEXT)
                .text_size(px(11.0))
                .letter_spacing(px(0.66)),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .mt(px(3.0))
                .mb(px(1.0))
                .child(mark(app, stage.mark, stage.hue))
                .child(status.font_family(FONT_TEXT).text_size(px(13.0))),
        )
        .child(sub.font_family(FONT_MONO).text_size(px(11.0)))
        .into_element()
}

/// The progress caption and its done/total count, flush with the stepper above it.
fn progress(app: AppState, label: &'static str, count: Option<String>) -> AnyElement {
    let caption = h3(app, label, count);
    if app.form == Form::Terminal {
        return caption;
    }
    view().mt(px(-18.0)).child(caption).into_element()
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

/// The member's commands (`.btn` row): wrapping, 8 pt apart, 16 pt under the facts.
fn commands(app: AppState, inspector: &Story) -> AnyElement {
    let buttons = inspector
        .buttons
        .iter()
        .map(|button| command(app, button, false))
        .collect::<Vec<_>>();
    if app.form == Form::Terminal {
        let mut spaced = vec![term::blank()];
        let mut row = Vec::with_capacity(buttons.len() * 2);
        for (at, button) in buttons.into_iter().enumerate() {
            if at > 0 {
                row.push(term::space(2.0));
            }
            row.push(button);
        }
        spaced.push(view().flex_row().flex_wrap().children(row).into_element());
        return view().children(spaced).into_element();
    }
    view()
        .flex_row()
        .flex_wrap()
        .gap(px(8.0))
        .mt(px(16.0))
        .children(buttons)
        .into_element()
}
