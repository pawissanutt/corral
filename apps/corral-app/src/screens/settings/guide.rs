//! Settings › Connection's guide while there is no connection: where the connection stands,
//! the numbered steps, the commands and configuration to copy (mono, selectable, never inline
//! backticks), why pairing fails, and what the app knows of its connection.

use nsh_core::prelude::*;

use super::live_sections::facts;
use super::parts::{cell, space};
use crate::model::Form;
use crate::model::settings::connect::{Guide, Progress};
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::{StepState, qtable, steps4};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color, text_size};

/// Where the connection stands, with the six-digit code to compare while pairing waits.
pub fn status(app: AppState, guide: &Guide) -> AnyElement {
    let mut parts = vec![notice(
        app,
        guide.hue,
        &vec![span(guide.status.clone())],
        None,
    )];
    if let Some(sas) = &guide.sas {
        parts.push(space(app, 10.0));
        parts.push(sas_code(app, sas));
    }
    view().min_w(px(0.0)).children(parts).into_element()
}

/// The code to compare, large and spaced so it reads at a glance beside the trusted device.
fn sas_code(app: AppState, sas: &str) -> AnyElement {
    let code = text(sas.to_owned())
        .test_id("pair-sas")
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    if app.form == Form::Terminal {
        return crate::ui::composer::terminal_selectable(app, view().selectable(true))
            .child(code.select_text())
            .into_element();
    }
    view()
        .flex_row()
        .child(
            code.font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(28.0))
                .letter_spacing(px(2.0))
                .select_text(),
        )
        .into_element()
}

/// The steps, commands, failure reasons and connection details.
pub fn body(app: AppState, guide: &Guide) -> Vec<AnyElement> {
    let mut parts = vec![
        h3(app, "How to connect", None),
        steps4(
            app,
            guide
                .steps
                .iter()
                .map(|step| {
                    let state = match step.progress {
                        Progress::Done => StepState::Done,
                        Progress::Current => StepState::On,
                        Progress::Later => StepState::Next,
                    };
                    (state, step.title.to_owned(), step.detail.to_owned())
                })
                .collect(),
        ),
    ];
    if !guide.commands.is_empty() {
        parts.push(h3(app, "Commands", None));
        parts.push(commands(app, &guide.commands));
    }
    if !guide.failures.is_empty() {
        parts.push(h3(app, "If pairing fails", None));
        parts.push(failures(app, &guide.failures));
    }
    parts.push(h3(app, "Connection details", None));
    parts.push(facts(app, guide.details.clone()));
    parts
}

/// Each command or snippet under what it is, in a sunk mono block that selects as a whole.
fn commands(app: AppState, commands: &[(&'static str, String)]) -> AnyElement {
    let form = app.form;
    view()
        .min_w(px(0.0))
        .gap(if form == Form::Terminal {
            term::rows(1.0)
        } else {
            px(10.0)
        })
        .children(commands.iter().map(|(label, code)| {
            view()
                .min_w(px(0.0))
                .gap(px(4.0))
                .child(cell(app, *label, Token::Ink3, false))
                .child(code_block(app, code))
                .into_element()
        }))
        .into_element()
}

fn code_block(app: AppState, code: &str) -> AnyElement {
    let form = app.form;
    let words = text(code.to_owned())
        .select_text()
        .text_color(color(Token::Ink));
    if form == Form::Terminal {
        return crate::ui::composer::terminal_selectable(app, view().selectable(true))
            .pl(term::cells(2.0))
            .child(words)
            .into_element();
    }
    let size = text_size(form) * 0.93;
    view()
        .min_w(px(0.0))
        .px(px(12.0))
        .py(px(9.0))
        .rounded(px(6.0))
        .bg(color(Token::BgSunk))
        .border_1()
        .border_color(color(Token::Rule))
        .child(
            words
                .font_family(FONT_MONO)
                .text_size(px(size))
                .line_height(px((size * 1.55).round())),
        )
        .into_element()
}

/// Why pairing fails, each with what to do.
fn failures(app: AppState, rows: &[(&'static str, &'static str)]) -> AnyElement {
    let widths: &[Width] = match app.form {
        Form::Desktop => &[Width::Fixed(220.0), Width::Fill],
        Form::Phone | Form::Terminal => &[Width::Fill],
    };
    let rows = rows
        .iter()
        .map(|&(problem, fix)| {
            if app.form == Form::Desktop {
                vec![
                    cell(app, problem, Token::Ink, true).into_element(),
                    cell(app, fix, Token::Ink3, false).into_element(),
                ]
            } else {
                vec![
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(cell(app, problem, Token::Ink, true))
                        .child(cell(app, fix, Token::Ink3, false))
                        .into_element(),
                ]
            }
        })
        .collect();
    qtable(app, widths, rows)
}
