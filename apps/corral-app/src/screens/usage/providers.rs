//! One provider's window: the desktop card of the provider grid (`.ucell`), the phone's stacked
//! block (`.pband`) and the terminal's gauge line. All words and figures are the model's
//! `ProviderView`. The card reads top to bottom as one observation: the window's use as the big
//! figure in its 70/90 band, a meter against those bands, the amounts in words, then the costs
//! and where the quota facts come from. Without a quota fact the big figure is the period's cost,
//! there is no meter, and the amounts line says what corral lacks. The band chart appears only
//! when today's window samples exist; the card is complete without it.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::usage::{Level, ProviderView};
use crate::screens::usage::cellbar::gauge;
use crate::screens::usage::chart::chart;
use crate::screens::usage::costs::qualifier;
use crate::screens::usage::quotas::quotas;
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The chart view boxes: tall enough that the 70 and 90 labels sit a label's height apart at
/// their band edges (desktop 380 by 104, phone 354 by 100).
const DESKTOP_CHART: (f32, f32) = (380.0, 104.0);
const PHONE_CHART: (f32, f32) = (354.0, 100.0);
/// The meter's band notches: where 70 and 90 percent of the window fall.
const BANDS: [f32; 2] = [0.7, 0.9];
/// Terminal gauge line: the label column, the figure column, and the bar's bounds.
const LABEL_CELLS: usize = 24;
const USED_CELLS: usize = 4;
const BAR_MIN: usize = 10;
const BAR_MAX: usize = 40;
/// Cells between the parts of a terminal line, and the detail lines' indent.
const GAP_CELLS: usize = 2;

/// The ink of a figure in its band: plain below 70, held from 70, failure from 90.
fn band_ink(provider_level: Level) -> Token {
    match provider_level {
        Level::None => Token::Ink,
        Level::Warn => Token::Held,
        Level::Bad => Token::Fail,
    }
}

/// The amounts line's ink: the failure hue once the window is exhausted, the band's hue once in
/// a zone, else the secondary ink.
fn amounts_ink(item: &ProviderView) -> Token {
    match (item.exhausted, item.level) {
        (true, _) | (false, Level::Bad) => Token::Fail,
        (false, Level::Warn) => Token::Held,
        (false, Level::None) => Token::Ink2,
    }
}

/// The window meter: a sunk track filled to the window's use in its band's ink, notched where 70
/// and 90 percent fall so the bands read without a legend. Nothing when use is unknown, so an
/// empty track is never mistaken for an idle window. Its numbers are the amounts line under it,
/// so screen readers skip it.
fn window_meter(item: &ProviderView, height: f32) -> Option<AnyElement> {
    let used = item.used?;
    let fill = match item.level {
        Level::None => Token::Ink2,
        Level::Warn => Token::Held,
        Level::Bad => Token::Fail,
    };
    let notches = BANDS.map(|at| {
        view()
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .left(relative(at))
            .w(px(2.0))
            .bg(color(Token::Bg))
            .into_element()
    });
    Some(
        view()
            .relative()
            .w_full()
            .h(px(height))
            .rounded(px(height / 2.0))
            .overflow_hidden()
            .bg(color(Token::Surface2))
            .accessibility_elements_hidden(true)
            .child(
                view()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .w(relative(used.clamp(0.0, 1.0)))
                    .bg(color(fill)),
            )
            .children(notches)
            .into_element(),
    )
}

/// The band chart when today's samples exist; nothing otherwise.
fn window_chart(item: &ProviderView, (width, height): (f32, f32)) -> Option<AnyElement> {
    item.chart
        .as_ref()
        .map(|source| chart(source.clone(), width, height))
}

