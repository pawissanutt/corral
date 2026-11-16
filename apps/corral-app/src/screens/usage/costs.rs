//! "Cost per day": one stacked bar per day, its segments the model's widths in percent of the
//! track (as the mockup draws them), the day before it and the total after it, then the legend.
//! Desktop and phone draw filled boxes, the desktop's no wider than reads at a glance; a terminal
//! draws Cell bars as wide as its room allows. The day and total beside each bar are its text;
//! the bar itself is hidden from screen readers. The series' coverage or staleness comes first,
//! in the held ink, and a day whose total is only partly known says so under its bar. With no
//! bars the model's words (loading, unavailable, nothing recorded) stand in their place.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::usage::{CostBar, Costs};
use crate::screens::usage::cellbar::{COST_LOOKS, stacked, swatch};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The segments' fills in legend order (`var(--ink-2)`, `var(--ink-4)`, `var(--held)`).
const FILLS: [Token; 3] = [Token::Ink2, Token::Ink4, Token::Held];
/// The desktop's bars stop growing here, so the longest stays readable against its day.
const DESKTOP_MAX: f32 = 880.0;
/// Points between a bar's end and its total.
const TOTAL_GAP: f32 = 8.0;
/// Terminal: the day column, the total column and the bar's bounds, in Cells.
const TERM_DAY: f32 = 5.0;
const TERM_TOTAL: usize = 7;
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

/// The section on desktop and phone: the coverage note, then the bars and the legend, or what
/// is said in their place. Rows `day | bar total`: the day in its column (34 pt on desktop), the
/// bar as long as its share of the costliest day, its total right after its end, bars 10 pt
/// high with 2 pt corners, a day's qualifier under its bar, the legend 10 pt under them.
/// test_id "usage-costs".
pub fn bars(app: AppState, costs: &Costs) -> AnyElement {
    let content = match &costs.empty {
        Some(words) => empty(app, words),
        None => drawn(app, costs),
    };
    view()
        .test_id("usage-costs")
        .when(app.form == Form::Desktop, |v| v.max_w(px(DESKTOP_MAX)))
        .children(costs.note.as_deref().map(|line| {
            view()
                .pb(px(8.0))
                .child(qualifier(app, line))
                .into_element()
        }))
        .child(content)
        .into_element()
}

