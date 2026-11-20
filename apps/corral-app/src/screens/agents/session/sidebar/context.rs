//! The Context tab's rows (`session::ContextTab`): AGENTS (mark, id in bold, profile and model
//! muted, what it does now under it), COMPLETED with the finished ids, JOBS while the host lists
//! one, then what the session works with (directory, worktree, branch, model, effort, context
//! window), the loaded context files, skills, prompts and tools, and the MCP servers with their
//! state. An agent or job row opens it in the Agents tab; an asking agent docks its question.
//! test ids `side-agent-<id>`, `side-job-<id>`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{ContextFact, ContextList, ContextTab, McpRow, SideAgent, SideJob};
use crate::state::AppState;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::rows::{list_row, list_row_tall};
use crate::ui::kit::text::{dim, h3, h3_flush, label_value, mono};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT};

use super::super::super::run;
use super::super::hue_words;
use super::inset;

/// Section captions: the first sits flush under the tab's summary, the rest are spaced.
struct Captions {
    first: bool,
}

impl Captions {
    fn next(
        &mut self,
        app: AppState,
        label: impl Into<SharedString>,
        note: Option<String>,
    ) -> AnyElement {
        let caption = if self.first {
            h3_flush(app, label, note)
        } else {
            h3(app, label, note)
        };
        self.first = false;
        inset(app, caption)
    }
}

/// The Agents section's rows: the agents at work, the completed ones, the jobs.
pub(super) fn agents(app: AppState, tab: &ContextTab) -> AnyElement {
    let mut captions = Captions { first: true };
    let mut out: Vec<AnyElement> = Vec::new();
    if let Some(empty) = &tab.empty {
        out.push(inset(app, dim(app, empty.clone())));
    }
    if !tab.agents.is_empty() || tab.agents_note.is_some() {
        out.extend(tab.agents.iter().map(|row| agent(app, row)));
        if let Some(note) = &tab.agents_note {
            out.push(inset(app, dim(app, note.clone())));
        }
    }
    if let Some((heading, ids)) = &tab.completed {
        out.push(captions.next(app, heading.clone(), None));
        out.push(inset(
            app,
            mono(app, ids.clone()).text_color(hue_words(app.form, Hue::Neutral)),
        ));
    }
    if !tab.jobs.is_empty() || tab.jobs_note.is_some() {
        out.push(captions.next(app, "Jobs", None));
        out.extend(tab.jobs.iter().map(|row| job(app, row)));
        if let Some((hue, note)) = &tab.jobs_note {
            out.push(inset(
                app,
                dim(app, note.clone()).text_color(hue_words(app.form, *hue)),
            ));
        }
    }
    column(app, out)
}

/// The Context section's rows: what the session works with, as its host reports it.
pub(super) fn context(app: AppState, tab: &ContextTab) -> AnyElement {
    let mut captions = Captions { first: false };
    let mut out: Vec<AnyElement> = vec![inset(app, facts(app, &tab.facts))];
    for list in &tab.lists {
        out.push(captions.next(
            app,
            list.label,
            (!list.items.is_empty()).then(|| list.items.len().to_string()),
        ));
        out.push(inset(app, items(app, list)));
    }
    if let Some(servers) = &tab.mcp {
        out.push(captions.next(
            app,
            "MCP servers",
            (!servers.is_empty()).then(|| servers.len().to_string()),
        ));
        out.push(inset(app, mcp(app, servers)));
    }
    column(app, out)
}

fn column(app: AppState, out: Vec<AnyElement>) -> AnyElement {
    view()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 2.0 }))
        .pb(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
        .children(out)
        .into_element()
}

