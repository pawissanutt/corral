//! Activity › Host performance and Home's Host performance widget (`model::host`): the machine
//! running corrald, never this device, host-wide whatever the project scope. The page leads
//! with the host and how fresh its readings are (`host-status`), says that the scope does not
//! apply, then a card per figure (`host-<slug>`): its value and detail, a gauge where it has a
//! share, its recent trend (`chart`), why it has no value when it has none, and what it covers.
//! Disk space (capacity) and Disk I/O (rates) are separate cards. Stale values are faded and
//! say so. Desktop keeps the page to `MEASURE_PAGE` with the cards flowing in a wrapping row;
//! the phone stacks them in one column with smaller trends; a terminal gives two rows a figure,
//! numbers first, with a Cell trend when the page is wide. Home's widget, its terminal line and
//! its phone row are `widget`. Every word and figure is `model::host`'s.

mod chart;
mod widget;

pub use widget::{line, phone_row, widget};

use nsh_core::prelude::*;

use crate::model::host::{self, HostView, Kind, NO_VALUE, SCOPE_NOTE, STALE, Standing, TITLE};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::{body, measure, measured_head, pane};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, MEASURE_PAGE, Token, color};

/// A card's least width on the desktop: three to a row on the measured page.
const CARD: f32 = 290.0;
/// The trend's height on the desktop and on the phone.
const TREND: f32 = 112.0;
const PHONE_TREND: f32 = 96.0;
/// A terminal's label column, and the page width from which a row also draws its trend.
const LABEL_CELLS: f32 = 12.0;
const TREND_FROM_CELLS: f32 = 96.0;
/// The trend's least and most Cells, and the room the figures keep before it.
const TREND_CELLS: usize = 20;
const TREND_MOST_CELLS: usize = 48;
const TREND_AFTER_CELLS: f32 = 64.0;

/// Activity › Host performance. test ids "host-page", "host-status", "host-<slug>".
pub fn screen(app: AppState) -> AnyElement {
    let data = computed(move || app.feed.with(host::view));
    match app.form {
        Form::Desktop => desktop(app, data),
        Form::Phone => phone(app, data),
        Form::Terminal => terminal(app, data),
    }
}

/// The standing's mark: a shape as well as a colour.
fn mark(standing: &Standing) -> (&'static str, Hue) {
    match standing {
        Standing::Live => ("●", Hue::Ok),
        Standing::Waiting | Standing::Connecting => ("○", Hue::Neutral),
        Standing::Stale { .. } => ("◐", Hue::Held),
        Standing::NotServed => ("⊘", Hue::Neutral),
    }
}

/// Words that wrap, at `size` points off the terminal.
fn said(form: Form, content: impl Into<SharedString>, size: f32, ink: Token) -> Text {
    let words = text(content.into()).min_w(px(0.0)).text_color(color(ink));
    match form {
        Form::Terminal => words,
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(size))
            .line_height(px(size + 4.5)),
    }
}

/// The status line with its mark, then the scope note.
fn status(app: AppState, data: Memo<HostView>) -> AnyElement {
    let form = app.form;
    let line = computed(move || data.with(|v| (v.standing.clone(), v.status.clone())));
    let status = dynamic(move || {
        let (standing, words) = line.get();
        let (glyph, hue) = mark(&standing);
        view()
            .test_id("host-status")
            .flex_row()
            .items_center()
            .gap(if form == Form::Terminal {
                term::cells(1.0)
            } else {
                px(7.0)
            })
            .min_w(px(0.0))
            .child(text(glyph).flex_shrink_0().text_color(hue_ink(hue)))
            .child(said(form, words, 13.0, Token::Ink).flex_shrink(1.0))
            .into_element()
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 3.0 }))
        .min_w(px(0.0))
        .child(status)
        .child(said(form, SCOPE_NOTE, 12.0, Token::Ink3))
        .into_element()
}

/// What a card's words show from the page's figures.
#[derive(Clone, PartialEq)]
struct Facts {
    value: Option<String>,
    detail: Option<String>,
    absent: Option<String>,
    share: Option<f32>,
    note: Option<String>,
    trend: bool,
    stale: bool,
}

fn facts(view: &HostView, kind: Kind) -> Option<Facts> {
    view.metric(kind).map(|m| Facts {
        value: m.value.clone(),
        detail: m.detail.clone(),
        absent: m.absent.clone(),
        share: m.share,
        note: m.note.clone(),
        trend: m
            .series
            .iter()
            .any(|s| s.points.iter().any(Option::is_some)),
        stale: view.stale(),
    })
}

