//! Above the composer, each only while it has content (`session::Dock`): the docked choice (a
//! build's exit choice, a child's question, a proposed board plan), the cards waiting on the
//! owner, and the queue of input waiting for the agent, in the host's delivery order (steers
//! first). The cards are listed as the host briefs them, with why they cannot be answered from
//! here while the host is unreachable or stopped. Also the bar standing in the composer's place
//! while the session takes no message, with Resume or Relaunch where the host can start again.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{Dock, DockCard, HostBar, QueueLine, QueueSummary, SessionPage};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::super::run;

pub(super) fn dock(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let dock = computed(move || page.with(|p| p.dock.clone()));
    dynamic(move || dock.with(|dock| (!dock.is_empty()).then(|| untracked(|| face(app, dock)))))
        .into_element()
}

fn face(app: AppState, dock: &Dock) -> AnyElement {
    let form = app.form;
    let cards: Vec<AnyElement> = dock
        .choice
        .iter()
        .chain(&dock.cards)
        .map(|card| self::card(app, card))
        .collect();
    let inert = dock
        .inert
        .clone()
        .map(|why| words(form, why, color(Token::Ink3)));
    let column = view()
        .test_id("session-dock")
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .children(cards)
        .children(inert)
        .children(dock.queue.as_ref().map(|queue| self::queue(app, queue)));
    match form {
        Form::Terminal => view()
            .flex_none()
            .child(term::hrule())
            .child(column.px(term::cells(1.0)))
            .into_element(),
        Form::Desktop => column
            .flex_none()
            .px(px(18.0))
            .pt(px(8.0))
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .into_element(),
        Form::Phone => column.flex_none().px(px(16.0)).pt(px(8.0)).into_element(),
    }
}

/// Words at the dock's size in `ink`, one line.
fn words(form: Form, content: String, ink: AdaptiveColor) -> AnyElement {
    let words = text(content)
        .number_of_lines(1u32)
        .truncate()
        .text_color(ink);
    match form {
        Form::Terminal => words.into_element(),
        Form::Desktop => words
            .font_family(FONT_TEXT)
            .text_size(px(12.5))
            .into_element(),
        Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(14.0))
            .into_element(),
    }
}

/// A waiting card's one line: its title in the owner's colour after `▲` (`▲ a2 asks: …`).
fn card(app: AppState, card: &DockCard) -> AnyElement {
    let form = app.form;
    let title = if card.title.starts_with('▲') {
        card.title.clone()
    } else {
        format!("▲ {}", card.title)
    };
    let line = view()
        .test_id(&card.id)
        .flex_row()
        .items_center()
        .child(words(form, title, hue_ink(card.hue)));
    match form {
        Form::Terminal => line.into_element(),
        Form::Desktop | Form::Phone => line
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(8.0))
            .bg(color(Token::YouBg))
            .border_1()
            .border_color(color(Token::Rule2))
            .into_element(),
    }
}

/// `QUEUE · 2 waiting · …`, then one row per item: its place, its receiver, `steer ·` before a
/// steer, the text cut to a line, and where input that is not the owner's came from.
fn queue(app: AppState, queue: &QueueSummary) -> AnyElement {
    let form = app.form;
    let head = text(queue.header.clone())
        .number_of_lines(1u32)
        .truncate()
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink3));
    let head = match form {
        Form::Terminal => head.into_element(),
        _ => head
            .font_family(FONT_TEXT)
            .text_size(px(11.0))
            .letter_spacing(px(0.4))
            .into_element(),
    };
    view()
        .test_id("session-queue")
        .gap(px(if form == Form::Terminal { 0.0 } else { 2.0 }))
        .child(head)
        .children(
            queue
                .rows
                .iter()
                .enumerate()
                .map(|(at, line)| row(app, at, line))
                .collect::<Vec<_>>(),
        )
        .children(
            queue
                .more
                .clone()
                .map(|more| words(form, more, color(Token::Ink3))),
        )
        .into_element()
}

fn row(app: AppState, at: usize, line: &QueueLine) -> AnyElement {
    let form = app.form;
    let mut said = format!("{} {}", at + 1, line.to);
    if line.steer {
        said.push_str("  steer ·");
    }
    view()
        .test_id(&line.id)
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            view()
                .flex_shrink_0()
                .child(words(form, said, color(Token::Ink3))),
        )
        .child(view().flex_1().min_w(px(0.0)).child(words(
            form,
            line.text.clone(),
            color(Token::Ink),
        )))
        .children(
            line.source
                .map(|source| words(form, format!("{source} · read-only"), color(Token::Ink3))),
        )
        .into_element()
}

/// The bar in the composer's place: why the session takes no message, and Resume or Relaunch
/// where its host can be started again here.
pub(super) fn host_bar(app: AppState, bar: Memo<Option<HostBar>>) -> AnyElement {
    let form = app.form;
    dynamic(move || {
        bar.get().map(|bar| {
            untracked(|| {
                let action = bar.action.map(|(label, action)| {
                    button(
                        app,
                        label,
                        ButtonKind::Primary,
                        format!("btn-{}", label.to_lowercase()),
                        move || run(app, action.clone()),
                    )
                });
                let line = view()
                    .test_id("session-host-bar")
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.0))
                    .child(view().flex_1().min_w(px(0.0)).child(words(
                        form,
                        bar.text,
                        hue_ink(Hue::Neutral),
                    )))
                    .children(action);
                match form {
                    Form::Terminal => view()
                        .flex_none()
                        .child(term::hrule())
                        .child(line.px(term::cells(1.0)))
                        .into_element(),
                    Form::Desktop => line
                        .flex_none()
                        .px(px(18.0))
                        .py(px(10.0))
                        .border_t(px(1.0))
                        .border_color(color(Token::Rule))
                        .bg(color(Token::Surface))
                        .into_element(),
                    Form::Phone => line.flex_none().px(px(16.0)).py(px(10.0)).into_element(),
                }
            })
        })
    })
    .into_element()
}
