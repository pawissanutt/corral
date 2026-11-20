//! Settings › Configuration: the `corral.toml` editor. The page reads the file once it opens,
//! shows one part at a time (Workspace, Projects, Profiles, Harnesses, Routing, Artifacts, or
//! the whole File as text), and keeps a save bar pinned under it: what is unsaved, Save,
//! Discard and Read again. A refused save leaves the draft and lists its reasons at the top, each
//! naming a field a button to it (a syntax error's to File), and on the field itself. Over them
//! the page says when the file changed elsewhere since it was read (with Read again) and when
//! the file on disk does not parse (opening File). IDs: `tab-config-<n>` for the parts,
//! `seg-config-<part>-<name>` for an entry, `btn-config-save`, `btn-config-discard`,
//! `btn-config-reload`, `btn-config-reread`, `btn-config-reason-<n>`, `field-config-file`.

use nsh_core::prelude::*;

use super::config_fields::{edit, entry, field};
use super::parts::{cell, frame, space};
use super::{config_profiles, config_routing};

mod harnesses;
use crate::model::Form;
use crate::model::settings::document::Path;
use crate::model::settings::editor::{self, Edit, Part};
use crate::model::settings::form::{self, Entries, Group, Live, Shape};
use crate::model::text::span;
use crate::screens::{PhoneBar, PhonePage};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with, tab_strip};
use crate::ui::kit::notice::error;
use crate::ui::kit::page::{body, measured_bar};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, MEASURE_FORM, Token, color};

const PARTS: [(Part, &str); 7] = [
    (Part::Workspace, "Workspace"),
    (Part::Projects, "Projects"),
    (Part::Profiles, "Profiles"),
    (Part::Harnesses, "Harnesses"),
    (Part::Routing, "Routing"),
    (Part::Artifacts, "Artifacts"),
    (Part::File, "File"),
];

/// The save bar's measure: the section's, with the 24 pt the bar and the body each pad their
/// sides with, so Save stands under the section's left edge.
const BAR_MEASURE: f32 = MEASURE_FORM + 2.0 * 24.0;

/// Desktop and terminal: the scrolling editor over its pinned save bar.
pub fn screen(app: AppState) -> AnyElement {
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(body(app, "settings-body", content(app)))
        .child(save_bar(app))
        .into_element()
}

/// The phone's pushed Configuration page, its save bar in place of the tabs.
pub fn phone(app: AppState) -> PhonePage {
    PhonePage {
        title: "Configuration".into(),
        body: content(app),
        trailing: None,
        bar: Some(PhoneBar {
            shown: computed(move || app.settings.with(|state| state.config.saved.is_some())),
            view: save_bar(app),
        }),
        ask: false,
        scroll: true,
    }
}

/// Reads the file whenever the editor has none, is not reading it, and has no problem to show;
/// after a failure, Read again asks once more. A file on disk that newly fails to parse opens
/// the plain-text view.
fn open(app: AppState) {
    let wanted = computed(move || {
        app.settings.with(|state| {
            let editor = &state.config;
            editor.saved.is_none() && editor.waiting.is_none() && editor.problem.is_none()
        })
    });
    effect(move || {
        if wanted.get() {
            untracked(|| app.act(app.settings, |state, _| state.open_config()));
        }
    });
    let disk = computed(move || {
        app.feed
            .with(|feed| feed.settings.as_ref().and_then(|s| s.problem.clone()))
    });
    effect(move || {
        let problem = disk.get();
        untracked(|| app.act(app.settings, move |state, _| state.disk_problem(problem)));
    });
}

fn content(app: AppState) -> AnyElement {
    open(app);
    let live = computed(move || app.settings.with(|state| Live::of(&state.config)));
    let shape = computed(move || {
        app.settings.with(|state| {
            (
                state.config.part,
                app.feed.with(|feed| form::shape(&state.config, feed)),
            )
        })
    });
    let loaded = computed(move || app.settings.with(|state| state.config.saved.is_some()));
    let parts = dynamic(move || {
        if !loaded.get() {
            return view().into_element();
        }
        untracked(move || {
            view()
                .min_w(px(0.0))
                .child(tab_strip(
                    app,
                    &PARTS,
                    move || app.settings.with(|state| state.config.part),
                    move |part| app.act(app.settings, move |state, _| state.config_part(part)),
                    "tab-config",
                ))
                .child(space(app, 12.0))
                .child(dynamic(move || {
                    let (selected, shape) = shape.get();
                    untracked(move || part(app, selected, shape, live))
                }))
                .into_element()
        })
    });
    frame(app, "Configuration", 12.0, vec![notices(app), parts])
}

