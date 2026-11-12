//! A provider's band chart (`.band-chart`): the 90 to 100 and 70 to 90 zones, the band and hour
//! labels, the window use line and its last point. The geometry is the
//! model's `band_chart`, asked again at the width the chart is laid out at, so the chart follows
//! its container while its labels keep their size. Desktop and phone only: a terminal shows the
//! cell bars of `cellbar` instead.

use std::f32::consts::TAU;

use nsh_core::SvgPaint;
use nsh_core::canvas::{CanvasTextAlign, LineJoin};
use nsh_core::prelude::*;

use crate::model::usage::{BandChart, Level, Point as ChartPoint, Provider, band_chart};
use crate::ui::tokens::{FONT_MONO, Token, color};

/// `.band-chart .z1`: the held hue at 14 percent; `.z2`: the failure hue at 16 percent.
const WARN_ZONE: f32 = 0.14;
const BAD_ZONE: f32 = 0.16;
/// `.band-chart .zl`, `.ax`: 9.5 pt mono.
const AXIS_SIZE: f32 = 9.5;
/// `.band-chart .ln`: 1.6 pt; `.pt`: a 3 pt dot.
const LINE_WIDTH: f32 = 1.6;
const DOT_RADIUS: f32 = 3.0;

/// The chart, `height` points high and as wide as its container. `start_width` is the mockup's
/// view box width (desktop 380, phone 354), drawn until the first layout reports the real one.
/// The chart is one image for screen readers, labelled with the model's description; the
/// figures beside it carry the same numbers as text.
pub fn chart(provider: &'static Provider, start_width: f32, height: f32) -> AnyElement {
    let width = signal(start_width);
    let label = band_chart(provider, start_width, height).accessibility_label;
    view()
        .relative()
        .w_full()
        .h(px(height))
        .flex_shrink_0()
        .role(AccessibilityRole::Image)
        .label(label)
        .on_layout(move |frame: &Bounds<Points>| {
            let measured = frame.size.width.0;
            if measured > 0.0 && (measured - width.get_untracked()).abs() > 0.5 {
                width.set(measured);
            }
        })
        .child(dynamic(move || {
            layers(&band_chart(provider, width.get(), height))
        }))
        .into_element()
}

/// The zones as filled boxes under two drawings: the labels in ink-4, the series in ink. Each
/// drawing takes its colour from its own view, so both follow the appearance natively.
fn layers(chart: &BandChart) -> AnyElement {
    let zones = chart
        .zones
        .iter()
        .map(|zone| {
            let fill = match zone.level {
                Level::Bad => color(Token::Fail).opacity(BAD_ZONE),
                Level::Warn | Level::None => color(Token::Held).opacity(WARN_ZONE),
            };
            view()
                .absolute()
                .left(px(zone.origin.x))
                .top(px(zone.origin.y))
                .w(px(zone.width.max(0.0)))
                .h(px(zone.height.max(0.0)))
                .bg(fill)
                .into_element()
        })
        .collect::<Vec<_>>();
    view()
        .absolute()
        .inset(px(0.0))
        .accessibility_elements_hidden(true)
        .pointer_events(PointerEvents::None)
        .children(zones)
        .child(
            canvas(labels(chart))
                .absolute()
                .inset(px(0.0))
                .fill_color(color(Token::Ink4)),
        )
        .child(
            canvas(series(chart))
                .absolute()
                .inset(px(0.0))
                .fill_color(color(Token::Ink))
                .stroke_color(color(Token::Ink)),
        )
        .into_element()
}

/// The band labels at the left edge and every third hour centred under its sample, at the
/// baselines the model gives.
fn labels(chart: &BandChart) -> Painter {
    let font = format!("{AXIS_SIZE}px '{FONT_MONO}'");
    let painter = Painter::new()
        .font(&font)
        .fill_style_from_view(SvgPaint::CurrentColor)
        .text_align(CanvasTextAlign::Left);
    let painter = chart.vertical_labels.iter().fold(painter, |p, label| {
        p.fill_text(label.text, label.position.x, label.position.y)
    });
    chart
        .hour_labels
        .iter()
        .fold(painter.text_align(CanvasTextAlign::Center), |p, label| {
            p.fill_text(label.text, label.position.x, label.position.y)
        })
}

/// The line through the samples and the dot on the last one.
fn series(chart: &BandChart) -> Painter {
    let painter = Painter::new()
        .fill_style_from_view(SvgPaint::CurrentColor)
        .stroke_style_from_view(SvgPaint::CurrentColor)
        .line_width(LINE_WIDTH)
        .line_join(LineJoin::Round);
    let painter = trace(painter.begin_path(), &chart.line).stroke();
    match chart.last {
        Some(last) => painter
            .begin_path()
            .arc(last.x, last.y, DOT_RADIUS, 0.0, TAU)
            .fill(),
        None => painter,
    }
}

/// A path through `points` in order.
fn trace(painter: Painter, points: &[ChartPoint]) -> Painter {
    points.iter().enumerate().fold(painter, |p, (at, point)| {
        if at == 0 {
            p.move_to(point.x, point.y)
        } else {
            p.line_to(point.x, point.y)
        }
    })
}
