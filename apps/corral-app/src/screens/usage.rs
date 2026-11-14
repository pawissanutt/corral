//! Usage: cost and quota per provider, profile and session, visibility only. Desktop follows the
//! mockup's `.ugrid` of provider cells over the profile table and the cost-per-day column; the
//! phone stacks the provider blocks, the cost bars and the note; a terminal draws a gauge line
//! per provider, the table in columns and the costs as Cell bars. Every word and figure is
//! `model::usage`'s: the mockup's illustrative page in the fixture preview, corral's recorded
//! usage in a daemon session (`shown`), whose loading, stale and failed states are said in place.
//! The period switch runs its reducer.

mod cellbar;
mod chart;
mod costs;
mod profiles;
mod providers;
mod sessions;
mod shown;

use nsh_core::element::cell_size;
use nsh_core::prelude::*;

use crate::model::text::span;
use crate::model::usage::{PROFILE_TITLE, Page, Period, SESSION_TITLE, SUBTITLE, TITLE};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::seg;
use crate::ui::kit::notice::{error, notice};
use crate::ui::kit::page::{body, head, pane, section};
use crate::ui::kit::text::{h3, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use shown::{Shown, Status};

/// Below this content width the cost column moves under the table (the mockup's 380 pt column
/// beside a table that needs about 720).
const SIDE_BY_SIDE: f32 = 1100.0;
/// `.cols` second column: 380 pt.
const COST_COLUMN: f32 = 380.0;
/// One Cell where the surface reports none yet.
const COLUMN: f32 = 8.0;

/// The Usage destination on every form. Desktop and terminal fill the content area and scroll
/// below the page head; the phone returns its scrolling body (the shell draws the pushed
/// header with "Usage" and the back to More). test ids: "page-title" (desktop and terminal
/// head), "usage" (the scroll), "seg-period-<n>", "usage-<provider id>", "usage-profiles",
/// "usage-sessions", "usage-costs".
pub fn screen(app: AppState) -> AnyElement {
    if untracked(|| app.daemon()) {
        shown::keep_loaded(app);
    }
    let data = computed(move || shown::shown(app));
    match app.form {
        Form::Desktop => desktop(app, data),
        Form::Phone => phone(app, data),
        Form::Terminal => terminal(app, data),
    }
}

/// Today, 7 days, 30 days (`.seg`, `.pseg`): the selected period from `app.usage`.
fn period_seg(app: AppState) -> AnyElement {
    let options = Period::ALL.map(|period| (period, period.label()));
    seg(
        app,
        &options,
        move || app.usage.with(|state| state.period),
        move |period| app.act(app.usage, |state, _| state.select_period(period)),
        "seg-period",
    )
}

/// The page's loading, stale or failed state, above its content; nothing when current.
fn status(app: AppState, status: &Status) -> Option<AnyElement> {
    let pad = |element: AnyElement| match app.form {
        Form::Terminal => element,
        Form::Desktop | Form::Phone => view()
            .px(px(18.0))
            .pt(px(10.0))
            .child(element)
            .into_element(),
    };
    match status {
        Status::Ready => None,
        Status::Loading => Some(pad(notice(
            app,
            Hue::Neutral,
            &vec![span("Loading recorded usage from corrald…")],
            None,
        ))),
        Status::Stale => Some(pad(notice(
            app,
            Hue::Held,
            &vec![span(
                "Showing the last usage received; it may be out of date until corrald answers again.",
            )],
            None,
        ))),
        Status::Failed(message) => Some(error(
            app,
            &vec![span("Usage unavailable: ").bold(), span(message.clone())],
            None,
        )),
    }
}

/// The page body, built again when the shown page changes.
fn live_body(data: Memo<Shown>, build: impl Fn(&Shown) -> AnyElement + 'static) -> AnyElement {
    dynamic(move || {
        let shown = data.get();
        untracked(|| build(&shown))
    })
    .into_element()
}

fn quiet_line(app: AppState, words: impl Into<SharedString>) -> AnyElement {
    plain(app, words)
        .text_color(color(Token::Ink3))
        .into_element()
}

fn desktop(app: AppState, data: Memo<Shown>) -> AnyElement {
    let width = signal(None::<f32>);
    let wide = computed(move || width.get().is_none_or(|w| w >= SIDE_BY_SIDE));
    let content = view()
        .on_layout(move |frame: &Bounds<Points>| {
            let measured = Some(frame.size.width.0);
            if width.get_untracked() != measured {
                width.set(measured);
            }
        })
        .child(live_body(data, move |shown| {
            let page = &shown.page;
            let cells = page
                .providers
                .iter()
                .enumerate()
                .map(|(at, item)| providers::desktop_cell(item, at == 0))
                .collect::<Vec<_>>();
            view()
                .children(status(app, &shown.status))
                .children(page.providers_empty.map(|line| {
                    view()
                        .px(px(18.0))
                        .pt(px(16.0))
                        .child(quiet_line(app, line))
                        .into_element()
                }))
                .child(
                    view()
                        .flex_row()
                        .border_b(px(1.0))
                        .border_color(color(Token::Rule))
                        .children(cells),
                )
                .child(dynamic(move || {
                    let wide = wide.get();
                    data.with(|shown| untracked(|| lower(app, &shown.page, wide)))
                }))
                .children(page.sessions.as_ref().map(|rows| {
                    view()
                        .px(px(18.0))
                        .pt(px(6.0))
                        .pb(px(18.0))
                        .child(h3(app, SESSION_TITLE, None))
                        .child(sessions::table(app, rows, page.sessions_note.as_deref()))
                        .into_element()
                }))
                .child(
                    view()
                        .px(px(18.0))
                        .pb(px(16.0))
                        .child(note(app, page.disclaimer)),
                )
                .into_element()
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
            vec![period_seg(app)],
        ))
        .child(pane(app, "usage", content))
        .into_element()
}

/// The table and the cost column: side by side (`.cols`, `minmax(0, 1fr) 380px`) when `wide`,
/// else the costs under the table with a rule between.
fn lower(app: AppState, page: &Page, wide: bool) -> AnyElement {
    let table = view()
        .px(px(18.0))
        .pt(px(6.0))
        .pb(px(18.0))
        .child(h3(app, PROFILE_TITLE, None))
        .child(profiles::desktop(app, page));
    let spend = view()
        .px(px(18.0))
        .pt(px(6.0))
        .pb(px(18.0))
        .child(h3(app, page.costs.title.clone(), None))
        .child(costs::bars(app, &page.costs));
    if wide {
        view()
            .flex_row()
            .child(table.flex_1().min_w(px(0.0)))
            .child(
                spend
                    .w(px(COST_COLUMN))
                    .flex_shrink_0()
                    .border_l(px(1.0))
                    .border_l_color(color(Token::Rule)),
            )
            .into_element()
    } else {
        view()
            .child(table)
            .child(spend.border_t(px(1.0)).border_t_color(color(Token::Rule)))
            .into_element()
    }
}

/// The phone body: the period switch (`.pseg`, 4 16 8 around it), a block per provider, the
/// cost bars and the sessions as sections, and the note. It is the page's scroll and keeps the
/// clearance under the floating Ask button.
fn phone(app: AppState, data: Memo<Shown>) -> AnyElement {
    let content = view()
        .child(
            view()
                .px(px(16.0))
                .pt(px(4.0))
                .pb(px(8.0))
                .child(period_seg(app)),
        )
        .child(live_body(data, move |shown| {
            let page = &shown.page;
            let blocks = page
                .providers
                .iter()
                .map(providers::phone_block)
                .collect::<Vec<_>>();
            view()
                .children(status(app, &shown.status))
                .children(page.providers_empty.map(|line| {
                    view()
                        .px(px(18.0))
                        .py(px(12.0))
                        .child(quiet_line(app, line))
                        .into_element()
                }))
                .children(blocks)
                .child(section(
                    app,
                    Some(page.costs.title.clone()),
                    None,
                    Hue::Neutral,
                    costs::bars(app, &page.costs),
                ))
                .children(page.sessions.as_ref().map(|rows| {
                    section(
                        app,
                        Some(SESSION_TITLE.to_owned()),
                        None,
                        Hue::Neutral,
                        sessions::table(app, rows, page.sessions_note.as_deref()),
                    )
                }))
                .child(
                    view()
                        .px(px(18.0))
                        .pt(px(10.0))
                        .pb(px(24.0))
                        .child(note(app, page.phone_disclaimer)),
                )
                .into_element()
        }))
        .into_element();
    pane(app, "usage", content)
}

fn terminal(app: AppState, data: Memo<Shown>) -> AnyElement {
    let width = signal(None::<f32>);
    let content = view()
        .on_layout(move |frame: &Bounds<Points>| {
            let measured = Some(frame.size.width.0);
            if width.get_untracked() != measured {
                width.set(measured);
            }
        })
        .child(dynamic(move || {
            let column = cell_size().map_or(COLUMN, |cell| cell.width.0);
            let room = width.get().map_or_else(term::columns, |w| {
                (w / column + 0.01).floor().max(0.0) as usize
            });
            let shown = data.get();
            untracked(|| terminal_content(app, &shown, room))
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
            vec![period_seg(app)],
        ))
        .child(body(app, "usage", content))
        .into_element()
}

/// The terminal page at `room` Cells: the gauge lines, the table, the cost bars, the sessions,
/// the note, a blank row between the parts.
fn terminal_content(app: AppState, shown: &Shown, room: usize) -> AnyElement {
    let page = &shown.page;
    let lines = page
        .providers
        .iter()
        .map(|item| providers::terminal_line(item, room))
        .collect::<Vec<_>>();
    view()
        .children(status(app, &shown.status))
        .child(term::blank())
        .children(page.providers_empty.map(|line| quiet_line(app, line)))
        .children(lines)
        .child(term::blank())
        .child(h3(app, PROFILE_TITLE, None))
        .child(profiles::terminal(app, page, room))
        .child(term::blank())
        .child(h3(app, page.costs.title.clone(), None))
        .child(costs::terminal(app, &page.costs, room))
        .children(page.sessions.as_ref().map(|rows| {
            view()
                .child(term::blank())
                .child(h3(app, SESSION_TITLE, None))
                .child(sessions::table(app, rows, page.sessions_note.as_deref()))
                .into_element()
        }))
        .child(term::blank())
        .child(note(app, page.disclaimer))
        .child(term::blank())
        .into_element()
}

/// The long note as a paragraph that wraps: the look of `kit::chip::illus` (italic, ink-4 at
/// 10.5 pt; italic ink-3 in a terminal), which keeps to one line.
fn note(app: AppState, content: &'static str) -> AnyElement {
    let words = text(content).italic();
    match app.form {
        Form::Terminal => words.text_color(color(Token::Ink3)).into_element(),
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(10.5))
            .line_height(px(15.0))
            .letter_spacing(px(0.1))
            .text_color(color(Token::Ink4))
            .into_element(),
    }
}
