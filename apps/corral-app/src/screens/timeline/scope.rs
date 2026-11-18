//! The goal burn-up's scope (RC-21): the whole workspace, one project or one plan, from the
//! projects and plans corrald serves (`timeline::scope_options`). A workspace can hold many
//! plans, so the scopes are a menu from one trigger naming the current scope rather than a row
//! of segments that would run off the page. Choosing one runs the model's `set_burnup_scope`,
//! and `shown` requests that scope's burn-up. test ids: "btn-burnup-scope" for the trigger,
//! "seg-burnup-<scope id>" for each scope.

use nsh_base::{menu, menu_item};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::timeline::{self, BURNUP_SCOPE_LABEL, ScopeOption};
use crate::screens::plans::menu_trigger;
use crate::state::AppState;
use crate::ui::kit::overlay::counted;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The phone's least touch target.
const TARGET: f32 = 44.0;
/// The desktop menu's width range: room for "Project · <name>" without a page-wide line.
const MENU_MIN: f32 = 200.0;
const MENU_MAX: f32 = 320.0;

/// The scope menu, its trigger naming the scope shown.
pub fn picker(app: AppState) -> AnyElement {
    let form = app.form;
    let current = computed(move || {
        app.feed.with(|feed| {
            app.timeline.with(|state| {
                timeline::scope_options(state, feed)
                    .into_iter()
                    .find(|option| option.selected)
                    .map(|option| option.label)
                    .unwrap_or_default()
            })
        })
    });
    dynamic(move || {
        let label = current.get();
        untracked(move || {
            let spoken = format!("{BURNUP_SCOPE_LABEL}: {label}");
            menu()
                .popup_style(move |s| {
                    let s = s
                        .bg(color(Token::Surface))
                        .border_1()
                        .border_color(color(Token::Rule2));
                    match form {
                        Form::Desktop => s
                            .min_w(px(MENU_MIN))
                            .max_w(px(MENU_MAX))
                            .py(px(4.0))
                            .rounded(px(8.0))
                            .shadow_lg()
                            .shadow_color(color(Token::Shell)),
                        Form::Phone => s.rounded_t(px(14.0)).pt(px(8.0)).pb(px(24.0)),
                        Form::Terminal => s.min_w(term::cells(24.0)),
                    }
                })
                .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
                .on_open_change(counted(app))
                .trigger(menu_trigger(app, label, spoken, "btn-burnup-scope"))
                // The scopes are read when the menu opens: a plan published since is there.
                .child(dynamic(move || untracked(move || items(app))))
                .into_element()
        })
    })
    .into_element()
}

fn items(app: AppState) -> AnyElement {
    let options = app.feed.with(|feed| {
        app.timeline
            .with(|state| timeline::scope_options(state, feed))
    });
    view()
        .children(
            options
                .into_iter()
                .map(|option| item(app, option))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// One scope: its words, bold while it is the one shown.
fn item(app: AppState, option: ScopeOption) -> AnyElement {
    let form = app.form;
    let ScopeOption {
        id,
        label,
        scope,
        selected,
    } = option;
    let words = text(label.clone())
        .number_of_lines(1u32)
        .truncate()
        .text_color(color(Token::Ink))
        .when(selected, |t| t.font_weight(FontWeight::BOLD));
    let content = match form {
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(words),
        Form::Desktop => view()
            .px(px(12.0))
            .py(px(6.0))
            .child(words.font_family(FONT_TEXT).text_size(px(12.5))),
        Form::Phone => view()
            .justify_center()
            .min_h(px(TARGET + 4.0))
            .px(px(18.0))
            .py(px(8.0))
            .child(words.font_family(FONT_TEXT).text_size(px(16.0))),
    };
    menu_item()
        .accessibility_label(if selected {
            format!("{label}, shown")
        } else {
            label
        })
        .on_select(move || {
            let scope = scope.clone();
            app.act(app.timeline, move |state, _| {
                state.set_burnup_scope(scope.clone())
            })
        })
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .child(
            content
                .when(selected, |c| c.bg(color(Token::Sel)))
                .test_id(&format!("seg-burnup-{id}")),
        )
        .into_element()
}