/// The file on disk not parsing, an edit made elsewhere, why the file is not read or the last
/// save failed, and a refusal's reasons.
fn notices(app: AppState) -> AnyElement {
    let shown = computed(move || {
        app.settings.with(|state| {
            app.feed.with(|feed| {
                (
                    feed.settings
                        .as_ref()
                        .and_then(|s| s.problem.as_ref())
                        .map(editor::on_disk),
                    state
                        .config
                        .changed_elsewhere(feed)
                        .then(|| editor::changed_elsewhere_notice(state.config.dirty())),
                    state.config.problem.clone(),
                    state.config.reasons.clone(),
                    state.config.saved.is_some(),
                )
            })
        })
    });
    dynamic(move || {
        let (disk, elsewhere, problem, reasons, loaded) = shown.get();
        untracked(move || {
            let mut parts = Vec::new();
            if let Some(disk) = disk {
                parts.push(error(app, &vec![span(disk)], None));
                parts.push(space(app, 10.0));
            }
            if let Some(notice) = elsewhere {
                let again = button(
                    app,
                    "Read again",
                    ButtonKind::Ghost,
                    "btn-config-reread",
                    move || {
                        app.act(app.settings, |state, _| state.reread_config());
                    },
                );
                parts.push(error(app, &vec![span(notice)], Some(again)));
                parts.push(space(app, 10.0));
            }
            if let Some(problem) = problem {
                let again = (!loaded).then(|| {
                    button(
                        app,
                        "Read again",
                        ButtonKind::Ghost,
                        "btn-config-read",
                        move || {
                            app.act(app.settings, |state, _| state.reload_config());
                        },
                    )
                });
                parts.push(error(app, &vec![span(problem)], again));
                parts.push(space(app, 10.0));
            }
            if !reasons.is_empty() {
                let mut lines: Vec<AnyElement> =
                    vec![cell(app, form::REFUSED, Token::Ink, true).into_element()];
                for (at, reason) in reasons.into_iter().enumerate() {
                    lines.push(match (reason.at, reason.position) {
                        (Some(path), _) => reason_button(app, at, Some(path), reason.line),
                        (None, Some(_)) => reason_button(app, at, None, reason.line),
                        (None, None) => cell(app, reason.line, Token::Fail, false).into_element(),
                    });
                }
                parts.push(
                    view()
                        .test_id("config-refused")
                        .min_w(px(0.0))
                        .gap(px(4.0))
                        .children(lines)
                        .into_element(),
                );
                parts.push(space(app, 10.0));
            }
            view().min_w(px(0.0)).children(parts).into_element()
        })
    })
    .into_element()
}

/// A reason with a place: on a key, where it is and what is wrong, pressing it shows that
/// field; at a line and column, pressing it shows the File.
fn reason_button(app: AppState, at: usize, path: Option<Path>, line: String) -> AnyElement {
    let label = match &path {
        Some(path) => format!("{}: {line}", form::place(path)),
        None => line,
    };
    view()
        .flex_row()
        .child(button(
            app,
            label,
            ButtonKind::Quiet,
            format!("btn-config-reason-{at}"),
            move || {
                let path = path.clone();
                app.act(app.settings, move |state, _| match &path {
                    Some(path) => state.show_config(path),
                    None => state.config_part(Part::File),
                });
            },
        ))
        .into_element()
}

