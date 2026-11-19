//! A plan's Actions menu (a plan is its parent task, so every engine operation applies): the
//! menu at the end of the detail head and the context menu of its list row (desktop and
//! terminal: the secondary press, Shift+F10 or the Menu key; phone: a held finger opens the
//! action sheet). Both are Now's verb menus over `work::record_verbs`, so no screen keeps a
//! second list of commands. A verb with fields opens the shared verb composer the shells host.
//!
//! Also the press-opened menu's trigger, shared with Review and Timeline's burn-up scope.

use nsh_core::prelude::*;

use crate::model::work::{Record, VERB_MENU_LABEL};
use crate::model::{Form, Id};
use crate::screens::now::{verb_menu, with_menu};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The phone's least touch target.
const TARGET: f32 = 44.0;
const RING: f32 = 2.0;

/// The Actions menu of the plan `plan`, for the detail head. test_id "btn-plan-actions"; the
/// items are "btn-<verb slug>".
pub(super) fn head_menu(app: AppState, plan: Id) -> AnyElement {
    let spoken = format!("{VERB_MENU_LABEL} for {plan}");
    verb_menu(
        app,
        Record::Task(plan),
        menu_trigger(app, VERB_MENU_LABEL, spoken, "btn-plan-actions"),
    )
}

/// `row` with the verb menu of the served plan `plan`.
pub(super) fn row_menu(app: AppState, plan: Id, row: AnyElement) -> AnyElement {
    with_menu(app, Record::Task(plan), row)
}

/// What opens a press-opened menu: outlined as a ghost button, the words then a downward
/// chevron; 30 pt high on desktop, at least 44 on the phone, a band of Cells in a terminal. The
/// menu owns the press, so this is a focusable button for screen readers named `spoken`, not a
/// kit button with a press of its own.
pub(crate) fn menu_trigger(
    app: AppState,
    words: impl Into<SharedString>,
    spoken: impl Into<SharedString>,
    test_id: &str,
) -> AnyElement {
    let form = app.form;
    let spoken: SharedString = spoken.into();
    let words = text(words.into())
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .text_color(color(Token::Ink));
    let target = view()
        .test_id(test_id)
        .role(AccessibilityRole::Button)
        .label(spoken)
        .accessible(true)
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .min_w(px(0.0))
        .cursor_pointer();
    match form {
        Form::Terminal => target
            .px(term::cells(1.0))
            .bg(color(Token::Surface))
            .child(words)
            .child(term::space(1.0))
            .child(term::quiet("▾"))
            .into_element(),
        Form::Desktop | Form::Phone => {
            let phone = form == Form::Phone;
            target
                .gap(px(6.0))
                .when(phone, |t| {
                    t.min_h(px(TARGET)).px(px(16.0)).rounded(px(10.0))
                })
                .when(!phone, |t| t.h(px(30.0)).px(px(12.0)).rounded(px(6.0)))
                .border_1()
                .border_color(color(Token::Rule2))
                .hover(|s| s.bg(color(Token::Hover)))
                .active(|s| s.bg(color(Token::Sel)))
                .focus_visible(|s| {
                    s.outline(px(RING))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(RING))
                })
                .child(
                    words
                        .font_family(FONT_TEXT)
                        .text_size(px(if phone { 15.0 } else { 12.5 }))
                        .font_weight(if phone {
                            FontWeight::BOLD
                        } else {
                            FontWeight::SEMIBOLD
                        }),
                )
                .child(
                    view()
                        .rotate(deg(90.0))
                        .accessibility_elements_hidden(true)
                        .child(icon(
                            Icon::Chevron,
                            if phone { 14.0 } else { 12.0 },
                            color(Token::Ink2),
                        )),
                )
                .into_element()
        }
    }
}
