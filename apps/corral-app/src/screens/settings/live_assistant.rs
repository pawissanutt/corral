//! Settings › Assistant in a daemon session: who assistants answer with, the corral-pi profiles
//! the composer's selector offers, every configured profile, and the owner's edits: make a
//! profile the assistant profile, change a profile's model, add a profile as a copy of a
//! configured one, or remove one corrald would let go. Model and assistant edits are the
//! hash-guarded `config.set`; adding and removing are hash-guarded `config.profile-*` commands
//! corrald previews for a fresh confirmation here. A configuration changed meanwhile comes back
//! as a conflict beside the control, never as a silent overwrite.

use corral_protocol::rows::SettingsSnapshot;
use nsh_base::{toggle, toggle_group};
use nsh_core::prelude::*;

use super::enrolment::{bound_field, group};
use super::live_sections::{act_line, in_flight, refusal, snapshot_section};
use super::parts::{
    Record, button_row, cell, first_heading, intro, mono_cell, records, ring, space,
};
use crate::model::settings::live::{self, LiveProfile};
use crate::model::settings::profiles::{self, ManagedProfile, ProfileField};
use crate::model::settings::{ADD_PROFILE, remove_slot};
use crate::model::{ActStatus, Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::qtable;
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

const INTRO: &str = "Assistants answer with the assistant profile. The composer's model selector offers every corral-pi profile that names a model; a switch applies to the next message and keeps the conversation.";

const PROFILES_NOTE: &str = "Every profile corral can dispatch with. A new profile is a copy of a configured one under a new name; corrald previews each add and removal for you to confirm here, and refuses to remove a profile the configuration or open work still uses.";

const ADD_NOTE: &str = "The copy keeps the chosen profile's harness and settings as configured. Leave model or effort blank to keep the copied one.";

pub fn page(app: AppState) -> AnyElement {
    snapshot_section(app, "Assistant", |app, settings, source| {
        let editable = source.editable();
        let mut parts = vec![intro(app, INTRO), space(app, 10.0)];
        parts.extend(refusal(app, source));
        parts.push(cell(app, live::assistant_line(settings), Token::Ink, false).into_element());
        parts.push(space(app, 12.0));
        parts.push(first_heading(
            app,
            "Models in the selector",
            Some("assistant.agent"),
        ));
        parts.push(selector(app, settings, editable));
        parts.push(h3(
            app,
            "Configured profiles",
            Some("profiles.<name>".into()),
        ));
        parts.push(cell(app, PROFILES_NOTE, Token::Ink3, false).into_element());
        parts.push(space(app, 6.0));
        parts.push(configured(app, settings.clone(), editable));
        if editable {
            parts.push(space(app, 8.0));
            parts.push(add(app, settings.clone()));
        }
        parts.push(h3(
            app,
            "When the quick thread rolls over",
            Some("assistant.summary_profile".into()),
        ));
        parts.push(super::live_summary::summary(app, settings, editable));
        parts
    })
}

/// The selector's profiles, the assistant profile chipped and the others offered for it.
fn selector(app: AppState, settings: &SettingsSnapshot, editable: bool) -> AnyElement {
    let rows = live::profiles(settings);
    if rows.is_empty() {
        return cell(
            app,
            "No corral-pi profile names a model yet, so the selector is empty.",
            Token::Ink3,
            false,
        )
        .into_element();
    }
    let widths: &[Width] = match app.form {
        Form::Desktop => &[Width::Fixed(230.0), Width::Fill, Width::Fixed(200.0)],
        Form::Phone | Form::Terminal => &[Width::Fixed(96.0), Width::Fill, Width::Fill],
    };
    table(
        app,
        widths,
        rows.into_iter()
            .map(|row| selector_row(app, settings, row, editable))
            .collect(),
    )
}

/// Rows of three cells (what leads, its details, its controls) as a table; on a phone, where
/// three columns break names and model ids mid-token, as records with the details under the
/// name and the controls on its line.
pub(super) fn table(app: AppState, widths: &[Width], rows: Vec<Vec<AnyElement>>) -> AnyElement {
    if app.form != Form::Phone {
        return qtable(app, widths, rows);
    }
    records(
        rows.into_iter()
            .filter_map(|row| {
                let mut cells = row.into_iter();
                let lead = cells.next()?;
                let under = cells.next().into_iter().collect();
                Some(Record {
                    lead,
                    under,
                    end: cells.next(),
                })
            })
            .collect(),
    )
}

fn selector_row(
    app: AppState,
    settings: &SettingsSnapshot,
    row: LiveProfile,
    editable: bool,
) -> Vec<AnyElement> {
    let name = row.name.clone();
    let lead = view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(2.0))
        .child(cell(app, row.name.clone(), Token::Ink, true))
        .child(act_line(app, format!("use-{name}")))
        .into_element();
    let end = if row.default {
        chip(app, "assistant profile", Hue::Ok)
    } else if editable {
        let settings = settings.clone();
        button_with(
            app,
            "Use for assistants",
            ButtonKind::Quiet,
            ButtonOptions::default(),
            format!("btn-use-{name}"),
            move || {
                let settings = settings.clone();
                let profile = name.clone();
                app.act(app.settings, move |state, _| {
                    state.use_profile(&settings, &profile)
                });
            },
        )
    } else {
        view().into_element()
    };
    vec![
        lead,
        mono_cell(app, row.model, Token::Ink3).into_element(),
        view()
            .flex_1()
            .flex_row()
            .justify_end()
            .child(end)
            .into_element(),
    ]
}