fn part(app: AppState, selected: Part, shape: Shape, live: Memo<Live>) -> AnyElement {
    match shape {
        Shape::File => file(app),
        Shape::Harnesses(configured) => harnesses::catalog(app, &configured, live),
        Shape::Unparsed(problem) => view()
            .min_w(px(0.0))
            .child(error(
                app,
                &vec![
                    span("The draft is not valid TOML, so the form cannot show it: "),
                    span(problem).mono(),
                ],
                Some(button(
                    app,
                    "Open File",
                    ButtonKind::Ghost,
                    "btn-config-file",
                    move || {
                        app.act(app.settings, |state, _| state.config_part(Part::File));
                    },
                )),
            ))
            .into_element(),
        Shape::Form {
            note,
            entries: listed,
            groups,
            empty,
        } => {
            if selected == Part::Profiles
                && let Some(listed) = &listed
            {
                return config_profiles::profiles(app, listed, groups.first(), live);
            }
            if selected == Part::Routing {
                return config_routing::routing(app, &groups, live);
            }
            let mut parts = Vec::new();
            if let Some(note) = note {
                parts.push(cell(app, note, Token::Ink3, false).into_element());
                parts.push(space(app, 10.0));
            }
            if let Some(listed) = &listed {
                parts.push(entries(app, listed));
                parts.push(space(app, 12.0));
            }
            if groups.is_empty()
                && let Some(empty) = empty
            {
                parts.push(cell(app, empty, Token::Ink3, false).into_element());
            }
            for group_shape in groups {
                parts.push(group(app, group_shape, live));
            }
            view().min_w(px(0.0)).children(parts).into_element()
        }
    }
}

