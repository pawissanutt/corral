//! Settings › Assistant's rollover summary writer: who writes a quick thread's summary when it
//! rolls over, and, for an administrator device, the configured profiles to choose from (those
//! corrald says cannot write summaries are shown with why) or each thread's own model. The
//! choice is the hash-guarded `config.set` of `assistant.summary_profile`.

use corral_protocol::rows::SettingsSnapshot;
use nsh_core::prelude::*;

use super::live_assistant::table;
use super::live_sections::act_line;
use super::parts::{button_row, cell, mono_cell, space};
use crate::model::Hue;
use crate::model::settings::SUMMARY;
use crate::model::settings::live::{self, SummaryChoice};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::page::Width;
use crate::ui::tokens::Token;

pub fn summary(app: AppState, settings: &SettingsSnapshot, editable: bool) -> AnyElement {
    let mut parts =
        vec![cell(app, live::summary_line(settings), Token::Ink3, false).into_element()];
    if editable {
        let choices = live::summary_choices(settings);
        if !choices.is_empty() {
            parts.push(space(app, 8.0));
            parts.push(table(
                app,
                &[Width::Fixed(200.0), Width::Fill, Width::Fixed(200.0)],
                choices
                    .into_iter()
                    .map(|choice| choice_row(app, settings, choice))
                    .collect(),
            ));
        }
        if settings.summary_profile.is_some() {
            let settings = settings.clone();
            parts.push(space(app, 8.0));
            parts.push(button_row(
                app,
                vec![button_with(
                    app,
                    "Use each thread's own model",
                    ButtonKind::Ghost,
                    ButtonOptions::default(),
                    "btn-summary-thread-model",
                    move || {
                        let settings = settings.clone();
                        app.act(app.settings, move |state, _| {
                            state.set_summary(&settings, None)
                        });
                    },
                )],
            ));
        }
    }
    parts.push(act_line(app, SUMMARY));
    view()
        .min_w(px(0.0))
        .gap(px(2.0))
        .children(parts)
        .into_element()
}

/// A profile as a summary writer: its name, its model (and why corrald won't use it for
/// summaries), and the writer's chip or the choice.
fn choice_row(
    app: AppState,
    settings: &SettingsSnapshot,
    choice: SummaryChoice,
) -> Vec<AnyElement> {
    let name = choice.name.clone();
    let end = if choice.current {
        chip(app, "summary writer", Hue::Ok)
    } else {
        let settings = settings.clone();
        button_with(
            app,
            "Use for summaries",
            ButtonKind::Quiet,
            ButtonOptions {
                disabled: choice.refusal.is_some(),
                ..ButtonOptions::default()
            },
            format!("btn-summary-{name}"),
            move || {
                let settings = settings.clone();
                let profile = name.clone();
                app.act(app.settings, move |state, _| {
                    state.set_summary(&settings, Some(&profile))
                });
            },
        )
    };
    vec![
        cell(app, choice.name, Token::Ink, true).into_element(),
        view()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(2.0))
            .child(mono_cell(app, choice.model, Token::Ink3))
            .children(
                choice
                    .refusal
                    .map(|why| cell(app, why, Token::Ink3, false).into_element()),
            )
            .into_element(),
        view()
            .flex_1()
            .flex_row()
            .justify_end()
            .child(end)
            .into_element(),
    ]
}