/// A desktop card: the label and window, the big figure in its band with its word, the meter,
/// the amounts, what keeps the figure from being whole, the chart when there are samples, the
/// cost line, then where the quota facts come from. `first` leaves out the rule on its left.
/// test_id `usage-<provider id>`.
pub fn desktop_cell(app: AppState, item: &ProviderView, first: bool) -> AnyElement {
    view()
        .test_id(&format!("usage-{}", item.id))
        .role(AccessibilityRole::Group)
        .label(item.label.clone())
        .flex_1()
        .min_w(px(0.0))
        .px(px(18.0))
        .pt(px(16.0))
        .pb(px(16.0))
        .when(!first, |cell| {
            cell.border_l(px(1.0)).border_l_color(color(Token::Rule))
        })
        .child(
            view()
                .flex_row()
                .items_baseline()
                .gap(px(8.0))
                .child(
                    words_text(item.label.clone(), 13.5, FontWeight::BOLD, Token::Ink)
                        .number_of_lines(1u32)
                        .flex_shrink(1.0)
                        .min_w(px(0.0)),
                )
                .child(
                    words_text(item.window.clone(), 11.5, FontWeight::NORMAL, Token::Ink3)
                        .number_of_lines(1u32)
                        .flex_shrink_0()
                        .ml(auto()),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_baseline()
                .gap(px(5.0))
                .mt(px(8.0))
                .child(big(item.big.clone(), 28.0, band_ink(item.level)))
                .child(words_text(
                    item.big_label,
                    12.0,
                    FontWeight::MEDIUM,
                    Token::Ink3,
                )),
        )
        .children(window_meter(item, 6.0).map(|meter| below(10.0, meter)))
        .child(
            words_text(
                item.quota_line.clone(),
                12.5,
                FontWeight::MEDIUM,
                amounts_ink(item),
            )
            .tabular_nums()
            .mt(px(8.0)),
        )
        .children(big_note(app, item))
        .children(window_chart(item, DESKTOP_CHART).map(|chart| below(10.0, chart)))
        .child(
            words_text(item.costs.clone(), 12.0, FontWeight::NORMAL, Token::Ink3)
                .tabular_nums()
                .mt(px(6.0)),
        )
        .children(quotas(app, item))
        .into_element()
}

/// A phone block (`.pband`): the label and window, the big figure in its band, the meter, the
/// amounts, what keeps the figure from being whole, the chart when there are samples, each
/// profile with its share and cost, the cost line, then where the quota facts come from.
/// test_id `usage-<provider id>`.
pub fn phone_block(app: AppState, item: &ProviderView) -> AnyElement {
    let profiles = item
        .profiles
        .iter()
        .map(|profile| {
            view()
                .flex_row()
                .items_baseline()
                .gap(px(8.0))
                .child(
                    text(profile.name.clone())
                        .number_of_lines(1u32)
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .font_family(FONT_MONO)
                        .tabular_nums()
                        .text_size(px(13.0))
                        .text_color(color(Token::Ink)),
                )
                .child(
                    words_text(
                        profile.detail.clone(),
                        14.0,
                        FontWeight::NORMAL,
                        Token::Ink3,
                    )
                    .tabular_nums()
                    .flex_shrink_0()
                    .ml(auto()),
                )
                .into_element()
        })
        .collect::<Vec<_>>();
    view()
        .test_id(&format!("usage-{}", item.id))
        .role(AccessibilityRole::Group)
        .label(item.label.clone())
        .px(px(18.0))
        .pt(px(14.0))
        .pb(px(16.0))
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .child(
            view()
                .flex_row()
                .items_baseline()
                .gap(px(8.0))
                .child(
                    words_text(item.label.clone(), 15.0, FontWeight::BOLD, Token::Ink)
                        .role(AccessibilityRole::Header)
                        .flex_shrink(1.0)
                        .min_w(px(0.0)),
                )
                .child(
                    words_text(item.window.clone(), 13.0, FontWeight::NORMAL, Token::Ink3)
                        .flex_shrink_0()
                        .ml(auto()),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_baseline()
                .gap(px(6.0))
                .mt(px(6.0))
                .child(big(item.big.clone(), 30.0, band_ink(item.level)))
                .child(words_text(
                    item.big_label,
                    13.0,
                    FontWeight::MEDIUM,
                    Token::Ink3,
                )),
        )
        .children(window_meter(item, 8.0).map(|meter| below(10.0, meter)))
        .child(
            words_text(
                item.quota_line.clone(),
                14.0,
                FontWeight::MEDIUM,
                amounts_ink(item),
            )
            .tabular_nums()
            .mt(px(8.0)),
        )
        .children(big_note(app, item))
        .children(window_chart(item, PHONE_CHART).map(|chart| below(10.0, chart)))
        .when(!profiles.is_empty(), |block| {
            block.child(view().gap(px(4.0)).mt(px(12.0)).children(profiles))
        })
        .child(
            words_text(item.costs.clone(), 13.0, FontWeight::NORMAL, Token::Ink3)
                .tabular_nums()
                .mt(px(8.0)),
        )
        .children(quotas(app, item))
        .into_element()
}

/// A terminal gauge line and its detail: the label (bold, in a fixed column), the big figure
/// (bold, in its band), the window gauge as wide as `room` Cells allow (blank where use is
/// unknown, so the columns stay aligned), the headroom; under it, indented, the window and the
/// amounts, the cost line, what keeps the figure from being whole, then where the quota facts
/// come from, wrapped onto as many lines as `room` needs. test_id `usage-<provider id>`.
pub fn terminal_line(app: AppState, item: &ProviderView, room: usize) -> AnyElement {
    let headroom_cells = item.headroom.chars().count();
    let figure_cells = item.big.chars().count().max(USED_CELLS);
    let bar = room
        .saturating_sub(LABEL_CELLS + figure_cells + 1 + GAP_CELLS + headroom_cells)
        .clamp(BAR_MIN, BAR_MAX);
    let in_zone = item.exhausted || item.level != Level::None;
    let headroom = text(item.headroom.clone())
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(amounts_ink(item)))
        .when(in_zone, |t| t.font_weight(FontWeight::BOLD));
    let meter = match item.used {
        Some(used) => gauge(used, bar),
        None => view()
            .w(term::cells(bar as f32))
            .flex_shrink_0()
            .into_element(),
    };
    let details = pack(
        &[
            item.window.as_str(),
            item.quota_line.as_str(),
            item.costs.as_str(),
        ],
        room.saturating_sub(GAP_CELLS),
    );
    let indent = |element: AnyElement| {
        view()
            .pl(term::cells(GAP_CELLS as f32))
            .child(element)
            .into_element()
    };
    view()
        .test_id(&format!("usage-{}", item.id))
        .role(AccessibilityRole::Group)
        .label(item.label.clone())
        .child(
            view()
                .flex_row()
                .items_center()
                .h(term::rows(1.0))
                .child(
                    view()
                        .w(term::cells(LABEL_CELLS as f32))
                        .flex_shrink_0()
                        .child(
                            text(item.label.clone())
                                .number_of_lines(1u32)
                                .font_weight(FontWeight::BOLD)
                                .text_color(color(Token::Ink)),
                        ),
                )
                .child(
                    view()
                        .w(term::cells(figure_cells as f32))
                        .flex_shrink_0()
                        .flex_row()
                        .justify_end()
                        .child(
                            text(item.big.clone())
                                .number_of_lines(1u32)
                                .font_weight(FontWeight::BOLD)
                                .text_color(color(band_ink(item.level))),
                        ),
                )
                .child(term::space(1.0))
                .child(meter)
                .child(term::space(GAP_CELLS as f32))
                .child(headroom),
        )
        .children(details.into_iter().map(|line| {
            indent(
                view()
                    .flex_row()
                    .h(term::rows(1.0))
                    .child(term::quiet(line).number_of_lines(1u32))
                    .into_element(),
            )
        }))
        .children(big_note(app, item).map(indent))
        .children(quotas(app, item).map(indent))
        .into_element()
}

/// `content` set `top` points under what comes before it.
fn below(top: f32, content: AnyElement) -> AnyElement {
    view().mt(px(top)).child(content).into_element()
}

/// What keeps the big figure from being whole (a partial scan, unpriced entries), under it.
fn big_note(app: AppState, item: &ProviderView) -> Option<AnyElement> {
    item.big_note.as_deref().map(|line| {
        view()
            .when(app.form != Form::Terminal, |v| v.mt(px(4.0)))
            .child(qualifier(app, line))
            .into_element()
    })
}

/// Lines of at most `room` Cells holding `parts` in order, two Cells apart; a part longer than
/// a line gets a line of its own and is cut there by the line.
fn pack(parts: &[&str], room: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for part in parts {
        let fits = lines
            .last()
            .is_some_and(|line| line.chars().count() + GAP_CELLS + part.chars().count() <= room);
        match lines.last_mut() {
            Some(line) if fits => {
                line.push_str(&" ".repeat(GAP_CELLS));
                line.push_str(part);
            }
            _ => lines.push((*part).to_owned()),
        }
    }
    lines
}

/// Text in the app's font at `size`, one paragraph.
fn words_text(content: impl Into<SharedString>, size: f32, weight: FontWeight, ink: Token) -> Text {
    text(content.into())
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .font_weight(weight)
        .text_color(color(ink))
}

/// The big figure (`.ucell .big`): bold, tabular, drawn a little tight, in its band's ink.
fn big(content: String, size: f32, ink: Token) -> Text {
    text(content)
        .flex_shrink_0()
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .line_height(px(size * 1.15))
        .letter_spacing(px(size * -0.02))
        .font_weight(FontWeight::BOLD)
        .tabular_nums()
        .text_color(color(ink))
}
