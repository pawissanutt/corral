//! The Harnesses catalog and the selected harness's setup. The catalog stays mounted when
//! selection changes, and editing a command updates live facts without replacing its field.

use nsh_core::prelude::*;

use super::super::config_fields::{edit, entry, field};
use super::super::parts::{button_row, cell, mono_cell, space};
use crate::model::Form;
use crate::model::settings::document::Path;
use crate::model::settings::editor::{Edit, Part};
use crate::model::settings::form::{Live, harness_fields};
use crate::model::settings::harnesses::{
    self, Details, Detection, Entry, Facts, Profile, Template,
};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::rows::list_row_tall;
use crate::ui::kit::text::h3_flush;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

pub(super) fn catalog(app: AppState, configured: &[String], live: Memo<Live>) -> AnyElement {
    let entries = harnesses::entries(configured);
    let chosen = computed({
        let entries = entries.clone();
        move || {
            app.settings.with(|state| {
                harnesses::selected(&entries, harnesses::choice(&state.config)).cloned()
            })
        }
    });
    let facts = computed(move || {
        app.settings
            .with(|state| app.feed.with(|feed| Details::read(&state.config, feed)))
    });
    let width = signal(None::<f32>);
    let beside = computed(move || match (app.form, width.get()) {
        (Form::Phone, _) => false,
        (Form::Desktop, width) => width.is_none_or(|width| width >= 675.0),
        (Form::Terminal, width) => width.map_or_else(
            || term::page_columns(app) >= 77.0,
            |width| width >= term::cells(77.0).0,
        ),
    });
    let configured = configured.to_vec();
    let layout = dynamic(move || {
        let beside = beside.get();
        untracked(|| {
            let sidebar = list(app, &entries, &configured, chosen, facts);
            let detail = dynamic(move || {
                let item = chosen.get();
                untracked(move || {
                    item.map_or_else(
                        || view().into_element(),
                        |item| selected(app, item, facts, live),
                    )
                })
            });
            if beside {
                let side_width = if app.form == Form::Terminal {
                    term::cells(30.0)
                } else {
                    px(245.0)
                };
                view()
                    .flex_row()
                    .items_start()
                    .min_w(px(0.0))
                    .gap(if app.form == Form::Terminal {
                        term::cells(1.0)
                    } else {
                        px(16.0)
                    })
                    .child(view().w(side_width).flex_shrink_0().child(sidebar))
                    .child(view().flex_1().min_w(px(0.0)).child(detail))
                    .into_element()
            } else {
                view()
                    .min_w(px(0.0))
                    .child(sidebar)
                    .child(space(app, 12.0))
                    .child(detail)
                    .into_element()
            }
        })
    });
    view()
        .test_id("config-harnesses")
        .min_w(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| {
            let measured = Some(frame.size.width.0);
            if width.get_untracked() != measured {
                width.set(measured);
            }
        })
        .child(cell(app, harnesses::NOTE, Token::Ink3, false))
        .child(space(app, 10.0))
        .child(layout)
        .into_element()
}

fn list(
    app: AppState,
    entries: &[Entry],
    configured: &[String],
    chosen: Memo<Option<Entry>>,
    facts: Memo<Details>,
) -> AnyElement {
    let count = entries.len().to_string();
    let rows = entries
        .iter()
        .map(|item| {
            let key = item.key.clone();
            let check = key.clone();
            let read = key.clone();
            let title = item.title().to_owned();
            let select = move || {
                app.act(app.settings, {
                    let key = key.clone();
                    move |state, _| state.choose_entry(Part::Harnesses, key)
                })
            };
            let picked =
                move || chosen.with(|item| item.as_ref().is_some_and(|item| item.key == check));
            let id = format!("seg-config-harnesses-{}", item.key);
            let summary = computed(move || {
                facts.with(|facts| {
                    facts
                        .rows
                        .get(&read)
                        .map(|row| {
                            if app.form == Form::Terminal {
                                row.discovery.status.label().to_owned()
                            } else {
                                format!("{} · {}", row.discovery.status.label(), row.usage())
                            }
                        })
                        .unwrap_or_default()
                })
            });
            let subtitle = text(move || summary.get())
                .min_w(px(0.0))
                .number_of_lines(1u32)
                .text_color(color(Token::Ink3))
                .when(app.form != Form::Terminal, |text| {
                    text.font_family(crate::ui::tokens::FONT_TEXT)
                        .text_size(px(super::super::parts::cell_size(app.form)))
                });
            let words = if app.form == Form::Terminal {
                view().min_w(px(0.0)).flex_row().gap(term::cells(1.0))
            } else {
                view().min_w(px(0.0)).gap(px(2.0))
            }
            .child(cell(app, title.clone(), Token::Ink, true).number_of_lines(1u32))
            .child(subtitle);
            list_row_tall(
                app,
                title,
                id,
                picked,
                select,
                vec![mark(app, item.mark(), false), words.into_element()],
            )
        })
        .collect::<Vec<_>>();
    panel(
        app,
        vec![
            h3_flush(app, "Harness catalog", Some(count)),
            view()
                .min_w(px(0.0))
                .label("Select a harness")
                .role(AccessibilityRole::Group)
                .children(rows)
                .into_element(),
            space(app, 8.0),
            add_custom(app, configured),
        ],
    )
}

