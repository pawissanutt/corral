//! Usage: cost and quota per provider and profile, visibility only. Desktop follows the mockup's
//! `.ugrid` of provider cells over the profile table and the 7-day cost column; the phone
//! stacks the provider blocks, the cost bars and the illustrative note; a terminal draws a gauge
//! line per provider, the table in columns and the costs as Cell bars. Every word and figure is
//! `model::usage`'s; the period switch runs its reducer.

mod cellbar;
mod chart;
mod costs;
mod profiles;
mod providers;

use nsh_core::element::cell_size;
use nsh_core::prelude::*;

use crate::model::usage::{
    COST_TITLE, DESKTOP_DISCLAIMER, PHONE_DISCLAIMER, PROFILE_TITLE, Period, SUBTITLE, TITLE,
    providers as provider_views,
};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::seg;
use crate::ui::kit::chip::illus;
use crate::ui::kit::page::{body, head, pane, section};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

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
/// "usage-costs".
pub fn screen(app: AppState) -> AnyElement {
    match app.form {
        Form::Desktop => desktop(app),
        Form::Phone => phone(app),
        Form::Terminal => terminal(app),
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

fn desktop(app: AppState) -> AnyElement {
    let width = signal(None::<f32>);
    let wide = computed(move || width.get().is_none_or(|w| w >= SIDE_BY_SIDE));
    let cells = provider_views()
        .iter()
        .enumerate()
        .map(|(at, item)| providers::desktop_cell(item, at == 0))
        .collect::<Vec<_>>();
    let content = view()
        .on_layout(move |frame: &Bounds<Points>| {
            let measured = Some(frame.size.width.0);
            if width.get_untracked() != measured {
                width.set(measured);
            }
        })
        .child(
            view()
                .flex_row()
                .border_b(px(1.0))
                .border_color(color(Token::Rule))
                .children(cells),
        )
        .child(dynamic(move || lower(app, wide.get())))
        .child(
            view()
                .px(px(18.0))
                .pb(px(16.0))
                .child(note(app, DESKTOP_DISCLAIMER)),
        )
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
fn lower(app: AppState, wide: bool) -> AnyElement {
    let table = view()
        .px(px(18.0))
        .pt(px(6.0))
        .pb(px(18.0))
        .child(h3(app, PROFILE_TITLE, None))
        .child(profiles::desktop(app));
    let spend = view()
        .px(px(18.0))
        .pt(px(6.0))
        .pb(px(18.0))
        .child(h3(app, COST_TITLE, None))
        .child(costs::bars(app));
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
/// cost bars as a section, and the illustrative note. It is the page's scroll and keeps the
/// clearance under the floating Ask button.
fn phone(app: AppState) -> AnyElement {
    let blocks = provider_views()
        .iter()
        .map(providers::phone_block)
        .collect::<Vec<_>>();
    let content = view()
        .child(
            view()
                .px(px(16.0))
                .pt(px(4.0))
                .pb(px(8.0))
                .child(period_seg(app)),
        )
        .children(blocks)
        .child(section(
            app,
            Some(COST_TITLE.to_owned()),
            None,
            Hue::Neutral,
            costs::bars(app),
        ))
        .child(
            view()
                .px(px(18.0))
                .pt(px(10.0))
                .pb(px(24.0))
                .child(illus(app, PHONE_DISCLAIMER)),
        )
        .into_element();
    pane(app, "usage", content)
}

fn terminal(app: AppState) -> AnyElement {
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
            terminal_content(app, room)
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

/// The terminal page at `room` Cells: the gauge lines, the table, the cost bars, the note, a
/// blank row between the parts.
fn terminal_content(app: AppState, room: usize) -> AnyElement {
    let lines = provider_views()
        .iter()
        .map(|item| providers::terminal_line(item, room))
        .collect::<Vec<_>>();
    view()
        .child(term::blank())
        .children(lines)
        .child(term::blank())
        .child(h3(app, PROFILE_TITLE, None))
        .child(profiles::terminal(app, room))
        .child(term::blank())
        .child(h3(app, COST_TITLE, None))
        .child(costs::terminal(room))
        .child(term::blank())
        .child(note(app, DESKTOP_DISCLAIMER))
        .child(term::blank())
        .into_element()
}

/// The long illustrative note as a paragraph that wraps: the look of `kit::chip::illus`
/// (italic, ink-4 at 10.5 pt; italic ink-3 in a terminal), which keeps to one line.
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