/// Every configured profile, drawn again while a model is being edited.
fn configured(app: AppState, settings: SettingsSnapshot, editable: bool) -> AnyElement {
    let rows = profiles::managed(&settings);
    if rows.is_empty() {
        return cell(
            app,
            "corrald reports no configured profiles, so there is nothing to copy or remove here.",
            Token::Ink3,
            false,
        )
        .into_element();
    }
    let editing = computed(move || {
        app.settings
            .with(|state| state.edits.model.as_ref().map(|(p, _)| p.clone()))
    });
    dynamic(move || {
        let editing = editing.get();
        let settings = settings.clone();
        let rows = rows.clone();
        untracked(move || {
            let widths: &[Width] = match app.form {
                Form::Desktop => &[Width::Fixed(200.0), Width::Fill, Width::Fixed(230.0)],
                Form::Phone | Form::Terminal => &[Width::Fixed(96.0), Width::Fill, Width::Fill],
            };
            table(
                app,
                widths,
                rows.into_iter()
                    .map(|row| {
                        let editing = editing.as_deref() == Some(row.name.as_str());
                        profile_row(app, &settings, row, editable, editing)
                    })
                    .collect(),
            )
        })
    })
    .into_element()
}

/// A configured profile: its name and where it is configured; its harness, model and effort
/// (or the model being edited), what holds it and its commands' status; then its edits.
fn profile_row(
    app: AppState,
    settings: &SettingsSnapshot,
    row: ManagedProfile,
    editable: bool,
    editing: bool,
) -> Vec<AnyElement> {
    let name = row.name.clone();
    let lead = view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(2.0))
        .child(cell(app, row.name.clone(), Token::Ink, true))
        .children(
            row.origin
                .map(|origin| cell(app, origin, Token::Ink3, false).into_element()),
        )
        .into_element();
    let setup = if editing {
        model_editor(app, settings, &name)
    } else {
        mono_cell(app, row.setup.clone(), Token::Ink3).into_element()
    };
    let middle = view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(2.0))
        .child(setup)
        .children(
            row.held
                .clone()
                .map(|held| cell(app, held, Token::Ink3, false).into_element()),
        )
        .child(act_line(app, format!("model-{name}")))
        .child(act_line(app, remove_slot(&name)))
        .into_element();
    let mut ends = Vec::new();
    if editable && !editing {
        let edit_name = name.clone();
        let current = row.model.clone();
        ends.push(button_with(
            app,
            "Edit model",
            ButtonKind::Quiet,
            ButtonOptions::default(),
            format!("btn-edit-model-{name}"),
            move || {
                let profile = edit_name.clone();
                let model = current.clone();
                app.act(app.settings, move |state, _| {
                    state.edit_model(profile.clone(), model.clone())
                });
            },
        ));
        if let Some(label) = row.remove {
            let settings = settings.clone();
            let profile = name.clone();
            ends.push(button_with(
                app,
                label,
                ButtonKind::Ghost,
                ButtonOptions {
                    danger: true,
                    ..ButtonOptions::default()
                },
                format!("btn-remove-{name}"),
                move || {
                    let settings = settings.clone();
                    let profile = profile.clone();
                    app.reduce(app.settings, move |state, feed, _, _| {
                        state.remove_configured_profile(&settings, feed, &profile)
                    });
                },
            ));
        }
    }
    vec![
        lead,
        middle,
        view()
            .flex_1()
            .flex_row()
            .justify_end()
            .child(button_row(app, ends))
            .into_element(),
    ]
}

