//! Host performance's trends (`ui::chart`'s conventions). The page draws each figure's recent
//! readings as lines over a value scale from a visible zero baseline, the span under its
//! oldest end and "now" under its newest; a readout over the plot says the reading pointed at,
//! touched, or reached with ← → Home End once the chart has the keyboard focus. Home's widget
//! draws the inline form: the baseline and the lines, nothing else. A reading corrald did not
//! take is a gap in the line, never a fall to zero; a reading between two gaps is a dot. A
//! second line (send, write) is dashed as well as coloured apart. Stale readings draw faded.
//! A terminal draws a line of block heights in Cells, a gap as a blank Cell.

use std::f32::consts::TAU;
use std::rc::Rc;

use nsh_core::AccessibilityValue;
use nsh_core::canvas::CanvasTextAlign;
use nsh_core::keymap::Keystroke;
use nsh_core::prelude::*;

use crate::model::chart::scale;
use crate::model::host::{self, HostView, Kind, NEWEST, PROMPT, Series};
use crate::ui::chart::{AXIS_SIZE, PICKED, SERIES, grid, gutter, ink, mono_font};
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// Room over the top gridline for its label, and under the baseline for the span.
const TOP: f32 = 8.0;
const FOOT: f32 = 16.0;
/// Gridlines over zero at most.
const LINES: u64 = 2;
/// The second line's colour, beside [`SERIES`].
const SECOND: Token = Token::Ink2;
/// Block heights from an eighth to full.
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// What a trend keeps while the readings move under it: its measured width and the reading
/// pointed at and picked. Made once per card.
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

/// The value scale: its top and each gridline's value and label.
struct Axis {
    top: f64,
    ticks: Vec<(f64, String)>,
}

/// CPU from 0 to 100 %; bytes on round steps of their own binary unit, so a gridline reads
/// "512 MiB", not an odd figure.
fn axis(kind: Kind, series: &[Series]) -> Axis {
    if kind == Kind::Cpu {
        return Axis {
            top: 100.0,
            ticks: [0.0, 50.0, 100.0]
                .into_iter()
                .map(|v| (v, host::figure(kind, v)))
                .collect(),
        };
    }
    let max = series.iter().map(|s| s.max).fold(1.0, f64::max);
    let mut unit = 1.0_f64;
    while max / unit >= 1024.0 && unit < 1024_f64.powi(5) {
        unit *= 1024.0;
    }
    let steps = scale((max / unit).ceil() as u64, LINES);
    Axis {
        top: steps.top as f64 * unit,
        ticks: steps
            .ticks
            .iter()
            .map(|&tick| {
                let value = tick as f64 * unit;
                (value, host::figure(kind, value))
            })
            .collect(),
    }
}

/// Where a trend falls at one width.
struct Geometry {
    left: f32,
    right: f32,
    top: f32,
    base: f32,
    axis: Option<Axis>,
    count: usize,
}

impl Geometry {
    /// The full chart: a gutter for the scale, the span under it.
    fn full(kind: Kind, series: &[Series], room: f32, height: f32) -> Self {
        let axis = axis(kind, series);
        let left = gutter(axis.ticks.iter().map(|(_, label)| label.as_str()));
        Self {
            left,
            right: (room - 4.0).max(left + 1.0),
            top: TOP,
            base: (height - FOOT).round(),
            axis: Some(axis),
            count: count(series),
        }
    }

    /// The inline form: the whole box, up to the model's top (100 % for CPU, the whole memory).
    fn inline(series: &[Series], room: f32, height: f32) -> Self {
        let top = series.iter().map(|s| s.max).fold(1.0, f64::max);
        Self {
            left: 1.0,
            right: (room - 2.0).max(2.0),
            top: 2.0,
            base: height - 1.5,
            axis: Some(Axis {
                top,
                ticks: Vec::new(),
            }),
            count: count(series),
        }
    }

    fn x(&self, at: usize) -> f32 {
        if self.count <= 1 {
            (self.left + self.right) / 2.0
        } else {
            self.left + (self.right - self.left) * at as f32 / (self.count - 1) as f32
        }
    }