fn agent(app: AppState, row: &SideAgent) -> AnyElement {
    let form = app.form;
    let action = row.action.clone();
    let label = format!(
        "{}, {}, {}, {}",
        row.child, row.state, row.profile, row.activity
    );
    let id = mono(app, row.child.clone())
        .font_weight(FontWeight::BOLD)
        .flex_shrink_0()
        .number_of_lines(1u32);
    let profile = dim(app, row.profile.clone())
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .flex_1();
    let activity_ink = if row.hue == Hue::You {
        hue_words(form, Hue::You)
    } else {
        hue_words(form, Hue::Neutral)
    };
    let activity = dim(app, row.activity.clone())
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .text_color(activity_ink);
    let press = move || run(app, action.clone());
    if form == Form::Terminal {
        let line = list_row(
            app,
            label,
            row.test_id.clone(),
            || false,
            press,
            vec![
                mark(app, row.mark, row.hue),
                id.into_element(),
                profile.into_element(),
            ],
        );
        return view()
            .child(line)
            .child(view().pl(term::cells(4.0)).child(activity))
            .into_element();
    }
    list_row_tall(
        app,
        label,
        row.test_id.clone(),
        || false,
        press,
        vec![
            mark(app, row.mark, row.hue),
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(1.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(id)
                        .child(profile),
                )
                .child(activity)
                .into_element(),
        ],
    )
}

fn job(app: AppState, row: &SideJob) -> AnyElement {
    let action = row.action.clone();
    list_row(
        app,
        format!("job {}, {}, {}", row.job, row.state, row.command),
        row.test_id.clone(),
        || false,
        move || run(app, action.clone()),
        vec![
            mark(app, row.mark, row.hue),
            mono(app, row.command.clone())
                .number_of_lines(1u32)
                .truncate()
                .min_w(px(0.0))
                .flex_1()
                .into_element(),
            dim(app, row.state.clone())
                .number_of_lines(1u32)
                .flex_shrink_0()
                .into_element(),
        ],
    )
}

/// A fact's value at the size `label_value` sets its label: mono for a path, branch or model.
fn value(app: AppState, fact: &ContextFact) -> AnyElement {
    let form = app.form;
    let words = text(fact.value.clone())
        .min_w(px(0.0))
        .text_color(hue_words(form, fact.hue));
    match form {
        Form::Terminal => words.number_of_lines(1u32).truncate().into_element(),
        Form::Desktop | Form::Phone => {
            let (face, size) = if fact.mono {
                (FONT_MONO, if form == Form::Phone { 13.0 } else { 11.5 })
            } else {
                (FONT_TEXT, if form == Form::Phone { 14.0 } else { 12.5 })
            };
            words
                .font_family(face)
                .text_size(px(size))
                .number_of_lines(2u32)
                .into_element()
        }
    }
}

fn facts(app: AppState, facts: &[ContextFact]) -> AnyElement {
    view()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 5.0 }))
        .children(
            facts
                .iter()
                .map(|fact| label_value(app, fact.label, value(app, fact)))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// A list's names: context files one per line (they are paths), the rest run together.
fn items(app: AppState, list: &ContextList) -> AnyElement {
    if list.items.is_empty() {
        return dim(app, list.empty).into_element();
    }
    if list.label == "Context files" {
        return view()
            .children(
                list.items
                    .iter()
                    .map(|path| {
                        mono(app, path.clone())
                            .number_of_lines(1u32)
                            .truncate()
                            .min_w(px(0.0))
                            .text_color(hue_words(app.form, Hue::Neutral))
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
            )
            .into_element();
    }
    dim(app, list.items.join(" · ")).into_element()
}

fn mcp(app: AppState, servers: &[McpRow]) -> AnyElement {
    if servers.is_empty() {
        return dim(app, "no MCP servers").into_element();
    }
    view()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 3.0 }))
        .children(
            servers
                .iter()
                .map(|server| {
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            mono(app, server.name.clone())
                                .number_of_lines(1u32)
                                .truncate()
                                .min_w(px(0.0))
                                .flex_1(),
                        )
                        .child(
                            dim(app, server.state.clone())
                                .number_of_lines(1u32)
                                .flex_shrink_0()
                                .text_color(hue_ink(server.hue)),
                        )
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}
