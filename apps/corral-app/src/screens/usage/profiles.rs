//! The per-profile table (`.utable`, "By profile · current window"): every provider's profiles
//! with their share of the window (a meter and its figure), the period's cost and the provider's
//! headroom on its first row. Figures sit at the trailing edge of their columns, in tabular mono.
//! A narrow desktop window gets [`compact`]: each provider as a heading with its headroom over its
//! profiles' share and cost. A terminal sets each provider as a group row over its indented
//! profiles, so the profile names keep their room, and leaves the meter out where the grid is
//! too narrow for it. The phone shows the profiles inside each provider block instead
//! (`providers::phone_block`).

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::usage::{Level, Page, ProfileRow, UNKNOWN};
use crate::screens::usage::cellbar::meter;
use crate::screens::usage::costs::qualifier;
use crate::state::AppState;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::utable;
use crate::ui::kit::text::{dim, mono};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// Desktop columns: provider, profile (the rest), share (meter and figure), cost, headroom.
const DESKTOP_WIDTHS: [Width; 5] = [
    Width::Fixed(200.0),
    Width::Fill,
    Width::Fixed(200.0),
    Width::Fixed(96.0),
    Width::Fixed(96.0),
];
const DESKTOP_END: [bool; 5] = [false, false, false, true, true];
/// `.meter`: 120 by 6, round ends; its figure in a 44 pt column after it.
const METER_WIDTH: f32 = 120.0;
const METER_HEIGHT: f32 = 6.0;
const SHARE_FIGURE: f32 = 44.0;
/// The compact rows' cost column: the widest period cost, "~$1,234", in 12 pt mono.
const COMPACT_COST: f32 = 72.0;
/// Terminal columns at 8 points a Cell: share 18 wide (meter, figure) or 15 narrow (its
/// header's length), cost 9, headroom 9.
const TERM_WIDE: [Width; 4] = [
    Width::Fill,
    Width::Fixed(144.0),
    Width::Fixed(72.0),
    Width::Fixed(72.0),
];
const TERM_WIDE_END: [bool; 4] = [false, false, true, true];
const TERM_NARROW: [Width; 4] = [
    Width::Fill,
    Width::Fixed(120.0),
    Width::Fixed(72.0),
    Width::Fixed(72.0),
];
const TERM_NARROW_END: [bool; 4] = [false, true, true, true];
/// The meter fits once the grid has this many Cells; it is 12 of them.
const TERM_METER_ROOM: usize = 72;
const TERM_METER_CELLS: usize = 12;
/// A profile sits two Cells in under its provider's group row.
const TERM_INDENT: f32 = 2.0;

/// The desktop table: the model's five column names over one row per profile. A profile name too
/// long for its column wraps to a second line, then ends in an ellipsis, so it never runs into
/// the share beside it. test_id "usage-profiles" around the kit's "utable".
pub fn desktop(app: AppState, page: &Page) -> AnyElement {
    let form = app.form;
    let rows = page
        .profiles
        .iter()
        .cloned()
        .map(|row| {
            let share = share(&row);
            vec![
                provider_name(row.provider),
                mono(app, row.profile)
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .number_of_lines(2u32)
                    .into_element(),
                share,
                cost(form, row.cost, row.cost_note.is_some()),
                row.headroom
                    .map_or_else(|| view().into_element(), |text| figure(form, text)),
            ]
        })
        .collect();
    view()
        .test_id("usage-profiles")
        .child(utable(
            app,
            &DESKTOP_WIDTHS,
            &page.profile_headers,
            &DESKTOP_END,
            rows,
        ))
        .children(cost_notes(app, page))
        .into_element()
}

