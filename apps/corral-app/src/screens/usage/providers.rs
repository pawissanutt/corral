//! One provider's window: the desktop cell of the provider grid (`.ucell`), the phone's stacked
//! block (`.pband`) and the terminal's gauge line. All words and figures are the model's
//! `ProviderView`.

use nsh_core::prelude::*;

use crate::model::usage::{Level, ProviderView, USED_LABEL};
use crate::screens::usage::cellbar::gauge;
use crate::screens::usage::chart::chart;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The mockup's chart view boxes: desktop 380 by 96, phone 354 by 92.
const DESKTOP_CHART: (f32, f32) = (380.0, 96.0);
const PHONE_CHART: (f32, f32) = (354.0, 92.0);
/// Terminal gauge line: the label column, the percent column, and the bar's bounds.
const LABEL_CELLS: usize = 24;
const USED_CELLS: usize = 4;
const BAR_MIN: usize = 10;
const BAR_MAX: usize = 40;
/// Cells between the parts of a terminal line, and the detail line's indent.
const GAP_CELLS: usize = 2;

/// The headroom's ink: the held hue once the window is in a zone (`.c-held`), quiet before.
fn headroom_ink(provider_level: Level) -> Token {
    match provider_level {
        Level::None => Token::Ink3,
        Level::Warn | Level::Bad => Token::Held,
    }
}

/// A desktop cell: the label and window, the big percent with "used", the headroom in its
/// level, the reset time at the trailing edge, the band chart, the cost line. `first` leaves
/// out the rule on its left. test_id `usage-<provider id>`.
pub fn desktop_cell(item: &ProviderView, first: bool) -> AnyElement {
    let provider = item.provider;
    view()
        .test_id(&format!("usage-{}", provider.id))
        .role(AccessibilityRole::Group)
        .label(provider.label)
        .flex_1()
        .min_w(px(0.0))
        .px(px(18.0))
        .pt(px(16.0))
        .pb(px(14.0))
        .when(!first, |cell| {
            cell.border_l(px(1.0)).border_l_color(color(Token::Rule))
        })
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_baseline()
                .gap_x(px(8.0))
                .child(words(provider.label, 13.5, FontWeight::BOLD, Token::Ink))
                .child(words(
                    provider.window,
                    11.5,
                    FontWeight::NORMAL,
                    Token::Ink3,
                )),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_baseline()
                .gap_x(px(14.0))
                .mt(px(6.0))
                .child(
                    view()
                        .flex_row()
                        .items_baseline()
                        .gap(px(4.0))
                        .child(big(item.used.clone(), 26.0))
                        .child(words(USED_LABEL, 12.0, FontWeight::MEDIUM, Token::Ink3)),
                )
                .child(words(
                    item.headroom.clone(),
                    13.0,
                    FontWeight::SEMIBOLD,
                    headroom_ink(item.level),
                ))
                .child(
                    words(item.resets.clone(), 12.0, FontWeight::NORMAL, Token::Ink3).ml(auto()),
                ),
        )
        .child(
            view()
                .mt(px(6.0))
                .child(chart(provider, DESKTOP_CHART.0, DESKTOP_CHART.1)),
        )
        .child(words(item.costs.clone(), 12.0, FontWeight::NORMAL, Token::Ink3).mt(px(4.0)))
        .into_element()
}

