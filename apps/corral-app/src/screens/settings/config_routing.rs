//! Settings › Configuration › Routing: the order a worker's profile is chosen in, then the
//! rules one compact row each in the file's order (what it matches, the profile it picks, and
//! whether it is a catch-all or never reached), with Edit opening one rule's fields at a time
//! and Move up and Move down as the keyboard's way to reorder. The focus follows a moved rule,
//! goes to the new rule's first field after Add and to the row taking a removed rule's place.
//! IDs: `config-routing-order`, `btn-config-default-edit`, `config-rules`, `config-rule-<n>`,
//! `btn-config-rule-<n>-<edit|up|down|remove|profile>`, `btn-config-rule-add`.

use std::rc::Rc;

use nsh_core::prelude::*;

use super::config_fields::{edit, field};
use super::parts::{button_row, cell, mono_cell, space};
use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::settings::document::{self, Path, RuleEdit};
use crate::model::settings::editor::{Edit, Part};
use crate::model::settings::form::{Field, Group, Live};
use crate::model::settings::routing::{self, Routing, Rule, Step, Target, Tone};
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::notice::notice;
use crate::ui::kit::text::{h3, h3_flush};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// A rule's controls the keyboard focus is handed between as rules move.
#[derive(Clone, Copy)]
struct Handles {
    toggle: ElementRef,
    up: ElementRef,
    down: ElementRef,
}

/// The key `added` names after a removal so the row now at that place takes the focus; no
/// field has it, so no field takes the focus for it.
const ROW: &str = "@row";

/// The Routing part, for a draft that parses; `groups` holds each rule's fields.
pub(super) fn routing(app: AppState, groups: &[Group], live: Memo<Live>) -> AnyElement {
    let read = computed(move || {
        app.settings.with(|state| {
            app.feed.with(|feed| {
                routing::read(
                    &state.config.draft,
                    feed.settings
                        .as_ref()
                        .and_then(|settings| settings.built_in_defaults.as_ref()),
                )
            })
        })
    });
    let rules: Vec<&Group> = groups.iter().filter(|group| group.rule.is_some()).collect();
    let count = rules.len();
    let handles: Rc<[Handles]> = (0..count)
        .map(|_| Handles {
            toggle: element_ref(),
            up: element_ref(),
            down: element_ref(),
        })
        .collect();
    let add = element_ref();
    focus_after_removal(app, &handles, add);
    let mut parts = vec![
        order(app, read),
        space(app, 12.0),
        h3(app, "Rules", Some("first match wins".into())),
        catch_all(app, read),
    ];
    if rules.is_empty() {
        parts.push(cell(app, routing::NO_RULES, Token::Ink3, false).into_element());
    }
    parts.push(
        view()
            .test_id("config-rules")
            .min_w(px(0.0))
            .gap(gap(app.form, 10.0))
            .children(
                rules
                    .into_iter()
                    .map(|group| row(app, group, count, read, live, handles.clone())),
            )
            .into_element(),
    );
    parts.push(space(app, 10.0));
    parts.push(add_rule(app, read, add));
    view().min_w(px(0.0)).children(parts).into_element()
}

/// Room between stacked parts: `points` on a screen, a blank row in a terminal.
fn gap(form: Form, points: f32) -> Points {
    if form == Form::Terminal {
        term::rows(1.0)
    } else {
        px(points)
    }
}

/// Room between words in a line: `points` on a screen, a Cell in a terminal.
fn beside(form: Form, points: f32) -> Points {
    if form == Form::Terminal {
        term::cells(1.0)
    } else {
        px(points)
    }
}