/// The share cell: the meter and its figure, or "unknown" in the quiet ink with no track, so an
/// unknown share never reads as an empty one.
fn share(row: &ProfileRow) -> AnyElement {
    let Some(percent) = row.share_percent else {
        return figure_text(Form::Desktop, row.share.clone()).into_element();
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(share_meter(percent, row.level))
        .child(
            view()
                .w(px(SHARE_FIGURE))
                .flex_shrink_0()
                .flex_row()
                .justify_end()
                .child(figure_text(Form::Desktop, row.share.clone())),
        )
        .into_element()
}

/// The table where the width has no room for its grid: each provider (with its headroom at the
/// trailing edge) over its profiles, each profile's name taking what room is left before its
/// share and cost. test_id "usage-profiles".
pub fn compact(app: AppState, page: &Page) -> AnyElement {
    let form = app.form;
    let headroom = page.profile_headers[4];
    let mut rows = Vec::with_capacity(page.profiles.len() * 2);
    for row in &page.profiles {
        if !row.provider.is_empty() {
            rows.push(
                view()
                    .flex_row()
                    .items_baseline()
                    .gap(px(8.0))
                    .pt(px(if rows.is_empty() { 0.0 } else { 10.0 }))
                    .pb(px(4.0))
                    .child(provider_name(row.provider.clone()))
                    .children(row.headroom.as_deref().map(|value| {
                        dim(app, format!("{headroom} {value}"))
                            .number_of_lines(1u32)
                            .tabular_nums()
                            .flex_shrink_0()
                            .ml(auto())
                            .into_element()
                    }))
                    .into_element(),
            );
        }
        rows.push(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .py(px(5.0))
                .border_b(px(1.0))
                .border_color(color(Token::Rule))
                .child(
                    view().flex_1().min_w(px(0.0)).child(
                        mono(app, row.profile.clone())
                            .number_of_lines(1u32)
                            .flex_shrink(1.0)
                            .min_w(px(0.0)),
                    ),
                )
                .child(
                    view()
                        .w(px(SHARE_FIGURE))
                        .flex_shrink_0()
                        .flex_row()
                        .justify_end()
                        .child(figure_text(form, row.share.clone())),
                )
                .child(
                    view()
                        .w(px(COMPACT_COST))
                        .flex_shrink_0()
                        .flex_row()
                        .justify_end()
                        .child(cost(form, row.cost.clone(), row.cost_note.is_some())),
                )
                .into_element(),
        );
    }
    view()
        .test_id("usage-profiles")
        .child(view().children(rows))
        .children(cost_notes(app, page))
        .into_element()
}

/// The terminal table, `room` Cells wide: a bold group row per provider with its headroom,
/// then its profiles indented with their share (with its meter when it fits) and cost.
pub fn terminal(app: AppState, page: &Page, room: usize) -> AnyElement {
    let wide = room >= TERM_METER_ROOM;
    let header: Vec<&str> = page.profile_headers.iter().skip(1).copied().collect();
    let (widths, end): (&[Width], &[bool]) = if wide {
        (&TERM_WIDE, &TERM_WIDE_END)
    } else {
        (&TERM_NARROW, &TERM_NARROW_END)
    };
    let mut rows = Vec::new();
    for row in page.profiles.iter().cloned() {
        if !row.provider.is_empty() {
            rows.push(group_row(app.form, &row));
        }
        rows.push(profile_row(app, row, wide));
    }
    view()
        .test_id("usage-profiles")
        .child(utable(app, widths, &header, end, rows))
        .children(cost_notes(app, page))
        .into_element()
}

/// A provider's group row: its label in bold in the profile column, its headroom at the end.
fn group_row(form: Form, row: &ProfileRow) -> Vec<AnyElement> {
    vec![
        text(row.provider.clone())
            .number_of_lines(1u32)
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink))
            .into_element(),
        view().into_element(),
        view().into_element(),
        row.headroom
            .clone()
            .map_or_else(|| view().into_element(), |text| figure(form, text)),
    ]
}

/// A profile's row, indented under its provider; the headroom column stays empty.
fn profile_row(app: AppState, row: ProfileRow, wide: bool) -> Vec<AnyElement> {
    let form = app.form;
    let share = match row.share_percent {
        Some(percent) if wide => view()
            .flex_row()
            .child(meter(percent, row.level, TERM_METER_CELLS))
            .child(term::space(1.0))
            .child(figure_text(form, row.share.clone()))
            .into_element(),
        _ => figure_text(form, row.share.clone()).into_element(),
    };
    vec![
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
        share,
        cost(form, row.cost, row.cost_note.is_some()),
        view().into_element(),
    ]
}

/// The provider label on a provider's first row; nothing on the rows after it.
fn provider_name(provider: String) -> AnyElement {
    if provider.is_empty() {
        return view().into_element();
    }
    text(provider)
        .number_of_lines(2u32)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .font_family(FONT_TEXT)
        .text_size(px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink))
        .into_element()
}

/// A figure (`td.n`): mono 12 pt, tabular, in ink; [`UNKNOWN`] in the quiet ink. A terminal
/// shows it in its own font.
fn figure_text(form: Form, content: String) -> Text {
    let ink = if content == UNKNOWN {
        Token::Ink3
    } else {
        Token::Ink
    };
    let words = text(content).number_of_lines(1u32).text_color(color(ink));
    if form == Form::Terminal {
        words
    } else {
        words
            .font_family(FONT_MONO)
            .text_size(px(12.0))
            .tabular_nums()
    }
}

fn figure(form: Form, content: String) -> AnyElement {
    figure_text(form, content).into_element()
}

/// A period cost: a figure, in the held ink when a note under the table qualifies it.
fn cost(form: Form, content: String, qualified: bool) -> AnyElement {
    let words = figure_text(form, content);
    if qualified {
        words.text_color(color(Token::Held)).into_element()
    } else {
        words.into_element()
    }
}

/// What keeps the profiles' period costs from being whole, under the table: said once when every
/// qualified cost shares it, else per profile. A cost column is too narrow to carry it.
fn cost_notes(app: AppState, page: &Page) -> Option<AnyElement> {
    let noted: Vec<(&str, &str)> = page
        .profiles
        .iter()
        .filter_map(|row| Some((row.profile.as_str(), row.cost_note.as_deref()?)))
        .collect();
    let (_, first) = *noted.first()?;
    let lines: Vec<AnyElement> = if noted.iter().all(|&(_, note)| note == first) {
        vec![qualifier(app, first)]
    } else {
        noted
            .iter()
            .map(|(profile, note)| qualifier(app, &format!("{profile} · {note}")))
            .collect()
    };
    let terminal = app.form == Form::Terminal;
    Some(
        view()
            .gap(px(if terminal { 0.0 } else { 4.0 }))
            .pt(px(if terminal { 0.0 } else { 8.0 }))
            .children(lines)
            .into_element(),
    )
}

/// The share meter (`.meter`): a sunk track, the share filled in the provider's band (ink-2,
/// held, failure). Its number is the figure beside it, so screen readers skip it.
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
        .into_element()
}

#[cfg(test)]
#[path = "profiles_tests.rs"]
mod tests;
