//! The Usage view under Chat (`Chat › Usage`, `session::UsageView`): what the session and each
//! of its agents spent as the host reports it. The session's facts (model, effort, context), one
//! table row for the pane and one per child or helper (a row without a price says `unpriced`,
//! never `$0.00`), then the total in the status bar's own words, with the sentence that says
//! what a partial total leaves out and why. Desktop and terminal draw the figures in columns;
//! the phone sets each agent as a record. test ids `session-usage`, `usage-row-<id>`,
//! `usage-total`, `usage-total-cost`, `btn-usage-all`.

use nsh_core::prelude::*;

use crate::model::nav::Destination;
use crate::model::session::{Body, SessionPage, UsageLine, UsageTotalLine, UsageView};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::{kv, utable};
use crate::ui::kit::text::{dim, h3, h3_flush, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::tabs::{part, surface};

const HEADERS: [&str; 7] = [
    "Agent", "Turns", "Tokens", "Input", "Output", "Context", "Cost",
];
/// Desktop figure columns: room for "12,345", "1.2M", "$1,061.00" and "unpriced".
const DESKTOP: [Width; 7] = [
    Width::Fill,
    Width::Fixed(64.0),
    Width::Fixed(76.0),
    Width::Fixed(76.0),
    Width::Fixed(76.0),
    Width::Fixed(84.0),
    Width::Fixed(96.0),
];
/// Terminal figure columns at 8 points a Cell: 7, 9, 9, 9, 9 and 11 Cells.
const TERMINAL: [Width; 7] = [
    Width::Fill,
    Width::Fixed(56.0),
    Width::Fixed(72.0),
    Width::Fixed(72.0),
    Width::Fixed(72.0),
    Width::Fixed(72.0),
    Width::Fixed(88.0),
];
const ALIGN_END: [bool; 7] = [false, true, true, true, true, true, true];

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Usage(view) => Some(view.clone()),
            _ => None,
        })
    });
    let content =
        dynamic(move || shown.with(|view| view.as_ref().map(|view| untracked(|| body(app, view)))));
    surface(app, "session-usage", content)
}

/// The part frame, then what the host read: facts, the table, the total, what the figures are.
fn body(app: AppState, view_: &UsageView) -> AnyElement {
    let form = app.form;
    let mut column = view()
        .gap(px(if form == Form::Terminal { 16.0 } else { 10.0 }))
        .child(part(app, &view_.part, "usage"));
    if !view_.facts.is_empty() {
        column = column.child(h3_flush(app, "Session", None)).child(kv(
            app,
            view_
                .facts
                .iter()
                .map(|(label, value)| (label.clone(), plain(app, value.clone()).into_element()))
                .collect(),
        ));
    }
    if !view_.rows.is_empty() {
        column = column
            .child(h3(app, "Spend by agent", None))
            .child(table(app, &view_.rows));
    }
    if let Some(total) = &view_.total {
        column = column.child(total_row(app, total));
    }
    if let Some(why) = &view_.empty {
        column = column.child(dim(app, why.clone()));
    }
    if !view_.rows.is_empty() || view_.total.is_some() {
        column = column.child(footer(app, view_));
    }
    column.into_element()
}

/// A figure: mono and tabular on desktop and phone; the quiet ink where the host reported none.
fn figure(form: Form, content: &str, ink: AdaptiveColor) -> AnyElement {
    let ink = if content == "–" {
        color(Token::Ink3)
    } else {
        ink
    };
    let words = text(content.to_owned())
        .number_of_lines(1u32)
        .text_color(ink);
    if form == Form::Terminal {
        words.into_element()
    } else {
        words
            .font_family(FONT_MONO)
            .text_size(px(12.0))
            .tabular_nums()
            .into_element()
    }
}

