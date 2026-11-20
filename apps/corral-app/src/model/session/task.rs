//! The Task tab of a task-bound session (worker, reviewer, planner): the run's task as the
//! engine records it (`DetailRef::Task`), shown as its page's Overview shows it, which the
//! screen draws; this module adds what the session page says around it: the summary from the
//! board's row, every attempt the engine recorded for the task with this session's own marked,
//! and the links out (the task's page, the Timeline). A session with no task run has no Task
//! tab at all.

use corral_protocol::detail::DetailRef;
use corral_types::{Outcome, Role};

use crate::model::daemon::{Feed, Group};
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::work::loaded_task;
use crate::model::{Destination, Id, Live};

use super::actions::{ActionGroup, row};
use super::keys::{KeyAction, KeyBinding, bind};
use super::tabs::{PartBody, load, part};
use super::{Action, Cx, Focus, State, Tab};

/// The Task tab's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskUi {}

impl TaskUi {
    pub const EMPTY: Self = Self {};
}

/// What the Task tab does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAction {
    /// The Timeline destination, where the task's lane holds its lifecycle and events.
    Timeline,
}

/// One run the engine recorded for the task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// `attempt-<run>`.
    pub id: String,
    /// `R2 · worker · deep · gpt-6 high`.
    pub text: String,
    /// `landed`, `failed`, `running`.
    pub outcome: String,
    pub glyph: Option<&'static str>,
    pub spinner: bool,
    pub hue: Hue,
    /// `started Nov 19 12:00 · ended 12:20`.
    pub when: String,
    /// The run this session is.
    pub this: bool,
}

/// A way out of the tab to the app's own page for it.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskLink {
    /// `btn-task-<slug>`.
    pub id: &'static str,
    pub label: &'static str,
    pub key: Option<&'static str>,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskTab {
    pub part: PartBody,
    /// The task the story is built for; none when the session has no task run.
    pub task: Option<Id>,
    /// Oldest first; empty until the task's record is read.
    pub attempts: Vec<Attempt>,
    pub links: Vec<TaskLink>,
}

/// The session's task, when it runs for one.
pub(super) fn bound<'a>(cx: &Cx<'a>) -> Option<&'a Id> {
    cx.task.filter(|_| cx.role.task_bound())
}

/// The run's task record; `Err` when the session has no task run.
pub fn target(cx: &Cx<'_>) -> Result<DetailRef, String> {
    match bound(cx) {
        Some(task) => Ok(DetailRef::Task {
            id: task.to_string(),
        }),
        None => Err("this session has no task run".into()),
    }
}

/// `T-12 · Fix the gate cache · Running`, as the board's row says it.
pub fn summary(cx: &Cx<'_>) -> String {
    let Some(task) = cx.task else {
        return "no task".into();
    };
    match cx.feed.board.task(task) {
        Some(row) => format!("{task} · {} · {}", row.title(), row.now()),
        None => match loaded_task(cx.feed, task.as_str()) {
            Some(detail) => format!(
                "{task} · {} · {}",
                detail.task.title,
                detail.task.status.as_str()
            ),
            None => task.to_string(),
        },
    }
}

