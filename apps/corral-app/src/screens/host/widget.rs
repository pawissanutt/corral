//! Host performance in Home's forms. The board's widget (`panel-host`): the host and that its
//! figures are host-wide in the head with the link to the page (`btn-open-host`), the status
//! when the readings are not live, then a tile per figure (`host-tile-<slug>`, each opening
//! the page) with its value and inline trend; narrower than four tiles' room it lists the
//! figures as label and value rows instead. A terminal's Home has one numeric line
//! (`home-host`), the phone's Home one row (`home-host`); both open the page.

use nsh_core::prelude::*;

use crate::model::host::{self, HostView, Kind, LINE_LABEL, NO_VALUE, OPEN, TITLE};
use crate::model::{Destination, Form};
use crate::screens::home::{Link, panel};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::hint::hinted;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::{chart, mark};

/// Below this width the widget lists its figures as rows: three tiles do not fit.
const ROWS_BELOW: f32 = 400.0;
/// A tile's least width, and its trend's height.
const TILE: f32 = 118.0;
const SPARK: f32 = 22.0;

fn open(app: AppState) {
    app.go(Destination::Host);
}

/// Home's Host performance widget.
pub fn widget(app: AppState) -> AnyElement {
    if app.form == Form::Terminal {
        return line(app);
    }
    let data = computed(move || app.feed.with(host::view));
    let note = computed(move || data.with(|v| format!("{} · {}", v.host_line, host::HOST_WIDE)));
    dynamic(move || {
        let note = note.get();
        untracked(|| {
            panel(
                app,
                "panel-host",
                TITLE.to_owned(),
                Some(note),
                Some(Link {
                    words: OPEN.to_owned(),
                    test_id: "btn-open-host".to_owned(),
                    press: Box::new(move || open(app)),
                }),
                body(app, data),
            )
        })
    })
    .into_element()
}

/// The status while not live, then the tiles or the rows.
fn body(app: AppState, data: Memo<HostView>) -> AnyElement {
    let width = signal(0.0_f32);
    let rows = computed(move || {
        let w = width.get();
        w > 0.0 && w < ROWS_BELOW
    });
    let standing = computed(move || {
        data.with(|v| {
            (!matches!(v.standing, host::Standing::Live))
                .then(|| (v.standing.clone(), v.status.clone()))
        })
    });
    let status = dynamic(move || {
        let (standing, words) = standing.get()?;
        let (glyph, hue) = mark(&standing);
        Some(
            view()
                .test_id("panel-host-status")
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .px(px(12.0))
                .pt(px(8.0))
                .child(text(glyph).text_color(hue_ink(hue)))
                .child(
                    text(words)
                        .min_w(px(0.0))
                        .font_family(FONT_TEXT)
                        .text_size(px(12.0))
                        .text_color(color(Token::Ink2)),
                )
                .into_element(),
        )
    });
    let figures = dynamic(move || {
        let rows = rows.get();
        untracked(|| {
            let items: Vec<AnyElement> = Kind::ALL
                .into_iter()
                .map(|kind| {
                    if rows {
                        figure_row(app, data, kind)
                    } else {
                        tile(app, data, kind)
                    }
                })
                .collect();
            if rows {
                view().children(items).into_element()
            } else {
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .pt(px(10.0))
                    .pb(px(12.0))
                    .children(items)
                    .into_element()
            }
        })
    });
    view()
        .border_t_1()
        .border_color(color(Token::Rule))
        .min_w(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| {
            if (width.get_untracked() - frame.size.width.0).abs() > 0.5 {
                width.set(frame.size.width.0);
            }
        })
        .child(status)
        .child(figures)
        .into_element()
}

/// A figure's value and the words said for it: the value, or why it has none.
fn value_of(view: &HostView, kind: Kind) -> (Option<String>, String, bool) {
    let metric = view.metric(kind);
    let value = metric.and_then(|m| m.value.clone());
    let why = metric
        .and_then(|m| m.absent.clone().or_else(|| m.detail.clone()))
        .unwrap_or_default();
    (value, why, view.stale())
}

fn value_ink(value: &Option<String>, stale: bool) -> Token {
    match (value, stale) {
        (None, _) => Token::Ink4,
        (Some(_), true) => Token::Ink3,
        (Some(_), false) => Token::Ink,
    }
}

