//! "By session": each agent or assistant session's recorded tokens and cost, costliest first, in
//! the device table on desktop and terminal and as two-line records on the phone; the model's
//! note says what is not listed. test_id "usage-sessions".

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::usage::{SESSION_HEADERS, SessionRow};
use crate::state::AppState;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::devtable_aligned;
use crate::ui::kit::text::{mono, plain};
use crate::ui::tokens::{FONT_TEXT, Token, color};

const WIDTHS: [Width; 5] = [
    Width::Fixed(110.0),
    Width::Fill,
    Width::Fixed(130.0),
    Width::Fixed(72.0),
    Width::Fixed(80.0),
];
const ALIGN_END: [bool; 5] = [false, false, false, true, true];

fn quiet(app: AppState, words: impl Into<SharedString>) -> AnyElement {
    plain(app, words)
        .text_color(color(Token::Ink3))
        .into_element()
}

/// The session table and its note.
pub fn table(app: AppState, rows: &[SessionRow], note: Option<&str>) -> AnyElement {
    let note = note.map(|words| quiet(app, words.to_owned()));
    if rows.is_empty() {
        return view()
            .test_id("usage-sessions")
            .child(quiet(app, "No session has recorded tokens or cost yet."))
            .children(note)
            .into_element();
    }
    let body = if app.form == Form::Phone {
        view()
            .gap(px(10.0))
            .children(
                rows.iter()
                    .map(|row| {
                        view()
                            .gap(px(2.0))
                            .child(
                                view()
                                    .flex_row()
                                    .items_baseline()
                                    .gap(px(8.0))
                                    .child(mono(app, row.session.clone()).flex_shrink_0())
                                    .child(
                                        plain(app, row.what.clone())
                                            .number_of_lines(1u32)
                                            .flex_shrink(1.0)
                                            .min_w(px(0.0)),
                                    ),
                            )
                            .child(
                                text(format!(
                                    "{} · {} tokens · {}",
                                    row.profile, row.tokens, row.cost
                                ))
                                .font_family(FONT_TEXT)
                                .text_size(px(13.0))
                                .text_color(color(Token::Ink3)),
                            )
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
            .into_element()
    } else {
        devtable_aligned(
            app,
            &WIDTHS,
            &SESSION_HEADERS,
            &ALIGN_END,
            rows.iter()
                .map(|row| {
                    vec![
                        mono(app, row.session.clone()).into_element(),
                        plain(app, row.what.clone())
                            .number_of_lines(1u32)
                            .into_element(),
                        mono(app, row.profile.clone()).into_element(),
                        mono(app, row.tokens.clone()).into_element(),
                        mono(app, row.cost.clone()).into_element(),
                    ]
                })
                .collect(),
        )
    };
    view()
        .test_id("usage-sessions")
        .gap(px(6.0))
        .child(body)
        .children(note)
        .into_element()
}
