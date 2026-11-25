//! Activity › Usage: the tokens and cost agent hosts reported over a period of local days,
//! visibility only. The head says the period's days; then four tiles (tokens, the estimate,
//! the reported cost, the runs), one quiet line of what the totals leave out with Details for
//! every reason (`gaps`), the tokens of each day, one table grouped by the selected key
//! (profile, model, task or day) with the period's total under it, and a line on what the
//! estimate is. Desktop puts the period and grouping switches in the page head and keeps the
//! page to `MEASURE_PAGE`, centred; the terminal and the phone put them at the top of the
//! scroll, where they wrap. Loading, stale and failed reads are said once, compactly, above
//! the content. Every word and figure is `model::usage`'s.

mod controls;
mod days;
mod gaps;
mod shown;
mod table;

use nsh_core::element::cell_size;
use nsh_core::prelude::*;

use crate::model::text::span;
use crate::model::usage::{Page, Status, TITLE};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::{body, measure, measured_head, pane, section};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, MEASURE_PAGE, Token, color};

/// Below this content width the table has no room for its six columns and its groups stack
/// as records (a narrow window, or a collapsed sidebar on a small screen).
const TABLE_MIN: f32 = 680.0;
/// A terminal below this many Cells lists the groups as records too.
const TERM_TABLE_MIN: usize = 72;
/// A desktop section's sides, as the page head's.
const SIDES: f32 = 18.0;
/// One Cell where the surface reports none yet.
const COLUMN: f32 = 8.0;

/// The Usage page on every form. test ids: "page-title" (desktop and terminal head),
/// "usage" (the scroll), "seg-period-<n>", "seg-by-<n>", "field-usage-from", "field-usage-to",
/// "btn-usage-apply", "usage-head", "usage-stats", "stat-usage-<key>", "usage-coverage",
/// "btn-usage-details", "usage-details", "usage-days", "usage-groups", "usage-row-<key>",
/// "usage-estimate", "btn-retry-usage".
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
/// its Retry.
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
        .map(pad)
        .collect()
}

/// Quiet small words that wrap rather than clip: the closing line.
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

/// The content under the controls, at `room` (desktop points, or terminal Cells): the status,
/// the tiles, the coverage line, the days, the groups with their total and the estimate line.
fn content(
    app: AppState,
    page: &Page,
    records: bool,
    room: usize,
    reading: days::Reading,
) -> AnyElement {
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
        .children(status(app, page))
        .children((form == Form::Terminal).then(term::blank))
        .children(gaps::stats(app, page).map(|stats| block(stats, 12.0, 4.0)))
        .children(
            page.gaps
                .as_ref()
                .map(|gaps| block(gaps::strip(app, gaps), 8.0, 4.0)),
        )
        .children(
            page.days
                .as_ref()
                .map(|strip| block(days::strip(app, strip, room, reading), 12.0, 4.0)),
        )
        .child(groups)
        .child(block(
            quiet(app, page.estimate_note.clone(), "usage-estimate"),
            0.0,
            24.0,
        ))
        .into_element()
}

/// The head with the period's days as its line, made again when that line changes.
fn page_head(
    app: AppState,
    data: Memo<Page>,
    actions: fn(AppState) -> Vec<AnyElement>,
) -> AnyElement {
    let line = computed(move || data.with(|page| page.head.clone()));
    view()
        .flex_none()
        .test_id("usage-head")
        .child(dynamic(move || {
            let line = line.get();
            untracked(|| measured_head(app, MEASURE_PAGE, TITLE, Some(line), actions(app)))
        }))
        .into_element()
}

fn desktop(app: AppState, data: Memo<Page>) -> AnyElement {
    let reading = days::Reading::new();
    let width = signal(None::<f32>);
    let records = computed(move || width.get().is_some_and(|w| w < TABLE_MIN));
    let scroll = view()
        .on_layout(measured(width))
        .child(controls::custom_row(app, data))
        .child(dynamic(move || {
            let records = records.get();
            let page = data.get();
            untracked(|| content(app, &page, records, 0, reading))
        }))
        .into_element();
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(page_head(app, data, |app| {
            vec![controls::period_seg(app), controls::grouping_seg(app)]
        }))
        .child(pane(app, "usage", measure(app, MEASURE_PAGE, scroll)))
        .into_element()
}

/// The phone body: the switches and the custom range, then the content as sections. It is the
/// page's scroll and keeps the clearance under the floating Ask button.
fn phone(app: AppState, data: Memo<Page>) -> AnyElement {
    let reading = days::Reading::new();
    let scroll = view()
        .child(controls::stacked(app, data))
        .child(dynamic(move || {
            let page = data.get();
            untracked(|| content(app, &page, true, 0, reading))
        }))
        .into_element();
    pane(app, "usage", scroll)
}

fn terminal(app: AppState, data: Memo<Page>) -> AnyElement {
    let reading = days::Reading::new();
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
            untracked(|| content(app, &page, room < TERM_TABLE_MIN, room, reading))
        }))
        .into_element();
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(page_head(app, data, |_| Vec::new()))
        .child(body(app, "usage", scroll))
        .into_element()
}
