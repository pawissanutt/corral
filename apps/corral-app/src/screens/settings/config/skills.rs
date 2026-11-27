//! The catalog and one complete SKILL.md, with the daemon's exact save target.
use corral_protocol::config::{PiSkillRow, PiSkillSource};
use nsh_core::prelude::*;

use super::super::parts::{cell, mono_cell};
use crate::model::{
    Form,
    settings::skills::{self, Context, Editor},
};
use crate::screens::markdown::markdown_without_images;
use crate::state::{
    AppState,
    skills::{context, edit, get, list, save, toggle},
};
use crate::ui::{
    composer::copyable_input,
    kit::button::{ButtonKind, ButtonOptions, button, button_with},
    term,
    tokens::{FONT_MONO, Token, color},
};

pub(super) fn panel(app: AppState) -> AnyElement {
    let ctx = computed(move || context(app));
    let reason = computed(move || app.feed.with(skills::unavailable));
    dynamic(move || {
        let ctx = ctx.get();
        let reason = reason.get();
        untracked(|| match (ctx, reason) {
            (_, Some(reason)) => cell(app, reason, Token::Ink3, false).into_element(),
            (None, _) => {
                cell(app, "Waiting for workspace identity…", Token::Ink3, false).into_element()
            }
            (Some(ctx), None) => workspace(app, ctx),
        })
    })
    .into_element()
}

fn workspace(app: AppState, ctx: Context) -> AnyElement {
    let open = ctx.clone();
    let wanted = computed(move || {
        app.settings.with(|s| {
            s.skills
                .get(&open)
                .is_none_or(|e| e.catalog.is_none() && e.pending.is_none() && e.problem.is_none())
        })
    });
    let open = ctx.clone();
    effect(move || {
        if wanted.get() {
            untracked(|| list(app, &open));
        }
    });
    let shape_ctx = ctx.clone();
    let shape = computed(move || {
        app.settings.with(|s| {
            let e = s.skills.get(&shape_ctx)?;
            Some((
                e.catalog.clone(),
                e.row().cloned(),
                e.saved.is_some(),
                e.pending.is_some(),
                e.problem.clone(),
                e.review,
            ))
        })
    });
    dynamic(move || {
        let data = shape.get();
        let ctx = ctx.clone();
        untracked(|| {
            let mut parts = vec![
                cell(
                    app,
                    match &ctx.1 {
                        Some(project) => format!("Project scope: {project}"),
                        None => "Workspace scope · select a project to include its skills".into(),
                    },
                    Token::Ink3,
                    false,
                )
                .into_element(),
            ];
            if app.automation_preview() {
                parts.push(
                    cell(
                        app,
                        "Preview only · no files are written",
                        Token::Ink3,
                        false,
                    )
                    .into_element(),
                );
            }
            if let Some((catalog, row, loaded, waiting, problem, review)) = data {
                if let Some(problem) = problem {
                    parts.push(
                        cell(app, problem, Token::Held, false)
                            .test_id("pi-skills-problem")
                            .into_element(),
                    );
                }
                if waiting {
                    parts.push(
                        cell(app, "Waiting for skill response…", Token::Ink3, false).into_element(),
                    );
                }
                if let Some(row) = row {
                    parts.push(detail(app, &ctx, row, loaded, waiting, review));
                } else {
                    let reload = ctx.clone();
                    parts.push(inline(control(
                        app,
                        "Reload catalog",
                        "btn-pi-skills-reload",
                        waiting,
                        false,
                        move || list(app, &reload),
                    )));
                    if let Some(catalog) = catalog {
                        if catalog.skills.is_empty() {
                            parts.push(
                                cell(
                                    app,
                                    "No skills were found in this scope.",
                                    Token::Ink3,
                                    false,
                                )
                                .into_element(),
                            );
                        }
                        for (index, row) in catalog.skills.into_iter().enumerate() {
                            parts.push(catalog_row(app, &ctx, index, row, waiting));
                        }
                    }
                }
            }
            view()
                .test_id("pi-skills")
                .min_w(px(0.0))
                .gap(px(8.0))
                .children(parts)
                .into_element()
        })
    })
    .into_element()
}

/// One catalog skill, as Extensions lists its entries: the name to press, then where it comes
/// from and whether it is on.
fn catalog_row(
    app: AppState,
    ctx: &Context,
    index: usize,
    row: PiSkillRow,
    waiting: bool,
) -> AnyElement {
    let pick_ctx = ctx.clone();
    let facts = format!("{} · {}", skills::source(row.source), skills::status(&row));
    let name = row.name.clone();
    let id = row.id;
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .min_w(px(0.0))
        .child(button_with(
            app,
            name,
            ButtonKind::Ghost,
            ButtonOptions {
                disabled: waiting,
                ..Default::default()
            },
            format!("btn-pi-skill-{index}"),
            move || get(app, &pick_ctx, &id),
        ))
        .child(cell(app, facts, Token::Ink3, false))
        .into_element()
}

/// A button at its own width rather than stretched across the column.
fn inline(button: AnyElement) -> AnyElement {
    view()
        .flex_row()
        .min_w(px(0.0))
        .child(button)
        .into_element()
}

fn control(
    app: AppState,
    label: &'static str,
    id: &'static str,
    disabled: bool,
    primary: bool,
    run: impl Fn() + 'static,
) -> AnyElement {
    button_with(
        app,
        label,
        if primary {
            ButtonKind::Primary
        } else {
            ButtonKind::Ghost
        },
        ButtonOptions {
            disabled,
            ..Default::default()
        },
        id,
        run,
    )
}

