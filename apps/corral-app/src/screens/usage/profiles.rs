//! The per-profile table (`.utable`, "By profile · current window"): every provider's profiles
//! with their window share, a share meter with the model's marker, today's cost and the
//! provider's headroom. Desktop draws the mockup's six columns; a terminal sets each provider as
//! a group row over its indented profiles, so the profile names keep their room, and leaves the
//! meter out where the grid is too narrow for it. The phone shows the profiles inside each
//! provider block instead (`providers::phone_block`).

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::usage::{Level, PROFILE_HEADERS, ProfileRow, SHARE_MARKER_PERCENT, profile_rows};
use crate::screens::usage::cellbar::meter;
use crate::state::AppState;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::devtable;
use crate::ui::kit::text::mono;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// Desktop columns: provider, profile (the rest), window, share meter, today, headroom.
const DESKTOP_WIDTHS: [Width; 6] = [
    Width::Fixed(160.0),
    Width::Fill,
    Width::Fixed(72.0),
    Width::Fixed(160.0),
    Width::Fixed(72.0),
    Width::Fixed(88.0),
];
/// `.meter`: 120 by 6, round ends.
const METER_WIDTH: f32 = 120.0;
const METER_HEIGHT: f32 = 6.0;
/// Terminal columns at 8 points a Cell: window 6, share 18 (its header's length), today 7,
/// headroom 8 Cells.
const TERM_WIDE: [Width; 5] = [
    Width::Fill,
    Width::Fixed(48.0),
    Width::Fixed(144.0),
    Width::Fixed(56.0),
    Width::Fixed(64.0),
];
const TERM_NARROW: [Width; 4] = [
    Width::Fill,
    Width::Fixed(48.0),
    Width::Fixed(56.0),
    Width::Fixed(64.0),
];
/// The share column fits once the grid has this many Cells; its meter is 12 of them.
const TERM_SHARE_ROOM: usize = 72;
const TERM_METER_CELLS: usize = 12;
/// A profile sits two Cells in under its provider's group row.
const TERM_INDENT: f32 = 2.0;

/// The desktop table: the model's six column names over one row per profile. test_id
/// "usage-profiles" around the kit's "devtable".
pub fn desktop(app: AppState) -> AnyElement {
    let form = app.form;
    let rows = profile_rows()
        .into_iter()
        .map(|row| {
            vec![
                provider_name(row.provider),
                mono(app, row.profile).into_element(),
                figure(form, row.window),
                share_meter(row.share_percent, row.level),
                figure(form, row.today),
                figure(form, row.headroom),
            ]
        })
        .collect();
    view()
        .test_id("usage-profiles")
        .child(devtable(app, &DESKTOP_WIDTHS, &PROFILE_HEADERS, rows))
        .into_element()
}

/// The terminal table, `room` Cells wide: a bold group row per provider with its headroom,
/// then its profiles indented with their window, meter (when it fits) and cost.
pub fn terminal(app: AppState, room: usize) -> AnyElement {
    let wide = room >= TERM_SHARE_ROOM;
    let header: Vec<&str> = PROFILE_HEADERS
        .iter()
        .skip(1)
        .enumerate()
        .filter(|&(at, _)| wide || at != 2)
        .map(|(_, &name)| name)
        .collect();
    let widths: &[Width] = if wide { &TERM_WIDE } else { &TERM_NARROW };
    let mut rows = Vec::new();
    for row in profile_rows() {
        if !row.provider.is_empty() {
            rows.push(group_row(app.form, &row, wide));
        }
        rows.push(profile_row(app, row, wide));
    }
    view()
        .test_id("usage-profiles")
        .child(devtable(app, widths, &header, rows))
        .into_element()
}

/// A provider's group row: its label in bold in the profile column, its headroom at the end.
fn group_row(form: Form, row: &ProfileRow, wide: bool) -> Vec<AnyElement> {
    let mut cells = vec![
        text(row.provider)
            .number_of_lines(1u32)
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink))
            .into_element(),
        view().into_element(),
    ];
    if wide {
        cells.push(view().into_element());
    }
    cells.push(view().into_element());
    cells.push(figure(form, row.headroom.clone()));
    cells
}

/// A profile's row, indented under its provider; the headroom column stays empty.
fn profile_row(app: AppState, row: ProfileRow, wide: bool) -> Vec<AnyElement> {
    let form = app.form;
    let mut cells = vec![
        view()
            .flex_row()
            .flex_1()
            .min_w(px(0.0))
            .child(term::space(TERM_INDENT))
            .child(
                mono(app, row.profile)
                    .number_of_lines(1u32)
                    .flex_shrink(1.0)
                    .min_w(px(0.0)),
            )
            .into_element(),
        figure(form, row.window),
    ];
    if wide {
        cells.push(meter(row.share_percent, row.level, TERM_METER_CELLS));
    }
    cells.push(figure(form, row.today));
    cells.push(view().into_element());
    cells
}

/// The provider label on a provider's first row; nothing on the rows after it.
fn provider_name(provider: &'static str) -> AnyElement {
    if provider.is_empty() {
        return view().into_element();
    }
    text(provider)
        .font_family(FONT_TEXT)
        .text_size(px(12.5))
        .text_color(color(Token::Ink))
        .into_element()
}

/// A figure at the trailing edge of its column (`td.n`): mono 12 pt, tabular, in ink; a
/// terminal shows it in its own font.
fn figure(form: Form, content: String) -> AnyElement {
    let words = text(content)
        .number_of_lines(1u32)
        .text_color(color(Token::Ink));
    let words = if form == Form::Terminal {
        words
    } else {
        words
            .font_family(FONT_MONO)
            .text_size(px(12.0))
            .tabular_nums()
    };
    view()
        .flex_1()
        .flex_row()
        .justify_end()
        .child(words)
        .into_element()
}

/// The share meter (`.meter`): a sunk track, the share filled in the provider's level (ink-2,
/// held, failure) and a hairline at the model's marker. Its numbers are the window column
/// beside it, so screen readers skip it.
fn share_meter(share: u32, provider_level: Level) -> AnyElement {
    let fill = match provider_level {
        Level::None => Token::Ink2,
        Level::Warn => Token::Held,
        Level::Bad => Token::Fail,
    };
    view()
        .relative()
        .w(px(METER_WIDTH))
        .h(px(METER_HEIGHT))
        .flex_shrink_0()
        .rounded(px(METER_HEIGHT / 2.0))
        .overflow_hidden()
        .bg(color(Token::Surface2))
        .accessibility_elements_hidden(true)
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(relative(share.min(100) as f32 / 100.0))
                .rounded(px(METER_HEIGHT / 2.0))
                .bg(color(fill)),
        )
        .child(
            view()
                .absolute()
                .left(relative(SHARE_MARKER_PERCENT as f32 / 100.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(px(1.0))
                .bg(color(Token::Ink3)),
        )
        .into_element()
}
