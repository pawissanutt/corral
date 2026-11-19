//! The period's tokens per day: one bar a day against the busiest day, the first and last day
//! under them. Desktop and phone draw the bars as views, each with its day's facts as its hover
//! hint; a terminal draws block heights across the Cells it has, a Cell holding several days
//! when the period is longer than the row. The bars are not keyboard stops: the Day grouping
//! lists the same facts as rows. Screen readers hear the strip's summary. test_id "usage-days".

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::usage::DayStrip;
use crate::state::AppState;
use crate::ui::kit::hint::tip;
use crate::ui::kit::text::h3;
use crate::ui::tokens::{FONT_MONO, Token, color};

/// The bars' height at the busiest day; a quiet day keeps a hairline.
const HEIGHT: f32 = 56.0;
const HAIRLINE: f32 = 1.0;
/// Block heights from an eighth to full.
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// From this many Cells a day, its last Cell is a gap, so neighbouring days read apart.
const GAPPED_DAY: usize = 3;

/// The strip in `room` terminal Cells (unused on desktop and the phone, which measure).
pub(super) fn strip(app: AppState, strip: &DayStrip, room: usize) -> AnyElement {
    let drawn = match app.form {
        Form::Terminal => cells(strip, room),
        Form::Desktop | Form::Phone => bars(app, strip),
    };
    view()
        .test_id("usage-days")
        .child(h3(app, strip.title, Some(strip.note.clone())))
        .child(
            view()
                .accessible(true)
                .role(AccessibilityRole::Image)
                .label(strip.spoken.clone())
                .child(drawn),
        )
        .into_element()
}

fn bars(app: AppState, strip: &DayStrip) -> AnyElement {
    let form = app.form;
    let gap = if strip.bars.len() > 62 { 0.0 } else { 2.0 };
    let bars = strip.bars.iter().map(|bar| {
        let height = (bar.fraction * HEIGHT).max(HAIRLINE);
        let fill = if bar.fraction > 0.0 {
            Token::Ink3
        } else {
            Token::Rule2
        };
        let column = view()
            .flex_1()
            .min_w(px(0.0))
            .h(px(HEIGHT))
            .justify_end()
            .child(
                view()
                    .h(px(height))
                    .rounded_t(px(1.5))
                    .bg(color(fill))
                    .hover(|s| s.bg(color(Token::Ink))),
            );
        tip(form, column, bar.hint.clone().into()).into_element()
    });
    let axis = |words: &str| {
        text(words.to_owned())
            .number_of_lines(1u32)
            .font_family(FONT_MONO)
            .text_size(px(10.5))
            .tabular_nums()
            .text_color(color(Token::Ink3))
    };
    view()
        .gap(px(4.0))
        .child(
            view()
                .flex_row()
                .items_end()
                .gap(px(gap))
                .accessibility_elements_hidden(true)
                .children(bars.collect::<Vec<_>>()),
        )
        .child(
            view()
                .flex_row()
                .justify_between()
                .accessibility_elements_hidden(true)
                .child(axis(&strip.first))
                .child(axis(&strip.last)),
        )
        .into_element()
}

/// Each day as an equal share of the row, the strip across the whole of it (a short period's
/// days wide, a gap closing each), several days to a Cell when the row is short of them; then
/// the first and last day under them.
fn cells(strip: &DayStrip, room: usize) -> AnyElement {
    let room = room.max(1);
    let per_day = (room / strip.bars.len().max(1)).max(1);
    let (drawn, gap) = if per_day >= GAPPED_DAY {
        (per_day - 1, 1)
    } else {
        (per_day, 0)
    };
    let columns = strip.columns(room);
    let blocks: String = columns
        .iter()
        .flat_map(|fraction| {
            let glyph = if *fraction > 0.0 {
                let step = (fraction * BLOCKS.len() as f32).ceil() as usize;
                BLOCKS[step.clamp(1, BLOCKS.len()) - 1]
            } else {
                '·'
            };
            std::iter::repeat_n(glyph, drawn).chain(std::iter::repeat_n(' ', gap))
        })
        .collect();
    let blocks = blocks.trim_end().to_owned();
    let width = blocks.chars().count();
    let between = width.saturating_sub(strip.first.chars().count() + strip.last.chars().count());
    view()
        .accessibility_elements_hidden(true)
        .child(
            text(blocks)
                .number_of_lines(1u32)
                .text_color(color(Token::Ink2)),
        )
        .child(
            text(format!(
                "{}{}{}",
                strip.first,
                " ".repeat(between),
                strip.last
            ))
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3)),
        )
        .into_element()
}
