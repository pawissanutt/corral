//! The period's tokens per day. Desktop, web and phone draw a chart on a canvas measured to its
//! container (`ui::chart`'s conventions): a token scale from a visible zero baseline with a
//! gridline per round step, one bar a day no wider than `WIDEST` in its slot, the days under
//! the baseline thinned from the newest so labels never collide. A readout line above the plot
//! gives the exact figures of the day pointed at, touched, or reached with ← → Home End once the
//! chart has the keyboard focus (the Day grouping's row in words). A terminal draws block
//! heights across the Cells it has, a Cell holding several days when the period is longer than
//! the row. Screen readers hear the strip's summary. test_ids "usage-days", "usage-days-chart",
//! "usage-days-readout".

use std::rc::Rc;

use nsh_core::AccessibilityValue;
use nsh_core::canvas::CanvasTextAlign;
use nsh_core::keymap::Keystroke;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::chart::{bar_width, compact, labelled, scale};
use crate::model::usage::{DayBar, DayStrip};
use crate::state::AppState;
use crate::ui::chart::{AXIS_ADVANCE, AXIS_SIZE, PICKED, SERIES, grid, gutter, ink, mono_font};
use crate::ui::kit::text::h3;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The canvas's height on desktop and web, and on the phone.
const HEIGHT: f32 = 180.0;
const PHONE_HEIGHT: f32 = 150.0;
/// Room over the top gridline for its label, and under the baseline for the days.
const TOP: f32 = 8.0;
const FOOT: f32 = 22.0;
/// The widest a bar gets, however wide the window.
const WIDEST: f32 = 28.0;
/// Gridlines over zero at most.
const LINES: u64 = 4;
/// Room kept between two day labels.
const TICK_GAP: f32 = 12.0;
/// Block heights from an eighth to full.
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// From this many Cells a day, its last Cell is a gap, so neighbouring days read apart.
const GAPPED_DAY: usize = 3;

/// What the chart keeps while the page's figures update under it: its measured width and the
/// day pointed at and picked, so a live update neither blanks the plot for a frame nor drops
/// the day being read. Made once per page.
#[derive(Clone, Copy)]
pub(super) struct Reading {
    width: Signal<f32>,
    hovered: Signal<Option<usize>>,
    picked: Signal<Option<usize>>,
}

impl Reading {
    pub(super) fn new() -> Self {
        Self {
            width: signal(0.0),
            hovered: signal(None),
            picked: signal(None),
        }
    }
}

/// The strip in `room` terminal Cells (unused on desktop and the phone, which measure).
pub(super) fn strip(app: AppState, strip: &DayStrip, room: usize, reading: Reading) -> AnyElement {
    let drawn = match app.form {
        Form::Terminal => view()
            .accessible(true)
            .role(AccessibilityRole::Image)
            .label(strip.spoken.clone())
            .child(cells(strip, room))
            .into_element(),
        Form::Desktop | Form::Phone => graphic(app, strip, reading),
    };
    view()
        .test_id("usage-days")
        .child(h3(app, strip.title, Some(strip.note.clone())))
        .child(drawn)
        .into_element()
}

/// Where the plot and its labels fall at one width.
struct Geometry {
    left: f32,
    right: f32,
    base: f32,
    slot: f32,
    bar: f32,
    top_value: u64,
    lines: Vec<(String, f32)>,
    ticks: Vec<usize>,
    tick_width: f32,
}

fn geometry(bars: &[DayBar], peak: u64, room: f32, height: f32) -> Geometry {
    let scale = scale(peak, LINES);
    let base = (height - FOOT).round();
    let y = |value: u64| base - value as f32 / scale.top as f32 * (base - TOP);
    let lines: Vec<(String, f32)> = scale
        .ticks
        .iter()
        .map(|&value| (compact(value), y(value)))
        .collect();
    let left = gutter(lines.iter().map(|(label, _)| label.as_str()));
    let right = (room - 2.0).max(left + 1.0);
    let slot = (right - left) / bars.len().max(1) as f32;
    let tick_width = bars
        .iter()
        .map(|bar| bar.tick.chars().count())
        .max()
        .unwrap_or(0) as f32
        * AXIS_ADVANCE;
    let mut g = Geometry {
        left,
        right,
        base,
        slot,
        bar: bar_width(slot, WIDEST),
        top_value: scale.top,
        ticks: labelled(bars.len(), right - left, tick_width + TICK_GAP),
        lines,
        tick_width,
    };
    g.ticks = spaced(&g);
    g
}