fn drawn(app: AppState, costs: &Costs) -> AnyElement {
    let form = app.form;
    let m = metrics(form);
    let rows = costs
        .bars
        .iter()
        .map(|bar| day_row(app, bar, &m))
        .collect::<Vec<_>>();
    let legend = costs
        .legend
        .iter()
        .cloned()
        .zip(FILLS)
        .map(|(label, fill)| {
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(
                    view()
                        .w(px(18.0))
                        .h(px(8.0))
                        .flex_shrink_0()
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

/// One day: its row, and under the bar what keeps its total from being whole. The track keeps
/// the total's width free at its end, so the costliest day's bar fills the rest and every total
/// sits right after its own bar.
fn day_row(app: AppState, bar: &CostBar, m: &Metrics) -> AnyElement {
    let drawn: u32 = bar.values.iter().sum::<u32>().min(100);
    let segments = bar
        .values
        .into_iter()
        .zip(FILLS)
        .filter(|&(value, _)| value > 0)
        .map(|(value, fill)| {
            view()
                .h_full()
                .flex_shrink_0()
                .w(relative(value as f32 / drawn.max(1) as f32))
                .bg(color(fill))
                .into_element()
        })
        .collect::<Vec<_>>();
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            text(bar.day.clone())
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
                .flex_row()
                .items_center()
                .pr(px(m.total + TOTAL_GAP))
                .child(
                    view()
                        .w(relative(drawn as f32 / 100.0))
                        .h(px(m.bar))
                        .flex_shrink_0()
                        .flex_row()
                        .rounded(px(2.0))
                        .overflow_hidden()
                        .accessibility_elements_hidden(true)
                        .children(segments),
                )
                .child(
                    text(bar.total_label.clone())
                        .ml(px(TOTAL_GAP))
                        .flex_shrink_0()
                        .number_of_lines(1u32)
                        .font_family(FONT_MONO)
                        .tabular_nums()
                        .text_size(px(m.size * 0.93))
                        .text_color(color(Token::Ink3)),
                ),
        );
    match &bar.total_note {
        None => row.into_element(),
        Some(line) => view()
            .gap(px(2.0))
            .child(row)
            // Under the bar: past the day column and its 8 pt gap.
            .child(view().pl(px(m.day + 8.0)).child(qualifier(app, line)))
            .into_element(),
    }
}

/// The terminal's section, `room` Cells wide: the coverage note, then the day, the Cell bar and
/// the total at the end, a day's qualifier on the line under it; then the legend, each
/// segment's Cell before its name. test_id "usage-costs".
pub fn terminal(app: AppState, costs: &Costs, room: usize) -> AnyElement {
    let note = costs.note.as_deref().map(|line| qualifier(app, line));
    if let Some(words) = &costs.empty {
        return view()
            .test_id("usage-costs")
            .children(note)
            .child(empty(app, words))
            .into_element();
    }
    let cells = room
        .saturating_sub(TERM_DAY as usize + 1 + TERM_GAP + TERM_TOTAL)
        .clamp(TERM_BAR_MIN, TERM_BAR_MAX);
    let mut rows = Vec::with_capacity(costs.bars.len());
    for bar in &costs.bars {
        rows.push(
            view()
                .flex_row()
                .items_center()
                .h(term::rows(1.0))
                .child(
                    view()
                        .w(term::cells(TERM_DAY))
                        .flex_shrink_0()
                        .child(term::quiet(bar.day.clone()).number_of_lines(1u32)),
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
                            text(bar.total_label.clone())
                                .number_of_lines(1u32)
                                .text_color(color(Token::Ink)),
                        ),
                )
                .into_element(),
        );
        if let Some(line) = &bar.total_note {
            rows.push(
                view()
                    .pl(term::cells(TERM_DAY + 1.0))
                    .child(qualifier(app, line))
                    .into_element(),
            );
        }
    }
    let mut legend = Vec::new();
    for (at, (label, look)) in costs.legend.iter().cloned().zip(COST_LOOKS).enumerate() {
        if at > 0 {
            legend.push(term::space(TERM_GAP as f32));
        }
        legend.push(swatch(look));
        legend.push(term::space(1.0));
        legend.push(term::quiet(label).number_of_lines(1u32).into_element());
    }
    view()
        .test_id("usage-costs")
        .children(note)
        .children(rows)
        .child(term::blank())
        .child(view().flex_row().flex_wrap().children(legend))
        .into_element()
}

/// What keeps a figure from being whole (partial scan, unpriced entries, a retained series):
/// small words in the held ink, wrapping, so it is read before the figures it qualifies.
pub(crate) fn qualifier(app: AppState, words: &str) -> AnyElement {
    let line = text(words.to_owned()).text_color(hue_ink(Hue::Held));
    if app.form == Form::Terminal {
        return line.into_element();
    }
    line.font_family(FONT_TEXT)
        .text_size(px(if app.form == Form::Phone { 13.0 } else { 11.5 }))
        .line_height(px(if app.form == Form::Phone { 18.0 } else { 16.0 }))
        .into_element()
}

/// What the section says in place of its bars (loading, failed, nothing recorded).
fn empty(app: AppState, words: &str) -> AnyElement {
    let words = text(words.to_owned()).text_color(color(Token::Ink3));
    if app.form == Form::Terminal {
        return words.into_element();
    }
    words
        .font_family(FONT_TEXT)
        .text_size(px(if app.form == Form::Phone { 14.0 } else { 12.5 }))
        .into_element()
}