/// After a removal the row now in the removed rule's place, or Add rule once none is left,
/// takes the focus the removed rule's Remove had.
fn focus_after_removal(app: AppState, handles: &Rc<[Handles]>, add: ElementRef) {
    let at = app.settings.with_untracked(|state| {
        (0..=handles.len()).find(|at| state.config.added.as_ref() == Some(&Path::rule(*at, ROW)))
    });
    let Some(at) = at else {
        return;
    };
    let handle = handles.get(at).map_or(add, |handles| handles.toggle);
    after_answer(move || {
        handle.focus();
        app.act(app.settings, |state, _| {
            state.config.added = None;
            Vec::new()
        });
    });
}

/// The order a dispatch picks a worker's profile in, with this draft's facts in each step.
fn order(app: AppState, read: Memo<Routing>) -> AnyElement {
    let steps = computed(move || read.with(routing::steps));
    let body = dynamic(move || {
        let steps = steps.get();
        untracked(move || {
            let last = steps.len().saturating_sub(1);
            view()
                .test_id("config-routing-order")
                .min_w(px(0.0))
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
                .children(
                    steps
                        .into_iter()
                        .enumerate()
                        .map(|(at, step)| step_line(app, at, step, at == last)),
                )
                .into_element()
        })
    })
    .into_element();
    view()
        .min_w(px(0.0))
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(h3_flush(app, "Which profile a worker runs", None))
        .child(body)
        .child(cell(app, routing::WORKER_ONLY, Token::Ink3, false))
        .into_element()
}

/// One step: its place, title and this draft's facts; the workspace default with a way to it.
fn step_line(app: AppState, at: usize, step: Step, workspace: bool) -> AnyElement {
    let ink = if step.unreached {
        Token::Ink4
    } else {
        Token::Ink
    };
    let detail_ink = if step.unreached {
        Token::Held
    } else {
        Token::Ink3
    };
    let words = if app.form == Form::Terminal {
        view()
            .flex_row()
            .flex_wrap()
            .flex_1()
            .min_w(px(0.0))
            .gap(term::cells(1.0))
            .child(cell(app, step.title, ink, true))
            .child(cell(app, step.detail, detail_ink, false))
    } else {
        view()
            .flex_1()
            .min_w(px(0.0))
            .child(cell(app, step.title, ink, true))
            .child(cell(app, step.detail, detail_ink, false))
    };
    let edit_default = workspace.then(|| {
        button(
            app,
            "Edit default",
            ButtonKind::Quiet,
            "btn-config-default-edit",
            move || {
                app.act(app.settings, |state, _| {
                    state.show_config(&Path::of(&["defaults", "profile"]))
                });
            },
        )
    });
    view()
        .flex_row()
        .items_start()
        .gap(beside(app.form, 10.0))
        .min_w(px(0.0))
        .child(mono_cell(app, format!("{}.", at + 1), Token::Ink3))
        .child(words)
        .children(edit_default)
        .into_element()
}

/// The callout while a rule matches every task.
fn catch_all(app: AppState, read: Memo<Routing>) -> AnyElement {
    let shown = computed(move || {
        read.with(|routing| {
            routing.catch_all.map(|index| {
                routing::catch_all_notice(index, routing.rules.len().saturating_sub(index + 1))
            })
        })
    });
    dynamic(move || {
        let words = shown.get();
        untracked(move || match words {
            Some(words) => view()
                .test_id("config-routing-catch-all")
                .min_w(px(0.0))
                .child(notice(app, Hue::Held, &vec![span(words)], None))
                .child(space(app, 8.0))
                .into_element(),
            None => view().into_element(),
        })
    })
    .into_element()
}