/// The thinned ticks whose labels, once kept inside the plot by [`label_x`], still stand a
/// label and a gap apart: `labelled` spaces the slots' centres, but an edge label moved inward
/// can close on its neighbour. Counted back from the newest, which always keeps its label.
fn spaced(g: &Geometry) -> Vec<usize> {
    let need = g.tick_width + TICK_GAP;
    let mut kept: Vec<usize> = Vec::with_capacity(g.ticks.len());
    for &index in g.ticks.iter().rev() {
        let clear = kept
            .last()
            .is_none_or(|&after| label_x(g, after) - label_x(g, index) >= need);
        if clear {
            kept.push(index);
        }
    }
    kept.reverse();
    kept
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Points> {
    bounds(point(px(x), px(y)), size(px(w), px(h)))
}

/// Where day `index`'s label centres: under its slot, kept inside the plot.
fn label_x(g: &Geometry, index: usize) -> f32 {
    let half = g.tick_width / 2.0;
    // Not `clamp`, which panics when a chart too narrow for one label inverts its bounds.
    (g.left + (index as f32 + 0.5) * g.slot)
        .max(g.left + half)
        .min(g.right - half)
}

/// The plot with the day read now (`at`) banded and its bar and label in the picked colour.
fn paint(g: &Geometry, bars: &[DayBar], at: Option<usize>) -> Painter {
    let mut p = Painter::new();
    if let Some(at) = at.filter(|&at| at < bars.len()) {
        p = p
            .fill_color(ink(Token::Hover))
            .begin_path()
            .round_rect(
                rect(g.left + at as f32 * g.slot, TOP, g.slot, g.base - TOP),
                px(3.0),
            )
            .fill();
    }
    p = grid(p, g.left, g.right, &g.lines);
    for (index, day) in bars.iter().enumerate() {
        if day.tokens == 0 {
            continue;
        }
        let height = (day.tokens as f32 / g.top_value as f32 * (g.base - TOP)).max(1.0);
        let x = g.left + index as f32 * g.slot + (g.slot - g.bar) / 2.0;
        let fill = if at == Some(index) { PICKED } else { SERIES };
        p = p
            .fill_color(ink(fill))
            .begin_path()
            .round_rect(
                rect(x, g.base - height, g.bar, height),
                px((g.bar / 2.0).min(2.0)),
            )
            .fill();
    }
    let centre = |index: usize| label_x(g, index);
    p = p
        .font(&mono_font(AXIS_SIZE))
        .text_align(CanvasTextAlign::Center);
    // A label too near the day read now gives way to that day's own.
    let clear = |index: usize| {
        at.is_none_or(|at| (centre(index) - centre(at)).abs() >= g.tick_width + TICK_GAP / 2.0)
    };
    for &index in &g.ticks {
        if !clear(index) {
            continue;
        }
        p = p.fill_color(ink(Token::Ink3)).fill_text(
            &bars[index].tick,
            centre(index),
            g.base + 15.0,
        );
    }
    // The day read now always shows its date, over any thinned label near it.
    if let Some(at) = at.filter(|&at| at < bars.len()) {
        let x = centre(at);
        p = p
            .fill_color(ink(PICKED))
            .fill_text(&bars[at].tick, x, g.base + 15.0);
    }
    p
}

/// The chart, its readout over it, the days as hover and press targets over the canvas, and
/// the keys that walk them once it has the focus.
fn graphic(app: AppState, strip: &DayStrip, reading: Reading) -> AnyElement {
    let phone = app.form == Form::Phone;
    let height = if phone { PHONE_HEIGHT } else { HEIGHT };
    let Reading {
        width,
        hovered,
        picked,
    } = reading;
    let bars: Rc<[DayBar]> = strip.bars.clone().into();
    let count = bars.len();
    let shown = computed(move || hovered.get().or(picked.get()).filter(|&at| at < count));
    let peak = strip.peak;
    let last = count.saturating_sub(1);

    let words = bars.clone();
    let prompt = strip.prompt;
    let readout = dynamic(move || {
        let day = shown.get().and_then(|at| words.get(at));
        let line = day.map_or_else(|| prompt.to_owned(), |day| day.hint.clone());
        text(line)
            .test_id("usage-days-readout")
            .font_family(FONT_TEXT)
            .text_size(px(if phone { 12.5 } else { 12.0 }))
            .line_height(px(16.0))
            .tabular_nums()
            .text_color(color(if day.is_some() {
                Token::Ink
            } else {
                Token::Ink3
            }))
            .into_element()
    });

    // The day read now as the chart's value, which a screen reader says as the keys move it.
    let said = bars.clone();
    let spoken_day = computed(move || {
        shown
            .get()
            .and_then(|at| said.get(at))
            .map(|day| AccessibilityValue {
                text: Some(day.hint.as_str().into()),
                ..AccessibilityValue::default()
            })
    });
    let plot = dynamic(move || {
        let room = width.get();
        let bars = bars.clone();
        (room > 0.0).then(|| untracked(|| plot(bars, peak, room, height, shown, hovered, picked)))
    });

    let keys = ["left", "right", "home", "end"]
        .into_iter()
        .filter_map(|key| Keystroke::parse_with(key, false).ok());
    view()
        .gap(px(6.0))
        .child(view().min_h(px(32.0)).justify_end().child(readout))
        .child(
            view()
                .test_id("usage-days-chart")
                .w_full()
                .h(px(height))
                .rounded(px(4.0))
                .on_layout(move |frame: &Bounds<Points>| {
                    if width.get_untracked() != frame.size.width.0 {
                        width.set(frame.size.width.0);
                    }
                })
                .on_mouse_leave(move |_: &MouseEvent| hovered.set(None))
                .focusable(true)
                .focus_visible(|s| {
                    s.outline(px(2.0))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(2.0))
                })
                .on_focus(move || {
                    if count > 0 && picked.get_untracked().is_none_or(|at| at >= count) {
                        picked.set(Some(last));
                    }
                })
                .capture_keys(KeyCapture::keys(keys))
                .on_key_down(move |event: &KeyEvent| {
                    let now = shown.get_untracked();
                    let next = match event.key.as_ref() {
                        "ArrowLeft" => now.map_or(last, |at| at.saturating_sub(1)),
                        "ArrowRight" => now.map_or(last, |at| (at + 1).min(last)),
                        "Home" => 0,
                        "End" => last,
                        _ => return,
                    };
                    if count == 0 {
                        return;
                    }
                    hovered.set(None);
                    picked.set(Some(next));
                })
                .accessible(true)
                .role(AccessibilityRole::Image)
                .label(strip.spoken.clone())
                .accessibility_value(spoken_day)
                .child(plot),
        )
        .into_element()
}

