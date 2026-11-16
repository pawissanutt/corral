//! Settings › Autopilot & grants in a daemon session: the operating mode and configured grants
//! as corrald reports them, the mode changes that apply now, and the finite autopilot grant.
//! Every change is an owner-only `mode.*` command, previewed by corrald and confirmed on this
//! device; a device without admin authority reads the section and is told why it cannot act.

use nsh_base::toggle;
use nsh_core::prelude::*;

use super::enrolment::{bound_field, group};
use super::live_sections::{act_line, facts, refusal, snapshot_section};
use super::parts::{button_row, cell, first_heading, mono_cell, ring, space};
use crate::model::settings::autopilot::{self, Field, ModeVerb, Power, ProjectPicker};
use crate::model::settings::live;
use crate::model::{Form, Hue, Id};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::page::Width;
use crate::ui::kit::table::qtable;
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

const GRANT_NOTE: &str = "corrald previews the exact tasks, projects and authority a grant gives; you confirm that preview on this device. Nothing is granted to all work.";

pub fn page(app: AppState) -> AnyElement {
    snapshot_section(app, "Autopilot & grants", |app, settings, source| {
        let mut parts: Vec<AnyElement> = refusal(app, source).into_iter().collect();
        if source.refusal.is_some() {
            parts.push(space(app, 10.0));
        }
        let mut rows = vec![("Operating mode", live::mode_line(settings))];
        rows.extend(live::switches(settings));
        parts.push(facts(app, rows));
        parts.push(space(app, 12.0));
        parts.push(first_heading(app, "Configured grants", None));
        parts.push(grants(app, live::grants(settings)));
        if source.editable() {
            // Assist has no switch of its own: the way out of it is the grant below.
            let verbs_now = autopilot::verbs(settings);
            if !verbs_now.is_empty() {
                parts.push(h3(app, "Change the mode", None));
                parts.push(verbs(app, verbs_now));
            }
            parts.push(h3(app, "Grant autopilot", None));
            parts.push(grant(app));
        }
        parts
    })
}

fn grants(app: AppState, rows: Vec<live::GrantView>) -> AnyElement {
    if rows.is_empty() {
        return cell(
            app,
            "No automation grants are configured.",
            Token::Ink3,
            false,
        )
        .into_element();
    }
    let widths: &[Width] = match app.form {
        Form::Desktop => &[
            Width::Fill,
            Width::Fixed(64.0),
            Width::Fill,
            Width::Fixed(170.0),
        ],
        Form::Phone | Form::Terminal => {
            &[Width::Fill, Width::Fixed(48.0), Width::Fill, Width::Fill]
        }
    };
    qtable(
        app,
        widths,
        rows.into_iter()
            .map(|row| {
                vec![
                    cell(app, row.name, Token::Ink, true).into_element(),
                    chip(app, row.chip.0, row.chip.1),
                    mono_cell(app, row.scope, Token::Ink3).into_element(),
                    mono_cell(app, row.expires, Token::Ink3).into_element(),
                ]
            })
            .collect(),
    )
}

/// One button per change that applies, each with what it does and its command's status.
fn verbs(app: AppState, verbs: Vec<ModeVerb>) -> AnyElement {
    let rows = verbs
        .into_iter()
        .map(|verb| {
            let press = button_with(
                app,
                verb.label(),
                ButtonKind::Ghost,
                ButtonOptions {
                    danger: verb.hue() == Hue::Fail,
                    ..ButtonOptions::default()
                },
                format!("btn-mode-{}", verb.slug()),
                move || app.act(app.settings, move |state, _| state.change_mode(verb)),
            );
            vec![
                view().flex_row().child(press).into_element(),
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(cell(app, verb.meaning(), Token::Ink3, false))
                    .child(act_line(app, format!("mode-{}", verb.slug())))
                    .into_element(),
            ]
        })
        .collect();
    let widths: &[Width] = match app.form {
        Form::Desktop => &[Width::Fixed(240.0), Width::Fill],
        Form::Phone | Form::Terminal => &[Width::Fill, Width::Fill],
    };
    qtable(app, widths, rows)
}

