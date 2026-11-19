//! The session's action catalog, one list for the ⋯ menu, the palette's session group and the
//! `/` command menu. Each row says its key and its terminal command where it has one; a row the
//! host cannot run now stays listed with its reason and does not run. The run controls (Cancel
//! turn, Pause, Resume, Rewind…, Terminate or Stop, Relaunch or Resume) and the task's Actions
//! menu are rows here; every part of the page adds its own rows through its `actions(cx)`,
//! merged into the group its rows name.

use crate::model::agents::{Control, control_label};

use super::{Action, Cx, Tab, board, changes, exit, proof, questions, sidebar, task, usage};

/// Each part's rows, in catalog order.
const PARTS: &[fn(&Cx<'_>) -> Option<ActionGroup>] = &[
    questions::actions,
    exit::actions,
    sidebar::actions,
    usage::actions,
    changes::actions,
    board::actions,
    task::actions,
    proof::actions,
];

#[derive(Debug, Clone, PartialEq)]
pub struct SessionActions {
    pub groups: Vec<ActionGroup>,
}

impl SessionActions {
    /// Every row, in group order.
    pub fn rows(&self) -> impl Iterator<Item = &ActionRow> {
        self.groups.iter().flat_map(|group| &group.rows)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionGroup {
    /// Views · Message · Display · Run · Task.
    pub title: &'static str,
    pub rows: Vec<ActionRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionRow {
    /// `act-<slug>`.
    pub id: String,
    pub label: String,
    /// The terminal UI's command (`/finish`), matched by the `/` menu.
    pub command: Option<&'static str>,
    pub key: Option<&'static str>,
    /// Listed muted with ` · unavailable: <reason>`; does not run.
    pub unavailable: Option<String>,
    pub action: Action,
}

pub(super) fn row(
    slug: &str,
    label: impl Into<String>,
    key: Option<&'static str>,
    action: Action,
) -> ActionRow {
    ActionRow {
        id: format!("act-{slug}"),
        label: label.into(),
        command: None,
        key,
        unavailable: None,
        action,
    }
}

fn views(cx: &Cx<'_>, tabs: &[Tab]) -> ActionGroup {
    ActionGroup {
        title: "Views",
        rows: tabs
            .iter()
            .map(|tab| {
                row(
                    &format!("view-{}", tab.slug()),
                    format!("Open {}", tab.label()),
                    Some(tab.key(&cx.role)),
                    Action::Tab {
                        session: cx.id.clone(),
                        tab: *tab,
                    },
                )
            })
            .collect(),
    }
}

fn display(cx: &Cx<'_>) -> ActionGroup {
    ActionGroup {
        title: "Display",
        rows: vec![row(
            "thinking",
            if cx.ui.hide_thinking {
                "Show thinking"
            } else {
                "Hide thinking"
            },
            Some("Alt+H"),
            Action::Thinking(cx.id.clone()),
        )],
    }
}

fn run(cx: &Cx<'_>) -> ActionGroup {
    let kind = crate::model::agents::kind(cx.feed, cx.id.as_str());
    ActionGroup {
        title: "Run",
        rows: cx
            .controls()
            .into_iter()
            .map(|control| {
                let mut found = row(
                    control.slug(),
                    control_label(control, kind),
                    (control == Control::Cancel).then_some("esc"),
                    Action::Control {
                        session: cx.id.clone(),
                        control,
                    },
                );
                found.command = match control {
                    Control::Rewinds => Some("/rewind"),
                    _ => None,
                };
                found
            })
            .collect(),
    }
}

fn task(cx: &Cx<'_>) -> Option<ActionGroup> {
    let task = cx.task.filter(|_| cx.role.task_bound())?.clone();
    Some(ActionGroup {
        title: "Task",
        rows: vec![row(
            "task-actions",
            format!("Task {task} actions…"),
            None,
            Action::TaskMenu(task),
        )],
    })
}

/// The catalog of the session `cx` reads, over the tabs its page shows.
pub fn actions(cx: &Cx<'_>) -> SessionActions {
    let tabs = super::shown_tabs(cx);
    let mut groups: Vec<ActionGroup> = [
        Some(views(cx, &tabs)),
        Some(display(cx)),
        Some(run(cx)),
        task(cx),
    ]
    .into_iter()
    .flatten()
    .collect();
    for group in PARTS.iter().filter_map(|part| part(cx)) {
        match groups.iter_mut().find(|known| known.title == group.title) {
            Some(known) => known.rows.extend(group.rows),
            None => groups.push(group),
        }
    }
    groups.retain(|group| !group.rows.is_empty());
    SessionActions { groups }
}
