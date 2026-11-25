//! Agents and rules share the regular Configuration draft and save bar.
use nsh_core::prelude::*;

use super::super::config_fields::{edit, entry, field, row};

use crate::model::settings::automation::{self, Change};
use crate::model::settings::document::{self, Path};
use crate::model::settings::editor::Edit;
use crate::model::settings::form::{Kind, Live};
use crate::model::{Id, Live as Record};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::text::h3;
use crate::ui::tokens::Token;

fn cell(app: AppState, words: impl Into<SharedString>, ink: Token, strong: bool) -> AnyElement {
    super::super::parts::cell(app, words, ink, strong).into_element()
}

pub fn part(app: AppState, live: Memo<Live>) -> AnyElement {
    let shape = computed(move || {
        app.settings.with(|state| {
            document::parse(&state.config.draft).map(|doc| {
                let agents = automation::names(&doc, "agents");
                let profiles = automation::profiles(&doc);
                let rules = automation::names(&doc, "rules")
                    .into_iter()
                    .map(|id| {
                        let cron = document::get(&doc, &automation::path("rules", &id, "cron"))
                            .is_some()
                            || document::get(&doc, &automation::path("rules", &id, "event"))
                                .is_none();
                        (id, cron)
                    })
                    .collect::<Vec<_>>();
                (agents, profiles, rules)
            })
        })
    });
    let agent_name = signal(String::new());
    let rule_name = signal(String::new());
    dynamic(move || {
        let layout = shape.get();
        let editable = app.automations_available();
        let preview = app.automation_preview();
        untracked(move || {
            let Ok((agents, profiles, rules)) = layout else {
                return cell(
                    app,
                    "This draft is not valid TOML. Open File to fix it.",
                    Token::Fail,
                    false,
                );
            };
            let mut parts = Vec::new();
            if preview {
                parts.push(cell(app, automation::PREVIEW, Token::Ink2, false));
            }
            if !editable {
                parts.push(cell(app, automation::UNAVAILABLE, Token::Ink2, false));
            }
            parts.push(cell(app, automation::AUTHORITY, Token::Ink2, false));
            parts.push(h3(app, "Agents", None));
            if agents.is_empty() {
                parts.push(cell(
                    app,
                    "Add your first Pi agent before creating a rule.",
                    Token::Ink3,
                    false,
                ));
            }
            if editable {
                parts.push(add(app, "agent", agent_name, agents.clone(), None));
            }
            for id in &agents {
                parts.push(h3(app, id.clone(), None));
                if editable {
                    parts.push(picker(
                        app,
                        "Pi profile",
                        automation::path("agents", id, "profile"),
                        profiles.clone(),
                        live,
                    ));
                    parts.push(field(
                        app,
                        &automation::field(
                            "agents",
                            id,
                            "instructions",
                            "Instructions",
                            Kind::Multiline,
                            None,
                        ),
                        live,
                    ));
                    parts.push(remove(app, "agents", "Delete agent", id));
                }
            }
            parts.push(h3(app, "Rules", None));
            if editable && let Some(agent) = agents.first() {
                parts.push(add(
                    app,
                    "rule",
                    rule_name,
                    rules.iter().map(|(id, _)| id.clone()).collect(),
                    Some(agent.clone()),
                ));
            }
            for (id, cron) in rules {
                parts.push(rule(app, id, cron, &agents, editable, live));
            }
            view()
                .test_id("config-automations")
                .min_w(px(0.0))
                .gap(px(10.0))
                .children(parts)
                .into_element()
        })
    })
    .into_element()
}

fn add(
    app: AppState,
    noun: &'static str,
    name: Signal<String>,
    existing: Vec<String>,
    agent: Option<String>,
) -> AnyElement {
    let control = entry(
        app,
        format!("field-config-new-{noun}-id"),
        format!("New {noun} ID"),
        move || name.get(),
        move |text| name.set(text),
    );
    view().gap(px(4.0)).child(control)
        .child(cell(app, "IDs are permanent: start with a lowercase letter; use up to 40 lowercase letters, digits, hyphens or underscores.", Token::Ink3, false))
        .child(dynamic(move || {
            let id = name.get();
            let disabled = !automation::valid_id(&id) || existing.contains(&id);
            let agent = agent.clone();
            untracked(move || button_with(app, format!("Add {noun}"), ButtonKind::Ghost,
                ButtonOptions { disabled, ..ButtonOptions::default() }, format!("btn-config-add-{noun}"), move || {
                    let change = match &agent {
                        Some(agent) => Change::AddRule(id.clone(), agent.clone()),
                        None => Change::AddAgent(id.clone()),
                    };
                    edit(app, Edit::Automation(change));
                    name.set(String::new());
                }))
        })).into_element()
}

fn remove(app: AppState, section: &'static str, label: &'static str, id: &str) -> AnyElement {
    let id = id.to_owned();
    button_with(
        app,
        label,
        ButtonKind::Ghost,
        ButtonOptions {
            danger: true,
            ..ButtonOptions::default()
        },
        format!("btn-config-delete-{section}-{id}"),
        move || edit(app, Edit::Automation(Change::Remove(section, id.clone()))),
    )
}

