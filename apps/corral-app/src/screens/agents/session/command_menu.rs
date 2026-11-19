//! The `/` command menu: while the draft starts with `/`, the session's catalog rows
//! (`session::SessionActions`) whose terminal command or label matches what follows (`/fin` →
//! Finish…) list over the composer, each with its command on the right; a row the host cannot
//! run now stays listed, muted with its reason. Enter runs the first row that can run and clears
//! the `/text`; pressing a row does the same. A `/text` no row matches is sent to the host as
//! typed, so the host's own commands answer or refuse there. test ids `cmd-<slug>`.

use nsh_core::prelude::*;

use crate::model::session::{Action, ActionRow, SessionPage};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;

/// At most this many rows show.
const SHOWN: usize = 8;

/// What follows the `/` of a draft that starts with one, lower-cased; none otherwise.
fn query(draft: &str) -> Option<String> {
    let rest = draft.strip_prefix('/')?;
    (!rest.contains('\n')).then(|| rest.trim().to_lowercase())
}

/// The catalog rows the draft's `/text` matches, in catalog order.
pub(super) fn matches(page: Memo<SessionPage>) -> Memo<Vec<ActionRow>> {
    computed(move || {
        page.with(|p| {
            let Some(query) = p.composer.as_ref().and_then(|c| query(&c.draft)) else {
                return Vec::new();
            };
            p.actions
                .rows()
                .filter(|row| {
                    row.command.is_some_and(|command| {
                        command.trim_start_matches('/').starts_with(query.as_str())
                    }) || row.label.to_lowercase().contains(query.as_str())
                })
                .take(SHOWN)
                .cloned()
                .collect()
        })
    })
}

/// What Enter runs instead of sending: the first matched row that can run, then the `/text`
/// cleared.
pub(super) fn chosen(id: Id, rows: Memo<Vec<ActionRow>>) -> Memo<Option<Vec<Action>>> {
    computed(move || {
        rows.with(|rows| {
            rows.iter()
                .find(|row| row.unavailable.is_none())
                .map(|row| vec![clear(&id), row.action.clone()])
        })
    })
}

fn clear(id: &Id) -> Action {
    Action::Draft {
        session: id.clone(),
        text: String::new(),
    }
}

/// The list over the composer while the `/text` matches rows.
pub(super) fn menu(app: AppState, id: Id, rows: Memo<Vec<ActionRow>>) -> AnyElement {
    let form = app.form;
    dynamic(move || {
        let rows = rows.get();
        if rows.is_empty() {
            return None;
        }
        let id = id.clone();
        Some(untracked(|| {
            let first = rows.iter().position(|row| row.unavailable.is_none());
            let items: Vec<AnyElement> = rows
                .into_iter()
                .enumerate()
                .map(|(at, row)| item(app, &id, row, first == Some(at)))
                .collect();
            let list = view()
                .test_id("session-commands")
                .role(AccessibilityRole::List)
                .label("Commands");
            match form {
                Form::Terminal => list
                    .flex_none()
                    .child(term::hrule())
                    .child(view().px(term::cells(1.0)).children(items))
                    .into_element(),
                Form::Desktop | Form::Phone => view()
                    .flex_none()
                    .px(px(14.0))
                    .pt(px(6.0))
                    .child(
                        list.py(px(4.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(color(Token::Rule2))
                            .bg(color(Token::Surface))
                            .children(items),
                    )
                    .into_element(),
            }
        }))
    })
}

/// One row: its label, why it cannot run (muted) where it cannot, its command at the end; `⏎`
/// before the row Enter runs.
fn item(app: AppState, id: &Id, row: ActionRow, next: bool) -> AnyElement {
    let form = app.form;
    let enabled = row.unavailable.is_none();
    let size = |t: Text, mono: bool| match form {
        Form::Terminal => t,
        _ => t
            .font_family(if mono { FONT_MONO } else { FONT_TEXT })
            .text_size(px(if form == Form::Phone { 14.0 } else { 12.5 })),
    };
    let lead = size(
        text(if next { "⏎" } else { " " }).text_color(color(Token::Ink3)),
        true,
    )
    .flex_shrink_0()
    .w(if form == Form::Terminal {
        term::cells(2.0)
    } else {
        px(16.0)
    });
    let label = size(
        text(row.label.clone())
            .number_of_lines(1u32)
            .truncate()
            .text_color(color(if enabled { Token::Ink } else { Token::Ink3 })),
        false,
    )
    .flex_shrink(1.0)
    .min_w(px(0.0));
    let why = row.unavailable.clone().map(|why| {
        size(
            text(format!("unavailable: {why}"))
                .number_of_lines(1u32)
                .truncate()
                .text_color(color(Token::Ink3)),
            false,
        )
        .flex_shrink(1.0)
        .min_w(px(0.0))
    });
    let command = row
        .command
        .map(|command| size(text(command).text_color(color(Token::Ink3)), true).flex_shrink_0());
    let slug = row.id.trim_start_matches("act-").to_owned();
    let actions = [clear(id), row.action];
    view()
        .test_id(&format!("cmd-{slug}"))
        .role(AccessibilityRole::Button)
        .label(row.label)
        .accessibility_state(AccessibilityState {
            disabled: !enabled,
            ..AccessibilityState::default()
        })
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
        .when(form != Form::Terminal, |line| {
            line.px(px(8.0))
                .py(px(4.0))
                .hover(|s| s.bg(color(Token::Hover)))
        })
        .when(enabled, |line| {
            line.cursor_pointer().on_press(move || {
                for action in &actions {
                    run(app, action.clone());
                }
            })
        })
        .child(lead)
        .child(label)
        .children(why)
        .child(view().flex_1().min_w(px(0.0)))
        .children(command)
        .into_element()
}
