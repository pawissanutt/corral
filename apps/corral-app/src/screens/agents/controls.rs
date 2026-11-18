//! The strip under a run's head: its session state chip, what a blocked screen waits on, and
//! the Actions menu of the run's task, the shared table of every engine operation (Reply, Stop,
//! Retry and the rest), never a list of its own. Desktop: one strip, wrapping when narrow.
//! Phone: the chip and the menu in one wrapping row under the view switch, so the screen keeps
//! the page. Terminal: one row, then a rule.

use nsh_core::prelude::*;

use crate::model::agents::{Bar, Session};
use crate::model::work::{Record, VERB_MENU_LABEL};
use crate::model::{Form, Id};
use crate::screens::now::verb_menu;
use crate::screens::plans::menu_trigger;
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The strip, redrawn only when it changes: a new screen or transcript page leaves its focus.
pub fn bar(app: AppState, session: Memo<Session>) -> AnyElement {
    let shown = computed(move || session.with(|s| s.controls.clone()));
    view()
        .flex_none()
        .child(dynamic(move || shown.with(|bar| strip(app, bar))))
        .into_element()
}

/// The Actions menu of `task` (test id `btn-session-actions`; its items `btn-<verb slug>`).
pub(super) fn actions(app: AppState, task: Id) -> AnyElement {
    let spoken = format!("{VERB_MENU_LABEL} for {task}");
    verb_menu(
        app,
        Record::Task(task),
        menu_trigger(app, VERB_MENU_LABEL, spoken, "btn-session-actions"),
    )
}

/// The strip for the form; nothing for a run corrald does not list.
fn strip(app: AppState, bar: &Bar) -> AnyElement {
    let Some((state, hue)) = bar.state else {
        return view().into_element();
    };
    let lead = bar.lead.clone();
    let menu = bar.task.clone().map(|task| actions(app, task));
    let root = view()
        .test_id("session-controls")
        .role(AccessibilityRole::Toolbar)
        .label("Session")
        .flex_none();
    match app.form {
        Form::Terminal => root
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .h(term::rows(1.0))
                    .px(term::cells(1.0))
                    .child(chip(app, state, hue))
                    .children(lead.map(|lead| {
                        view()
                            .flex_row()
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                            .child(term::space(2.0))
                            .child(term::quiet(lead).flex_shrink(1.0).min_w(px(0.0)))
                    }))
                    .child(view().flex_1().min_w(term::cells(2.0)))
                    .children(menu),
            )
            .child(term::hrule())
            .into_element(),
        Form::Phone => root
            .px(px(16.0))
            .pb(px(8.0))
            .gap(px(6.0))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(chip(app, state, hue))
                    .child(view().flex_1().min_w(px(0.0)))
                    .children(menu),
            )
            .children(lead.map(|lead| {
                text(lead)
                    .font_family(FONT_TEXT)
                    .text_size(px(12.5))
                    .text_color(color(Token::Ink2))
            }))
            .into_element(),
        Form::Desktop => root
            .px(px(18.0))
            .py(px(8.0))
            .bg(color(Token::Bg))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(chip(app, state, hue))
                    .children(lead.map(|lead| {
                        text(lead)
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                            .font_family(FONT_TEXT)
                            .text_size(px(12.5))
                            .text_color(color(Token::Ink2))
                    }))
                    .child(view().flex_1().min_w(px(0.0)))
                    .children(menu),
            )
            .into_element(),
    }
}
