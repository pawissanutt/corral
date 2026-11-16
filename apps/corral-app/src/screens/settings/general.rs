//! Settings › General in every data state: this device's own preferences first (theme, the
//! sidebar on desktop and terminal, and where its data comes from), which need no connection;
//! then the workspace as the served feed reports it, or what waits for a connection.

use nsh_core::prelude::*;

use super::live_sections::{facts, snapshot_parts};
use super::parts::{button_row, cell, first_heading, frame, space};
use crate::model::settings::{connect, live};
use crate::model::{Destination, Form};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with, seg};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::Token;

pub fn page(app: AppState) -> AnyElement {
    let workspace = if untracked(|| app.daemon()) {
        snapshot_parts(app, |app, settings, _| {
            let rows = app
                .feed
                .with_untracked(|feed| live::general_facts(feed, settings));
            vec![
                facts(app, rows),
                space(app, 10.0),
                cell(
                    app,
                    format!(
                        "Workspace settings change from an administrator device; changes made here are recorded with {}.",
                        app.via()
                    ),
                    Token::Ink3,
                    false,
                )
                .into_element(),
            ]
        })
    } else {
        crate::screens::unavailable_body(app, Destination::Settings)
    };
    frame(
        app,
        "General",
        12.0,
        vec![
            first_heading(app, "This device", None),
            local(app),
            h3(app, "Workspace", None),
            workspace,
        ],
    )
}

/// The preferences kept on this device alone.
fn local(app: AppState) -> AnyElement {
    let form = app.form;
    let mut rows = vec![row(
        app,
        "Theme",
        seg(
            app,
            &connect::THEMES,
            move || app.theme.get(),
            move |choice| app.set_theme(choice),
            "seg-theme",
        ),
        None,
    )];
    if form != Form::Phone {
        rows.push(row(
            app,
            "Sidebar",
            seg(
                app,
                &connect::SIDEBAR,
                move || app.sidebar.get(),
                move |choice| app.sidebar.set(choice),
                "seg-sidebar",
            ),
            Some(connect::SIDEBAR_NOTE),
        ));
    }
    rows.push(row(app, "Data", data_source(app), None));
    view()
        .min_w(px(0.0))
        .gap(if form == Form::Terminal {
            term::rows(1.0)
        } else {
            px(14.0)
        })
        .children(rows)
        .into_element()
}

/// Where this device's data comes from, and the switch to the preview or back.
fn data_source(app: AppState) -> AnyElement {
    dynamic(move || {
        let v = connect::data_source(app.data.get(), crate::client::configured());
        untracked(move || {
            let target = v.target;
            view()
                .min_w(px(0.0))
                .gap(px(6.0))
                .child(cell(app, v.line, Token::Ink3, false))
                .child(button_row(
                    app,
                    vec![button_with(
                        app,
                        v.label,
                        ButtonKind::Ghost,
                        ButtonOptions::default(),
                        "btn-data-source",
                        move || app.set_data(target),
                    )],
                ))
                .into_element()
        })
    })
    .into_element()
}

/// A preference: its label beside the control (over it on a phone), with an optional note.
fn row(
    app: AppState,
    label: &'static str,
    control: AnyElement,
    note: Option<&'static str>,
) -> AnyElement {
    let control = view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(4.0))
        .child(control)
        .children(note.map(|note| cell(app, note, Token::Ink3, false).into_element()))
        .into_element();
    let label = cell(app, label, Token::Ink, true);
    match app.form {
        Form::Phone => view()
            .min_w(px(0.0))
            .gap(px(6.0))
            .child(label)
            .child(control)
            .into_element(),
        Form::Desktop => view()
            .flex_row()
            .items_start()
            .gap(px(12.0))
            .child(view().w(px(128.0)).flex_shrink_0().pt(px(4.0)).child(label))
            .child(control)
            .into_element(),
        Form::Terminal => view()
            .flex_row()
            .items_start()
            .child(view().w(term::cells(14.0)).flex_shrink_0().child(label))
            .child(control)
            .into_element(),
    }
}