/// The model draft of the profile being edited, with Save and Cancel.
fn model_editor(app: AppState, settings: &SettingsSnapshot, name: &str) -> AnyElement {
    let save_settings = settings.clone();
    view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(6.0))
        .child(bound_field(
            app,
            format!("model-{name}"),
            "Model",
            "model id",
            move || {
                app.settings.with(|state| {
                    state
                        .edits
                        .model
                        .as_ref()
                        .map(|(_, draft)| draft.clone())
                        .unwrap_or_default()
                })
            },
            move |text| {
                app.act(app.settings, move |state, _| {
                    state.draft_model(text.clone())
                })
            },
        ))
        .child(button_row(
            app,
            vec![
                button_with(
                    app,
                    "Save",
                    ButtonKind::Primary,
                    ButtonOptions::default(),
                    format!("btn-save-model-{name}"),
                    move || {
                        let settings = save_settings.clone();
                        app.act(app.settings, move |state, _| state.save_model(&settings));
                    },
                ),
                button_with(
                    app,
                    "Cancel",
                    ButtonKind::Quiet,
                    ButtonOptions::default(),
                    format!("btn-cancel-model-{name}"),
                    move || app.act(app.settings, |state, _| state.cancel_model()),
                ),
            ],
        ))
        .into_element()
}

/// Adding a profile: closed behind "Add a profile…", or the form naming the copy, until
/// corrald has added it.
fn add(app: AppState, settings: SettingsSnapshot) -> AnyElement {
    let open = computed(move || {
        let (open, key) = app.settings.with(|state| {
            (
                state.edits.adding.is_some(),
                state.edits.acts.get(ADD_PROFILE).cloned(),
            )
        });
        open && key.and_then(|key| app.act_status(&key)) != Some(ActStatus::Done)
    });
    dynamic(move || {
        let open = open.get();
        let settings = settings.clone();
        untracked(move || {
            if open {
                add_form(app, settings)
            } else {
                add_closed(app, settings)
            }
        })
    })
    .into_element()
}

fn add_closed(app: AppState, settings: SettingsSnapshot) -> AnyElement {
    let none = settings.profiles.is_empty();
    view()
        .gap(px(6.0))
        .child(button_row(
            app,
            vec![button_with(
                app,
                "Add a profile…",
                ButtonKind::Ghost,
                ButtonOptions {
                    disabled: none,
                    ..ButtonOptions::default()
                },
                "btn-add-profile",
                move || {
                    let settings = settings.clone();
                    app.reduce(app.settings, move |state, feed, _, _| {
                        state.open_add(&settings, feed)
                    });
                },
            )],
        ))
        .child(act_line(app, ADD_PROFILE))
        .into_element()
}

fn add_field(
    app: AppState,
    field: ProfileField,
    label: &'static str,
    placeholder: &'static str,
) -> AnyElement {
    bound_field(
        app,
        format!("profile-{}", field.slug()),
        label,
        placeholder,
        move || {
            app.settings.with(|state| {
                state
                    .edits
                    .adding
                    .as_ref()
                    .map(|form| form.field(field).to_owned())
                    .unwrap_or_default()
            })
        },
        move |text| {
            app.act(app.settings, move |state, _| {
                state.add_field(field, text.clone())
            })
        },
    )
}