fn add_custom(app: AppState, configured: &[String]) -> AnyElement {
    let open = signal(false);
    let name = signal(String::new());
    let configured = configured.to_vec();
    let invalid = computed(move || name.with(|name| harnesses::invalid_custom(name, &configured)));
    let toggle = dynamic(move || {
        let label = if open.get() {
            "Close custom harness form"
        } else {
            "Custom harness…"
        };
        untracked(move || {
            button(
                app,
                label,
                ButtonKind::Ghost,
                "btn-config-new-harnesses",
                move || open.update(|open| *open = !*open),
            )
        })
    });
    let fields = dynamic(move || {
        if !open.get() {
            return view().into_element();
        }
        untracked(move || {
            let add = dynamic(move || {
                let why = invalid.get();
                let disabled = name.with(|name| name.trim().is_empty()) || why.is_some();
                untracked(move || {
                    let mut children = vec![button_with(
                        app,
                        "Add harness",
                        ButtonKind::Primary,
                        ButtonOptions {
                            disabled,
                            ..ButtonOptions::default()
                        },
                        "btn-config-add-harnesses",
                        move || {
                            edit(app, Edit::AddEntry(Part::Harnesses, name.get_untracked()));
                        },
                    )];
                    if let Some(why) = why {
                        children.push(cell(app, why, Token::Held, false).into_element());
                    }
                    view().gap(px(4.0)).children(children).into_element()
                })
            });
            view()
                .min_w(px(0.0))
                .gap(px(6.0))
                .child(cell(app, harnesses::CUSTOM_NOTE, Token::Ink3, false))
                .child(entry(
                    app,
                    "field-config-new-harnesses".into(),
                    "Harness name",
                    move || name.get(),
                    move |text| name.set(text),
                ))
                .child(add)
                .into_element()
        })
    });
    view()
        .min_w(px(0.0))
        .gap(px(6.0))
        .child(toggle)
        .child(fields)
        .into_element()
}

fn selected(app: AppState, item: Entry, facts: Memo<Details>, live: Memo<Live>) -> AnyElement {
    let pi = item.key == "pi";
    let key = item.key.clone();
    let selected =
        computed(move || facts.with(|facts| facts.rows.get(&key).cloned().unwrap_or_default()));
    let mut parts = vec![
        hero(app, &item, selected),
        space(app, 12.0),
        h3_flush(app, "Setup & configuration", None),
    ];
    if pi {
        parts.push(panel(
            app,
            vec![
                h3_flush(app, "Provider access", None),
                cell(app, harnesses::LOGIN_NOTE, Token::Ink3, false).into_element(),
                code(app, harnesses::LOGIN_COMMAND),
            ],
        ));
        parts.push(space(app, 10.0));
    } else {
        parts.push(command(app, &item, live));
        parts.push(space(app, 10.0));
    }
    parts.push(task_profiles(app, pi, selected));
    if pi {
        parts.push(space(app, 10.0));
        parts.push(children(app, facts));
        parts.push(space(app, 10.0));
        parts.push(command(app, &item, live));
    }
    view()
        .test_id("config-harness-detail")
        .min_w(px(0.0))
        .children(parts)
        .into_element()
}