fn hover_on(hovered: Signal<Option<usize>>, index: usize) {
    if hovered.get_untracked() != Some(index) {
        hovered.set(Some(index));
    }
}

/// The canvas at `room` wide, redrawn when the day read now changes, under one target a day.
fn plot(
    bars: Rc<[DayBar]>,
    peak: u64,
    room: f32,
    height: f32,
    shown: Memo<Option<usize>>,
    hovered: Signal<Option<usize>>,
    picked: Signal<Option<usize>>,
) -> AnyElement {
    let g = Rc::new(geometry(&bars, peak, room, height));
    let targets: Vec<AnyElement> = (0..bars.len())
        .map(|index| {
            view()
                .absolute()
                .top(px(0.0))
                .left(px(g.left + index as f32 * g.slot))
                .w(px(g.slot))
                .h(px(height))
                .accessibility_elements_hidden(true)
                .on_mouse_enter(move |_: &MouseEvent| hover_on(hovered, index))
                .on_mouse_move(move |_: &MouseEvent| hover_on(hovered, index))
                .on_press(move || picked.set(Some(index)))
                .focusable(false)
                .into_element()
        })
        .collect();
    let painted = g.clone();
    view()
        .relative()
        .w(px(room))
        .h(px(height))
        .child(
            canvas(move || paint(&painted, &bars, shown.get()))
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .w(px(room))
                .h(px(height))
                .accessibility_elements_hidden(true),
        )
        .children(targets)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chart_narrower_than_one_date_label_places_its_labels() {
        let day = |tokens| DayBar {
            fraction: 1.0,
            tokens,
            tick: "25 Nov".into(),
            hint: String::new(),
        };
        let bars = [day(10), day(1_800_000)];
        let narrow = geometry(&bars, 1_800_000, 40.0, HEIGHT);
        assert!(label_x(&narrow, 1).is_finite());
    }

    #[test]
    fn edge_labels_moved_inward_do_not_close_on_their_neighbour() {
        // The captured phone's 354 pt chart, a 3M scale and 46 days ending 25 Nov: the newest
        // label moves in from the edge and met 18 Nov's before thinning saw final positions.
        let bars: Vec<DayBar> = (0..46)
            .map(|at| DayBar {
                fraction: 0.0,
                tokens: if at == 45 { 2_800_000 } else { 0 },
                tick: "25 Nov".into(),
                hint: String::new(),
            })
            .collect();
        let g = geometry(&bars, 2_800_000, 354.0, PHONE_HEIGHT);
        assert_eq!(g.ticks.last(), Some(&45));
        for pair in g.ticks.windows(2) {
            let apart = label_x(&g, pair[1]) - label_x(&g, pair[0]);
            assert!(apart >= g.tick_width + TICK_GAP, "{pair:?} {apart}");
        }
    }
}