/// A gauge `share` full: the series colour, held once nearly full.
fn gauge(share: f32, stale: bool) -> AnyElement {
    let fill = if share >= 0.9 {
        Token::Held
    } else {
        Token::Worker
    };
    view()
        .relative()
        .w_full()
        .h(px(6.0))
        .rounded(px(3.0))
        .bg(color(Token::Rule))
        .accessibility_elements_hidden(true)
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(relative(share.clamp(0.0, 1.0)))
                .rounded(px(3.0))
                .opacity(if stale { 0.45_f32 } else { 1.0_f32 })
                .bg(color(fill)),
        )
        .into_element()
}

/// One figure's card on the desktop and the phone.
fn card(app: AppState, data: Memo<HostView>, kind: Kind) -> AnyElement {
    let phone = app.form == Form::Phone;
    let shown = computed(move || data.with(|v| facts(v, kind)));
    let reading = chart::Reading::new();
    let words = dynamic(move || {
        let facts = shown.get()?;
        Some(untracked(|| card_words(app, kind, &facts)))
    });
    let trend = computed(move || shown.with(|f| f.as_ref().is_some_and(|f| f.trend)));
    let drawn = dynamic(move || {
        trend.get().then(|| {
            untracked(|| {
                let height = if phone { PHONE_TREND } else { TREND };
                chart::trend(data, kind, reading, height, phone)
            })
        })
    });
    let note = dynamic(move || {
        let note = shown.with(|f| f.as_ref().and_then(|f| f.note.clone()))?;
        Some(said(app.form, note, 11.5, Token::Ink3).into_element())
    });
    view()
        .test_id(&format!("host-{}", kind.slug()))
        .flex_grow(1.0)
        .when(!phone, |card| card.w(px(CARD)))
        .min_w(px(0.0))
        .gap(px(8.0))
        .px(px(14.0))
        .pt(px(11.0))
        .pb(px(12.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::Surface))
        .child(words)
        .child(drawn)
        .child(note)
        .into_element()
}

