//! The Remove sheet of a stopped build (`agents::RemoveSheet`), over every page while it is open:
//! what removing it deletes (the worktree, the branch, the record) as prune's dry run reports
//! it, prune's own lines, what it keeps and why, then Remove, the one confirmation, offered only
//! once the dry run answered and only when it would remove something. A refusal says corrald's
//! or prune's reason and the sheet stays. Escape, the scrim and Cancel close it. A dialog on
//! desktop, a sheet on the phone, the framed region over the page in a terminal; mounted once by
//! each shell. test ids `remove`, `remove-lines`, `btn-remove-cancel`, `btn-remove-confirm`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Action, RemoveSheet, RemoveStatus, remove_sheet};
use crate::model::hue::Hue;
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::{error, loading};
use crate::ui::kit::overlay::{self, Modal};
use crate::ui::kit::text::{h3, mono, plain};
use crate::ui::tokens::{Token, color};

use super::run_agents as run;

/// The sheet, mounted once by each shell beside the confirm and refusal sheets.
pub fn sheet(app: AppState) -> AnyElement {
    overlay::modal(
        app,
        move || app.agents.with(|state| state.removing.is_some()),
        move || run(app, Action::CloseRemove),
        move || modal(app),
    )
}

fn modal(app: AppState) -> Modal {
    let shown = computed(move || {
        app.agents
            .with(|state| app.feed.with(|feed| remove_sheet(state, feed)))
    });
    let title = untracked(|| shown.with(|sheet| sheet.as_ref().map(|sheet| sheet.title.clone())))
        .unwrap_or_default();
    // The dry run answers after the sheet opens, so its body follows it while the frame stays.
    let body = dynamic(move || shown.get().map(|sheet| untracked(|| body(app, sheet))));
    let foot = dynamic(move || shown.get().map(|sheet| untracked(|| foot(app, sheet))));
    Modal {
        test_id: String::from("remove"),
        title,
        sub: Some("This is what will be removed.".to_owned()),
        body: body.into_element(),
        foot: Some(foot.into_element()),
        done: None,
        confirm: None,
    }
}

fn body(app: AppState, sheet: RemoveSheet) -> AnyElement {
    let form = app.form;
    let gap = if form == Form::Terminal { 0.0 } else { 10.0 };
    let status = match &sheet.status {
        RemoveStatus::Checking => Some(loading(
            app,
            "Checking what would be removed…",
            &[(0.6, 10.0), (0.45, 10.0)],
        )),
        RemoveStatus::Removing => Some(plain(app, "Removing…").into_element()),
        RemoveStatus::Failed(why) => Some(error(app, &vec![span(why.clone())], None)),
        RemoveStatus::Ready => None,
    };
    let removes = (!sheet.removes.is_empty())
        .then(|| plain(app, format!("Removes {}.", sheet.removes.join(", "))).into_element());
    let nothing = (sheet.removes.is_empty() && sheet.status == RemoveStatus::Ready)
        .then(|| plain(app, "Nothing would be removed.").into_element());
    let kept = sheet.kept.map(|kept| {
        plain(app, format!("Kept: {kept}"))
            .text_color(hue_ink(Hue::Held))
            .into_element()
    });
    let lines = (!sheet.lines.is_empty()).then(|| {
        view()
            .test_id("remove-lines")
            .gap(px(if form == Form::Terminal { 0.0 } else { 3.0 }))
            .child(h3(app, "Will be removed", None))
            .children(
                sheet
                    .lines
                    .into_iter()
                    .map(|line| {
                        mono(app, line)
                            .text_color(color(Token::Ink2))
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
            .into_element()
    });
    view()
        .gap(px(gap))
        .children(status)
        .children(removes)
        .children(nothing)
        .children(kept)
        .children(lines)
        .into_element()
}

/// Cancel, then Remove once the dry run says what goes.
fn foot(app: AppState, sheet: RemoveSheet) -> AnyElement {
    let phone = app.form == Form::Phone;
    let cancel = sheet.cancel;
    let cancel = button_with(
        app,
        "Cancel",
        ButtonKind::Ghost,
        ButtonOptions {
            wide: phone,
            key_hint: Some("Esc"),
            ..ButtonOptions::default()
        },
        "btn-remove-cancel",
        move || run(app, cancel.clone()),
    );
    let confirm = sheet.confirm;
    let remove = button_with(
        app,
        "Remove",
        ButtonKind::Primary,
        ButtonOptions {
            wide: phone,
            danger: true,
            disabled: confirm.is_none(),
            ..ButtonOptions::default()
        },
        "btn-remove-confirm",
        move || {
            if let Some(action) = &confirm {
                run(app, action.clone());
            }
        },
    );
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .when(!phone, |row| row.justify_end())
        .child(cancel)
        .child(remove)
        .into_element()
}