fn hero(app: AppState, item: &Entry, selected: Memo<Facts>) -> AnyElement {
    let title = cell(app, item.title().to_owned(), Token::Ink, true)
        .role(AccessibilityRole::Header)
        .test_id("config-harness-title")
        .when(app.form != Form::Terminal, |text| {
            text.text_size(px(24.0)).line_height(px(28.0))
        });
    let head = view()
        .min_w(px(0.0))
        .flex_row()
        .items_center()
        .gap(if app.form == Form::Terminal {
            term::cells(1.0)
        } else {
            px(10.0)
        })
        .child(mark(app, item.mark(), true))
        .child(
            view()
                .min_w(px(0.0))
                .child(cell(app, item.kind(), Token::Corral, true))
                .child(title),
        )
        .into_element();
    let fields = dynamic(move || {
        let facts = selected.get();
        untracked(move || {
            let ink = match facts.discovery.status {
                Detection::Detected => Token::Ok,
                Detection::NotFound => Token::Held,
                _ => Token::Ink2,
            };
            let executable = facts
                .discovery
                .executable
                .as_deref()
                .unwrap_or("Not reported");
            view()
                .min_w(px(0.0))
                .flex_row()
                .flex_wrap()
                .gap(if app.form == Form::Terminal {
                    term::cells(1.0)
                } else {
                    px(12.0)
                })
                .child(fact(
                    app,
                    "Daemon detection",
                    facts.discovery.status.label().into(),
                    ink,
                    false,
                ))
                .child(fact(app, "Executable", executable.into(), Token::Ink, true))
                .child(fact(app, "Used by", facts.usage(), Token::Ink, false))
                .into_element()
        })
    })
    .into_element();
    panel(
        app,
        vec![
            head,
            cell(app, item.description(), Token::Ink2, false).into_element(),
            space(app, 4.0),
            fields,
            cell(app, harnesses::HOST_NOTE, Token::Ink3, false).into_element(),
        ],
    )
}

fn fact(app: AppState, label: &'static str, value: String, ink: Token, mono: bool) -> AnyElement {
    view()
        .min_w(px(0.0))
        .child(cell(app, label, Token::Ink3, false))
        .child(if mono {
            mono_cell(app, value, ink)
        } else {
            cell(app, value, ink, true)
        })
        .into_element()
}

fn task_profiles(app: AppState, pi: bool, selected: Memo<Facts>) -> AnyElement {
    let profiles = computed(move || selected.with(|facts| facts.profiles.clone()));
    let listed = dynamic(move || {
        let profiles = profiles.get();
        untracked(move || profile_rows(app, profiles, true))
    })
    .into_element();
    let mut body = vec![
        h3_flush(app, "Task profiles", Some("corral.toml".into())),
        cell(app, harnesses::PROFILES_NOTE, Token::Ink3, false).into_element(),
    ];
    if pi {
        body.push(cell(app, harnesses::PI_PROFILE_NOTE, Token::Ink3, false).into_element());
    }
    body.push(listed);
    panel(app, body)
}

fn children(app: AppState, facts: Memo<Details>) -> AnyElement {
    let children = computed(move || facts.with(|facts| facts.children.clone()));
    let body = dynamic(move || {
        let children = children.get();
        untracked(move || {
            let mut parts = vec![
                h3_flush(app, "Pi child profiles", None),
                mono_cell(app, children.path, Token::Ink2).into_element(),
                cell(app, harnesses::CHILD_NOTE, Token::Ink3, false).into_element(),
                mono_cell(app, "[build.profiles.<name>]", Token::Ink2).into_element(),
            ];
            if !children.status.is_empty() {
                parts.push(cell(app, children.status, Token::Ink3, false).into_element());
            } else {
                parts.push(profile_rows(app, children.profiles, false));
            }
            panel(app, parts)
        })
    });
    view()
        .test_id("config-pi-child-profiles")
        .child(body)
        .into_element()
}

fn profile_rows(app: AppState, profiles: Vec<Profile>, task: bool) -> AnyElement {
    if profiles.is_empty() {
        return cell(
            app,
            "No task profiles use this harness.",
            Token::Ink3,
            false,
        )
        .into_element();
    }
    view()
        .min_w(px(0.0))
        .gap(px(4.0))
        .children(profiles.into_iter().map(|profile| {
            let name = profile.name.clone();
            let title = if task && profile.editable {
                button(
                    app,
                    name.clone(),
                    ButtonKind::Quiet,
                    format!("btn-config-harness-profile-{name}"),
                    move || {
                        let name = name.clone();
                        app.act(app.settings, move |state, _| {
                            state.choose_entry(Part::Profiles, name)
                        });
                    },
                )
            } else {
                let label = if task {
                    format!("{name} · Built-in")
                } else {
                    name
                };
                cell(app, label, Token::Ink, true).into_element()
            };
            let mut parts = vec![title];
            if let Some(model) = profile.model {
                parts.push(mono_cell(app, model, Token::Ink2).into_element());
            }
            if let Some(effort) = profile.effort {
                parts
                    .push(cell(app, format!("{effort} effort"), Token::Ink3, false).into_element());
            }
            view()
                .min_w(px(0.0))
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(if app.form == Form::Terminal {
                    term::cells(1.0)
                } else {
                    px(8.0)
                })
                .children(parts)
                .into_element()
        }))
        .into_element()
}

