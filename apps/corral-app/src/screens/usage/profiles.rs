//! The profile table (`.utable`, "By profile"): each provider as a group row carrying its period
//! total, its profiles indented under it with their share of the window (a meter and its figure)
//! and the period's cost. Figures sit at the trailing edge of their columns, in tabular mono; a
//! figure corral does not hold is the model's quiet dash. The share column appears only when some
//! profile's share is known, so a page without quota facts is not a column of dashes. A profile's
//! own unpriced entries follow its name, quietly. A narrow desktop window gets [`compact`]: the
//! same groups as headings over flexible rows. A terminal sets the groups as bold rows and leaves
//! the meter out where the grid is too narrow for it. The phone lists the profiles inside each
//! provider block instead (`providers::phone_block`).

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::usage::{Level, Page, ProfileGroup, ProfileRow, UNKNOWN};
use crate::screens::usage::cellbar::meter;
use crate::state::AppState;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::utable;
use crate::ui::kit::text::mono;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// Desktop columns: the share (meter and figure) and the cost; the profile takes the rest of
/// [`desktop_width`].
const SHARE_COLUMN: f32 = 196.0;
const COST_COLUMN: f32 = 104.0;
/// The table's width beside the daily costs: room for a long profile name and its unpriced
/// count, no wider, so the figures stay near the names they belong to.
const WIDTH_WITH_SHARES: f32 = 660.0;
const WIDTH_WITHOUT_SHARES: f32 = 500.0;
/// A profile sits this far in from its provider's group row.
const INDENT: f32 = 14.0;
/// `.meter`: 120 by 6, round ends; its figure in a 44 pt column after it.
const METER_WIDTH: f32 = 120.0;
const METER_HEIGHT: f32 = 6.0;
const SHARE_FIGURE: f32 = 44.0;
/// The compact rows' cost column: the widest period cost, "~$1,234", in 12 pt mono.
const COMPACT_COST: f32 = 72.0;
/// Terminal columns at 8 points a Cell: share 18 wide (meter, figure) or 15 narrow (its
/// header's length), cost 9.
const TERM_SHARE_WIDE: f32 = 144.0;
const TERM_SHARE_NARROW: f32 = 120.0;
const TERM_COST: f32 = 72.0;
/// The meter fits once the grid has this many Cells; it is 12 of them.
const TERM_METER_ROOM: usize = 64;
const TERM_METER_CELLS: usize = 12;
/// A profile sits two Cells in under its provider's group row.
const TERM_INDENT: f32 = 2.0;

/// The desktop table's width for `page`: wide enough for its columns, never the whole page.
pub fn desktop_width(page: &Page) -> f32 {
    if page.shares_known {
        WIDTH_WITH_SHARES
    } else {
        WIDTH_WITHOUT_SHARES
    }
}

/// The columns of `page`: widths, names and which sit at the trailing edge.
fn columns(page: &Page, share: f32, cost: f32) -> (Vec<Width>, Vec<&'static str>, Vec<bool>) {
    let [profile, share_name, cost_name] = page.profile_headers;
    if page.shares_known {
        (
            vec![Width::Fill, Width::Fixed(share), Width::Fixed(cost)],
            vec![profile, share_name, cost_name],
            vec![false, false, true],
        )
    } else {
        (
            vec![Width::Fill, Width::Fixed(cost)],
            vec![profile, cost_name],
            vec![false, true],
        )
    }
}

/// The desktop table: a group row per provider, its profiles under it. A profile name too long
/// for its column ends in an ellipsis rather than run into the share beside it. test_id
/// "usage-profiles" around the kit's "utable".
pub fn desktop(app: AppState, page: &Page) -> AnyElement {
    let (widths, headers, ends) = columns(page, SHARE_COLUMN, COST_COLUMN);
    let mut rows = Vec::new();
    for group in &page.groups {
        let mut cells = vec![group_label(app, group)];
        if page.shares_known {
            cells.push(view().into_element());
        }
        cells.push(group_cost(app.form, group));
        rows.push(cells);
        for row in &group.rows {
            let mut cells = vec![
                view()
                    .flex_row()
                    .items_baseline()
                    .flex_1()
                    .min_w(px(0.0))
                    .pl(px(INDENT))
                    .gap(px(8.0))
                    .child(profile_name(app, row))
                    .children(row.gaps.clone().map(|gaps| gaps_text(app.form, gaps)))
                    .into_element(),
            ];
            if page.shares_known {
                cells.push(share(row));
            }
            cells.push(figure(app.form, row.cost.clone()));
            rows.push(cells);
        }
    }
    view()
        .test_id("usage-profiles")
        .child(utable(app, &widths, &headers, &ends, rows))
        .into_element()
}