    fn y(&self, value: f64) -> f32 {
        let top = self.axis.as_ref().map_or(1.0, |axis| axis.top.max(1.0));
        self.base - (value / top).clamp(0.0, 1.0) as f32 * (self.base - self.top)
    }

    /// The reading nearest `x`.
    fn index(&self, x: f32) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let span = (self.right - self.left).max(1.0);
        let share = ((x - self.left) / span).clamp(0.0, 1.0);
        Some((share * (self.count - 1) as f32).round() as usize)
    }
}

fn count(series: &[Series]) -> usize {
    series.iter().map(|s| s.points.len()).max().unwrap_or(0)
}

/// One line with its gaps: the pen lifts over a missing reading, and a reading with gaps on
/// both sides is a dot.
fn line(mut p: Painter, g: &Geometry, points: &[Option<f64>], hue: Hsla) -> Painter {
    p = p.begin_path();
    let mut pen = false;
    for (at, point) in points.iter().enumerate() {
        match point {
            Some(value) => {
                let (x, y) = (g.x(at), g.y(*value));
                p = if pen {
                    p.line_to(x, y)
                } else {
                    p.move_to(x, y)
                };
                pen = true;
            }
            None => pen = false,
        }
    }
    p = p.stroke().fill_color(hue);
    for (at, point) in points.iter().enumerate() {
        let alone = at
            .checked_sub(1)
            .is_none_or(|before| points[before].is_none())
            && points.get(at + 1).is_none_or(Option::is_none);
        if let (Some(value), true) = (point, alone) {
            p = p
                .begin_path()
                .arc(g.x(at), g.y(*value), 1.8, 0.0, TAU)
                .fill();
        }
    }
    p
}

/// The trend with the reading read now (`at`) ruled and its points marked.
fn paint(
    g: &Geometry,
    series: &[Series],
    at: Option<usize>,
    stale: bool,
    span: Option<&str>,
) -> Painter {
    let mut p = Painter::new();
    let full = g.axis.as_ref().is_some_and(|axis| !axis.ticks.is_empty());
    if let Some(axis) = g.axis.as_ref().filter(|_| full) {
        let lines: Vec<(String, f32)> = axis
            .ticks
            .iter()
            .map(|(value, label)| (label.clone(), g.y(*value)))
            .collect();
        p = grid(p, g.left, g.right, &lines);
    } else {
        let y = g.base.round() + 0.5;
        p = p
            .stroke_color(ink(Token::Rule2))
            .line_width(1.0)
            .begin_path()
            .move_to(g.left, y)
            .line_to(g.right, y)
            .stroke();
    }
    if let Some(at) = at.filter(|&at| at < g.count) {
        p = p
            .stroke_color(ink(Token::Rule2))
            .line_width(1.0)
            .begin_path()
            .move_to(g.x(at).round() + 0.5, g.top)
            .line_to(g.x(at).round() + 0.5, g.base)
            .stroke();
    }
    p = p.global_alpha(if stale { 0.45 } else { 1.0 });
    for (n, s) in series.iter().enumerate() {
        let (hue, dash): (Token, &[Points]) = if n == 0 {
            (SERIES, &[])
        } else {
            (SECOND, &[px(4.0), px(3.0)])
        };
        p = p
            .stroke_color(ink(hue))
            .line_width(if full { 1.6 } else { 1.3 })
            .line_dash(dash);
        p = line(p, g, &s.points, ink(hue));
    }
    p = p.line_dash(&[]).global_alpha(1.0);
    if let Some(at) = at.filter(|&at| at < g.count) {
        p = p.fill_color(ink(PICKED));
        for value in series
            .iter()
            .filter_map(|s| s.points.get(at).copied().flatten())
        {
            p = p
                .begin_path()
                .arc(g.x(at), g.y(value), 2.6, 0.0, TAU)
                .fill();
        }
    }
    if full {
        p = p.font(&mono_font(AXIS_SIZE)).fill_color(ink(Token::Ink3));
        if let Some(span) = span {
            p = p
                .text_align(CanvasTextAlign::Start)
                .fill_text(span, g.left, g.base + 13.0);
        }
        p = p
            .text_align(CanvasTextAlign::End)
            .fill_text(NEWEST, g.right, g.base + 13.0);
    }
    p
}

