//! Settings sections of a served feed (corrald's, or the synthetic preview's) that read the
//! settings snapshot or say what this device can do: Machines and Notifications; and the parts
//! every live section shares (the snapshot's loading/stale/unreachable/failed state and a
//! facts table).

use corral_protocol::rows::SettingsSnapshot;
use nsh_core::prelude::*;

use super::parts::{button_row, cell, frame, mono_cell, space};
use crate::model::daemon::Feed;
use crate::model::settings::live::{self, Fact, Standing};
use crate::model::settings::push;
use crate::model::text::span;
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::notice::{error, notice};
use crate::ui::kit::page::Width;
use crate::ui::kit::table::{devtable, qtable};
use crate::ui::tokens::Token;

/// A facts table: quiet labels beside mono values.
pub fn facts(app: AppState, rows: Vec<Fact>) -> AnyElement {
    qtable(
        app,
        &[Width::Fixed(140.0), Width::Fill],
        rows.into_iter()
            .map(|(key, value)| {
                vec![
                    cell(app, key, Token::Ink3, false).into_element(),
                    mono_cell(app, value, Token::Ink).into_element(),
                ]
            })
            .collect(),
    )
}

/// The snapshot's state when it is not simply current.
fn standing(app: AppState, standing: &Standing) -> Option<AnyElement> {
    match standing {
        Standing::Ready => None,
        Standing::Loading => Some(notice(
            app,
            Hue::Neutral,
            &vec![span("Loading settings from corrald…")],
            None,
        )),
        Standing::Stale => Some(notice(
            app,
            Hue::Held,
            &vec![span(
                "Showing the last settings received; they update when corrald answers again.",
            )],
            None,
        )),
        Standing::Unreachable(problem) => {
            let mut line = vec![span(
                "corrald can't be reached, so settings show once it is.",
            )];
            if let Some(problem) = problem {
                line.push(span(" "));
                line.push(span(problem.clone()).mono());
            }
            Some(notice(app, Hue::Held, &line, None))
        }
        Standing::Failed(message) => Some(error(
            app,
            &vec![span("Settings unavailable: ").bold(), span(message.clone())],
            None,
        )),
    }
}

/// What a live snapshot section is built from.
#[derive(Debug, Clone, PartialEq)]
struct Source {
    settings: Option<SettingsSnapshot>,
    standing: Standing,
}

fn source(feed: &Feed) -> Source {
    Source {
        settings: live::snapshot(feed).cloned(),
        standing: live::standing(feed),
    }
}

/// The parts drawn from the settings snapshot, again whenever it changes: the standing notice,
/// then `build`'s parts (absent while there is no snapshot).
pub fn snapshot_parts(
    app: AppState,
    build: impl Fn(AppState, &SettingsSnapshot) -> Vec<AnyElement> + 'static,
) -> AnyElement {
    let shown = computed(move || app.feed.with(source));
    dynamic(move || {
        let source = shown.get();
        untracked(|| {
            let mut parts: Vec<AnyElement> = standing(app, &source.standing).into_iter().collect();
            if let Some(settings) = &source.settings {
                if !parts.is_empty() {
                    parts.push(space(app, 12.0));
                }
                parts.extend(build(app, settings));
            }
            view().min_w(px(0.0)).children(parts).into_element()
        })
    })
    .into_element()
}

/// A section drawn from the settings snapshot (see [`snapshot_parts`]) in its frame.
pub fn snapshot_section(
    app: AppState,
    title: &'static str,
    build: impl Fn(AppState, &SettingsSnapshot) -> Vec<AnyElement> + 'static,
) -> AnyElement {
    frame(app, title, 12.0, vec![snapshot_parts(app, build)])
}

