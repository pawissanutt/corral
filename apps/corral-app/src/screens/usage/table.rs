//! The grouped table: each profile, model, task or day with its recorded runs and run time
//! beside the requests, tokens and cost reported for it. Figures sit at the trailing edge in
//! tabular mono; a figure corral does not hold is the model's quiet dash. Where the width has no
//! room for seven columns the groups are two-line records instead; the phone's table sets each
//! row as a record on its own. test_id "usage-groups".

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::usage::{GroupRow, Page, UNKNOWN};
use crate::state::AppState;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::utable;
use crate::ui::kit::text::{dim, mono, plain};
use crate::ui::tokens::{FONT_MONO, Token, color};

/// Desktop figure columns: room for "12,345", "3h12m", "1.2M", "$1,061".
const DESKTOP: [Width; 7] = [
    Width::Fill,
    Width::Fixed(64.0),
    Width::Fixed(84.0),
    Width::Fixed(84.0),
    Width::Fixed(84.0),
    Width::Fixed(88.0),
    Width::Fixed(84.0),
];
/// Terminal figure columns at 8 points a Cell: 6, 9, 9, 8, 8 and 9 Cells.
const TERMINAL: [Width; 7] = [
    Width::Fill,
    Width::Fixed(48.0),
    Width::Fixed(72.0),
    Width::Fixed(72.0),
    Width::Fixed(64.0),
    Width::Fixed(64.0),
    Width::Fixed(72.0),
];
const ALIGN_END: [bool; 7] = [false, true, true, true, true, true, true];

/// A figure (`td.n`): mono, tabular, in ink; [`UNKNOWN`] in the quiet ink.
fn figure(form: Form, content: &str) -> AnyElement {
    let ink = if content == UNKNOWN {
        Token::Ink3
    } else {
        Token::Ink
    };
    let words = text(content.to_owned())
        .number_of_lines(1u32)
        .text_color(color(ink));
    if form == Form::Terminal {
        words.into_element()
    } else {
        words
            .font_family(FONT_MONO)
            .text_size(px(12.0))
            .tabular_nums()
            .into_element()
    }
}

/// A group's key in mono, quiet when the group has none, then a task's title in ink-2, cut to
/// its room.
fn key(app: AppState, row: &GroupRow) -> AnyElement {
    let name = mono(app, row.key.clone())
        .number_of_lines(1u32)
        .flex_shrink_0();
    let name = if row.named {
        name
    } else {
        name.text_color(color(Token::Ink3))
    };
    view()
        .flex_row()
        .items_baseline()
        .gap(px(8.0))
        .flex_1()
        .min_w(px(0.0))
        .child(name)
        .children(row.detail.clone().map(|title| {
            plain(app, title)
                .number_of_lines(1u32)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .text_color(color(Token::Ink2))
        }))
        .into_element()
}

/// One group as a record: its key, then its figures in one quiet line.
fn record(app: AppState, row: &GroupRow) -> AnyElement {
    view()
        .gap(px(2.0))
        .child(key(app, row))
        .child(dim(app, row.summary()).tabular_nums())
        .into_element()
}

/// The table, or `records` where the width has no room for its columns.
pub fn table(app: AppState, page: &Page, records: bool) -> AnyElement {
    let form = app.form;
    let body = if records {
        view()
            .gap(px(if form == Form::Terminal { 0.0 } else { 12.0 }))
            .children(
                page.rows
                    .iter()
                    .map(|row| record(app, row))
                    .collect::<Vec<_>>(),
            )
            .into_element()
    } else {
        utable(
            app,
            if form == Form::Terminal {
                &TERMINAL
            } else {
                &DESKTOP
            },
            &page.headers,
            &ALIGN_END,
            page.rows
                .iter()
                .map(|row| {
                    vec![
                        key(app, row),
                        figure(form, &row.runs),
                        figure(form, &row.run_time),
                        figure(form, &row.requests),
                        figure(form, &row.input),
                        figure(form, &row.output),
                        figure(form, &row.cost),
                    ]
                })
                .collect(),
        )
    };
    view()
        .test_id("usage-groups")
        .gap(px(8.0))
        .children(page.rows_empty.map(|words| dim(app, words).into_element()))
        .when(!page.rows.is_empty(), |table| table.child(body))
        .into_element()
}