/// What a trend draws from the page's figures: its lines, whether they are stale, the span.
#[derive(Clone, PartialEq)]
struct Drawn {
    series: Vec<Series>,
    stale: bool,
    span: Option<String>,
}

fn drawn(view: &HostView, kind: Kind) -> Drawn {
    Drawn {
        series: view
            .metric(kind)
            .map(|metric| metric.series.clone())
            .unwrap_or_default(),
        stale: view.stale(),
        span: host::window(view),
    }
}

/// The page's trend of `kind`: the readout over the plot, the plot `height` tall, a legend
/// under it when it draws two lines. test ids "host-<slug>-chart", "host-<slug>-readout".
pub(super) fn trend(
    data: Memo<HostView>,
    kind: Kind,
    reading: Reading,
    height: f32,
    phone: bool,
) -> AnyElement {
    let Reading {
        width,
        hovered,
        picked,
    } = reading;
    let shown_lines = computed(move || data.with(|view| drawn(view, kind)));
    let total = computed(move || shown_lines.with(|d| count(&d.series)));
    let shown = computed(move || {
        let total = total.get();
        hovered.get().or(picked.get()).filter(|&at| at < total)
    });
    let readout = dynamic(move || {
        let at = shown.get();
        let line = at
            .and_then(|at| data.with(|view| view.metric(kind).map(|m| host::reading(view, m, at))))
            .unwrap_or_else(|| PROMPT.to_owned());
        text(line)
            .test_id(&format!("host-{}-readout", kind.slug()))
            .number_of_lines(1u32)
            .truncate()
            .font_family(FONT_TEXT)
            .text_size(px(if phone { 12.5 } else { 11.5 }))
            .line_height(px(16.0))
            .tabular_nums()
            .text_color(color(if at.is_some() {
                Token::Ink
            } else {
                Token::Ink3
            }))
            .into_element()
    });
    let spoken = computed(move || {
        data.with(|view| {
            view.metric(kind)
                .map(|m| SharedString::from(host::spoken(view, m)))
                .unwrap_or_default()
        })
    });
    let spoken_at = computed(move || {
        let at = shown.get()?;
        data.with(|view| {
            view.metric(kind).map(|m| AccessibilityValue {
                text: Some(host::reading(view, m, at).into()),
                ..AccessibilityValue::default()
            })
        })
    });
    let plot = dynamic(move || {
        let room = width.get();
        let lines = shown_lines.get();
        (room > 0.0).then(|| {
            untracked(|| {
                let lines = lines.clone();
                let g = Rc::new(Geometry::full(kind, &lines.series, room, height));
                let at_point = g.clone();
                let painted = g.clone();
                view()
                    .relative()
                    .w(px(room))
                    .h(px(height))
                    .on_mouse_move(move |event: &MouseEvent| {
                        let at = at_point.index(event.position.x.0);
                        if hovered.get_untracked() != at {
                            hovered.set(at);
                        }
                    })
                    .on_press(move |event: &PressEvent| {
                        picked.set(g.index(event.position.x.0));
                    })
                    .child(
                        canvas(move || {
                            paint(
                                &painted,
                                &lines.series,
                                shown.get(),
                                lines.stale,
                                lines.span.as_deref(),
                            )
                        })
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .w(px(room))
                        .h(px(height))
                        .accessibility_elements_hidden(true),
                    )
                    .into_element()
            })
        })
    });
    let keys = ["left", "right", "home", "end"]
        .into_iter()
        .filter_map(|key| Keystroke::parse_with(key, false).ok());
    let chart = view()
        .test_id(&format!("host-{}-chart", kind.slug()))
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
            let total = total.get_untracked();
            if total > 0 && picked.get_untracked().is_none_or(|at| at >= total) {
                picked.set(Some(total - 1));
            }
        })
        .capture_keys(KeyCapture::keys(keys))
        .on_key_down(move |event: &KeyEvent| {
            let total = total.get_untracked();
            if total == 0 {
                return;
            }
            let last = total - 1;
            let now = shown.get_untracked();
            let next = match event.key.as_ref() {
                "ArrowLeft" => now.map_or(last, |at| at.saturating_sub(1)),
                "ArrowRight" => now.map_or(last, |at| (at + 1).min(last)),
                "Home" => 0,
                "End" => last,
                _ => return,
            };
            hovered.set(None);
            picked.set(Some(next));
        })
        .accessible(true)
        .role(AccessibilityRole::Image)
        .label(spoken)
        .accessibility_value(spoken_at)
        .child(plot);
    let legend = dynamic(move || {
        let labels: Vec<&'static str> =
            shown_lines.with(|d| d.series.iter().map(|s| s.label).collect());
        (labels.len() > 1).then(|| untracked(|| legend(&labels)))
    });
    view()
        .gap(px(4.0))
        .child(readout)
        .child(chart)
        .child(legend)
        .into_element()
}

