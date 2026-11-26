//! The Columns control in the Tasks head (`model::tasks::columns`): a menu from one trigger
//! (`btn-columns`) with a checkable row per optional column (`seg-column-<key>`), which stays
//! open while columns are toggled, and Restore default columns (`btn-columns-reset`). Desktop
//! and terminal only; the phone's rows have no columns.

use nsh_base::{menu, menu_item};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::tasks::columns::{self, Choice};
use crate::state::AppState;
use crate::ui::kit::button::menu_trigger;
use crate::ui::kit::overlay::counted;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

const MENU_MIN: f32 = 200.0;
const MARK: f32 = 16.0;

/// The trigger and its menu.
pub fn picker(app: AppState) -> AnyElement {
    let form = app.form;
    menu()
        .popup_style(move |s| {
            let s = s
                .bg(color(Token::Surface))
                .border_1()
                .border_color(color(Token::Rule2));
            match form {
                Form::Desktop | Form::Phone => s
                    .min_w(px(MENU_MIN))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .shadow_color(color(Token::Shell)),
                Form::Terminal => s.min_w(term::cells(26.0)),
            }
        })
        .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
        .on_open_change(counted(app))
        .trigger(menu_trigger(
            app,
            columns::CONTROL,
            columns::CONTROL_LABEL,
            "btn-columns",
        ))
        // The offered columns are read when the menu opens: a scope changed since is followed.
        .child(dynamic(move || untracked(move || items(app))))
        .into_element()
}

fn items(app: AppState) -> AnyElement {
    let scope = app.project_scope();
    let choices = app
        .task_columns
        .with(|chosen| columns::choices(chosen, &scope));
    view()
        .children(
            choices
                .into_iter()
                .map(|choice| item(app, choice))
                .collect::<Vec<_>>(),
        )
        .child(reset(app))
        .into_element()
}

fn content(form: Form, words: AnyElement) -> View {
    match form {
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(words),
        Form::Desktop | Form::Phone => view()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(12.0))
            .py(px(6.0))
            .child(words),
    }
}

fn words(form: Form, label: &'static str) -> AnyElement {
    let words = text(label)
        .number_of_lines(1u32)
        .truncate()
        .text_color(color(Token::Ink));
    match form {
        Form::Terminal => words.into_element(),
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(12.5))
            .into_element(),
    }
}

/// One optional column: checked while it shows. A terminal draws its own check mark at the
/// row's end; the desktop's leads the row.
fn item(app: AppState, choice: Choice) -> AnyElement {
    let form = app.form;
    let Choice { key, label, .. } = choice;
    let on = computed(move || app.task_columns.with(|chosen| chosen.shows(key)));
    let mark = (form != Form::Terminal).then(|| {
        text(move || if on.get() { "✓" } else { "" })
            .w(px(MARK))
            .text_color(color(Token::Ink2))
            .into_element()
    });
    let row = content(form, words(form, label));
    let row = match mark {
        Some(mark) => view()
            .flex_row()
            .items_center()
            .pl(px(8.0))
            .child(mark)
            .child(row),
        None => view().child(row),
    };
    menu_item()
        .checked(move || Some(on.get()))
        .close_on_select(false)
        .accessibility_label(label)
        .on_select(move || app.toggle_task_column(key))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .child(row.test_id(&format!("seg-column-{key}")))
        .into_element()
}

fn reset(app: AppState) -> AnyElement {
    let form = app.form;
    menu_item()
        .accessibility_label(columns::RESET)
        .on_select(move || app.reset_task_columns())
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .child(
            content(form, words(form, columns::RESET))
                .border_t_1()
                .border_color(color(Token::Rule))
                .test_id("btn-columns-reset"),
        )
        .into_element()
}
