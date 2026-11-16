//! "By session": each agent or assistant session's recorded tokens and cost, costliest first, in
//! the usage table where the width has room for its five columns, else as two-line records (the
//! phone, a narrow desktop window, a narrow terminal). Facts corral does not hold read "unknown"
//! in the quiet ink, so known figures stand out; the model's note says what is not listed.
//! test_id "usage-sessions".

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::usage::{SESSION_HEADERS, SessionRow, UNKNOWN};
use crate::state::AppState;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::utable;
use crate::ui::kit::text::{dim, mono, plain};
use crate::ui::tokens::{Token, color};

const WIDTHS: [Width; 5] = [
    Width::Fixed(120.0),
    Width::Fill,
    Width::Fixed(150.0),
    Width::Fixed(80.0),
    Width::Fixed(96.0),
];
const ALIGN_END: [bool; 5] = [false, false, false, true, true];

/// A known fact in the mono figure face, or "unknown" in the quiet ink.
fn fact(app: AppState, value: Option<&str>) -> AnyElement {
    match value {
        Some(value) => mono(app, value.to_owned())
            .number_of_lines(1u32)
            .into_element(),
        None => mono(app, UNKNOWN)
            .text_color(color(Token::Ink3))
            .into_element(),
    }
}

/// One session as a record: its id and what it works on, then profile, tokens and cost in one
/// quiet line.
fn record(app: AppState, row: &SessionRow) -> AnyElement {
    view()
        .gap(px(2.0))
        .child(
            view()
                .flex_row()
                .items_baseline()
                .gap(px(8.0))
                .child(
                    mono(app, row.session.clone())
                        .number_of_lines(1u32)
                        .flex_shrink_0(),
                )
                .child(
                    plain(app, row.what.clone())
                        .number_of_lines(1u32)
                        .flex_shrink(1.0)
                        .min_w(px(0.0)),
                ),
        )
        .child(dim(app, row.summary()).tabular_nums())
        .into_element()
}

/// The session table and its note; `records` sets the rows as stacked records instead.
pub fn table(app: AppState, rows: &[SessionRow], note: Option<&str>, records: bool) -> AnyElement {
    let note = note.map(|words| dim(app, words.to_owned()).into_element());
    if rows.is_empty() {
        return view()
            .test_id("usage-sessions")
            .gap(px(6.0))
            .child(dim(app, "No session has recorded tokens or cost yet."))
            .children(note)
            .into_element();
    }
    let body = if records {
        view()
            .gap(px(if app.form == Form::Terminal {
                0.0
            } else {
                12.0
            }))
            .children(rows.iter().map(|row| record(app, row)).collect::<Vec<_>>())
            .into_element()
    } else {
        utable(
            app,
            &WIDTHS,
            &SESSION_HEADERS,
            &ALIGN_END,
            rows.iter()
                .map(|row| {
                    vec![
                        mono(app, row.session.clone())
                            .number_of_lines(1u32)
                            .into_element(),
                        plain(app, row.what.clone())
                            .number_of_lines(1u32)
                            .text_color(color(Token::Ink2))
                            .into_element(),
                        fact(app, row.profile.as_deref()),
                        fact(app, row.tokens.as_deref()),
                        fact(app, row.cost.as_deref()),
                    ]
                })
                .collect(),
        )
    };
    view()
        .test_id("usage-sessions")
        .gap(px(8.0))
        .child(body)
        .children(note)
        .into_element()
}