/// Each line's swatch and name: solid, then dashed.
fn legend(labels: &[&'static str]) -> AnyElement {
    view()
        .flex_row()
        .flex_wrap()
        .gap(px(12.0))
        .accessibility_elements_hidden(true)
        .children(labels.iter().enumerate().map(|(n, label)| {
            let hue = if n == 0 { SERIES } else { SECOND };
            let swatch = if n == 0 {
                view().w(px(14.0)).h(px(2.0)).bg(color(hue))
            } else {
                view()
                    .flex_row()
                    .gap(px(2.0))
                    .child(view().w(px(4.0)).h(px(2.0)).bg(color(hue)))
                    .child(view().w(px(4.0)).h(px(2.0)).bg(color(hue)))
                    .child(view().w(px(4.0)).h(px(2.0)).bg(color(hue)))
            };
            view()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(swatch)
                .child(
                    text(*label)
                        .font_family(FONT_TEXT)
                        .text_size(px(11.0))
                        .text_color(color(Token::Ink3)),
                )
                .into_element()
        }))
        .into_element()
}

/// Home's inline trend of `kind`, `height` tall across its box: the baseline and the lines.
pub(super) fn spark(data: Memo<HostView>, kind: Kind, height: f32) -> AnyElement {
    let width = signal(0.0_f32);
    let lines = computed(move || data.with(|view| drawn(view, kind)));
    let plot = dynamic(move || {
        let room = width.get();
        let lines = lines.get();
        (room > 0.0 && count(&lines.series) > 0).then(|| {
            untracked(|| {
                let lines = lines.clone();
                let g = Geometry::inline(&lines.series, room, height);
                canvas(move || paint(&g, &lines.series, None, lines.stale, None))
                    .w(px(room))
                    .h(px(height))
                    .into_element()
            })
        })
    });
    view()
        .w_full()
        .h(px(height))
        .accessibility_elements_hidden(true)
        .on_layout(move |frame: &Bounds<Points>| {
            if width.get_untracked() != frame.size.width.0 {
                width.set(frame.size.width.0);
            }
        })
        .child(plot)
        .into_element()
}

/// A terminal's trend: the newest `room` readings as block heights against the model's top, a
/// reading not taken as a blank Cell. None when there is nothing to draw.
pub(super) fn cells(series: &Series, room: usize) -> Option<String> {
    let skip = series.points.len().saturating_sub(room);
    let shown = &series.points[skip..];
    if shown.iter().all(Option::is_none) {
        return None;
    }
    let top = series.max.max(1.0);
    Some(
        shown
            .iter()
            .map(|point| match point {
                Some(value) => {
                    let share = (value / top).clamp(0.0, 1.0);
                    let step = (share * BLOCKS.len() as f64).ceil() as usize;
                    BLOCKS[step.clamp(1, BLOCKS.len()) - 1]
                }
                None => ' ',
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reading corrald did not take must not draw as a zero-height block.
    #[test]
    fn a_missing_reading_is_a_blank_cell_not_zero() {
        let series = Series {
            label: "Busy",
            points: vec![Some(50.0), None, Some(0.0)],
            max: 100.0,
        };
        assert_eq!(cells(&series, 10).as_deref(), Some("▄ ▁"));
    }
}
