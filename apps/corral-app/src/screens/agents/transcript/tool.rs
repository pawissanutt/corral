//! A tool row (`.tool`) in the terminal UI's grammar:
//! `<state> <verb> <target> · <outcome> · <detail>   <label> <hidden> <time> <view>`, the verb in
//! ink semibold, the outcome in its state's colour (muted when it has none). A failure keeps its
//! reason rows under it, a running call its tail. Narrow rows drop the detail, then the hidden
//! count, then the label; the state, verb and time never drop and the target clips. Pressing the
//! row opens its full view; its view link opens the tab it names.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::transcript::{Tool, ToolView};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::super::run;
use super::cell_lines;
use super::parts::{glyph, hidden, pressable, sections, state};

/// What a screen reader hears for the row.
fn label(tool: &Tool) -> String {
    let mut words = format!("{} {} {}", tool.state.label(), tool.name, tool.summary);
    if !tool.outcome.is_empty() {
        words.push_str(&format!(", {}", tool.outcome));
    }
    if let Some(note) = &tool.note {
        words.push_str(&format!(", {note}"));
    }
    words
}

fn mono(content: impl Into<SharedString>, size: f32, ink: AdaptiveColor) -> Text {
    text(content.into())
        .number_of_lines(1u32)
        .font_family(FONT_MONO)
        .text_size(px(size))
        .text_color(ink)
}

/// The summary on one line: a recorded argument can carry newlines and runs of spaces.
fn one_line(words: &str) -> String {
    words.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The outcome's ink: its state's colour, else muted.
fn outcome_ink(tool: &Tool) -> AdaptiveColor {
    if tool.outcome_hue == Hue::Neutral {
        color(Token::Ink3)
    } else {
        hue_ink(tool.outcome_hue)
    }
}

/// The view a row opens, as a small link (`btn-view-<id>`).
fn view_link(app: AppState, link: &ToolView, id: &str) -> AnyElement {
    let action = link.action.clone();
    let words = text(link.label)
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(color(Token::Ink2))
        .font_weight(FontWeight::SEMIBOLD);
    let words = if app.form == Form::Terminal {
        words.font_weight(FontWeight::BOLD)
    } else {
        words.font_family(FONT_MONO).text_size(px(11.0))
    };
    view()
        .flex_shrink_0()
        .test_id(&format!("btn-view-{id}"))
        .role(AccessibilityRole::Link)
        .label(format!("Open {}", link.label))
        .cursor_pointer()
        .on_press(move || run(app, action.clone()))
        .child(words)
        .into_element()
}

/// The row's one line, without its box: drawn alone where the row is nested (a group's rows,
/// a cell's calls).
pub(super) fn line(app: AppState, tool: &Tool, id: &str) -> View {
    let form = app.form;
    let narrow = match form {
        Form::Terminal => term::narrow(),
        Form::Phone => true,
        Form::Desktop => false,
    };
    let summary = one_line(&tool.summary);
    let hides = if narrow {
        None
    } else {
        hidden(tool.hidden.as_deref(), &tool.more)
    };
    let detail = tool.detail.clone().filter(|_| !narrow);
    let right: Vec<String> = [
        tool.note.clone().filter(|_| !narrow),
        hides,
        tool.time.clone(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if form == Form::Terminal {
        return view()
            .flex_row()
            .h(term::rows(1.0))
            .child(glyph(app, tool.state))
            .child(term::space(1.0))
            .child(
                text(tool.name.clone())
                    .flex_shrink_0()
                    .number_of_lines(1u32)
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink)),
            )
            .when(!summary.is_empty(), |row| {
                row.child(term::space(1.0)).child(
                    text(summary.clone())
                        .flex_shrink(1.0)
                        .min_w(term::cells(4.0))
                        .truncate()
                        .text_color(color(Token::Ink)),
                )
            })
            .when(!tool.outcome.is_empty(), |row| {
                row.child(term::quiet(" · ").flex_shrink_0()).child(
                    text(tool.outcome.clone())
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .truncate()
                        .text_color(outcome_ink(tool)),
                )
            })
            .children(detail.map(|detail| {
                view()
                    .flex_row()
                    .flex_shrink(4.0)
                    .min_w(px(0.0))
                    .child(term::quiet(" · ").flex_shrink_0())
                    .child(term::quiet(detail).truncate().min_w(px(0.0)))
            }))
            .child(view().flex_1().min_w(px(0.0)))
            .children(right.into_iter().map(|words| {
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .child(term::space(1.0))
                    .child(term::quiet(words))
            }))
            .children(tool.view.as_ref().filter(|_| !narrow).map(|link| {
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .child(term::space(1.0))
                    .child(view_link(app, link, id))
            }));
    }
    let phone = form == Form::Phone;
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(10.0))
        .py(px(5.0))
        .min_h(px(if phone { 36.0 } else { 26.0 }))
        // The phone row reaches the 44 pt target into the 4 pt margins around it.
        .when(phone, |row| {
            row.hit_slop(EdgeInsets {
                top: px(4.0),
                right: px(0.0),
                bottom: px(4.0),
                left: px(0.0),
            })
        })
        .child(
            view()
                .w(px(14.0))
                .flex_shrink_0()
                .child(glyph(app, tool.state)),
        )
        .child(
            mono(tool.name.clone(), 11.5, color(Token::Ink))
                .flex_shrink_0()
                .font_weight(FontWeight::SEMIBOLD),
        )
        .when(!summary.is_empty(), |row| {
            row.child(
                mono(summary.clone(), 11.5, color(Token::Ink2))
                    .flex_shrink(1.0)
                    .min_w(px(32.0))
                    .truncate(),
            )
        })
        .when(!tool.outcome.is_empty(), |row| {
            row.child(
                mono(format!("· {}", tool.outcome), 11.0, outcome_ink(tool))
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .truncate(),
            )
        })
        .children(detail.map(|detail| {
            mono(format!("· {detail}"), 11.0, color(Token::Ink4))
                .flex_shrink(4.0)
                .min_w(px(0.0))
                .truncate()
        }))
        .child(view().flex_1().min_w(px(0.0)))
        .children(
            right
                .into_iter()
                .map(|words| mono(words, 11.0, color(Token::Ink3)).flex_shrink_0()),
        )
        .children(tool.view.as_ref().map(|link| view_link(app, link, id)))
}