/// One rule: its summary and controls, and its fields while it is the one open.
fn row(
    app: AppState,
    group: &Group,
    count: usize,
    read: Memo<Routing>,
    live: Memo<Live>,
    handles: Rc<[Handles]>,
) -> AnyElement {
    let Some(index) = group.rule.filter(|index| *index < handles.len()) else {
        return view().into_element();
    };
    let rule = computed(move || read.with(|routing| routing.rules.get(index).cloned()));
    let reasons = computed(move || {
        app.settings
            .with(|state| routing::reasons(&state.config, index))
    });
    let open = computed(move || {
        app.settings
            .with(|state| state.config.open_rule == Some(index))
    });
    let summary = dynamic(move || {
        let (rule, reasons) = (rule.get(), reasons.get());
        untracked(move || summary(app, index, rule, reasons))
    })
    .into_element();
    let controls = controls(app, index, count, open, &handles);
    let head = match app.form {
        Form::Desktop => view()
            .flex_row()
            .items_start()
            .gap(px(12.0))
            .min_w(px(0.0))
            .child(view().flex_1().min_w(px(0.0)).child(summary))
            .child(controls),
        Form::Phone => view()
            .min_w(px(0.0))
            .gap(px(8.0))
            .child(summary)
            .child(controls),
        Form::Terminal => view().min_w(px(0.0)).child(summary).child(controls),
    };
    let fields = group.fields.clone();
    let editor = dynamic(move || {
        let shown = open.get();
        let fields = fields.clone();
        untracked(move || {
            if shown {
                editor(app, index, &fields, rule, live)
            } else {
                view().into_element()
            }
        })
    })
    .into_element();
    let framed = view()
        .test_id(&format!("config-rule-{index}"))
        .min_w(px(0.0))
        .gap(gap(app.form, 12.0))
        .child(head)
        .child(editor);
    match app.form {
        Form::Terminal => framed.into_element(),
        Form::Desktop | Form::Phone => framed
            .p(px(if app.form == Form::Phone { 12.0 } else { 10.0 }))
            .border_1()
            .border_color(color(Token::Rule))
            .rounded(px(if app.form == Form::Phone { 10.0 } else { 8.0 }))
            .into_element(),
    }
}

/// What the rule matches and the profile it picks (a configured one opens in Profiles), then
/// what is said about it.
fn summary(app: AppState, index: usize, rule: Option<Rule>, reasons: usize) -> AnyElement {
    let Some(rule) = rule else {
        return view().into_element();
    };
    let target = match &rule.target {
        Target::Configured(name) => {
            let name = name.clone();
            button(
                app,
                format!("→ {name}"),
                ButtonKind::Quiet,
                format!("btn-config-rule-{index}-profile"),
                move || {
                    let name = name.clone();
                    app.act(app.settings, move |state, _| {
                        state.choose_entry(Part::Profiles, name)
                    });
                },
            )
        }
        other => cell(
            app,
            format!("→ {}", other.words()),
            if other.refused() {
                Token::Fail
            } else {
                Token::Ink
            },
            false,
        )
        .into_element(),
    };
    let matcher_ink = if rule.shadowed_by.is_some() {
        Token::Ink3
    } else {
        Token::Ink
    };
    let line = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(beside(app.form, 8.0))
        .min_w(px(0.0))
        .child(mono_cell(app, format!("{}.", index + 1), Token::Ink3))
        .child(cell(app, rule.matcher.clone(), matcher_ink, true))
        .child(target);
    let notes = rule.notes(reasons).into_iter().map(|(tone, words)| {
        let ink = match tone {
            Tone::Aside => Token::Ink3,
            Tone::Caution => Token::Held,
            Tone::Refused => Token::Fail,
        };
        cell(app, words, ink, false).into_element()
    });
    view()
        .min_w(px(0.0))
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 2.0 }))
        .child(line)
        .children(notes)
        .into_element()
}

