//! The grouped table: each profile, model, task or day with the runs it covers beside the
//! tokens and cost reported for it, the period's total under them. Figures sit at the trailing
//! edge in tabular mono;
//! a figure corral does not hold is the model's quiet dash. A group whose figures are partial
//! holds its coverage in the held tone and marks it ◐, its reasons in the hint (and behind
//! the page's Details, where the phone, which has no hint, reads them). Where the width has no room
//! for its columns the groups are records instead; the phone's table sets each row as a record
//! on its own. test_id "usage-groups".

use nsh_core::prelude::*;

use crate::model::usage::{GroupRow, Page, UNKNOWN};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::hint::hinted;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::utable;
use crate::ui::kit::text::{dim, mono, plain};
use crate::ui::tokens::{FONT_MONO, Token, color};

/// Desktop figure columns: room for "12 of 14", "1.2M", "$1,061", "Est. cost" and "Reported".
const DESKTOP: [Width; 6] = [
    Width::Fill,
    Width::Fixed(96.0),
    Width::Fixed(76.0),
    Width::Fixed(80.0),
    Width::Fixed(84.0),
    Width::Fixed(84.0),
];
/// Terminal figure columns at 8 points a Cell: 12, 9, 10, 9 and 10 Cells.
const TERMINAL: [Width; 6] = [
    Width::Fill,
    Width::Fixed(96.0),
    Width::Fixed(72.0),
    Width::Fixed(80.0),
    Width::Fixed(72.0),
    Width::Fixed(80.0),
];
const ALIGN_END: [bool; 6] = [false, true, true, true, true, true];

/// A figure (`td.n`): mono, tabular, in `ink`; [`UNKNOWN`] in the quiet ink.
fn figure(form: Form, content: &str, ink: AdaptiveColor) -> AnyElement {
    let ink = if content == UNKNOWN {
        color(Token::Ink3)
    } else {
        ink
    };
    let words = text(content.to_owned())
        .number_of_lines(1u32)
        .text_color(ink);
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
/// its room; a partial group's held ◐ after them, its reasons in the hint.
fn key(app: AppState, row: &GroupRow) -> AnyElement {
    let name = mono(app, row.key.clone())
        .number_of_lines(1u32)
        .flex_shrink_0();
    let name = if row.named {
        name
    } else {
        name.text_color(color(Token::Ink3))
    };
    let line = view()
        .flex_row()
        .items_baseline()
        .gap(px(8.0))
        .min_w(px(0.0))
        .child(name)
        .children(row.detail.clone().map(|title| {
            plain(app, title)
                .number_of_lines(1u32)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .text_color(color(Token::Ink2))
        }))
        .children(row.reasons.clone().map(|why| {
            let marker = text("◐").flex_shrink_0().text_color(hue_ink(Hue::Held));
            hinted(app, marker, why)
        }));
    view()
        .test_id(&row.id)
        .flex_1()
        .min_w(px(0.0))
        .child(line)
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
                    .chain(&page.total)
                    .map(|row| record(app, row))
                    .collect::<Vec<_>>(),
            )
            .into_element()
    } else {
        let ink = color(Token::Ink);
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
                .chain(&page.total)
                .map(|row| {
                    let covered = if row.runs_missing {
                        hue_ink(Hue::Held)
                    } else {
                        ink
                    };
                    vec![
                        key(app, row),
                        figure(form, &row.covered, covered),
                        figure(form, &row.input, ink),
                        figure(form, &row.output, ink),
                        figure(form, &row.estimate, ink),
                        figure(form, &row.cost, ink),
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
