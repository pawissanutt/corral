//! Usage: the tokens and cost agent hosts reported, visibility only. Every form leads with what
//! the totals cover ("Covers N of M runs", and why the rest are missing) in the held tone when
//! runs are missing, then the totals, then one table grouped by the selected key (profile,
//! model, task or day), then the note on where the figures come from. Desktop and terminal
//! scroll under the page head with the grouping switch in it; the phone puts the switch at the
//! top of its scroll. Loading, stale and failed reads are said once, compactly, above the
//! content. Every word and figure is `model::usage`'s.

mod shown;
mod table;

use nsh_core::element::cell_size;
use nsh_core::prelude::*;

use crate::model::text::span;
use crate::model::usage::{Figure, GROUPINGS, Page, SUBTITLE, Status, TITLE};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button, seg};
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::{body, head, pane, section};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// Below this content width the table has no room for its seven columns and its groups stack
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
/// "usage" (the scroll), "seg-by-<n>", "usage-coverage", "usage-figures", "usage-groups",
/// "btn-retry-usage".
pub fn screen(app: AppState) -> AnyElement {
    shown::keep_loaded(app);
    let data = computed(move || shown::shown(app));
    match app.form {
        Form::Desktop => desktop(app, data),
        Form::Phone => phone(app, data),
        Form::Terminal => terminal(app, data),
    }
}

/// Profile, Model, Task, Day: the selected grouping from `app.usage`.
fn grouping_seg(app: AppState) -> AnyElement {
    seg(
        app,
        &GROUPINGS,
        move || app.usage.with(|state| state.by),
        move |by| app.act(app.usage, |state, _| state.select_by(by)),
        "seg-by",
    )
}

/// Above the content: the loading or stale state, then corrald's read failure said once with
/// its Retry, then what the totals cover, held when runs are missing.
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
            &vec![span("Loading recorded usage from corrald…")],
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
    let coverage = page.coverage.as_ref().map(|coverage| {
        view()
            .test_id("usage-coverage")
            .child(notice(
                app,
                if coverage.partial {
                    Hue::Held
                } else {
                    Hue::Neutral
                },
                &vec![span(coverage.line.as_str())],
                None,
            ))
            .into_element()
    });
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

/// The totals: each name small and quiet over its figure in tabular mono, its qualifier under
/// it, wrapping across the width; a terminal sets one total a line.
fn figures(app: AppState, figures: &[Figure]) -> AnyElement {
    let form = app.form;
    let items = figures.iter().map(|figure| match form {
        Form::Terminal => text("")
            .child(
                text(format!("{:<TERM_LABEL$}", figure.label)).text_color(color(Token::Ink3)),
            )
            .child(text(figure.value.clone()).text_color(color(Token::Ink)))
            .children(figure.detail.clone().map(|detail| {
                text(format!("  {detail}")).text_color(color(Token::Ink3))
            }))
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

/// The page's note on where the figures come from, quiet and small; it wraps rather than clips.
fn note(app: AppState, content: &'static str) -> AnyElement {
    let words = text(content);
    match app.form {
        Form::Terminal => words.text_color(color(Token::Ink3)).into_element(),
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(11.5))
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

fn desktop(app: AppState, data: Memo<Page>) -> AnyElement {
    let width = signal(None::<f32>);
    let records = computed(move || width.get().is_some_and(|w| w < TABLE_MIN));
    let content = view()
        .on_layout(measured(width))
        .child(dynamic(move || {
            let records = records.get();
            let page = data.get();
            untracked(|| {
                view()
                    .children(status(app, &page))
                    .children((!page.figures.is_empty()).then(|| {
                        view()
                            .px(px(SIDES))
                            .pt(px(16.0))
                            .pb(px(8.0))
                            .child(figures(app, &page.figures))
                            .into_element()
                    }))
                    .child(
                        view()
                            .px(px(SIDES))
                            .pt(px(8.0))
                            .pb(px(10.0))
                            .child(h3(app, page.group_title.clone(), None))
                            .child(table::table(app, &page, records)),
                    )
                    .child(
                        view()
                            .px(px(SIDES))
                            .pt(px(8.0))
                            .pb(px(18.0))
                            .child(note(app, page.disclaimer)),
                    )
                    .into_element()
            })
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
            vec![grouping_seg(app)],
        ))
        .child(pane(app, "usage", content))
        .into_element()
}

/// The phone body: the grouping switch, then the notices, the totals and the groups as sections
/// and the note. It is the page's scroll and keeps the clearance under the floating Ask button.
fn phone(app: AppState, data: Memo<Page>) -> AnyElement {
    let content = view()
        .child(
            view()
                .px(px(16.0))
                .pt(px(4.0))
                .pb(px(8.0))
                .child(grouping_seg(app)),
        )
        .child(dynamic(move || {
            let page = data.get();
            untracked(|| {
                view()
                    .children(status(app, &page))
                    .children((!page.figures.is_empty()).then(|| {
                        section(
                            app,
                            None,
                            None,
                            Hue::Neutral,
                            figures(app, &page.figures),
                        )
                    }))
                    .child(section(
                        app,
                        Some(page.group_title.clone()),
                        None,
                        Hue::Neutral,
                        table::table(app, &page, false),
                    ))
                    .child(
                        view()
                            .px(px(18.0))
                            .pt(px(10.0))
                            .pb(px(24.0))
                            .child(note(app, page.disclaimer)),
                    )
                    .into_element()
            })
        }))
        .into_element();
    pane(app, "usage", content)
}

fn terminal(app: AppState, data: Memo<Page>) -> AnyElement {
    let width = signal(None::<f32>);
    let content = view()
        .on_layout(measured(width))
        .child(dynamic(move || {
            let column = cell_size().map_or(COLUMN, |cell| cell.width.0);
            let room = width.get().map_or_else(term::columns, |w| {
                (w / column + 0.01).floor().max(0.0) as usize
            });
            let page = data.get();
            untracked(|| {
                view()
                    .children(status(app, &page))
                    .child(term::blank())
                    .children((!page.figures.is_empty()).then(|| {
                        view()
                            .child(figures(app, &page.figures))
                            .child(term::blank())
                            .into_element()
                    }))
                    .child(h3(app, page.group_title.clone(), None))
                    .child(table::table(app, &page, room < TERM_TABLE_MIN))
                    .child(term::blank())
                    .child(note(app, page.disclaimer))
                    .child(term::blank())
                    .into_element()
            })
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
            vec![grouping_seg(app)],
        ))
        .child(body(app, "usage", content))
        .into_element()
}