/// The card's label, value, detail and gauge, or why it has no value.
fn card_words(app: AppState, kind: Kind, facts: &Facts) -> AnyElement {
    let form = app.form;
    let label = text(kind.label().to_uppercase())
        .number_of_lines(1u32)
        .font_family(FONT_TEXT)
        .text_size(px(10.5))
        .letter_spacing(px(0.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink4));
    let stale = facts.stale.then(|| {
        text(STALE)
            .font_family(FONT_TEXT)
            .text_size(px(11.0))
            .text_color(hue_ink(Hue::Held))
    });
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(label)
        .child(view().flex_1())
        .children(stale);
    let value = text(facts.value.clone().unwrap_or_else(|| NO_VALUE.to_owned()))
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .font_family(FONT_MONO)
        .tabular_nums()
        .text_size(px(22.0))
        .line_height(px(27.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(match (&facts.value, facts.stale) {
            (None, _) => Token::Ink4,
            (Some(_), true) => Token::Ink3,
            (Some(_), false) => Token::Ink,
        }));
    view()
        .gap(px(3.0))
        .min_w(px(0.0))
        .child(head)
        .child(value)
        .children(
            facts
                .detail
                .clone()
                .map(|detail| said(form, detail, 12.0, Token::Ink2)),
        )
        .children(
            facts
                .absent
                .clone()
                .map(|why| said(form, why, 12.5, Token::Ink3)),
        )
        .children(facts.share.map(|share| {
            view()
                .pt(px(5.0))
                .child(gauge(share, facts.stale))
                .into_element()
        }))
        .into_element()
}

fn cards(app: AppState, data: Memo<HostView>) -> AnyElement {
    view()
        .flex_row()
        .flex_wrap()
        .gap(px(12.0))
        .min_w(px(0.0))
        .children(
            Kind::ALL
                .into_iter()
                .map(|kind| card(app, data, kind))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// The head with the host as its line, made again when the host changes.
fn head(app: AppState, data: Memo<HostView>) -> AnyElement {
    let line = computed(move || data.with(|v| v.host_line.clone()));
    view()
        .flex_none()
        .child(dynamic(move || {
            let line = line.get();
            untracked(|| measured_head(app, MEASURE_PAGE, TITLE, Some(line), Vec::new()))
        }))
        .into_element()
}

fn desktop(app: AppState, data: Memo<HostView>) -> AnyElement {
    let content = view()
        .test_id("host-page")
        .gap(px(14.0))
        .px(px(18.0))
        .pt(px(14.0))
        .pb(px(24.0))
        .child(status(app, data))
        .child(cards(app, data))
        .into_element();
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head(app, data))
        .child(pane(app, "host", measure(app, MEASURE_PAGE, content)))
        .into_element()
}

/// The phone's body under the shell's head: the host, the status, the cards in one column.
fn phone(app: AppState, data: Memo<HostView>) -> AnyElement {
    let host_line = computed(move || data.with(|v| v.host_line.clone()));
    let content = view()
        .test_id("host-page")
        .gap(px(12.0))
        .px(px(16.0))
        .pt(px(4.0))
        .pb(px(18.0))
        .child(dynamic(move || {
            said(Form::Phone, host_line.get(), 14.0, Token::Ink2)
                .font_weight(FontWeight::SEMIBOLD)
                .into_element()
        }))
        .child(status(app, data))
        .child(cards(app, data))
        .into_element();
    pane(app, "host", content)
}

/// A terminal's two rows for a figure: its label, value (or a dash), detail and, on a wide
/// page, its trend; then why it has no value, or what it covers.
fn term_rows(app: AppState, data: Memo<HostView>, kind: Kind) -> AnyElement {
    // The trend takes what the page has over the figures, within bounds; none when narrow.
    let wide = computed(move || {
        let columns = term::page_columns(app);
        (columns >= TREND_FROM_CELLS)
            .then(|| ((columns - TREND_AFTER_CELLS) as usize).clamp(TREND_CELLS, TREND_MOST_CELLS))
    });
    let shown = computed(move || {
        let wide = wide.get();
        data.with(|v| {
            v.metric(kind).map(|m| {
                let trends: Vec<String> = match wide {
                    Some(room) => {
                        let labels: usize = m.series.iter().map(|s| s.label.len() + 2).sum();
                        let each = room.saturating_sub(labels) / m.series.len().max(1);
                        m.series
                            .iter()
                            .filter_map(|s| {
                                // One line needs no name: the value beside it says what it is.
                                let cells = chart::cells(s, each.max(4))?;
                                Some(if m.series.len() > 1 {
                                    format!("{} {cells}", s.label)
                                } else {
                                    cells
                                })
                            })
                            .collect()
                    }
                    None => Vec::new(),
                };
                (facts(v, kind), trends.join(" "))
            })
        })
    });
    dynamic(move || {
        let (facts, trend) = shown.get()?;
        let facts = facts?;
        let ink = match (&facts.value, facts.stale) {
            (None, _) => Token::Ink4,
            (Some(_), true) => Token::Ink3,
            (Some(_), false) => Token::Ink,
        };
        let first = view()
            .flex_row()
            .gap(term::cells(2.0))
            .min_w(px(0.0))
            .child(
                text(kind.label())
                    .w(term::cells(LABEL_CELLS - 2.0))
                    .flex_shrink_0()
                    .number_of_lines(1u32)
                    .text_color(color(Token::Ink2)),
            )
            .child(
                text(facts.value.clone().unwrap_or_else(|| NO_VALUE.to_owned()))
                    .flex_shrink_0()
                    .number_of_lines(1u32)
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(ink)),
            )
            .child(
                text(facts.detail.clone().unwrap_or_default())
                    .flex_1()
                    .min_w(px(0.0))
                    .number_of_lines(1u32)
                    .truncate()
                    .text_color(color(Token::Ink3)),
            )
            .children((!trend.is_empty()).then(|| {
                text(trend)
                    .flex_shrink_0()
                    .number_of_lines(1u32)
                    .text_color(color(if facts.stale {
                        Token::Ink3
                    } else {
                        Token::Ink2
                    }))
            }));
        let second = facts
            .absent
            .clone()
            .or_else(|| facts.note.clone())
            .unwrap_or_default();
        Some(
            view()
                .test_id(&format!("host-{}", kind.slug()))
                .min_w(px(0.0))
                .child(first)
                .child(
                    view().pl(term::cells(LABEL_CELLS)).min_w(px(0.0)).child(
                        text(second)
                            .number_of_lines(1u32)
                            .truncate()
                            .min_w(px(0.0))
                            .text_color(color(Token::Ink3)),
                    ),
                )
                .into_element(),
        )
    })
    .into_element()
}

fn terminal(app: AppState, data: Memo<HostView>) -> AnyElement {
    let content = view()
        .test_id("host-page")
        .min_w(px(0.0))
        .child(status(app, data))
        .child(term::blank())
        .children(
            Kind::ALL
                .into_iter()
                .map(|kind| term_rows(app, data, kind))
                .collect::<Vec<_>>(),
        );
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head(app, data))
        .child(body(app, "host", content.into_element()))
        .into_element()
}
