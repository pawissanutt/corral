//! The inspector for the selected task (`.insp`): its Story (goal, the run, gate, review, land
//! stepper, steps, who moves next, needs and unblocks with their marks, cost, actions), the
//! illustrative Evidence list, and the Session hand-off with its terminal tail.

use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::text::span;
use crate::model::work::{self, Dependency, Inspector, NowTab, OwnerNext, Stage, Story};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice::skeleton;
use crate::ui::kit::table::{kv, steps};
use crate::ui::kit::text::{dim, h3, mono, plain, rich, rich_small};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::context::tail;
use super::parts::{act, action_id, model_button};

/// The panel key of the inspector that follows the model's selected tab (the two-pane fallback).
pub(super) const FOLLOW: &str = "inspector";

/// The inspector for `tab`, or for the model's selected tab when `None`. The model decides what
/// shows: the loading placeholders, or the tab's content for the selected task.
pub(super) fn panel(app: AppState, tab: Option<NowTab>) -> AnyElement {
    let inspector = computed(move || {
        let env = app.env();
        app.work.with(|state| {
            let mut state = state.clone();
            if let Some(tab) = tab {
                state.now_tab = tab;
            }
            work::inspector(&state, env)
        })
    });
    dynamic(move || inspector.with(|inspector| content(app, inspector)))
}

/// `.insp`'s padding: 16 20 24 on desktop, 14 18 on the phone, a Cell in on a terminal.
fn padded(app: AppState, child: AnyElement) -> AnyElement {
    let frame = view();
    match app.form {
        Form::Desktop => frame.px(px(20.0)).pt(px(16.0)).pb(px(24.0)),
        Form::Phone => frame.px(px(18.0)).py(px(14.0)),
        Form::Terminal => frame.px(px(8.0)),
    }
    .child(child)
    .into_element()
}

fn content(app: AppState, inspector: &Inspector) -> AnyElement {
    let body = match inspector {
        Inspector::Loading => skeleton(app, &[(0.3, 10.0), (0.6, 18.0), (1.0, 60.0)]),
        Inspector::Story(story) => self::story(app, story),
        Inspector::Evidence(evidence) => view()
            .child(h3(app, evidence.title.clone(), None))
            .child(steps(
                app,
                evidence
                    .artifacts
                    .iter()
                    .map(|artifact| {
                        (
                            artifact.mark,
                            Hue::Neutral,
                            plain(app, artifact.label).into_element(),
                        )
                    })
                    .collect(),
            ))
            .child(
                view()
                    .flex_row()
                    .pt(px(10.0))
                    .child(illus(app, evidence.illustrative)),
            )
            .into_element(),
        Inspector::Session(session) => view()
            .gap(px(if app.form == Form::Terminal {
                0.0
            } else {
                12.0
            }))
            .child(dim(app, session.description))
            .child(view().flex_row().child(model_button(
                app,
                &session.open,
                "btn-session-open".into(),
                false,
            )))
            .child(tail(app, &session.tail, None))
            .into_element(),
    };
    padded(app, body)
}

/// The story of the selected task, as the inspector and the phone's task page show it. The
/// desktop's action row is the model's (the phone page lays its own under the story).
pub(super) fn story(app: AppState, story: &Story) -> AnyElement {
    let form = app.form;
    let mut out: Vec<AnyElement> = vec![head(app, story), title(app, story.title)];
    let goal = if story.facts.is_empty() {
        &story.summary
    } else {
        &story.facts
    };
    if !goal.is_empty() {
        out.push(
            view()
                .pb(px(if form == Form::Terminal { 0.0 } else { 14.0 }))
                .child(rich_small(app, goal))
                .into_element(),
        );
    }
    if let Some(stages) = &story.stages {
        out.push(stepper(app, stages));
    }
    if let Some(progress) = &story.progress {
        out.push(h3(app, progress.label, Some(progress.figure.clone())));
        out.push(steps(
            app,
            story
                .steps
                .iter()
                .map(|&(state, step)| {
                    let words = plain(app, step)
                        .when(state == Mark::Next, |t| t.text_color(color(Token::Ink3)));
                    (state, Hue::Neutral, words.into_element())
                })
                .collect(),
        ));
    }
    out.push(
        view()
            .pt(px(if form == Form::Terminal { 0.0 } else { 14.0 }))
            .child(kv(app, facts(app, story)))
            .into_element(),
    );
    if !story.actions.is_empty() {
        out.push(
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(8.0))
                .pt(px(16.0))
                .children(
                    story
                        .actions
                        .iter()
                        .map(|button| {
                            let id = button.action.map_or_else(String::new, action_id);
                            model_button(app, button, id, false)
                        })
                        .collect::<Vec<_>>(),
                )
                .into_element(),
        );
    }
    view().children(out).into_element()
}

/// The id, the plan it belongs to (a press opens the plan), and the changed-since chip.
fn head(app: AppState, story: &Story) -> AnyElement {
    let form = app.form;
    let plan = story
        .plan_label
        .clone()
        .zip(story.plan_action)
        .map(|(label, action)| {
            let words = text(label.clone())
                .number_of_lines(1u32)
                .text_color(color(Token::Ink3));
            let words = if form == Form::Terminal {
                words
            } else {
                words.font_family(FONT_TEXT).text_size(px(12.0))
            };
            view()
                .cursor_pointer()
                .role(AccessibilityRole::Link)
                .label(label)
                .test_id(&action_id(action))
                .on_press(move || act(app, action))
                .child(words)
                .into_element()
        });
    view()
        .flex_row()
        .items_center()
        .flex_wrap()
        .gap(px(8.0))
        .child(mono(app, story.id).text_color(color(Token::Ink3)))
        .children(plan)
        .children(
            story
                .changed
                .map(|changed| chip(app, changed, Hue::Neutral)),
        )
        .into_element()
}