fn attempts(cx: &Cx<'_>, task: &Id) -> Vec<Attempt> {
    let Some(detail) = loaded_task(cx.feed, task.as_str()) else {
        return Vec::new();
    };
    let this = cx.run_id();
    let mut runs: Vec<_> = detail.runs.iter().collect();
    runs.sort_by_key(|run| run.started);
    runs.into_iter()
        .map(|run| {
            let model = [run.model.as_deref(), run.effort.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            let mut text = format!("{} · {} · {}", run.id, run.role.as_str(), run.profile);
            if !model.is_empty() {
                text.push_str(&format!(" · {model}"));
            }
            let (outcome, glyph, spinner, hue) = match (run.outcome, run.ended) {
                (Some(Outcome::Landed), _) => ("landed", Some("✓"), false, Hue::Ok),
                (Some(Outcome::Done), _) => ("done", Some("✓"), false, Hue::Ok),
                (Some(Outcome::Failed), _) => ("failed", Some("✗"), false, Hue::Fail),
                (Some(Outcome::Stopped), _) => ("stopped", Some("○"), false, Hue::Neutral),
                (Some(Outcome::Superseded), _) => ("superseded", Some("○"), false, Hue::Neutral),
                (None, Some(_)) => ("ended, no outcome recorded", Some("○"), false, Hue::Neutral),
                (None, None) => (
                    "running",
                    None,
                    true,
                    if run.role == Role::Reviewer {
                        Hue::Corral
                    } else {
                        Hue::Worker
                    },
                ),
            };
            let mut when = format!(
                "started {}",
                crate::model::work::when_at(run.started, cx.feed)
            );
            if let Some(ended) = run.ended {
                when.push_str(&format!(
                    " · ended {}",
                    crate::model::work::when_at(ended, cx.feed)
                ));
            }
            Attempt {
                id: format!("attempt-{}", run.id),
                text,
                outcome: outcome.into(),
                glyph,
                spinner,
                hue,
                when,
                this: this.as_deref() == Some(run.id.as_str()),
            }
        })
        .collect()
}

fn links(task: &Id) -> Vec<TaskLink> {
    vec![
        TaskLink {
            id: "btn-task-page",
            label: "Open task",
            key: None,
            action: Action::OpenLive(Live::Task(task.clone())),
        },
        TaskLink {
            id: "btn-task-timeline",
            label: "Timeline",
            key: Some("g"),
            action: Action::Task(TaskAction::Timeline),
        },
    ]
}

pub fn task_tab(cx: &Cx<'_>) -> TaskTab {
    let task = bound(cx);
    TaskTab {
        part: part(cx, target(cx), None, summary(cx), Hue::Neutral),
        task: task.cloned(),
        attempts: task.map_or_else(Vec::new, |task| attempts(cx, task)),
        links: task.map_or_else(Vec::new, links),
    }
}

/// What the tab reads while it shows.
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    load(cx, target(cx), None).into_iter().collect()
}

pub(super) fn act(_: &mut State, action: TaskAction, _: &Feed) -> Vec<Effect> {
    match action {
        TaskAction::Timeline => vec![Effect::Go(Destination::Timeline)],
    }
}

/// The Task group: the task's decisions while it waits on the owner, the task's page, its
/// Timeline.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    let task = bound(cx)?;
    let mut rows = Vec::new();
    if cx
        .feed
        .board
        .in_group(Group::NeedsYou)
        .any(|found| found.id == *task)
    {
        rows.push(row(
            "open-decisions",
            "Open decisions",
            None,
            Action::OpenLive(Live::Task(task.clone())),
        ));
    }
    rows.push(row(
        "task-page",
        format!("Open {task}"),
        None,
        Action::OpenLive(Live::Task(task.clone())),
    ));
    rows.push(row(
        "task-timeline",
        "Task timeline",
        Some("g"),
        Action::Task(TaskAction::Timeline),
    ));
    Some(ActionGroup {
        title: "Task",
        rows,
    })
}

/// The tab's own keys, taken only while the Task tab has the keys.
pub fn keys() -> Vec<KeyBinding> {
    vec![
        bind("r", "r", "reread task", KeyAction::Part),
        bind("g", "g", "task timeline", KeyAction::Part),
        bind("l", "l", "task events", KeyAction::Part),
    ]
}

pub fn press(key: &str, cx: &Cx<'_>) -> Option<Action> {
    if cx.ui.tab != Tab::Task || cx.ui.focus == Focus::Composer {
        return None;
    }
    match key {
        "r" => Some(Action::Refresh(target(cx).ok()?)),
        "g" | "l" => bound(cx).map(|_| Action::Task(TaskAction::Timeline)),
        _ => None,
    }
}