/// The rows under a folded line: a failure's reason, a running call's tail.
pub(super) fn tail(app: AppState, tool: &Tool) -> Option<AnyElement> {
    if tool.tail.is_empty() {
        return None;
    }
    if app.form == Form::Terminal {
        return Some(
            view()
                .pl(term::cells(2.0))
                .child(cell_lines(&tool.tail))
                .into_element(),
        );
    }
    Some(
        view()
            .pl(px(32.0))
            .pr(px(10.0))
            .pb(px(6.0))
            .children(tool.tail.iter().map(|row| {
                mono(
                    row.text
                        .iter()
                        .map(|span| span.text.as_str())
                        .collect::<String>(),
                    11.0,
                    color(Token::Ink3),
                )
                .truncate()
            }))
            .into_element(),
    )
}

/// One tool row in its hairline box (a terminal draws it bare), pressable to open.
pub(super) fn tool(app: AppState, tool: &Tool, id: &str) -> AnyElement {
    let head = pressable(
        app,
        line(app, tool, id),
        tool.action.as_ref(),
        label(tool),
        tool.open,
        id,
    );
    let opened = tool.open.then(|| {
        let body = view()
            .child(sections(app, &tool.body))
            .children(state(app, &tool.more, id, false));
        if app.form == Form::Terminal {
            body.pl(term::cells(2.0)).into_element()
        } else {
            body.border_t_1()
                .border_t_color(color(Token::Rule))
                .px(px(10.0))
                .py(px(8.0))
                .into_element()
        }
    });
    let block = view()
        .min_w(px(0.0))
        .child(head)
        .children(tail(app, tool))
        .children(opened);
    if app.form == Form::Terminal {
        return block.into_element();
    }
    block
        .mt(px(4.0))
        .mb(px(4.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .overflow_hidden()
        .into_element()
}
