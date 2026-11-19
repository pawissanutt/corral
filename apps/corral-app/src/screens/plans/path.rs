//! Path to done (`.pathl`): what still stands between a plan and its completion, in order. Each
//! step is its mark and title, the current one in its hue and ink; under a step its open members
//! (mark, short text, a Blocked member's served next step) and its warning on the held ground
//! with the next step that resolves it, when one is served. Every line's sentence is its hint;
//! the phone, which has no hover, writes it under the line, and a terminal after it, dimmed.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::Mark;
use crate::model::glance::StatIcon;
use crate::model::hue::Hue;
use crate::model::plans::{
    BriefAction, Button, ButtonKind, PathItem, PathStep, PathToDone, PathWarning,
};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::hint::hinted;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::command;
use super::doc::{side_block, side_title};

/// The mark column (`.pathl > li` 18 pt) and the rule joining one step's mark to the next.
const MARK_COLUMN: f32 = 18.0;

/// The path: its title, then the steps in a block.
pub(super) fn path(app: AppState, path: &PathToDone) -> AnyElement {
    let last = path.steps.len().saturating_sub(1);
    view()
        .test_id("plan-path")
        .child(side_title(app, path.title, path.hint))
        .child(
            side_block(app).child(
                view()
                    .role(AccessibilityRole::List)
                    .label(path.title)
                    .children(
                        path.steps
                            .iter()
                            .enumerate()
                            .map(|(at, each)| step(app, each, at == last))
                            .collect::<Vec<_>>(),
                    ),
            ),
        )
        .into_element()
}

/// Text at `size` in `ink`, wrapping; a terminal keeps its one size.
fn words(app: AppState, content: impl Into<SharedString>, size: f32, ink: AdaptiveColor) -> Text {
    let line = text(content.into()).min_w(px(0.0)).text_color(ink);
    if app.form == Form::Terminal {
        return line;
    }
    line.font_family(FONT_TEXT)
        .text_size(px(size))
        .line_height(px((size * 1.45).round()))
}

/// A sentence the phone writes under its line, quiet.
fn spelled(app: AppState, hint: &str) -> AnyElement {
    words(app, hint.to_owned(), 13.0, color(Token::Ink3)).into_element()
}

/// A sentence a terminal writes after its line, dimmed and cut to the room left.
fn trailing(hint: &str) -> AnyElement {
    view()
        .flex_row()
        .flex_1()
        .min_w(px(0.0))
        .child(term::space(2.0))
        .child(
            term::quiet(hint.to_owned())
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate(),
        )
        .into_element()
}

/// A line's words, its action and its hint. Desktop: the words a stop of the focus whose
/// tooltip is the hint, the action after them, wrapping under them when the column runs short.
/// Phone, which has no hover: the hint written under the words. Terminal: one row of Cells,
/// the words, the action, then the hint dimmed in what room is left (and on the status line).
fn line(app: AppState, words: Text, hint: &str, act: Option<AnyElement>) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return view()
            .flex_row()
            .min_w(px(0.0))
            .child(hinted(app, words.flex_shrink_0(), hint.to_owned()))
            .children(act.map(|act| {
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .child(term::space(1.0))
                    .child(act)
                    .into_element()
            }))
            .when(!hint.is_empty(), |v| v.child(trailing(hint)))
            .into_element();
    }
    // The words give way beside the action, so a long line wraps rather than run past its
    // column.
    let said = view().flex_shrink(1.0).min_w(px(0.0));
    let said = if form == Form::Phone {
        said.gap(px(2.0))
            .child(words)
            .when(!hint.is_empty(), |v| v.child(spelled(app, hint)))
    } else {
        said.child(hinted(app, words, hint.to_owned()))
    };
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .min_w(px(0.0))
        .gap_x(px(8.0))
        .gap_y(px(4.0))
        .child(said)
        .children(act)
        .into_element()
}

/// A step's action as the model's command, at `kind`, with its hint.
fn action(app: AppState, brief: &BriefAction, kind: ButtonKind) -> AnyElement {
    let mut drawn =
        Button::new(brief.label.clone(), brief.action.clone()).hinted(brief.hint.clone());
    drawn.kind = kind;
    command(app, &drawn, false)
}

