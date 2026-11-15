//! A tool row (`.tool`): one quiet line (the state mark, the tool, what it acted on, a note and
//! what it hides, how it ended) in a hairline box; a failed call keeps its last output lines
//! and its outcome in the failure colour while folded. Pressing the line opens the diff or
//! written content, the arguments, the output and the recorded call and result.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::Tool;
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::kit::marks::mark;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::super::console;
use super::parts::{hidden, pressable, sections, state};

/// What a screen reader hears for the row.
fn label(tool: &Tool) -> String {
    let mut words = format!("{} {}, {}", tool.name, tool.summary, tool.outcome);
    if let Some(note) = &tool.note {
        words.push_str(&format!(", {note}"));
    }
    words
}

fn mono(content: impl Into<SharedString>, size: f32, ink: Token) -> Text {
    text(content.into())
        .number_of_lines(1u32)
        .font_family(FONT_MONO)
        .text_size(px(size))
        .text_color(color(ink))
}

pub(super) fn tool(app: AppState, tool: &Tool, n: usize) -> AnyElement {
    let form = app.form;
    let outcome_ink = if tool.failed {
        Token::Fail
    } else {
        Token::Ink3
    };
    let hides = hidden(tool.hidden.as_deref(), &tool.more);
    if form == Form::Terminal {
        let head = view()
            .flex_row()
            .h(term::rows(1.0))
            .child(mark(app, tool.mark, Hue::Neutral))
            .child(term::space(1.0))
            .child(term::quiet(tool.name.clone()))
            .child(term::space(1.0))
            .child(
                text(tool.summary.clone())
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(color(Token::Ink)),
            )
            .children(tool.note.clone().map(|note| {
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .child(term::space(1.0))
                    .child(term::quiet(note))
            }))
            .children(hides.map(|words| {
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .child(term::space(1.0))
                    .child(term::quiet(words))
            }))
            .child(term::space(1.0))
            .child(
                text(tool.outcome.clone())
                    .flex_shrink_0()
                    .number_of_lines(1u32)
                    .text_color(color(outcome_ink)),
            );
        return view()
            .child(pressable(
                app,
                head,
                tool.action.as_ref(),
                label(tool),
                tool.open,
                n,
            ))
            .when(!tool.tail.is_empty(), |block| {
                block.child(
                    view()
                        .pl(term::cells(2.0))
                        .child(console::cell_lines(app, &tool.tail)),
                )
            })
            .when(tool.open, |block| {
                block.child(
                    view()
                        .pl(term::cells(2.0))
                        .child(sections(app, &tool.body))
                        .children(state(app, &tool.more, n, false)),
                )
            })
            .into_element();
    }
    let phone = form == Form::Phone;
    let head = view()
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
                .child(mark(app, tool.mark, Hue::Neutral)),
        )
        .child(mono(tool.name.clone(), 11.0, Token::Ink3).flex_shrink_0())
        .child(
            mono(tool.summary.clone(), 11.5, Token::Ink)
                .flex_1()
                .min_w(px(0.0))
                .truncate(),
        )
        .children(
            tool.note
                .clone()
                .map(|note| mono(note, 11.0, Token::Ink3).flex_shrink_0()),
        )
        .children(hides.map(|words| mono(words, 11.0, Token::Ink4).flex_shrink_0()))
        .child(mono(tool.outcome.clone(), 11.0, outcome_ink).flex_shrink_0());
    view()
        .mt(px(4.0))
        .mb(px(4.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .overflow_hidden()
        .child(pressable(
            app,
            head,
            tool.action.as_ref(),
            label(tool),
            tool.open,
            n,
        ))
        .when(!tool.tail.is_empty(), |block| {
            block.child(view().pl(px(32.0)).pr(px(10.0)).pb(px(6.0)).children(
                tool.tail.iter().map(|row| {
                    mono(
                        row.text
                            .iter()
                            .map(|span| span.text.as_str())
                            .collect::<String>(),
                        11.0,
                        Token::Ink3,
                    )
                    .truncate()
                }),
            ))
        })
        .when(tool.open, |block| {
            block.child(
                view()
                    .border_t_1()
                    .border_t_color(color(Token::Rule))
                    .px(px(10.0))
                    .py(px(8.0))
                    .child(sections(app, &tool.body))
                    .children(state(app, &tool.more, n, false)),
            )
        })
        .into_element()
}