/// The entries of a listing part, one pressed; a new name with Add; Remove for the shown one;
/// a refusal's reasons about the shown entry itself.
fn entries(app: AppState, listed: &Entries) -> AnyElement {
    let part = listed.part;
    let slug = part.label().to_lowercase();
    let mut buttons: Vec<AnyElement> = listed
        .names
        .iter()
        .map(|name| {
            let chosen = listed.chosen.as_deref() == Some(name.as_str());
            let pick = name.clone();
            button(
                app,
                name.clone(),
                if chosen {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Ghost
                },
                format!("seg-config-{slug}-{name}"),
                move || {
                    let pick = pick.clone();
                    app.act(app.settings, move |state, _| state.choose_entry(part, pick));
                },
            )
        })
        .collect();
    let name = signal(String::new());
    let noun = listed.noun;
    buttons.push(entry(
        app,
        format!("field-config-new-{slug}"),
        format!("New {noun}"),
        move || name.get(),
        move |text| name.set(text),
    ));
    let add_id = format!("btn-config-add-{slug}");
    buttons.push(
        dynamic(move || {
            let disabled = name.with(|name| name.trim().is_empty());
            let add_id = add_id.clone();
            untracked(move || {
                button_with(
                    app,
                    format!("Add {noun}"),
                    ButtonKind::Ghost,
                    ButtonOptions {
                        disabled,
                        ..ButtonOptions::default()
                    },
                    add_id,
                    move || {
                        edit(app, Edit::AddEntry(part, name.get_untracked()));
                        name.set(String::new());
                    },
                )
            })
        })
        .into_element(),
    );
    let mut parts = vec![
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .min_w(px(0.0))
            .role(AccessibilityRole::Radiogroup)
            .children(buttons)
            .into_element(),
    ];
    if let (Some(chosen), Some(root)) = (listed.chosen.clone(), part.root()) {
        let anchor = Path::of(&[root, &chosen]);
        let reasons = computed(move || {
            app.settings.with(|state| {
                state
                    .config
                    .reasons_at(&anchor)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
        });
        let removed = chosen.clone();
        parts.push(space(app, 8.0));
        parts.push(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .min_w(px(0.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(h3(app, chosen.clone(), None)),
                )
                .child(button_with(
                    app,
                    format!("Remove {noun}"),
                    ButtonKind::Ghost,
                    ButtonOptions {
                        danger: true,
                        ..ButtonOptions::default()
                    },
                    format!("btn-config-remove-{slug}"),
                    move || edit(app, Edit::RemoveEntry(part, removed.clone())),
                ))
                .into_element(),
        );
        parts.push(
            dynamic(move || {
                let reasons = reasons.get();
                untracked(move || {
                    view()
                        .children(
                            reasons
                                .into_iter()
                                .map(|reason| cell(app, reason, Token::Fail, false).into_element()),
                        )
                        .into_element()
                })
            })
            .into_element(),
        );
    }
    view().min_w(px(0.0)).children(parts).into_element()
}

/// A group's heading, then its fields a row apart.
fn group(app: AppState, group: Group, live: Memo<Live>) -> AnyElement {
    let mut parts = Vec::new();
    if let Some(title) = group.title {
        parts.push(h3(app, title, None));
    }
    let gap = if app.form == Form::Terminal {
        term::rows(1.0)
    } else {
        px(14.0)
    };
    parts.push(
        view()
            .min_w(px(0.0))
            .gap(gap)
            .children(group.fields.iter().map(|shape| field(app, shape, live)))
            .into_element(),
    );
    view().min_w(px(0.0)).children(parts).into_element()
}

/// The whole file as text.
fn file(app: AppState) -> AnyElement {
    let form = app.form;
    let field = nsh_core::element::input()
        .test_id("field-config-file")
        .label("corral.toml")
        .multiline(true)
        .submit_behavior(SubmitBehavior::Newline)
        .value(move || app.settings.with(|state| state.config.draft.clone()))
        .on_change(move |text: &str| edit(app, Edit::File(text.to_owned())))
        .auto_correct(false)
        .auto_capitalize(AutoCapitalize::None)
        .min_w(px(0.0))
        .w_full()
        .text_color(color(Token::Ink))
        .bg(AdaptiveColor::TRANSPARENT)
        .border_1()
        .border_color(color(Token::Rule2))
        .focus(|s| s.border_color(color(Token::Focus)));
    let field = match form {
        Form::Terminal => field.min_h(term::rows(20.0)).px(term::cells(1.0)),
        Form::Desktop => field
            .font_family(FONT_MONO)
            .text_size(px(12.5))
            .min_h(px(480.0))
            .p(px(10.0))
            .rounded(px(6.0)),
        Form::Phone => field
            .font_family(FONT_MONO)
            .text_size(px(14.0))
            .min_h(px(360.0))
            .p(px(10.0))
            .rounded(px(10.0)),
    };
    view()
        .min_w(px(0.0))
        .gap(px(8.0))
        .child(cell(app, form::FILE_NOTE, Token::Ink3, false))
        .child(field)
        .into_element()
}

/// The save bar: Save, Discard and Read again, with what is unsaved beside them.
fn save_bar(app: AppState) -> AnyElement {
    let state = computed(move || {
        app.settings.with(|state| {
            let editor = &state.config;
            let live = Live::of(editor);
            (
                editor.dirty(),
                editor.waiting.is_some(),
                editor.saved.is_some(),
                form::status(editor, &live),
            )
        })
    });
    dynamic(move || {
        let (dirty, waiting, loaded, status) = state.get();
        untracked(move || {
            let wide = app.form == Form::Phone;
            let action = |label: &'static str,
                          kind: ButtonKind,
                          id: &'static str,
                          disabled: bool,
                          run: fn(AppState)| {
                button_with(
                    app,
                    label,
                    kind,
                    ButtonOptions {
                        disabled,
                        wide,
                        // Ctrl+S saves from any field (`state::keys`); the terminal shows it.
                        key_hint: (id == "btn-config-save" && app.form == Form::Terminal)
                            .then_some("^S"),
                        ..ButtonOptions::default()
                    },
                    id,
                    move || run(app),
                )
            };
            let actions = vec![
                action(
                    "Save",
                    ButtonKind::Primary,
                    "btn-config-save",
                    !dirty || waiting,
                    |app| app.act(app.settings, |state, _| state.save_config()),
                ),
                action(
                    "Discard",
                    ButtonKind::Ghost,
                    "btn-config-discard",
                    !dirty || waiting,
                    |app| app.act(app.settings, |state, _| state.discard_config()),
                ),
                action(
                    "Read again",
                    ButtonKind::Quiet,
                    "btn-config-reload",
                    dirty || waiting || !loaded,
                    |app| app.act(app.settings, |state, _| state.reload_config()),
                ),
            ];
            let words = text(status)
                .test_id("config-status")
                .number_of_lines(1u32)
                .text_color(color(if dirty { Token::Ink } else { Token::Ink3 }));
            measured_bar(app, BAR_MEASURE, actions, Some(words.into_element()))
        })
    })
    .into_element()
}