/// One step (`.pathl > li`): its mark, a rule down to the next step's, its title, then its open
/// items and its warning. The current step's mark takes its hue; the others keep the mark's own
/// colour, a step to come in the quiet ink.
fn step(app: AppState, step: &PathStep, last: bool) -> AnyElement {
    let form = app.form;
    let current = step.mark == Mark::Active;
    let ink = color(if step.mark == Mark::Next {
        Token::Ink2
    } else {
        Token::Ink
    });
    let size = if form == Form::Phone { 14.5 } else { 13.0 };
    let title = words(app, step.title.clone(), size, ink)
        .font_weight(if step.mark == Mark::Next {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::BOLD
        })
        .when(form == Form::Terminal && current, |t| {
            t.font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink))
        });
    let hue = if current { step.hue } else { Hue::Neutral };
    let body = view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(line(app, title, &step.hint, None))
        .when(!step.items.is_empty(), |body| {
            body.child(
                view()
                    .role(AccessibilityRole::List)
                    .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                    .children(
                        step.items
                            .iter()
                            .map(|each| item(app, each))
                            .collect::<Vec<_>>(),
                    ),
            )
        })
        .children(step.warning.as_ref().map(|warning| warned(app, warning)));
    let row = view().relative().flex_row().items_start();
    if form == Form::Terminal {
        return row
            .child(mark(app, step.mark, hue))
            .child(term::space(1.0))
            .child(body)
            .into_element();
    }
    row.gap(px(8.0))
        .pb(px(if last { 0.0 } else { 14.0 }))
        .when(!last, |row| {
            row.child(
                view()
                    .absolute()
                    .left(px(MARK_COLUMN / 2.0 - 0.5))
                    .top(px(20.0))
                    .bottom(px(2.0))
                    .w(px(1.0))
                    .bg(color(Token::Rule2))
                    .accessibility_elements_hidden(true),
            )
        })
        .child(
            view()
                .w(px(MARK_COLUMN))
                .h(px((size * 1.45).round()))
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .child(mark(app, step.mark, hue)),
        )
        .child(body)
        .into_element()
}

/// One open member under a step (`.pathl ul li`): its mark in its hue, the short text in the
/// hue where it waits on you or is held, and its action. Terminal: indented two Cells.
fn item(app: AppState, item: &PathItem) -> AnyElement {
    let form = app.form;
    let ink = match item.hue {
        Hue::You | Hue::Held => hue_ink(item.hue),
        _ if form == Form::Terminal => color(Token::Ink),
        _ => color(Token::Ink2),
    };
    let size = if form == Form::Phone { 13.5 } else { 12.5 };
    let said = line(
        app,
        words(app, item.text.clone(), size, ink),
        &item.hint,
        item.action
            .as_ref()
            .map(|brief| action(app, brief, ButtonKind::Standard)),
    );
    let row = view().flex_row().items_start();
    if form == Form::Terminal {
        return row
            .child(term::space(2.0))
            .child(mark(app, item.mark, item.hue))
            .child(term::space(1.0))
            .child(view().flex_1().min_w(px(0.0)).child(said))
            .into_element();
    }
    row.gap(px(6.0))
        .child(
            view()
                .w(px(16.0))
                .h(px((size * 1.45).round()))
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .child(mark(app, item.mark, item.hue)),
        )
        .child(view().flex_1().min_w(px(0.0)).child(said))
        .into_element()
}

/// The warning under a step (`.warn`): on the held ground, the held icon, the short text and
/// the action that resolves it; its sentence is its hint. Terminal: indented, the held glyph.
fn warned(app: AppState, warning: &PathWarning) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone { 13.5 } else { 12.5 };
    let said = view().flex_1().min_w(px(0.0)).child(line(
        app,
        words(app, warning.text.clone(), size, color(Token::Ink)),
        &warning.hint,
        warning
            .action
            .as_ref()
            .map(|brief| action(app, brief, ButtonKind::Ghost)),
    ));
    if form == Form::Terminal {
        return view()
            .flex_row()
            .items_start()
            .child(term::space(2.0))
            .child(
                text(StatIcon::Held.glyph().to_string())
                    .flex_shrink_0()
                    .text_color(hue_ink(Hue::Held)),
            )
            .child(term::space(1.0))
            .child(said)
            .into_element();
    }
    view()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(6.0))
        .bg(color(Token::HeldBg))
        .child(
            view()
                .h(px((size * 1.45).round()))
                .flex_shrink_0()
                .justify_center()
                .accessibility_elements_hidden(true)
                .child(icon(Icon::Held, 14.0, hue_ink(Hue::Held))),
        )
        .child(said)
        .into_element()
}
