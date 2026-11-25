//! The plan, as the sidebar's Plan tab (`session::PlanTab`) and the Plan ledger sheet
//! (`session::Ledger`). The tab: the heading `Plan <title> · r<n>` with `d/n done`, a stale
//! ledger named stale, finished steps folded into `✓ n done` when the plan is long, each step
//! with its mark and `…` when it waits, the selected step's id and criteria, waits and drop
//! reason under it, and the auto-continue record. The heading or a step opens the sheet: every
//! step with its state, criteria or drop reason and what it waits for, the auto-continue record
//! and the earlier plans the host kept, with Reread. Read-only. test ids `side-plan-heading`,
//! `side-step-<n>`, `ledger` (`ledger-done`, `ledger-step-<n>`, `btn-reread-plan`).

use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::session::{
    Action, Ledger, LedgerStep, PlanAction, PlanTab, SidebarAction, StepRow, ledger,
};
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::mark;
use crate::ui::kit::notice::notice;
use crate::ui::kit::overlay::{self, Modal};
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::{dim, h3, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::super::run;
use super::hue_words;
use super::sidebar::{inset, with_cx};

/// The sidebar's Plan tab.
pub(super) fn tab(app: AppState, tab: &PlanTab) -> AnyElement {
    let form = app.form;
    let mut out: Vec<AnyElement> = Vec::new();
    match (&tab.heading, &tab.progress) {
        (Some(heading), progress) => {
            let open = tab.open.clone();
            out.push(list_row(
                app,
                format!(
                    "{heading}{}, open the plan ledger",
                    progress
                        .as_ref()
                        .map(|progress| format!(", {progress}"))
                        .unwrap_or_default()
                ),
                "side-plan-heading",
                || false,
                move || run(app, open.clone()),
                vec![
                    plain(app, heading.clone())
                        .font_weight(FontWeight::SEMIBOLD)
                        .number_of_lines(1u32)
                        .truncate()
                        .min_w(px(0.0))
                        .flex_1()
                        .into_element(),
                    dim(app, progress.clone().unwrap_or_default())
                        .number_of_lines(1u32)
                        .flex_shrink_0()
                        .into_element(),
                ],
            ));
        }
        (None, Some(progress)) => out.push(inset(app, dim(app, progress.clone()))),
        (None, None) => {}
    }
    if let Some(stale) = &tab.stale {
        out.push(inset(
            app,
            notice(app, Hue::Held, &vec![span(stale.clone())], None),
        ));
    }
    if let Some(empty) = &tab.empty {
        out.push(inset(app, dim(app, empty.clone())));
    }
    if let Some(folded) = &tab.folded {
        out.push(inset(
            app,
            view()
                .flex_row()
                .items_center()
                .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
                .pl(if form == Form::Terminal {
                    term::cells(1.0)
                } else {
                    px(4.0)
                })
                .child(mark(app, Mark::Done, Hue::Ok))
                .child(dim(app, folded.clone())),
        ));
    }
    for step in &tab.steps {
        out.push(step_row(app, step));
    }
    if let Some((hue, continued)) = &tab.continued {
        out.push(inset(
            app,
            dim(app, continued.clone()).text_color(hue_words(form, *hue)),
        ));
    }
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 2.0 }))
        .pb(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .children(out)
        .into_element()
}

fn step_row(app: AppState, step: &StepRow) -> AnyElement {
    let (glyph, hue) = step.state.mark();
    let action = step.action.clone();
    let selected = step.selected;
    let mut content = vec![
        mark(app, glyph, hue),
        plain(app, step.text.clone())
            .number_of_lines(1u32)
            .truncate()
            .min_w(px(0.0))
            .flex_1()
            .text_color(color(if selected { Token::Ink } else { Token::Ink2 }))
            .into_element(),
    ];
    if step.wait.is_some() {
        content.push(dim(app, "…").flex_shrink_0().into_element());
    }
    let row = list_row(
        app,
        match &step.wait {
            Some(wait) => format!("{}, waits for {wait}", step.text),
            None => step.text.clone(),
        },
        step.test_id.clone(),
        move || selected,
        move || run(app, action.clone()),
        content,
    );
    if step.detail.is_empty() {
        return row;
    }
    let indent = if app.form == Form::Terminal {
        term::cells(4.0)
    } else {
        px(28.0)
    };
    view()
        .child(row)
        .child(
            view().pl(indent).pr(px(8.0)).children(
                step.detail
                    .iter()
                    .map(|line| dim(app, line.clone()).number_of_lines(2u32).into_element())
                    .collect::<Vec<_>>(),
            ),
        )
        .into_element()
}

pub(super) fn in_page(app: AppState, id: &Id) -> bool {
    app.session
        .with(|state| state.ui(id.as_str()).sidebar.plan.in_page)
}

/// The ledger opened by a link in the pane, in the main page instead of a window-wide sheet.
pub(super) fn page(app: AppState, id: Id) -> AnyElement {
    let close = id.clone();
    let shown = computed(move || with_cx(app, &id, ledger));
    view()
        .test_id("ledger")
        .flex_1()
        .min_h(px(0.0))
        .p(px(14.0))
        .child(button(
            app,
            crate::model::dock::CLOSE_PLAN,
            ButtonKind::Quiet,
            "ledger-done",
            move || {
                run(
                    app,
                    Action::Sidebar(SidebarAction::Plan(PlanAction::Close(close.clone()))),
                );
            },
        ))
        .child(scroll().flex_1().min_h(px(0.0)).child(dynamic(move || {
            shown.get().map(|ledger| untracked(|| body(app, &ledger)))
        })))
        .into_element()
}