/// The share cell: the meter and its figure, or the quiet dash with no track, so an unknown
/// share never reads as an empty one.
fn share(row: &ProfileRow) -> AnyElement {
    let Some(percent) = row.share_percent else {
        return figure(Form::Desktop, row.share.clone());
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

/// The table where the width has no room for its grid: each provider as a heading with its
/// total at the trailing edge, over its profiles, each name taking what room is left before its
/// share (when any is known) and cost. test_id "usage-profiles".
pub fn compact(app: AppState, page: &Page) -> AnyElement {
    let form = app.form;
    let mut rows = Vec::with_capacity(page.groups.len() * 3);
    for group in &page.groups {
        rows.push(
            view()
                .flex_row()
                .items_baseline()
                .gap(px(8.0))
                .pt(px(if rows.is_empty() { 0.0 } else { 12.0 }))
                .pb(px(4.0))
                .child(group_label(app, group))
                .child(view().ml(auto()).child(group_cost(form, group)))
                .into_element(),
        );
        for row in &group.rows {
            rows.push(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .py(px(5.0))
                    .border_b(px(1.0))
                    .border_color(color(Token::Rule))
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex_row()
                            .items_baseline()
                            .gap(px(8.0))
                            .child(profile_name(app, row))
                            .children(row.gaps.clone().map(|gaps| gaps_text(form, gaps))),
                    )
                    .when(page.shares_known, |line| {
                        line.child(
                            view()
                                .w(px(SHARE_FIGURE))
                                .flex_shrink_0()
                                .flex_row()
                                .justify_end()
                                .child(figure_text(form, row.share.clone())),
                        )
                    })
                    .child(
                        view()
                            .w(px(COMPACT_COST))
                            .flex_shrink_0()
                            .flex_row()
                            .justify_end()
                            .child(figure(form, row.cost.clone())),
                    )
                    .into_element(),
            );
        }
    }
    view()
        .test_id("usage-profiles")
        .children(rows)
        .into_element()
}

/// The terminal table, `room` Cells wide: a bold group row per provider with its total, then its
/// profiles indented with their share (with its meter when it fits) and cost.
pub fn terminal(app: AppState, page: &Page, room: usize) -> AnyElement {
    let wide = room >= TERM_METER_ROOM;
    let (widths, headers, mut ends) = columns(
        page,
        if wide {
            TERM_SHARE_WIDE
        } else {
            TERM_SHARE_NARROW
        },
        TERM_COST,
    );
    if page.shares_known && !wide {
        ends[1] = true;
    }
    let mut rows = Vec::new();
    for group in &page.groups {
        let mut cells = vec![
            text(group.label.clone())
                .number_of_lines(1u32)
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink))
                .into_element(),
        ];
        if page.shares_known {
            cells.push(view().into_element());
        }
        cells.push(group_cost(app.form, group));
        rows.push(cells);
        for row in &group.rows {
            let mut cells = vec![
                view()
                    .flex_row()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(term::space(TERM_INDENT))
                    .child(profile_name(app, row))
                    .children(row.gaps.clone().map(|gaps| {
                        view()
                            .flex_row()
                            .flex_shrink_0()
                            .child(term::space(2.0))
                            .child(term::quiet(gaps).number_of_lines(1u32))
                            .into_element()
                    }))
                    .into_element(),
            ];
            if page.shares_known {
                cells.push(match row.share_percent {
                    Some(percent) if wide => view()
                        .flex_row()
                        .child(meter(percent, row.level, TERM_METER_CELLS))
                        .child(term::space(1.0))
                        .child(figure_text(app.form, row.share.clone()))
                        .into_element(),
                    _ => figure(app.form, row.share.clone()),
                });
            }
            cells.push(figure(app.form, row.cost.clone()));
            rows.push(cells);
        }
    }
    view()
        .test_id("usage-profiles")
        .child(utable(app, &widths, &headers, &ends, rows))
        .into_element()
}

/// A provider's name on its group row, semibold.
fn group_label(app: AppState, group: &ProfileGroup) -> AnyElement {
    let words = text(group.label.clone())
        .number_of_lines(1u32)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(Token::Ink));
    match app.form {
        Form::Terminal => words.font_weight(FontWeight::BOLD).into_element(),
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(12.5))
            .font_weight(FontWeight::SEMIBOLD)
            .into_element(),
    }
}

/// A provider's period total on its group row: the figure in its weight.
fn group_cost(form: Form, group: &ProfileGroup) -> AnyElement {
    let words = figure_text(form, group.cost.clone());
    if group.cost == UNKNOWN {
        words.into_element()
    } else {
        words.font_weight(FontWeight::SEMIBOLD).into_element()
    }
}

/// A profile's name in mono; "no profile recorded" in the quiet ink, as a fact, not a name.
fn profile_name(app: AppState, row: &ProfileRow) -> AnyElement {
    let name = mono(app, row.profile.clone())
        .number_of_lines(1u32)
        .flex_shrink(1.0)
        .min_w(px(0.0));
    if row.named {
        name.into_element()
    } else {
        name.text_color(color(Token::Ink3)).into_element()
    }
}

/// A profile's own unpriced or undated entries, after its name: small, quiet, never cut.
fn gaps_text(form: Form, gaps: String) -> AnyElement {
    let words = text(gaps)
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    if form == Form::Terminal {
        words.into_element()
    } else {
        words
            .font_family(FONT_TEXT)
            .text_size(px(11.5))
            .tabular_nums()
            .into_element()
    }
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
