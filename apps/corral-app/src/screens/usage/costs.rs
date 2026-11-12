//! "Cost per day · 7 days": one stacked bar per day, its segments the model's dollar values as
//! percents of the track (as the mockup draws them), the day before it and the total after it,
//! then the legend. Desktop and phone draw filled boxes; a terminal draws Cell bars as wide as
//! its room allows. The day and total beside each bar are its text; the bar itself is hidden
//! from screen readers.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::usage::{COST_LEGEND, cost_bars};
use crate::screens::usage::cellbar::{COST_LOOKS, stacked, swatch};
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The segments' fills in legend order (`var(--ink-2)`, `var(--ink-4)`, `var(--held)`).
const FILLS: [Token; 3] = [Token::Ink2, Token::Ink4, Token::Held];
/// Terminal: the day column, the total column and the bar's bounds, in Cells.
const TERM_DAY: f32 = 4.0;
const TERM_TOTAL: usize = 5;
const TERM_BAR_MIN: usize = 10;
const TERM_BAR_MAX: usize = 50;
const TERM_GAP: usize = 2;

/// Metrics of a form: day column, total column, text size, bar height, row gap.
struct Metrics {
    day: f32,
    total: f32,
    size: f32,
    bar: f32,
    gap: f32,
}

fn metrics(form: Form) -> Metrics {
    match form {
        Form::Phone => Metrics {
            day: 40.0,
            total: 56.0,
            size: 14.0,
            bar: 12.0,
            gap: 10.0,
        },
        Form::Desktop | Form::Terminal => Metrics {
            day: 34.0,
            total: 52.0,
            size: 12.0,
            bar: 10.0,
            gap: 7.0,
        },
    }
}

/// The bars and the legend on desktop and phone: rows `day | bar | total` (34, the rest, 52 on
/// desktop), bars 10 pt high with 2 pt corners, the legend 10 pt under them. test_id
/// "usage-costs".
pub fn bars(app: AppState) -> AnyElement {
    let form = app.form;
    let m = metrics(form);
    let rows = cost_bars()
        .into_iter()
        .map(|bar| {
            let segments = bar
                .values
                .into_iter()
                .zip(FILLS)
                .map(|(value, fill)| {
                    view()
                        .h_full()
                        .flex_shrink_0()
                        .w(relative(value.min(100) as f32 / 100.0))
                        .bg(color(fill))
                        .into_element()
                })
                .collect::<Vec<_>>();
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    text(bar.day)
                        .w(px(m.day))
                        .flex_shrink_0()
                        .number_of_lines(1u32)
                        .font_family(FONT_TEXT)
                        .text_size(px(m.size))
                        .text_color(color(Token::Ink3)),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .h(px(m.bar))
                        .flex_row()
                        .rounded(px(2.0))
                        .overflow_hidden()
                        .accessibility_elements_hidden(true)
                        .children(segments),
                )
                .child(
                    view()
                        .w(px(m.total))
                        .flex_shrink_0()
                        .flex_row()
                        .justify_end()
                        .child(
                            text(bar.total_label)
                                .number_of_lines(1u32)
                                .font_family(FONT_MONO)
                                .tabular_nums()
                                .text_size(px(m.size * 0.93))
                                .text_color(color(Token::Ink3)),
                        ),
                )
                .into_element()
        })
        .collect::<Vec<_>>();
    let legend = COST_LEGEND
        .iter()
        .zip(FILLS)
        .map(|(&label, fill)| {
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(
                    view()
                        .w(px(18.0))
                        .h(px(8.0))
                        .rounded(px(2.0))
                        .bg(color(fill)),
                )
                .child(
                    text(label)
                        .font_family(FONT_TEXT)
                        .text_size(px(if form == Form::Phone { 13.0 } else { 11.5 }))
                        .text_color(color(Token::Ink3)),
                )
                .into_element()
        })
        .collect::<Vec<_>>();
    view()
        .test_id("usage-costs")
        .child(view().gap(px(m.gap)).children(rows))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap_x(px(16.0))
                .gap_y(px(6.0))
                .pt(px(10.0))
                .children(legend),
        )
        .into_element()
}

/// The terminal's bars, `room` Cells wide: the day, the Cell bar, the total at the end; then the
/// legend, each segment's Cell before its name. test_id "usage-costs".
pub fn terminal(room: usize) -> AnyElement {
    let cells = room
        .saturating_sub(TERM_DAY as usize + 1 + TERM_GAP + TERM_TOTAL)
        .clamp(TERM_BAR_MIN, TERM_BAR_MAX);
    let rows = cost_bars()
        .into_iter()
        .map(|bar| {
            view()
                .flex_row()
                .items_center()
                .h(term::rows(1.0))
                .child(
                    view()
                        .w(term::cells(TERM_DAY))
                        .flex_shrink_0()
                        .child(term::quiet(bar.day).number_of_lines(1u32)),
                )
                .child(term::space(1.0))
                .child(
                    view()
                        .w(term::cells(cells as f32))
                        .flex_shrink_0()
                        .child(stacked(bar.values, cells)),
                )
                .child(term::space(TERM_GAP as f32))
                .child(
                    view()
                        .w(term::cells(TERM_TOTAL as f32))
                        .flex_shrink_0()
                        .flex_row()
                        .justify_end()
                        .child(
                            text(bar.total_label)
                                .number_of_lines(1u32)
                                .text_color(color(Token::Ink)),
                        ),
                )
                .into_element()
        })
        .collect::<Vec<_>>();
    let mut legend = Vec::new();
    for (at, (&label, look)) in COST_LEGEND.iter().zip(COST_LOOKS).enumerate() {
        if at > 0 {
            legend.push(term::space(TERM_GAP as f32));
        }
        legend.push(swatch(look));
        legend.push(term::space(1.0));
        legend.push(term::quiet(label).number_of_lines(1u32).into_element());
    }
    view()
        .test_id("usage-costs")
        .children(rows)
        .child(term::blank())
        .child(view().flex_row().flex_wrap().children(legend))
        .into_element()
}