/// The Plan ledger sheet of session `id`, open while its model says so.
pub(super) fn sheet(app: AppState, id: Id) -> AnyElement {
    let opened = id.clone();
    let closed = id.clone();
    overlay::modal(
        app,
        move || {
            app.session.with(|state| {
                let plan = &state.ui(opened.as_str()).sidebar.plan;
                plan.ledger && !plan.in_page
            })
        },
        move || {
            run(
                app,
                Action::Sidebar(SidebarAction::Plan(PlanAction::Close(closed.clone()))),
            )
        },
        move || modal(app, id.clone()),
    )
}

fn modal(app: AppState, id: Id) -> Modal {
    let shown = computed(move || with_cx(app, &id, ledger));
    let body = dynamic(move || shown.get().map(|sheet| untracked(|| body(app, &sheet))));
    let reread =
        computed(move || shown.with(|sheet| sheet.as_ref().and_then(|s| s.reread.clone())));
    let foot = dynamic(move || {
        reread.get().map(|action| {
            untracked(|| {
                button(
                    app,
                    "Reread plan",
                    ButtonKind::Ghost,
                    "btn-reread-plan",
                    move || run(app, action.clone()),
                )
            })
        })
    });
    Modal {
        test_id: "ledger".into(),
        title: "Plan ledger".into(),
        sub: None,
        body,
        foot: Some(view().flex_row().child(foot).into_element()),
        done: Some("Close".into()),
        confirm: None,
    }
}

fn body(app: AppState, sheet: &Ledger) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let mut out: Vec<AnyElement> = vec![
        plain(app, sheet.title.clone())
            .font_weight(FontWeight::SEMIBOLD)
            .into_element(),
        dim(app, sheet.sub.clone()).into_element(),
    ];
    if let Some(why) = &sheet.unavailable {
        out.push(notice(
            app,
            Hue::Held,
            &vec![span(format!("unavailable: {why}"))],
            None,
        ));
    }
    if let Some(stale) = &sheet.stale {
        out.push(notice(app, Hue::Held, &vec![span(stale.clone())], None));
    }
    if let Some(empty) = &sheet.empty {
        out.push(dim(app, empty.clone()).into_element());
    }
    if !sheet.steps.is_empty() {
        out.push(h3(app, "Steps", Some(sheet.steps.len().to_string())));
        out.push(
            view()
                .gap(px(if terminal { 0.0 } else { 2.0 }))
                .children(
                    sheet
                        .steps
                        .iter()
                        .map(|step| ledger_step(app, step))
                        .collect::<Vec<_>>(),
                )
                .into_element(),
        );
    }
    out.push(h3(app, "Auto-continue", None));
    let (hue, continued) = &sheet.continued;
    out.push(
        dim(app, continued.clone())
            .text_color(hue_words(form, *hue))
            .into_element(),
    );
    out.push(h3(
        app,
        "Earlier plans",
        (!sheet.history.is_empty()).then(|| sheet.history.len().to_string()),
    ));
    if let Some(empty) = &sheet.history_empty {
        out.push(dim(app, empty.clone()).into_element());
    }
    out.extend(sheet.history.iter().map(|line| {
        view()
            .flex_row()
            .gap(px(if terminal { 16.0 } else { 10.0 }))
            .child(
                mono(app, line.at.clone())
                    .flex_shrink_0()
                    .text_color(color(Token::Ink3)),
            )
            .child(
                mono(app, line.revision.clone())
                    .flex_shrink_0()
                    .text_color(color(Token::Ink3)),
            )
            .child(plain(app, line.summary.clone()).flex_1().min_w(px(0.0)))
            .into_element()
    }));
    view()
        .gap(px(if terminal { 0.0 } else { 6.0 }))
        .children(out)
        .into_element()
}

fn ledger_step(app: AppState, step: &LedgerStep) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let (glyph, hue) = step.state.mark();
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(mark(app, glyph, hue))
        .child(plain(app, step.text.clone()).flex_1().min_w(px(0.0)))
        .child(
            dim(app, step.state_word.clone())
                .number_of_lines(1u32)
                .flex_shrink_0(),
        );
    let indent = if terminal { term::cells(2.0) } else { px(20.0) };
    let facts = view()
        .pl(indent)
        .child(mono(app, format!("{} · {}", step.id, step.detail)).text_color(color(Token::Ink3)))
        .children(
            step.wait
                .clone()
                .map(|wait| dim(app, format!("waits for: {wait}"))),
        );
    view()
        .test_id(&step.test_id)
        .accessibility_state(AccessibilityState {
            selected: step.selected,
            ..AccessibilityState::default()
        })
        .when(!terminal, |block| {
            block.py(px(4.0)).px(px(6.0)).rounded(px(6.0))
        })
        .when(step.selected, |block| block.bg(color(Token::Sel)))
        .child(head)
        .child(facts)
        .into_element()
}