/// Machines: this daemon host as the one machine, with what corrald reports of its runner and
/// its live runs, and why there is nothing to add.
pub fn machines(app: AppState) -> AnyElement {
    let row = computed(move || app.feed.with(live::machine));
    dynamic(move || {
        let [name, role, runtime, agents, reach] = row.get();
        untracked(move || {
            let widths = [
                Width::Fill,
                Width::Fixed(120.0),
                Width::Fill,
                Width::Fixed(72.0),
                Width::Fill,
            ];
            let bleed = if app.form == Form::Phone { -18.0 } else { 0.0 };
            frame(
                app,
                "Machines",
                12.0,
                vec![
                    view()
                        .mx(px(bleed))
                        .child(devtable(
                            app,
                            &widths,
                            &["Machine", "Role", "Runner", "Agents", "Reach"],
                            vec![vec![
                                cell(app, name, Token::Ink, true).into_element(),
                                cell(app, role, Token::Ink, false).into_element(),
                                cell(app, runtime, Token::Ink, false).into_element(),
                                cell(app, agents, Token::Ink, false).into_element(),
                                mono_cell(app, reach, Token::Ink).into_element(),
                            ]],
                        ))
                        .into_element(),
                    space(app, 14.0),
                    notice(app, Hue::Neutral, &vec![span(live::MACHINES_NOTE)], None),
                ],
            )
        })
    })
    .into_element()
}

/// Notifications: where this browser's Web Push stands, the switch that turns it on or off,
/// a switch per kind while it is on, and what corrald reports of the last send. A device that
/// cannot receive push says why and offers nothing to switch.
pub fn notifications(app: AppState) -> AnyElement {
    let web = crate::client::pairing_supported();
    let supported = crate::client::push_supported();
    let shown = computed(move || app.feed.with(|feed| push::view(feed, supported, web)));
    let body = dynamic(move || {
        let v = shown.get();
        untracked(move || {
            let mut parts = vec![notice(app, v.hue, &vec![span(v.line.clone())], None)];
            if let Some((label, op)) = v.action.clone() {
                parts.push(space(app, 10.0));
                parts.push(button_row(
                    app,
                    vec![button_with(
                        app,
                        label,
                        ButtonKind::Primary,
                        ButtonOptions::default(),
                        "btn-push",
                        move || {
                            let op = op.clone();
                            app.act(app.settings, move |state, _| state.device(op));
                        },
                    )],
                ));
            }
            parts.push(space(app, 12.0));
            parts.push(kinds(app, &v.kinds));
            if !v.facts.is_empty() {
                parts.push(space(app, 12.0));
                parts.push(facts(app, v.facts.clone()));
            }
            view().min_w(px(0.0)).children(parts).into_element()
        })
    });
    frame(app, "Notifications", 12.0, vec![body])
}

/// One row per kind: what it covers, and its on/off switch (a chip that reads "off" while push
/// is off for this device, never "not…" cut short).
fn kinds(app: AppState, rows: &[push::KindRow]) -> AnyElement {
    let rows = rows
        .iter()
        .map(|row| {
            let kind = row.kind;
            let (label, hue) = if row.on {
                ("on", Hue::Ok)
            } else {
                ("off", Hue::Neutral)
            };
            let face = chip(app, label, hue);
            let end = if row.enabled {
                button_with(
                    app,
                    if row.on { "Turn off" } else { "Turn on" },
                    ButtonKind::Quiet,
                    ButtonOptions::default(),
                    format!("btn-push-{}", push::slug(&kind)),
                    move || {
                        app.reduce(app.settings, move |state, feed, _, _| match &feed.push {
                            Some(status) => state.device(push::flip(status, &kind)),
                            None => Vec::new(),
                        });
                    },
                )
            } else {
                view().into_element()
            };
            vec![
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(cell(app, push::label(&kind), Token::Ink, true))
                    .child(cell(app, push::detail(&kind), Token::Ink3, false))
                    .into_element(),
                view()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(face)
                    .child(end)
                    .into_element(),
            ]
        })
        .collect();
    let end = match app.form {
        Form::Desktop => 170.0,
        Form::Phone | Form::Terminal => 150.0,
    };
    qtable(app, &[Width::Fill, Width::Fixed(end)], rows)
}
