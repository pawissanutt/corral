//! The rewind sheet over every page, open while a worker's turns are listed: what a rewind
//! does, the turns it can go back to (newest first, one button each), and the rewinds already
//! done with the rescue refs their work was saved under. Picking a turn closes it and asks once
//! in the confirm sheet before anything is sent. Escape, the scrim and Close dismiss it. test_id
//! "rewind" ("rewind-done", "btn-rewind-turn-<n>").

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Action, RewindSheet, rewind_sheet};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::overlay::{self, Modal};
use crate::ui::kit::text::{h3, mono, plain};
use crate::ui::tokens::{Token, color};

use super::run;

/// The sheet, mounted once by each shell beside the confirm and refusal sheets.
pub fn sheet(app: AppState) -> AnyElement {
    overlay::modal(
        app,
        move || app.agents.with(|state| state.rewind.is_some()),
        move || run(app, Action::CloseRewind),
        move || modal(app),
    )
}

fn modal(app: AppState) -> Modal {
    let shown = computed(move || {
        app.agents
            .with(|state| app.feed.with(|feed| rewind_sheet(state, feed)))
    });
    let (title, note) = untracked(|| {
        shown.with(|sheet| {
            sheet
                .as_ref()
                .map(|sheet| (sheet.title.clone(), sheet.note))
                .unwrap_or_default()
        })
    });
    // The turns arrive after the sheet opens, so its body follows them while the frame stays.
    let body = dynamic(move || shown.get().map(|sheet| body(app, sheet)));
    Modal {
        test_id: String::from("rewind"),
        title,
        sub: Some(note.to_owned()),
        body: body.into_element(),
        foot: None,
        done: Some("Close".to_owned()),
        confirm: None,
    }
}

fn body(app: AppState, sheet: RewindSheet) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let gap = if terminal { 0.0 } else { 12.0 };
    let points: Vec<AnyElement> = sheet
        .points
        .into_iter()
        .map(|point| {
            let turn = point.turn;
            let press = button_with(
                app,
                point.label,
                ButtonKind::Ghost,
                ButtonOptions {
                    disabled: !point.usable,
                    wide: app.form == Form::Phone,
                    ..ButtonOptions::default()
                },
                format!("btn-rewind-turn-{turn}"),
                move || run(app, Action::RewindTo(turn)),
            );
            view()
                .flex_row()
                .items_center()
                .gap(if terminal {
                    crate::ui::term::cells(2.0)
                } else {
                    px(10.0)
                })
                .child(press)
                .children(
                    point
                        .when
                        .map(|when| mono(app, when).text_color(color(Token::Ink2))),
                )
                .child(mono(app, point.detail).text_color(color(Token::Ink3)))
                .into_element()
        })
        .collect();
    let has_points = !points.is_empty();
    let earlier: Vec<AnyElement> = sheet
        .earlier
        .into_iter()
        .map(|line| {
            mono(app, line)
                .text_color(color(Token::Ink2))
                .into_element()
        })
        .collect();
    let has_earlier = !earlier.is_empty();
    view()
        .gap(px(gap))
        .children(sheet.status.map(|status| plain(app, status)))
        .when(has_points, |body| {
            body.child(
                view()
                    .gap(px(if terminal { 0.0 } else { 6.0 }))
                    .child(h3(app, sheet.turns, None))
                    .children(points),
            )
        })
        .when(has_earlier, |body| {
            body.child(
                view()
                    .gap(px(if terminal { 0.0 } else { 4.0 }))
                    .child(h3(app, sheet.done, None))
                    .children(earlier),
            )
        })
        .into_element()
}