/// An on/off switch shown as the on or off chip, read live.
fn switch(
    app: AppState,
    label: &'static str,
    id: String,
    on: impl Fn() -> bool + Copy + 'static,
    flip: impl Fn() + 'static,
) -> AnyElement {
    let face = dynamic(move || {
        let on = on();
        untracked(move || {
            chip(
                app,
                if on { "on" } else { "off" },
                if on { Hue::Ok } else { Hue::Neutral },
            )
        })
    });
    let control = toggle()
        .pressed(on)
        .on_change(move |_: &bool| flip())
        .accessibility_label(label)
        .flex_row()
        .items_center()
        .cursor_pointer()
        .when(app.form != Form::Terminal, |t| {
            t.rounded(px(4.0)).focus_visible(ring)
        })
        .when(app.form == Form::Phone, |t| t.min_h(px(44.0)))
        .child(view().test_id(&id).child(face));
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(control)
        .child(cell(app, label, Token::Ink, false))
        .into_element()
}

fn text_field(
    app: AppState,
    field: Field,
    label: &'static str,
    placeholder: &'static str,
) -> AnyElement {
    let slug = match field {
        Field::Projects => "projects",
        Field::Tasks => "tasks",
        Field::Plans => "plans",
        Field::Exclusions => "exclusions",
        Field::ExpiresDays => "expires",
        Field::MaxNew => "max-new",
    };
    bound_field(
        app,
        format!("grant-{slug}"),
        label,
        placeholder,
        move || {
            app.settings
                .with(|state| state.edits.grant.field(field).to_owned())
        },
        move |text| {
            app.act(app.settings, move |state, _| {
                state.grant_field(field, text.clone())
            })
        },
    )
}

/// The projects a grant names: a toggle for each project corrald lists, or, while it lists none
/// (an older corrald), the free-text field. Rebuilt only when that choice flips, so a field
/// being typed in keeps its focus.
fn projects_field(app: AppState) -> AnyElement {
    let picker = computed(move || {
        app.settings
            .with(|state| app.feed.with(|feed| state.edits.grant.project_picker(feed)))
    });
    let known = computed(move || picker.with(|picker| matches!(picker, ProjectPicker::Known(_))));
    dynamic(move || {
        if known.get() {
            untracked(|| project_toggles(app, picker))
        } else {
            untracked(|| {
                text_field(
                    app,
                    Field::Projects,
                    "Projects",
                    "project names, comma separated",
                )
            })
        }
    })
    .into_element()
}

/// The known projects as toggles under the field's label, wrapping. test_id "opt-project-<id>".
fn project_toggles(app: AppState, picker: Memo<ProjectPicker>) -> AnyElement {
    let ids = computed(move || {
        picker.with(|picker| match picker {
            ProjectPicker::Known(options) => options.iter().map(|o| o.id.clone()).collect(),
            ProjectPicker::FreeText => Vec::new(),
        })
    });
    let terminal = app.form == Form::Terminal;
    view()
        .min_w(px(0.0))
        .gap(px(4.0))
        .child(cell(app, "Projects", Token::Ink3, false))
        .child(dynamic(move || {
            let ids: Vec<Id> = ids.get();
            untracked(move || {
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(if terminal { term::cells(2.0) } else { px(6.0) })
                    .children(ids.into_iter().map(|id| project_toggle(app, picker, id)))
                    .into_element()
            })
        }))
        .into_element()
}

