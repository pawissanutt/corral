//! A build's exit: the exit choice's summary (what the branch holds, a warning such as
//! uncommitted changes in Held), and once the owner chose, where the exit stands
//! (`session::ExitProgress`): the step rail (rebase, full gate, land) with a spinner on the
//! running step, each step's note and time muted after it and the gate's log under the gate
//! step, a failed gate's repair round in Held, and the outcome: what was landed or kept, or what
//! failed, what to do next and Finish… to choose again. Every word is the host's, as the model
//! reads it. test ids `session-exit`, `exit-step-<n>`, `gate-log`, `exit-outcome`, `btn-finish`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{ExitProgress, ExitSummary, SessionPage, StepState};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions};
use crate::ui::kit::marks::hue_ink;
use crate::ui::tokens::{Token, color};

use super::super::transcript::gate_log;
use super::cards::{press, quiet, tint, words};
use super::state_mark;

/// The exit choice's summary lines: the branch's line, then each warning in Held.
pub(super) fn summary(app: AppState, summary: &ExitSummary) -> Vec<AnyElement> {
    let form = app.form;
    summary
        .line
        .iter()
        .map(|line| words(form, line.clone(), color(Token::Ink2)).into_element())
        .chain(
            summary
                .warnings
                .iter()
                .map(|warning| words(form, warning.clone(), hue_ink(Hue::Held)).into_element()),
        )
        .collect()
}

/// Where the build's exit stands, in the dock while it runs or has an outcome.
pub(super) fn progress(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || page.with(|p| p.exit.clone()));
    dynamic(move || shown.get().map(|exit| untracked(|| face(app, exit))))
}

fn face(app: AppState, exit: ExitProgress) -> AnyElement {
    let form = app.form;
    let steps: Vec<AnyElement> =
        exit.steps
            .iter()
            .enumerate()
            .map(|(n, step)| {
                let running = step.state == StepState::Running;
                let ink = match step.state {
                    StepState::Waiting => color(Token::Ink3),
                    _ => color(Token::Ink),
                };
                let row =
                    view()
                        .test_id(&format!("exit-step-{n}"))
                        .flex_row()
                        .items_center()
                        .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
                        .min_w(px(0.0))
                        .children(state_mark(app, running, step.glyph, step.hue))
                        .child(
                            words(form, step.label.clone(), ink)
                                .flex_shrink_0()
                                .when(running, |t| t.font_weight(FontWeight::SEMIBOLD)),
                        )
                        .children(step.note.clone().map(|note| {
                            quiet(form, note).flex_shrink(1.0).min_w(px(0.0)).truncate()
                        }))
                        .children(
                            step.time
                                .clone()
                                .map(|time| quiet(form, time).flex_shrink_0()),
                        );
                view()
                    .min_w(px(0.0))
                    .child(row)
                    .children(step.log.as_ref().map(|log| gate_log(app, log)))
                    .into_element()
            })
            .collect();
    let rail = (!steps.is_empty()).then(|| {
        view()
            .role(AccessibilityRole::List)
            .label("Exit steps")
            .gap(px(if form == Form::Terminal { 0.0 } else { 3.0 }))
            .children(steps)
            .into_element()
    });
    let detail = exit
        .detail
        .map(|(hue, said)| words(form, said, tint(hue)).into_element());
    let outcome = exit.outcome.map(|outcome| {
        view()
            .test_id("exit-outcome")
            .gap(px(2.0))
            .child(
                words(form, outcome.text, hue_ink(outcome.hue)).font_weight(FontWeight::SEMIBOLD),
            )
            .children(outcome.next.map(|next| quiet(form, next)))
            .into_element()
    });
    let finish = exit.finish.map(|action| {
        view()
            .flex_row()
            .child(press(
                app,
                "Finish…",
                ButtonKind::Ghost,
                "btn-finish",
                action,
                ButtonOptions::default(),
            ))
            .into_element()
    });
    let block = view()
        .test_id("session-exit")
        .role(AccessibilityRole::Summary)
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .children(rail)
        .children(detail)
        .children(outcome)
        .children(finish);
    match form {
        Form::Terminal => block.into_element(),
        Form::Desktop | Form::Phone => block
            .px(px(12.0))
            .py(px(10.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(color(Token::Rule2))
            .bg(color(Token::Surface))
            .into_element(),
    }
}
