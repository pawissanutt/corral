//! Tasks: one list of every task in scope, grouped by status in the order Blocked, Review,
//! Running, Ready, Backlog, Idea, with Done and Dropped folded at the bottom; and what a task's
//! row, inspector and page say about it. Every word comes from the served row.

use std::collections::BTreeSet;

use corral_types::{Priority, Status};

use crate::model::daemon::{Feed, Task};
use crate::model::list::{Column, Section};
use crate::model::next::{NextStep, next_step};
use crate::model::pipeline::{PipelineStep, phrase, pipeline};
use crate::model::projects::Scope;
use crate::model::status::{CLOSED, OPEN, folds, status_look, status_word};
use crate::model::{Id, work};

pub const TITLE: &str = "Tasks";
/// What the list says while the open tasks have not arrived.
pub const LOADING: &str = "Loading tasks…";
/// What the list says when the scope holds no task.
pub const EMPTY: &str = "No tasks in this scope.";

/// The list's own state: which of the folding sections are open.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub unfolded: BTreeSet<String>,
}

impl State {
    /// Opens or closes a folding section.
    pub fn toggle(&mut self, key: &str) {
        if !self.unfolded.remove(key) {
            self.unfolded.insert(key.to_owned());
        }
    }
}

fn in_scope<'a>(feed: &'a Feed, scope: &'a Scope) -> impl Iterator<Item = &'a Task> {
    feed.board
        .tasks()
        .iter()
        .filter(move |task| scope.matches(task.project()))
}

/// The list's sections: a group per status that has a task, the closed ones folding.
pub fn sections(feed: &Feed, scope: &Scope, state: &State) -> Vec<Section> {
    OPEN.into_iter()
        .chain(CLOSED)
        .filter_map(|status| {
            let rows: Vec<Id> = in_scope(feed, scope)
                .filter(|task| task.status() == status)
                .map(|task| task.id.clone())
                .collect();
            let key = status.as_str().to_owned();
            let open = state.unfolded.contains(&key);
            let section = Section {
                label: status_word(status).to_owned(),
                look: Some(status_look(status)),
                count: rows.len(),
                note: None,
                fold: folds(status).then_some(open),
                rows: if folds(status) && !open {
                    Vec::new()
                } else {
                    rows
                },
                key,
            };
            section.shown().then_some(section)
        })
        .collect()
}

/// `22 open` over the list: the open tasks in scope.
pub fn summary(feed: &Feed, scope: &Scope) -> String {
    let open = in_scope(feed, scope)
        .filter(|task| !folds(task.status()))
        .count();
    format!("{open} open")
}

/// One task as its row, its inspector and its page head show it.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskLine {
    pub id: Id,
    pub status: Status,
    pub title: String,
    /// `P0`: a priority other than the default, as a tag on the title.
    pub priority: Option<&'static str>,
    /// The plan the task is a child of.
    pub parent: Option<String>,
    /// What is happening, as one plain phrase.
    pub now: String,
    pub pipeline: [PipelineStep; 4],
    pub project: Option<String>,
    /// The run's profile, else the one dispatch would pick.
    pub profile: Option<String>,
    /// How long ago the task last changed: `3h`.
    pub age: String,
    pub next: Option<NextStep>,
}

impl TaskLine {
    /// What a screen reader hears for the row.
    pub fn spoken(&self) -> String {
        let mut spoken = format!("{} {}, {}", self.id, self.title, status_word(self.status));
        if !self.now.is_empty() {
            spoken.push_str(", ");
            spoken.push_str(&self.now);
        }
        spoken
    }
}

const fn priority_word(priority: Priority) -> &'static str {
    match priority {
        Priority::P0 => "P0",
        Priority::P1 => "P1",
        Priority::P2 => "P2",
        Priority::P3 => "P3",
    }
}

/// The default priority says nothing on a row; any other is a tag on the title.
const fn priority_tag(priority: Priority) -> Option<&'static str> {
    match priority {
        Priority::P2 => None,
        other => Some(priority_word(other)),
    }
}

fn line_of(task: &Task, feed: &Feed) -> TaskLine {
    let row = &task.row;
    TaskLine {
        id: task.id.clone(),
        status: task.status(),
        title: task.title().to_owned(),
        priority: priority_tag(task.task().priority),
        parent: task.parent().map(str::to_owned),
        now: phrase(task, feed),
        pipeline: pipeline(task),
        project: task.project().map(str::to_owned),
        profile: row
            .run
            .as_ref()
            .map(|run| run.profile.clone())
            .or_else(|| row.resolved_profile.clone()),
        age: work::age(task.since(), feed.now),
        next: next_step(task),
    }
}

/// The task's line; none for a task the open-task list does not hold.
pub fn line(feed: &Feed, id: &str) -> Option<TaskLine> {
    feed.board.task(id).map(|task| line_of(task, feed))
}