fn add_form(app: AppState, settings: SettingsSnapshot) -> AnyElement {
    let rows = profiles::managed(&settings);
    let names: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    let from = computed(move || {
        app.settings
            .with(|state| state.edits.adding.as_ref().map(|form| form.from.clone()))
            .unwrap_or_default()
    });
    let copied = dynamic(move || {
        let from = from.get();
        let line = rows
            .iter()
            .find(|row| row.name == from)
            .map(|row| format!("Copies {}: {}", row.name, row.setup));
        untracked(move || line.map(|line| mono_cell(app, line, Token::Ink3).into_element()))
    })
    .into_element();
    let submit_settings = settings.clone();
    let state = computed(move || {
        let problem = app.settings.with(|state| {
            state
                .edits
                .adding
                .as_ref()
                .and_then(|form| form.problem(&settings))
        });
        (problem, in_flight(app, ADD_PROFILE))
    });
    let actions = dynamic(move || {
        let (problem, busy) = state.get();
        let submit_settings = submit_settings.clone();
        untracked(move || {
            view()
                .gap(px(6.0))
                .children(
                    problem
                        .clone()
                        .map(|line| cell(app, line, Token::Ink3, false).into_element()),
                )
                .child(button_row(
                    app,
                    vec![
                        button_with(
                            app,
                            "Preview add",
                            ButtonKind::Primary,
                            ButtonOptions {
                                disabled: problem.is_some() || busy,
                                ..ButtonOptions::default()
                            },
                            "btn-add-profile-preview",
                            move || {
                                let settings = submit_settings.clone();
                                app.reduce(app.settings, move |state, feed, _, _| {
                                    state.submit_add(&settings, feed)
                                });
                            },
                        ),
                        button_with(
                            app,
                            "Close",
                            ButtonKind::Quiet,
                            ButtonOptions::default(),
                            "btn-add-profile-close",
                            move || app.act(app.settings, |state, _| state.close_add()),
                        ),
                    ],
                ))
                .child(act_line(app, ADD_PROFILE))
                .into_element()
        })
    })
    .into_element();
    group(
        app,
        vec![
            cell(app, ADD_NOTE, Token::Ink3, false).into_element(),
            view()
                .min_w(px(0.0))
                .gap(px(4.0))
                .child(cell(app, "Copy of", Token::Ink3, false))
                .child(source_picker(app, names))
                .child(copied)
                .into_element(),
            add_field(
                app,
                ProfileField::Name,
                "New profile name",
                "letters, digits, - and _",
            ),
            add_field(app, ProfileField::Model, "Model", "keep the copied model"),
            add_field(
                app,
                ProfileField::Effort,
                "Effort",
                "keep the copied effort",
            ),
            actions,
        ],
    )
}

/// The configured profile the new one copies: one pressed at a time, wrapping as the names
/// need (the segmented control's look, for names only corrald knows).
fn source_picker(app: AppState, names: Vec<String>) -> AnyElement {
    let group = toggle_group().flex_row().flex_wrap().items_center();
    let group = match app.form {
        Form::Terminal => group.gap(px(8.0)),
        Form::Desktop => group
            .self_start()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(7.0))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Rule)),
        Form::Phone => group
            .self_start()
            .gap(px(4.0))
            .p(px(3.0))
            .rounded(px(9.0))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Rule)),
    };
    group
        .children(names.into_iter().map(|name| source_option(app, name)))
        .into_element()
}

fn source_option(app: AppState, name: String) -> AnyElement {
    let form = app.form;
    let mine = name.clone();
    let on = computed(move || {
        app.settings
            .with(|state| state.edits.adding.as_ref().is_some_and(|f| f.from == mine))
    });
    let on = move || on.get();
    let (height, size, radius, pad) = match form {
        Form::Desktop => (px(24.0), 12.0, 5.0, 10.0),
        // 40 pt with 2 pt of slop above and below reaches the 44 pt target without reaching
        // into the next wrapped row.
        Form::Phone => (px(40.0), 13.5, 7.0, 12.0),
        Form::Terminal => (term::rows(1.0), text_size(form), 0.0, 8.0),
    };
    let words = text(name.clone())
        .number_of_lines(1u32)
        .text_color(move || {
            if on() || form == Form::Terminal {
                color(Token::Ink)
            } else {
                color(Token::Ink3)
            }
        })
        .style_when(on, |s| s.font_weight(FontWeight::BOLD))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT).text_size(px(size))
        });
    let pick = name.clone();
    toggle()
        .pressed(on)
        .on_change(move |_: &bool| {
            let from = pick.clone();
            app.act(app.settings, move |state, _| state.add_source(from.clone()));
        })
        .h(height)
        .px(px(pad))
        .rounded(px(radius))
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(form == Form::Phone, |t| {
            t.hit_slop(EdgeInsets {
                top: px(2.0),
                right: px(0.0),
                bottom: px(2.0),
                left: px(0.0),
            })
        })
        // In a terminal NSH bands a pressed toggle itself; a fill of the app's would be a second.
        .when(form != Form::Terminal, |t| {
            t.border_1()
                .bg(move || {
                    if on() {
                        color(Token::Surface)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .border_color(move || {
                    if on() {
                        color(Token::Rule2)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .hover(|s| s.bg(color(Token::Hover)))
                .focus_visible(ring)
        })
        .child(view().test_id(&format!("profile-from-{name}")).child(words))
        .into_element()
}
