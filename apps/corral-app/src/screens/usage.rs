//! Usage: the tokens and cost agent hosts reported over a period of local days, visibility
//! only. Every form leads with the period and what the totals cover ("Covers N of M runs", and
//! each reason the rest is missing) in the held tone when anything is missing, then the totals,
//! the tokens of each day, one table grouped by the selected key (profile, model, task or day)
//! with each group's own coverage, and the note on where the figures come from. Desktop puts the
//! period and grouping switches in the page head; the terminal and the phone put them at the top
//! of the scroll, where they wrap. Loading, stale and failed reads are said once, compactly,
//! above the content. Every word and figure is `model::usage`'s.

mod controls;
mod days;
mod shown;
mod table;

use nsh_core::element::cell_size;
use nsh_core::prelude::*;

use crate::model::text::span;
use crate::model::usage::{Coverage, Figure, Page, SUBTITLE, Status, TITLE};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::{body, head, pane, section};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// Below this content width the table has no room for its six columns and its groups stack
/// as records (a narrow window, or a collapsed sidebar on a small screen).
const TABLE_MIN: f32 = 720.0;
/// A terminal below this many Cells lists the groups as records too.
const TERM_TABLE_MIN: usize = 72;
/// A desktop section's sides, as the page head's.
const SIDES: f32 = 18.0;
/// One Cell where the surface reports none yet.
const COLUMN: f32 = 8.0;
/// A total's qualifier wraps at this measure rather than stretching the row.
const DETAIL_MEASURE: f32 = 240.0;
/// Terminal: a total's name column, in Cells.
const TERM_LABEL: usize = 12;

/// The Usage destination on every form. test ids: "page-title" (desktop and terminal head),
/// "usage" (the scroll), "seg-period-<n>", "seg-by-<n>", "field-usage-from", "field-usage-to",
/// "btn-usage-apply", "usage-period", "usage-coverage", "usage-figures", "usage-days",
/// "usage-groups", "btn-retry-usage".
pub fn screen(app: AppState) -> AnyElement {
    shown::keep_loaded(app);
    let data = computed(move || shown::shown(app));
    match app.form {
        Form::Desktop => desktop(app, data),
        Form::Phone => phone(app, data),
        Form::Terminal => terminal(app, data),
    }
}

/// Above the content: the loading or stale state, then corrald's read failure said once with
/// its Retry, then what the totals cover, held when anything is missing.
fn status(app: AppState, page: &Page) -> Vec<AnyElement> {
    let pad = |element: AnyElement| match app.form {
        Form::Terminal => element,
        Form::Desktop | Form::Phone => view()
            .px(px(SIDES))
            .pt(px(12.0))
            .child(element)
            .into_element(),
    };
    let state = match page.status {
        Status::Ready => None,
        Status::Loading => Some(notice(
            app,
            Hue::Neutral,
            &vec![span("Loading the period's usage from corrald…")],
            None,
        )),
        Status::Stale => Some(notice(
            app,
            Hue::Held,
            &vec![span(
                "Showing the last usage received; it may be out of date until corrald answers again.",
            )],
            None,
        )),
    };
    let coverage = page
        .coverage
        .as_ref()
        .map(|coverage| coverage_notice(app, coverage));
    state
        .into_iter()
        .chain(page.notices.iter().map(|line| {
            let retry = button(
                app,
                "Retry",
                ButtonKind::Ghost,
                "btn-retry-usage",
                move || shown::retry(app),
            );
            notice(app, Hue::Fail, &vec![span(line.as_str())], Some(retry))
        }))
        .chain(coverage)
        .map(pad)
        .collect()
}

/// The coverage line in bold, then each reason the rest is missing as its own sentence.
fn coverage_notice(app: AppState, coverage: &Coverage) -> AnyElement {
    let mut words = vec![span(coverage.line.as_str()).bold()];
    for reason in &coverage.reasons {
        words.push(span(" "));
        words.push(span(reason.as_str()));
    }
    view()
        .test_id("usage-coverage")
        .child(notice(
            app,
            if coverage.partial {
                Hue::Held
            } else {
                Hue::Neutral
            },
            &words,
            None,
        ))
        .into_element()
}

/// The totals: each name small and quiet over its figure in tabular mono, its qualifier under
/// it, wrapping across the width; a terminal sets one total a line.
fn figures(app: AppState, figures: &[Figure]) -> AnyElement {
    let form = app.form;
    let items = figures.iter().map(|figure| match form {
        Form::Terminal => text("")
            .child(text(format!("{:<TERM_LABEL$}", figure.label)).text_color(color(Token::Ink3)))
            .child(text(figure.value.clone()).text_color(color(Token::Ink)))
            .children(
                figure
                    .detail
                    .clone()
                    .map(|detail| text(format!("  {detail}")).text_color(color(Token::Ink3))),
            )
            .into_element(),
        Form::Desktop | Form::Phone => {
            let phone = form == Form::Phone;
            view()
                .flex_col()
                .gap(px(2.0))
                .child(
                    text(figure.label)
                        .font_family(FONT_TEXT)
                        .text_size(px(if phone { 12.5 } else { 11.5 }))
                        .text_color(color(Token::Ink3)),
                )
                .child(
                    text(figure.value.clone())
                        .number_of_lines(1u32)
                        .font_family(FONT_MONO)
                        .text_size(px(15.0))
                        .tabular_nums()
                        .text_color(color(Token::Ink)),
                )
                .children(figure.detail.clone().map(|detail| {
                    text(detail)
                        .max_w(px(DETAIL_MEASURE))
                        .font_family(FONT_TEXT)
                        .text_size(px(if phone { 12.5 } else { 11.5 }))
                        .line_height(px(16.0))
                        .text_color(color(Token::Ink3))
                }))
                .into_element()
        }
    });
    let row = view().test_id("usage-figures");
    match form {
        Form::Terminal => row.flex_col(),
        Form::Desktop | Form::Phone => row
            .flex_row()
            .flex_wrap()
            .items_start()
            .gap_x(px(32.0))
            .gap_y(px(14.0)),
    }
    .children(items.collect::<Vec<_>>())
    .into_element()
}