/// The task's title (`.insp h2`): 16 pt bold on desktop, 18 pt on the phone, bold in a terminal.
fn title(app: AppState, title: &'static str) -> AnyElement {
    let words = text(title)
        .role(AccessibilityRole::Header)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    match app.form {
        Form::Terminal => words.into_element(),
        form => words
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 18.0 } else { 16.0 }))
            .letter_spacing(px(-0.16))
            .mt(px(2.0))
            .mb(px(2.0))
            .into_element(),
    }
}

/// Run, gate, review, land side by side (`.stepper`): each stage's name in faint capitals, its
/// mark and state word, its detail in mono; the active stage under a 2 pt line in its hue.
fn stepper(app: AppState, stages: &[Stage; 4]) -> AnyElement {
    let form = app.form;
    let phone = form == Form::Phone;
    let columns = stages.iter().map(|stage| {
        let edge = if stage.active {
            hue_ink(stage.hue)
        } else {
            color(Token::Rule)
        };
        let label = text(stage.label)
            .uppercase()
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3));
        let word = text(stage.word)
            .number_of_lines(1u32)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color(Token::Ink));
        let detail = text(stage.detail.clone())
            .number_of_lines(1u32)
            .truncate()
            .text_color(color(Token::Ink3));
        let (label, word, detail) = if form == Form::Terminal {
            (label, word, detail)
        } else {
            (
                label
                    .font_family(FONT_TEXT)
                    .text_size(px(if phone { 12.0 } else { 11.0 }))
                    .letter_spacing(px(0.66)),
                word.font_family(FONT_TEXT)
                    .text_size(px(if phone { 16.0 } else { 13.0 })),
                detail
                    .font_family(FONT_MONO)
                    .text_size(px(if phone { 12.0 } else { 11.0 })),
            )
        };
        let mark_hue = if stage.active {
            stage.hue
        } else {
            Hue::Neutral
        };
        view()
            .flex_1()
            .min_w(px(0.0))
            .when(form != Form::Terminal, |column| {
                column
                    .pr(px(10.0))
                    .pt(px(if stage.active { 8.0 } else { 9.0 }))
                    .border_t(px(if stage.active { 2.0 } else { 1.0 }))
                    .border_t_color(edge)
            })
            .role(AccessibilityRole::Text)
            .label(format!("{} {} {}", stage.label, stage.word, stage.detail))
            .child(label)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .py(px(if form == Form::Terminal { 0.0 } else { 2.0 }))
                    .child(mark(app, stage.mark, mark_hue))
                    .child(word),
            )
            .child(detail)
            .into_element()
    });
    view()
        .flex_row()
        .gap(px(if form == Form::Terminal { 8.0 } else { 0.0 }))
        .pt(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .pb(px(if form == Form::Terminal { 0.0 } else { 16.0 }))
        .children(columns.collect::<Vec<_>>())
        .into_element()
}

/// The label-value facts: detailed stories Next, You, Needs, Unblocks, Cost; a queued task's
/// Waits on, Unblocks, You.
fn facts(app: AppState, story: &Story) -> Vec<(String, AnyElement)> {
    let needs = if story.needs.is_empty() {
        story.needs_fallback.map_or_else(
            || ids(app, &[]),
            |fallback| plain(app, fallback).into_element(),
        )
    } else {
        ids(app, &story.needs)
    };
    let unblocks = if story.unblocks.is_empty() {
        plain(app, story.unblocks_fallback).into_element()
    } else {
        ids(app, &story.unblocks)
    };
    let owner = match &story.owner {
        OwnerNext::Answer(button) => view()
            .flex_row()
            .child(model_button(app, button, "btn-answer".into(), false))
            .into_element(),
        OwnerNext::Nothing { label, hue } => rich(app, &vec![span(*label).hue(*hue)]),
    };
    if story.stages.is_none() {
        return vec![
            (story.needs_label.into(), needs),
            (story.unblocks_label.into(), unblocks),
            (story.owner_label.into(), owner),
        ];
    }
    vec![
        (story.next_label.into(), rich(app, &story.next)),
        (story.owner_label.into(), owner),
        (story.needs_label.into(), needs),
        (story.unblocks_label.into(), unblocks),
        (story.cost_label.into(), rich(app, &story.cost)),
    ]
}

/// Task ids with their marks where the model shows them; pressing an id opens that task.
fn ids(app: AppState, dependencies: &[Dependency]) -> AnyElement {
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(10.0))
        .children(
            dependencies
                .iter()
                .map(|dependency| {
                    let action = dependency.action;
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .cursor_pointer()
                        .role(AccessibilityRole::Link)
                        .label(format!("{} {}", dependency.id, dependency.mark.label()))
                        .test_id(&format!("btn-dep-{}", dependency.id))
                        .on_press(move || act(app, action))
                        .child(mono(app, dependency.id))
                        .when(dependency.show_mark, |link| {
                            link.child(mark(app, dependency.mark, Hue::Neutral))
                        })
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}