/// Edit or Close, Move up and Move down (each disabled where the rule cannot go). A moved rule
/// keeps the focus on the control that moved it, or at an end on the other direction's.
fn controls(
    app: AppState,
    index: usize,
    count: usize,
    open: Memo<bool>,
    handles: &Rc<[Handles]>,
) -> AnyElement {
    let mine = handles[index];
    let toggle = dynamic(move || {
        let shown = open.get();
        untracked(move || {
            button_with(
                app,
                if shown { "Close" } else { "Edit" },
                ButtonKind::Ghost,
                ButtonOptions {
                    focus: Some(mine.toggle),
                    ..ButtonOptions::default()
                },
                format!("btn-config-rule-{index}-edit"),
                move || {
                    app.act(app.settings, move |state, _| {
                        state.config.open_rule =
                            (state.config.open_rule != Some(index)).then_some(index);
                        Vec::new()
                    });
                    after_answer(move || mine.toggle.focus());
                },
            )
        })
    })
    .into_element();
    let step = |up: bool| {
        let to = if up {
            index.checked_sub(1)
        } else {
            Some(index + 1).filter(|to| *to < count)
        };
        let handles = handles.clone();
        button_with(
            app,
            if up { "Move up" } else { "Move down" },
            ButtonKind::Quiet,
            ButtonOptions {
                disabled: to.is_none(),
                focus: Some(if up { mine.up } else { mine.down }),
                ..ButtonOptions::default()
            },
            format!("btn-config-rule-{index}-{}", if up { "up" } else { "down" }),
            move || {
                let Some((to, there)) = to.and_then(|to| Some((to, *handles.get(to)?))) else {
                    return;
                };
                edit(app, Edit::Rules(RuleEdit::Move(index, up)));
                let further = if up { to > 0 } else { to + 1 < count };
                let handle = if up == further { there.up } else { there.down };
                after_answer(move || handle.focus());
            },
        )
    };
    button_row(app, vec![toggle, step(true), step(false)])
}

/// The open rule's fields, a word on what an unnarrowed rule does, and Remove.
fn editor(
    app: AppState,
    index: usize,
    fields: &[Field],
    rule: Memo<Option<Rule>>,
    live: Memo<Live>,
) -> AnyElement {
    let unnarrowed = computed(move || rule.with(|rule| rule.as_ref().is_some_and(|r| r.catch_all)));
    let hint = dynamic(move || {
        let shown = unnarrowed.get();
        untracked(move || {
            if shown {
                cell(app, routing::CATCH_ALL, Token::Ink3, false).into_element()
            } else {
                view().into_element()
            }
        })
    })
    .into_element();
    let remove = button_with(
        app,
        "Remove rule",
        ButtonKind::Ghost,
        ButtonOptions {
            danger: true,
            ..ButtonOptions::default()
        },
        format!("btn-config-rule-{index}-remove"),
        move || {
            app.act(app.settings, move |state, _| {
                let before = state.config.draft.clone();
                let effects = state.edit_config(Edit::Rules(RuleEdit::Remove(index)));
                if state.config.draft != before {
                    let left = document::parse(&state.config.draft)
                        .map(|doc| document::rule_count(&doc))
                        .unwrap_or_default();
                    state.config.added = Some(Path::rule(index.min(left.saturating_sub(1)), ROW));
                }
                effects
            });
        },
    );
    view()
        .test_id(&format!("config-rule-{index}-editor"))
        .min_w(px(0.0))
        .gap(gap(app.form, 14.0))
        .child(hint)
        .children(fields.iter().map(|shape| field(app, shape, live)))
        .child(button_row(app, vec![remove]))
        .into_element()
}

/// Add rule, and where a new rule goes and what it matches until narrowed.
fn add_rule(app: AppState, read: Memo<Routing>, handle: ElementRef) -> AnyElement {
    let add = button_with(
        app,
        "Add rule",
        ButtonKind::Ghost,
        ButtonOptions {
            focus: Some(handle),
            ..ButtonOptions::default()
        },
        "btn-config-rule-add",
        move || {
            let profile = untracked(|| {
                app.settings.with(|state| {
                    read.with(|routing| routing::new_rule_profile(&state.config.draft, routing))
                })
            });
            edit(app, Edit::Rules(RuleEdit::Add(profile)));
        },
    );
    view()
        .min_w(px(0.0))
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(button_row(app, vec![add]))
        .child(cell(app, routing::NEW_RULE, Token::Ink3, false))
        .into_element()
}
