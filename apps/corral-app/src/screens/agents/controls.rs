//! The strip under a run's head: its session state chip and what a blocked screen waits on; and
//! the run's Session menu (Reply, Stop, Retry, Focus pane, then its task's Actions menu), which
//! desktop and terminal put in the page head and the phone keeps in this strip. Desktop: one
//! strip, wrapping when narrow. Phone: the chip and the menu in one wrapping row under the view
//! switch, so the screen keeps the page. Terminal: one row, then a rule.

use nsh_core::prelude::*;

use crate::model::agents::{Bar, SESSION_MENU_LABEL, Session};
use crate::model::work::Record;
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

/// The Session menu of run session `session` (test id `btn-session-menu`; its items
/// `btn-<slug>`, the task's Actions under `btn-task-actions`).
pub(super) fn session_menu(app: AppState, session: Id) -> AnyElement {
    let spoken = format!("{SESSION_MENU_LABEL} menu for {session}");
    verb_menu(
        app,
        Record::Session(session),
        menu_trigger(app, SESSION_MENU_LABEL, spoken, "btn-session-menu"),
    )
}

/// The strip for the form; nothing for a run corrald does not list.
fn strip(app: AppState, bar: &Bar) -> AnyElement {
    let Some((state, hue)) = bar.state else {
        return view().into_element();
    };
    let lead = bar.lead.clone();
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
                    })),
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
                    .children(bar.menu.clone().map(|id| session_menu(app, id))),
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
                    })),
            )
            .into_element(),
    }
}