/// One project: pressed while the grant names it. The fill, the stronger outline and the
/// heavier words say so without colour; a terminal reads `[x]` and bands the pressed toggle.
fn project_toggle(app: AppState, picker: Memo<ProjectPicker>, id: Id) -> AnyElement {
    let form = app.form;
    let probe = id.clone();
    let on = computed(move || {
        picker.with(|picker| match picker {
            ProjectPicker::Known(options) => options.iter().any(|o| o.id == probe && o.selected),
            ProjectPicker::FreeText => false,
        })
    });
    let name = id.to_string();
    let words = text(move || {
        let mark = match (form, on.get()) {
            (Form::Terminal, true) => "[x] ",
            (Form::Terminal, false) => "[ ] ",
            (_, _) => "",
        };
        SharedString::from(format!("{mark}{name}"))
    })
    .number_of_lines(1u32)
    .text_color(move || color(if on.get() { Token::Ink } else { Token::Ink2 }));
    let face = view()
        .test_id(&format!("opt-project-{id}"))
        .flex_row()
        .items_center();
    let face = if form == Form::Terminal {
        face.child(words)
    } else {
        face.child(
            words
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 15.0 } else { 13.0 }))
                .style_when(move || on.get(), |s| s.font_weight(FontWeight::SEMIBOLD)),
        )
    };
    let label = id.to_string();
    toggle()
        .pressed(move || on.get())
        .on_change(move |_: &bool| {
            let id = id.clone();
            app.reduce(app.settings, move |state, feed, _, _| {
                state.grant_toggle_project(feed, id)
            })
        })
        .accessibility_label(label)
        .flex_row()
        .items_center()
        .cursor_pointer()
        .when(form != Form::Terminal, |t| {
            t.h(px(if form == Form::Phone { 44.0 } else { 28.0 }))
                .px(px(10.0))
                .rounded(px(if form == Form::Phone { 10.0 } else { 6.0 }))
                .border_1()
                .border_color(move || color(if on.get() { Token::Ink3 } else { Token::Rule2 }))
                .bg(move || {
                    if on.get() {
                        color(Token::Surface)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .hover(|s| s.bg(color(Token::Hover)))
                .focus_visible(ring)
        })
        .child(face)
        .into_element()
}

/// The grant: closed behind "Write a grant…", or its form with what corrald will be asked for.
fn grant(app: AppState) -> AnyElement {
    let open = computed(move || app.settings.with(|state| state.edits.granting));
    dynamic(move || {
        let open = open.get();
        untracked(move || {
            if !open {
                return view()
                    .gap(px(6.0))
                    .child(button_row(
                        app,
                        vec![button_with(
                            app,
                            "Write a grant…",
                            ButtonKind::Ghost,
                            ButtonOptions::default(),
                            "btn-grant-open",
                            move || app.act(app.settings, |state, _| state.open_grant(true)),
                        )],
                    ))
                    .child(act_line(app, "grant"))
                    .into_element();
            }
            let mut parts = vec![
                cell(app, GRANT_NOTE, Token::Ink3, false).into_element(),
                projects_field(app),
                text_field(app, Field::Tasks, "Tasks", "T-ids, comma separated"),
                text_field(app, Field::Plans, "Plans", "plan task ids, comma separated"),
                text_field(app, Field::Exclusions, "Except", "T-ids to leave out"),
                text_field(app, Field::ExpiresDays, "Expires after (days)", "no expiry"),
                text_field(app, Field::MaxNew, "At most new tasks", "no limit"),
                switch(
                    app,
                    "Include tasks created later in these projects",
                    "btn-grant-include-new".into(),
                    move || app.settings.with(|state| state.edits.grant.include_new),
                    move || app.act(app.settings, |state, _| state.grant_include_new()),
                ),
            ];
            for power in Power::ALL {
                parts.push(switch(
                    app,
                    power.label(),
                    format!("btn-grant-{}", power.slug()),
                    move || app.settings.with(|state| state.edits.grant.power(power)),
                    move || app.act(app.settings, move |state, _| state.grant_power(power)),
                ));
            }
            let problem = computed(move || {
                app.settings
                    .with(|state| app.feed.with(|feed| state.edits.grant.problem_for(feed)))
            });
            parts.push(
                dynamic(move || {
                    let problem = problem.get();
                    untracked(move || {
                        view()
                            .gap(px(6.0))
                            .children(
                                problem
                                    .map(|line| cell(app, line, Token::Ink3, false).into_element()),
                            )
                            .child(button_row(
                                app,
                                vec![
                                    button_with(
                                        app,
                                        "Preview grant",
                                        ButtonKind::Primary,
                                        ButtonOptions {
                                            disabled: problem.is_some(),
                                            ..ButtonOptions::default()
                                        },
                                        "btn-grant-preview",
                                        move || {
                                            app.reduce(app.settings, |state, feed, _, _| {
                                                state.submit_grant(feed)
                                            })
                                        },
                                    ),
                                    button_with(
                                        app,
                                        "Close",
                                        ButtonKind::Quiet,
                                        ButtonOptions::default(),
                                        "btn-grant-close",
                                        move || {
                                            app.act(app.settings, |state, _| {
                                                state.open_grant(false)
                                            })
                                        },
                                    ),
                                ],
                            ))
                            .child(act_line(app, "grant"))
                            .into_element()
                    })
                })
                .into_element(),
            );
            group(app, parts)
        })
    })
    .into_element()
}