fn picker(
    app: AppState,
    label: &'static str,
    path: Path,
    choices: Vec<String>,
    live: Memo<Live>,
) -> AnyElement {
    row(
        app,
        label,
        dynamic(move || {
            let selected = live.with(|live| live.draft.text(&path));
            let reasons = app.settings.with(|state| {
                state
                    .config
                    .reasons_at(&path)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            });
            untracked(|| {
                let mut picks = Vec::new();
                if !choices.contains(&selected) {
                    let value = if selected.is_empty() {
                        "No selection".into()
                    } else {
                        format!("Unavailable: {selected}")
                    };
                    picks.push(cell(app, value, Token::Ink3, false));
                }
                for value in &choices {
                    let pick = value.clone();
                    let set = path.clone();
                    picks.push(button(
                        app,
                        value.clone(),
                        if *value == selected {
                            ButtonKind::Primary
                        } else {
                            ButtonKind::Ghost
                        },
                        format!("seg-config-{path}-{value}"),
                        move || edit(app, Edit::Text(set.clone(), pick.clone())),
                    ));
                }
                view()
                    .min_w(px(0.0))
                    .child(view().flex_row().flex_wrap().gap(px(6.0)).children(picks))
                    .children(
                        reasons
                            .into_iter()
                            .map(|reason| cell(app, reason, Token::Fail, false)),
                    )
                    .into_element()
            })
        })
        .into_element(),
    )
}

fn rule(
    app: AppState,
    id: String,
    cron: bool,
    agents: &[String],
    editable: bool,
    live: Memo<Live>,
) -> AnyElement {
    let summary_id = id.clone();
    let summary = computed(move || {
        app.settings.with(|state| {
            document::parse(&state.config.draft)
                .map(|doc| automation::summary(&doc, &summary_id))
                .unwrap_or_default()
        })
    });
    let mut parts = vec![
        h3(app, id.clone(), None),
        dynamic(move || {
            let words = summary.get();
            untracked(move || cell(app, words, Token::Ink2, false))
        })
        .into_element(),
        health(app, id.clone()),
    ];
    if editable {
        let trigger = [
            (false, "Task blocked", "blocked"),
            (true, "Cron (UTC)", "cron"),
        ]
        .into_iter()
        .map(|(use_cron, label, slug)| {
            let rule = id.clone();
            button(
                app,
                label,
                if cron == use_cron {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Ghost
                },
                format!("seg-config-automation.rules.{id}.trigger-{slug}"),
                move || {
                    edit(
                        app,
                        Edit::Automation(Change::Trigger(rule.clone(), use_cron)),
                    )
                },
            )
        });
        parts.push(row(
            app,
            "Trigger",
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(6.0))
                .children(trigger)
                .into_element(),
        ));
        if cron {
            parts.push(field(
                app,
                &automation::field(
                    "rules",
                    &id,
                    "cron",
                    "UTC cron",
                    Kind::Text,
                    Some("Five fields. For 09:00 UTC on weekdays: 0 9 * * 1-5"),
                ),
                live,
            ));
        }
        parts.push(picker(
            app,
            "Agent",
            automation::path("rules", &id, "agent"),
            agents.to_vec(),
            live,
        ));
        parts.push(field(
            app,
            &automation::field(
                "rules",
                &id,
                "enabled",
                "Enabled",
                Kind::Switch { inherit: false },
                None,
            ),
            live,
        ));
        parts.push(remove(app, "rules", "Delete rule", &id));
    }
    view()
        .test_id(&format!("config-automation-rule-{id}"))
        .min_w(px(0.0))
        .gap(px(6.0))
        .children(parts)
        .into_element()
}

fn health(app: AppState, id: String) -> AnyElement {
    let observed = computed(move || {
        app.feed.with(|feed| {
            feed.settings
                .as_ref()?
                .automations
                .iter()
                .find(|health| health.rule == id)
                .cloned()
        })
    });
    dynamic(move || {
        let report = observed.get();
        untracked(move || {
            let Some(report) = report else {
                return cell(app, "Not observed yet", Token::Ink3, false);
            };
            let mut lines = report
                .last_at
                .into_iter()
                .chain(report.outcome)
                .map(|line| cell(app, line, Token::Ink2, false))
                .collect::<Vec<_>>();
            if let Some(error) = report.error {
                lines.push(cell(app, error, Token::Fail, false));
            }
            if let Some(chat) = report.chat {
                let session = Id::new(&chat);
                lines.push(button(
                    app,
                    format!("Open session {chat}"),
                    ButtonKind::Ghost,
                    format!("btn-automation-session-{}", report.rule),
                    move || app.open_live(Record::Session(session.clone())),
                ));
            }
            view().children(lines).into_element()
        })
    })
    .into_element()
}