/// A cost: the figure, or `unpriced` in words in the held tone.
fn cost(form: Form, line: &UsageLine) -> AnyElement {
    if !line.unpriced {
        return figure(form, &line.cost, color(Token::Ink));
    }
    let words = text(line.cost.clone())
        .number_of_lines(1u32)
        .text_color(hue_ink(Hue::Held));
    if form == Form::Terminal {
        words.into_element()
    } else {
        words
            .font_family(FONT_TEXT)
            .text_size(px(12.0))
            .into_element()
    }
}

/// The agent's name, what it is, and why a figure is missing.
fn member(app: AppState, line: &UsageLine) -> AnyElement {
    let name = plain(app, line.name.clone())
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .font_weight(if app.form == Form::Terminal {
            FontWeight::BOLD
        } else {
            FontWeight::MEDIUM
        });
    view()
        .test_id(&line.id)
        .flex_1()
        .min_w(px(0.0))
        .gap(px(1.0))
        .child(name)
        .children(line.kind.clone().map(|kind| dim(app, kind)))
        .children(
            line.note
                .clone()
                .map(|note| dim(app, note).text_color(hue_ink(Hue::Held))),
        )
        .into_element()
}

fn table(app: AppState, rows: &[UsageLine]) -> AnyElement {
    let form = app.form;
    let ink = color(Token::Ink);
    utable(
        app,
        if form == Form::Terminal {
            &TERMINAL
        } else {
            &DESKTOP
        },
        &HEADERS,
        &ALIGN_END,
        rows.iter()
            .map(|line| {
                vec![
                    member(app, line),
                    figure(form, &line.turns, ink),
                    figure(form, &line.tokens, ink),
                    figure(form, &line.input, ink),
                    figure(form, &line.output, ink),
                    figure(form, &line.context, ink),
                    cost(form, line),
                ]
            })
            .collect(),
    )
}

/// The total under the table: its tokens and its cost exactly as the status bar says it, then,
/// when it is partial, what it adds up and which agents are missing from it.
fn total_row(app: AppState, total: &UsageTotalLine) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let ink = color(Token::Ink);
    let label = plain(app, "Total").font_weight(if terminal {
        FontWeight::BOLD
    } else {
        FontWeight::SEMIBOLD
    });
    let figures = view()
        .flex_row()
        .items_baseline()
        .gap(px(if terminal { 16.0 } else { 18.0 }))
        .child(figure(form, &format!("{} tokens", total.tokens), ink))
        .child(
            view()
                .test_id("usage-total-cost")
                .child(figure(form, &total.cost, ink)),
        );
    let line = view()
        .flex_row()
        .items_baseline()
        .justify_between()
        .gap(px(12.0))
        .child(label)
        .child(figures);
    let why = total.why.clone().map(|why| {
        plain(app, why)
            .min_w(px(0.0))
            .text_color(hue_ink(Hue::Held))
    });
    let block = view().test_id("usage-total").gap(px(4.0)).child(line);
    match form {
        Form::Terminal => view()
            .child(term::hrule())
            .child(block.px(term::cells(1.0)).children(why))
            .into_element(),
        Form::Desktop => block
            .border_t(px(1.0))
            .border_color(color(Token::Rule2))
            .px(px(12.0))
            .py(px(9.0))
            .children(why)
            .into_element(),
        Form::Phone => block
            .border_t(px(1.0))
            .border_color(color(Token::Rule2))
            .px(px(18.0))
            .py(px(10.0))
            .children(why)
            .into_element(),
    }
}

/// What the figures are and are not, and the way to the period totals.
fn footer(app: AppState, view_: &UsageView) -> AnyElement {
    let form = app.form;
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .children(
            view_
                .notes
                .iter()
                .map(|note| dim(app, *note).into_element())
                .collect::<Vec<_>>(),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(dim(app, "Spend over days and weeks is on the Usage page."))
                .child(button(
                    app,
                    "All usage",
                    ButtonKind::Quiet,
                    "btn-usage-all",
                    move || app.go(Destination::Usage),
                )),
        )
        .into_element()
}