/// Quiet small words that wrap rather than clip: the period line and the closing note.
fn quiet(app: AppState, content: impl Into<SharedString>, test_id: &str) -> AnyElement {
    let words = text(content.into()).test_id(test_id);
    match app.form {
        Form::Terminal => words.text_color(color(Token::Ink3)).into_element(),
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(if app.form == Form::Phone { 12.5 } else { 11.5 }))
            .line_height(px(16.0))
            .text_color(color(Token::Ink3))
            .into_element(),
    }
}

/// Re-measures the content width into `width` when it changes.
fn measured(width: Signal<Option<f32>>) -> impl Fn(&Bounds<Points>) + 'static {
    move |frame: &Bounds<Points>| {
        let now = Some(frame.size.width.0);
        if width.get_untracked() != now {
            width.set(now);
        }
    }
}

/// The content under the controls, at `room` (desktop points, or terminal Cells): the period,
/// the status, the totals, the days, the groups and the note.
fn content(app: AppState, page: &Page, records: bool, room: usize) -> AnyElement {
    let form = app.form;
    let block = |child: AnyElement, top: f32, bottom: f32| match form {
        Form::Terminal => view().child(child).child(term::blank()).into_element(),
        Form::Desktop | Form::Phone => view()
            .px(px(SIDES))
            .pt(px(top))
            .pb(px(bottom))
            .child(child)
            .into_element(),
    };
    let period = block(quiet(app, page.period.clone(), "usage-period"), 4.0, 0.0);
    // The phone's table sets each group as a record of its own.
    let groups = if form == Form::Phone {
        section(
            app,
            Some(page.group_title.clone()),
            None,
            Hue::Neutral,
            table::table(app, page, false),
        )
    } else {
        let groups = view()
            .child(h3(app, page.group_title.clone(), None))
            .child(table::table(app, page, records))
            .into_element();
        block(groups, 8.0, 10.0)
    };
    view()
        .child(period)
        .children(status(app, page))
        .children((form == Form::Terminal).then(term::blank))
        .children((!page.figures.is_empty()).then(|| block(figures(app, &page.figures), 16.0, 4.0)))
        .children(
            page.days
                .as_ref()
                .map(|strip| block(days::strip(app, strip, room), 4.0, 4.0)),
        )
        .child(groups)
        .child(block(quiet(app, page.disclaimer, "usage-note"), 8.0, 24.0))
        .into_element()
}

fn desktop(app: AppState, data: Memo<Page>) -> AnyElement {
    let width = signal(None::<f32>);
    let records = computed(move || width.get().is_some_and(|w| w < TABLE_MIN));
    let scroll = view()
        .on_layout(measured(width))
        .child(controls::custom_row(app, data))
        .child(dynamic(move || {
            let records = records.get();
            let page = data.get();
            untracked(|| content(app, &page, records, 0))
        }))
        .into_element();
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head(
            app,
            TITLE,
            Some(SUBTITLE.to_owned()),
            vec![controls::period_seg(app), controls::grouping_seg(app)],
        ))
        .child(pane(app, "usage", scroll))
        .into_element()
}

/// The phone body: the switches and the custom range, then the content as sections. It is the
/// page's scroll and keeps the clearance under the floating Ask button.
fn phone(app: AppState, data: Memo<Page>) -> AnyElement {
    let scroll = view()
        .child(controls::stacked(app, data))
        .child(dynamic(move || {
            let page = data.get();
            untracked(|| content(app, &page, true, 0))
        }))
        .into_element();
    pane(app, "usage", scroll)
}

fn terminal(app: AppState, data: Memo<Page>) -> AnyElement {
    let width = signal(None::<f32>);
    let scroll = view()
        .on_layout(measured(width))
        .child(controls::stacked(app, data))
        .child(term::blank())
        .child(dynamic(move || {
            let column = cell_size().map_or(COLUMN, |cell| cell.width.0);
            let room = width.get().map_or_else(term::columns, |w| {
                (w / column + 0.01).floor().max(0.0) as usize
            });
            let page = data.get();
            untracked(|| content(app, &page, room < TERM_TABLE_MIN, room))
        }))
        .into_element();
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head(app, TITLE, Some(SUBTITLE.to_owned()), Vec::new()))
        .child(body(app, "usage", scroll))
        .into_element()
}