fn command(app: AppState, item: &Entry, live: Memo<Live>) -> AnyElement {
    if !item.configured {
        let key = item.key.clone();
        let template = computed(move || app.feed.with(|feed| Template::of(feed, &key)));
        let key = item.key.clone();
        return dynamic(move || {
            let template = template.get();
            let key = key.clone();
            untracked(move || {
                let mut parts = vec![h3_flush(
                    app,
                    "Command template",
                    Some("corral.toml".into()),
                )];
                if let Some(template) = template {
                    parts.push(
                        cell(app, harnesses::TEMPLATE_NOTE, Token::Ink3, false).into_element(),
                    );
                    parts.push(code(app, &template.example(&key)));
                    parts.push(button_row(
                        app,
                        vec![button(
                            app,
                            "Create override",
                            ButtonKind::Ghost,
                            "btn-config-override-harness",
                            move || {
                                edit(app, Edit::OverrideHarness(key.clone(), template.clone()));
                            },
                        )],
                    ));
                } else {
                    parts.push(
                        cell(
                            app,
                            "The built-in template was not reported.",
                            Token::Ink3,
                            false,
                        )
                        .into_element(),
                    );
                }
                panel(app, parts)
            })
        })
        .into_element();
    }
    let key = item.key.clone();
    let at = Path::of(&["harnesses", &key]);
    let reasons = computed(move || {
        app.settings.with(|state| {
            state
                .config
                .reasons_at(&at)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
    });
    let reasons = dynamic(move || {
        let reasons = reasons.get();
        untracked(move || {
            view()
                .min_w(px(0.0))
                .children(
                    reasons
                        .into_iter()
                        .map(|reason| cell(app, reason, Token::Fail, false)),
                )
                .into_element()
        })
    });
    let mut parts = vec![
        h3_flush(app, "Command template", Some("corral.toml draft".into())),
        reasons.into_element(),
    ];
    if item.builtin.is_some() {
        parts.push(cell(app, harnesses::OVERRIDE_NOTE, Token::Ink3, false).into_element());
    }
    parts.extend(
        harness_fields(&item.key)
            .iter()
            .map(|shape| field(app, shape, live)),
    );
    parts.push(button_row(
        app,
        vec![button_with(
            app,
            if item.builtin.is_some() {
                "Remove override"
            } else {
                "Remove harness"
            },
            ButtonKind::Ghost,
            ButtonOptions {
                danger: true,
                ..ButtonOptions::default()
            },
            "btn-config-remove-harnesses",
            move || edit(app, Edit::RemoveEntry(Part::Harnesses, key.clone())),
        )],
    ));
    panel(app, parts)
}

fn panel(app: AppState, body: Vec<AnyElement>) -> AnyElement {
    let panel = view().min_w(px(0.0));
    if app.form == Form::Terminal {
        return panel.children(body).into_element();
    }
    panel
        .gap(px(6.0))
        .p(px(12.0))
        .rounded(px(8.0))
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule))
        .children(body)
        .into_element()
}

fn mark(app: AppState, symbol: &'static str, large: bool) -> AnyElement {
    let mark = view().flex_shrink_0().accessibility_elements_hidden(true);
    let mark = if app.form == Form::Terminal {
        mark.w(term::cells(2.0))
    } else {
        let size = if large { 42.0 } else { 24.0 };
        mark.w(px(size))
            .h(px(size))
            .items_center()
            .justify_center()
            .rounded(px(5.0))
            .bg(color(Token::CorralBg))
    };
    mark.child(
        mono_cell(app, symbol, Token::Corral).when(large && app.form != Form::Terminal, |text| {
            text.text_size(px(20.0))
        }),
    )
    .into_element()
}

fn code(app: AppState, words: &str) -> AnyElement {
    let code = view().min_w(px(0.0));
    let code = if app.form == Form::Terminal {
        code
    } else {
        code.p(px(8.0)).rounded(px(5.0)).bg(color(Token::BgSunk))
    };
    code.children(
        words
            .lines()
            .map(|line| mono_cell(app, line.to_owned(), Token::Corral)),
    )
    .into_element()
}