/// One figure as a tile: its name, value and inline trend, opening the page.
fn tile(app: AppState, data: Memo<HostView>, kind: Kind) -> AnyElement {
    let shown = computed(move || data.with(|v| value_of(v, kind)));
    let has_trend = computed(move || {
        data.with(|v| {
            v.metric(kind).is_some_and(|m| {
                m.series
                    .iter()
                    .any(|s| s.points.iter().any(Option::is_some))
            })
        })
    });
    let words = dynamic(move || {
        let (value, why, stale) = shown.get();
        let spoken = format!(
            "{}: {}. {}",
            kind.label(),
            value.as_deref().unwrap_or(NO_VALUE),
            why
        );
        let shown_value = text(value.clone().unwrap_or_else(|| NO_VALUE.to_owned()))
            .number_of_lines(1u32)
            .truncate()
            .min_w(px(0.0))
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(15.0))
            .line_height(px(20.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color(value_ink(&value, stale)))
            .label(spoken);
        untracked(|| hinted(app, shown_value, why))
    });
    let spark = dynamic(move || {
        has_trend
            .get()
            .then(|| untracked(|| chart::spark(data, kind, SPARK)))
    });
    view()
        .test_id(&format!("host-tile-{}", kind.slug()))
        .role(AccessibilityRole::Link)
        .cursor_pointer()
        .flex_grow(1.0)
        .w(px(TILE))
        .min_w(px(0.0))
        .gap(px(3.0))
        .px(px(10.0))
        .pt(px(7.0))
        .pb(px(8.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .hover(|s| {
            s.bg(color(Token::Surface2))
                .border_color(color(Token::Rule2))
        })
        .on_press(move || open(app))
        .child(
            text(kind.label().to_uppercase())
                .number_of_lines(1u32)
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(10.0))
                .letter_spacing(px(0.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink4)),
        )
        .child(words)
        .child(view().h(px(SPARK)).child(spark))
        .into_element()
}

/// One figure as a row of a narrow widget: its name, then its value.
fn figure_row(app: AppState, data: Memo<HostView>, kind: Kind) -> AnyElement {
    let shown = computed(move || data.with(|v| value_of(v, kind)));
    let value = dynamic(move || {
        let (value, why, stale) = shown.get();
        let words = text(value.clone().unwrap_or_else(|| NO_VALUE.to_owned()))
            .number_of_lines(1u32)
            .truncate()
            .min_w(px(0.0))
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(12.0))
            .text_color(color(value_ink(&value, stale)));
        untracked(|| hinted(app, words, why))
    });
    view()
        .test_id(&format!("host-tile-{}", kind.slug()))
        .role(AccessibilityRole::Link)
        .cursor_pointer()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .min_h(px(26.0))
        .px(px(12.0))
        .py(px(3.0))
        .border_t_1()
        .border_color(color(Token::Rule))
        .hover(|s| s.bg(color(Token::Hover)))
        .on_press(move || open(app))
        .child(
            text(kind.label())
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_family(FONT_TEXT)
                .text_size(px(12.5))
                .text_color(color(Token::Ink2)),
        )
        .child(view().flex_1().min_w(px(0.0)).items_end().child(value))
        .into_element()
}

/// A terminal Home's line: "corrald host" and every figure in numbers, opening the page.
pub fn line(app: AppState) -> AnyElement {
    let shown = computed(move || {
        app.feed.with(|feed| {
            let view = host::view(feed);
            (host::compact(&view), view.stale(), view.status)
        })
    });
    let handle = element_ref();
    let focused = term::focus_of(handle);
    let words = dynamic(move || {
        let (figures, stale, _) = shown.get();
        text(figures)
            .flex_1()
            .min_w(px(0.0))
            .number_of_lines(1u32)
            .truncate()
            .text_color(color(if stale { Token::Ink3 } else { Token::Ink }))
            .into_element()
    });
    let spoken = computed(move || {
        SharedString::from(
            shown.with(|(figures, _, status)| format!("{LINE_LABEL}: {figures}. {status}")),
        )
    });
    view()
        .test_id("home-host")
        .element_ref(&handle)
        .role(AccessibilityRole::Button)
        .label(spoken)
        .cursor_pointer()
        .flex_row()
        .gap(term::cells(1.0))
        .min_w(px(0.0))
        .on_press(move || open(app))
        .child(term::focus_cell(focused))
        .child(
            text(LINE_LABEL)
                .flex_shrink_0()
                .number_of_lines(1u32)
                .text_color(color(Token::Ink3)),
        )
        .child(words)
        .into_element()
}

/// The phone Home's row: the title over the figures in numbers, opening the page.
pub fn phone_row(app: AppState) -> AnyElement {
    let shown = computed(move || {
        app.feed.with(|feed| {
            let view = host::view(feed);
            (host::compact(&view), view.stale(), view.host_line)
        })
    });
    let content = dynamic(move || {
        let (figures, stale, host_line) = shown.get();
        view()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(2.0))
            .child(
                text(TITLE)
                    .number_of_lines(1u32)
                    .font_family(FONT_TEXT)
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color(Token::Ink)),
            )
            .child(
                text(figures)
                    .number_of_lines(2u32)
                    .truncate()
                    .font_family(FONT_MONO)
                    .tabular_nums()
                    .text_size(px(12.0))
                    .text_color(color(if stale { Token::Ink3 } else { Token::Ink2 })),
            )
            .child(
                text(format!("{host_line} · {}", host::HOST_WIDE))
                    .number_of_lines(1u32)
                    .truncate()
                    .font_family(FONT_TEXT)
                    .text_size(px(12.0))
                    .text_color(color(Token::Ink3)),
            )
            .into_element()
    });
    let spoken = computed(move || {
        SharedString::from(shown.with(|(figures, _, line)| format!("{TITLE}, {line}: {figures}")))
    });
    view()
        .test_id("home-host")
        .role(AccessibilityRole::Button)
        .label(spoken)
        .cursor_pointer()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(18.0))
        .py(px(10.0))
        .border_t_1()
        .border_b_1()
        .border_color(color(Token::Rule))
        .active(|s| s.bg(color(Token::Hover)))
        .on_press(move || open(app))
        .child(icon(Icon::Host, 18.0, color(Token::Ink2)))
        .child(content)
        .child(icon(Icon::Chevron, 16.0, color(Token::Ink3)))
        .into_element()
}