/// What the inspector and the page's Overview add to the line.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskFacts {
    pub line: TaskLine,
    /// A Blocked task's detail under its reason.
    pub blocked_detail: Option<String>,
    /// Labelled facts in display order; a fact the row does not carry is absent.
    pub facts: Vec<(&'static str, String)>,
    /// The task body's Markdown, exactly as the engine stores it.
    pub body: Option<String>,
}

/// The task's facts; none for a task the open-task list does not hold.
pub fn facts(feed: &Feed, id: &str) -> Option<TaskFacts> {
    let task = feed.board.task(id)?;
    let line = line_of(task, feed);
    let inner = task.task();
    let mut facts = Vec::new();
    let mut fact = |label: &'static str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            facts.push((label, value));
        }
    };
    fact("project", line.project.clone());
    fact("profile", line.profile.clone());
    fact("priority", Some(priority_word(inner.priority).to_owned()));
    fact("plan", line.parent.clone());
    fact(
        "depends on",
        (!inner.depends_on.is_empty()).then(|| inner.depends_on.join(", ")),
    );
    fact(
        "base",
        task.row
            .run
            .as_ref()
            .map(|run| run.base.clone())
            .or_else(|| inner.base.clone()),
    );
    fact(
        "branch",
        task.row.run.as_ref().map(|run| run.branch.clone()),
    );
    fact("changed", Some(format!("{} ago", line.age)));
    Some(TaskFacts {
        blocked_detail: task.blocked().and_then(|blocked| blocked.detail.clone()),
        facts,
        body: Some(inner.body.clone()).filter(|body| !body.trim().is_empty()),
        line,
    })
}

/// A task row's columns. Profile gives way first as the list narrows, then project, then the
/// phrase; with one project in scope the project column is left out.
pub fn columns(scope: &Scope) -> Vec<Column> {
    let mut columns = vec![
        Column::fixed("status", "", 14.0, 1.0),
        Column::fixed("id", "ID", 58.0, 6.0),
        Column::flex("title", "Title", 140.0, 16.0, 1.4),
        Column::flex("now", "Now", 110.0, 14.0, 1.0).gives_way(3),
        Column::fixed("pipeline", "run gate rev land", 104.0, 7.0).short("stages"),
    ];
    if scope.project().is_none() {
        columns.push(Column::fixed("project", "Project", 104.0, 12.0).gives_way(2));
    }
    columns.extend([
        Column::fixed("profile", "Profile", 92.0, 11.0).gives_way(1),
        Column::fixed("age", "Age", 44.0, 6.0).end(),
        Column::fixed("next", "Next step", 176.0, 21.0).end(),
    ]);
    columns
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::{bare_task, task_row};

    fn feed() -> Feed {
        let mut feed = Feed::default();
        let task = |id: &str, project: &str, status| {
            let mut task = bare_task(id, id, status, jiff::Timestamp::UNIX_EPOCH);
            task.project = Some(project.into());
            Task::new(task_row(task))
        };
        feed.board.apply_batch(
            1,
            vec![
                task("T-1", "site", Status::Idea),
                task("T-2", "site", Status::Blocked),
                task("T-3", "docs", Status::Ready),
                task("T-4", "site", Status::Done),
                task("T-5", "site", Status::Blocked),
            ],
            &[],
        );
        feed
    }

    /// The list read in id order mixed what waits on the owner with ideas: the groups come in
    /// the order the owner acts on them, and a closed group keeps its rows until it is opened.
    #[test]
    fn sections_come_in_the_owners_order_and_closed_ones_fold() {
        let feed = feed();
        let mut state = State::default();
        let keys = |sections: &[Section]| -> Vec<(String, usize, usize)> {
            sections
                .iter()
                .map(|section| (section.key.clone(), section.count, section.rows.len()))
                .collect()
        };
        let all = sections(&feed, &Scope::All, &state);
        assert_eq!(
            keys(&all),
            [
                ("blocked".to_owned(), 2, 2),
                ("ready".to_owned(), 1, 1),
                ("idea".to_owned(), 1, 1),
                ("done".to_owned(), 1, 0),
            ]
        );
        assert_eq!(all[3].fold, Some(false));
        state.toggle("done");
        assert_eq!(sections(&feed, &Scope::All, &state)[3].rows.len(), 1);
        // The one scope narrows the list, and a status with nothing in scope has no header.
        let docs = sections(&feed, &Scope::Project("docs".into()), &state);
        assert_eq!(keys(&docs), [("ready".to_owned(), 1, 1)]);
        assert_eq!(summary(&feed, &Scope::Project("site".into())), "3 open");
    }

    /// With one project in scope a Project column would repeat it on every row.
    #[test]
    fn the_project_column_is_left_out_under_a_project_scope() {
        let has = |scope: &Scope| columns(scope).iter().any(|column| column.key == "project");
        assert!(has(&Scope::All));
        assert!(!has(&Scope::Project("site".into())));
    }
}