/// A phone block (`.pband`): the label and window, the big percent, the headroom, the reset
/// time, the band chart, then each profile with its share and cost. test_id
/// `usage-<provider id>`.
pub fn phone_block(item: &ProviderView) -> AnyElement {
    let provider = item.provider;
    let profiles = item
        .profiles
        .iter()
        .map(|profile| {
            view()
                .flex_row()
                .items_baseline()
                .gap(px(8.0))
                .child(
                    text(profile.name)
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .font_family(FONT_MONO)
                        .tabular_nums()
                        .text_size(px(13.0))
                        .text_color(color(Token::Ink)),
                )
                .child(
                    words(
                        profile.detail.clone(),
                        14.0,
                        FontWeight::NORMAL,
                        Token::Ink3,
                    )
                    .ml(auto()),
                )
                .into_element()
        })
        .collect::<Vec<_>>();
    view()
        .test_id(&format!("usage-{}", provider.id))
        .role(AccessibilityRole::Group)
        .label(provider.label)
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
                .mb(px(8.0))
                .child(
                    words(provider.label, 15.0, FontWeight::BOLD, Token::Ink)
                        .role(AccessibilityRole::Header),
                )
                .child(words(provider.window, 15.0, FontWeight::MEDIUM, Token::Ink).ml(auto())),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_baseline()
                .gap_x(px(12.0))
                .child(big(item.used.clone(), 30.0))
                .child(words(
                    item.headroom.clone(),
                    16.0,
                    FontWeight::BOLD,
                    headroom_ink(item.level),
                ))
                .child(
                    words(
                        item.phone_resets.clone(),
                        13.0,
                        FontWeight::NORMAL,
                        Token::Ink3,
                    )
                    .ml(auto()),
                ),
        )
        .child(
            view()
                .mt(px(6.0))
                .child(chart(provider, PHONE_CHART.0, PHONE_CHART.1)),
        )
        .child(view().gap(px(4.0)).mt(px(8.0)).children(profiles))
        .into_element()
}

/// A terminal gauge line and its detail: the label (bold, in a fixed column), the percent used
/// (bold), the window gauge as wide as `room` Cells allow, the headroom (bold in the held hue
/// once in a zone, quiet before); under it, indented, the window, the reset time and the cost
/// line, wrapped onto as many lines as `room` needs. test_id `usage-<provider id>`.
pub fn terminal_line(item: &ProviderView, room: usize) -> AnyElement {
    let provider = item.provider;
    let headroom_cells = item.headroom.chars().count();
    let bar = room
        .saturating_sub(LABEL_CELLS + USED_CELLS + 1 + GAP_CELLS + headroom_cells)
        .clamp(BAR_MIN, BAR_MAX);
    let in_zone = item.level != Level::None;
    let headroom = text(item.headroom.clone())
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(headroom_ink(item.level)))
        .when(in_zone, |t| t.font_weight(FontWeight::BOLD));
    let details = pack(
        &[provider.window, item.resets.as_str(), item.costs.as_str()],
        room.saturating_sub(GAP_CELLS),
    );
    view()
        .test_id(&format!("usage-{}", provider.id))
        .role(AccessibilityRole::Group)
        .label(provider.label)
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
                            text(provider.label)
                                .number_of_lines(1u32)
                                .font_weight(FontWeight::BOLD)
                                .text_color(color(Token::Ink)),
                        ),
                )
                .child(
                    view()
                        .w(term::cells(USED_CELLS as f32))
                        .flex_shrink_0()
                        .flex_row()
                        .justify_end()
                        .child(
                            text(item.used.clone())
                                .number_of_lines(1u32)
                                .font_weight(FontWeight::BOLD)
                                .text_color(color(Token::Ink)),
                        ),
                )
                .child(term::space(1.0))
                .child(gauge(provider.used, bar))
                .child(term::space(GAP_CELLS as f32))
                .child(headroom),
        )
        .children(details.into_iter().map(|line| {
            view()
                .flex_row()
                .h(term::rows(1.0))
                .pl(term::cells(GAP_CELLS as f32))
                .child(term::quiet(line).number_of_lines(1u32))
                .into_element()
        }))
        .into_element()
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
fn words(content: impl Into<SharedString>, size: f32, weight: FontWeight, ink: Token) -> Text {
    text(content.into())
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .font_weight(weight)
        .text_color(color(ink))
}

/// The big percent (`.ucell .big`): bold, tabular, drawn a little tight.
fn big(content: String, size: f32) -> Text {
    text(content)
        .flex_shrink_0()
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .line_height(px(size * 1.15))
        .letter_spacing(px(size * -0.02))
        .font_weight(FontWeight::BOLD)
        .tabular_nums()
        .text_color(color(Token::Ink))
}