fn detail(
    app: AppState,
    ctx: &Context,
    row: PiSkillRow,
    loaded: bool,
    waiting: bool,
    review: bool,
) -> AnyElement {
    let editable = skills::editable(&row);
    let target = if editable {
        "Save destination"
    } else {
        "Read-only source"
    };
    let note = match row.source {
        PiSkillSource::User if editable => {
            "Saving Markdown changes this shared user file, not just this workspace."
        }
        PiSkillSource::Project if editable => {
            "Save edits this project file. The session's worktree and ignore rules decide which project files it loads."
        }
        _ => "Markdown cannot be edited here.",
    };
    let dirty_ctx = ctx.clone();
    let dirty = computed(move || {
        app.settings
            .with(|s| s.skills.get(&dirty_ctx).is_some_and(Editor::dirty))
    });
    let controls_ctx = ctx.clone();
    let reread_id = row.id.clone();
    let actions = dynamic(move || {
        let dirty = dirty.get();
        let ctx = controls_ctx.clone();
        let id = reread_id.clone();
        untracked(|| {
            let back = ctx.clone();
            let reload = ctx.clone();
            let mut actions = vec![
                control(
                    app,
                    "Back to skills",
                    "btn-pi-skill-back",
                    waiting || dirty,
                    false,
                    move || edit(app, &back, Editor::close),
                ),
                control(
                    app,
                    "Read again",
                    "btn-pi-skill-read",
                    waiting || dirty,
                    false,
                    move || get(app, &reload, &id),
                ),
            ];
            if loaded && editable {
                let target = ctx.clone();
                actions.push(control(
                    app,
                    "Save Markdown",
                    "btn-pi-skill-save",
                    waiting || !dirty,
                    true,
                    move || save(app, &target),
                ));
                actions.push(control(
                    app,
                    "Discard draft",
                    "btn-pi-skill-discard",
                    waiting || !dirty,
                    false,
                    move || edit(app, &ctx, Editor::discard),
                ));
            }
            let mut panel = view().gap(px(4.0));
            if dirty {
                panel = panel.child(cell(
                    app,
                    "Unsaved Markdown · save or discard before changing workspace availability",
                    Token::Held,
                    false,
                ));
            }
            panel
                .child(view().flex_row().flex_wrap().gap(px(6.0)).children(actions))
                .into_element()
        })
    });
    let mut parts = vec![
        cell(app, row.name.clone(), Token::Ink, true).into_element(),
        cell(
            app,
            format!("{} · {}", skills::source(row.source), skills::status(&row)),
            Token::Ink3,
            false,
        )
        .into_element(),
        cell(app, target, Token::Ink, true).into_element(),
        mono_cell(app, row.path, Token::Ink)
            .test_id("pi-skill-path")
            .into_element(),
        cell(app, note, Token::Ink3, false).into_element(),
    ];
    if !row.required && !row.shadowed {
        let ctx = ctx.clone();
        parts.push(
            cell(
                app,
                "This workspace only · affects new Pi sessions, not running ones.",
                Token::Ink3,
                false,
            )
            .into_element(),
        );
        let policy = if app.automation_preview() {
            "/preview/corral/.corral-next/pi.toml".to_owned()
        } else {
            format!("{}/pi.toml", ctx.0.trim_end_matches('/'))
        };
        parts.push(
            mono_cell(app, format!("Workspace policy: {policy}"), Token::Ink3)
                .test_id("pi-skill-policy-path")
                .into_element(),
        );
        parts.push(
            dynamic(move || {
                let blocked = dirty.get() || waiting;
                let ctx = ctx.clone();
                inline(control(
                    app,
                    if row.enabled {
                        "Disable in workspace"
                    } else {
                        "Enable in workspace"
                    },
                    "btn-pi-skill-toggle",
                    blocked,
                    false,
                    move || toggle(app, &ctx),
                ))
            })
            .into_element(),
        );
    }
    parts.push(actions.into_element());
    if loaded {
        let toggle = ctx.clone();
        parts.push(inline(button(
            app,
            if review {
                "Show Markdown source"
            } else {
                "Preview Markdown"
            },
            ButtonKind::Ghost,
            "btn-pi-skill-preview",
            move || edit(app, &toggle, |e| e.review = !e.review),
        )));
        let target = ctx.clone();
        let value = move || {
            app.settings.with(|s| {
                s.skills
                    .get(&target)
                    .map(|e| e.draft.clone())
                    .unwrap_or_default()
            })
        };
        let target = ctx.clone();
        if review {
            parts.push(
                dynamic(move || {
                    let markdown = value();
                    untracked(|| markdown_without_images(app, &markdown))
                })
                .into_element(),
            );
        } else {
            let field = copyable_input(app, None, value)
                .test_id("field-pi-skill-markdown")
                .label("SKILL.md Markdown source")
                .readonly(!editable || waiting)
                .multiline(true)
                .submit_behavior(SubmitBehavior::Newline)
                .auto_correct(false)
                .auto_capitalize(AutoCapitalize::None)
                .on_change(move |text: &str| {
                    edit(app, &target, |editor| {
                        if editor.pending.is_none() && editor.row().is_some_and(skills::editable) {
                            editor.draft = text.to_owned();
                        }
                    })
                })
                .w_full()
                .min_w(px(0.0))
                .text_color(color(Token::Ink))
                .border_1()
                .border_color(color(Token::Rule2));
            let field = if app.form == Form::Terminal {
                field.min_h(term::rows(18.0)).px(term::cells(1.0))
            } else {
                field.font_family(FONT_MONO).min_h(px(360.0)).p(px(10.0))
            };
            parts.push(field.into_element());
        }
    }
    view()
        .min_w(px(0.0))
        .test_id("pi-skill-detail")
        .gap(px(8.0))
        .children(parts)
        .into_element()
}
