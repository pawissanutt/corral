//! Tasks: one list of every task in scope, grouped by status in the order Blocked, Review,
//! Running, Ready, Backlog, Idea, with Done and Dropped folded at the bottom and read a served
//! page at a time; a plan unfolds its children in place under its row. What a task's row, its
//! inspector and its page say about it, every word from the served row, the closed page that
//! lists it, or its served detail.

pub mod band;
pub mod closed;
pub mod page;

use std::borrow::Cow;
use std::collections::BTreeSet;

use corral_types::{Priority, Status};

use crate::model::daemon::{Feed, Link, Task};
use crate::model::list::{Column, Section};
use crate::model::next::{NextStep, next_step};
use crate::model::pipeline::{PipelineStep, phrase, pipeline};
use crate::model::projects::Scope;
use crate::model::status::{OPEN, folds, status_look, status_word};
use crate::model::{Id, TaskTab, work};

pub use closed::Closed;

pub const TITLE: &str = "Tasks";
/// What the list says while the open tasks have not arrived.
pub const LOADING: &str = "Loading tasks…";
/// What the list says when the open tasks cannot arrive; the board's banner says why.
pub const UNAVAILABLE: &str = "Tasks are unavailable until corrald serves the board.";
/// What the list says when the scope holds no task.
pub const EMPTY: &str = "No tasks in this scope.";
/// What the list says when the filter's words match no task.
pub const NO_MATCH: &str = "No task matches these words.";
/// The filter field's words before anything is typed, and its spoken name.
pub const FILTER: &str = "Filter tasks…";
pub const FILTER_LABEL: &str = "Filter tasks by id, title or project";
/// A fact corrald does not serve: the Cost column's every cell, as no per-task cost is on the
/// wire.
pub const ABSENT: &str = "–";
/// The Cost column's hint, saying why it is empty.
pub const COST_ABSENT: &str = "corrald serves no cost per task";

/// The list's own state: the folds that are open, the plans unfolded in place, the filter's
/// words and the closed pages loaded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub unfolded: BTreeSet<String>,
    pub plans: BTreeSet<Id>,
    pub filter: String,
    pub closed: Closed,
}

impl State {
    /// Opens or closes a folding section; folding Done and Dropped lets go of its older pages.
    pub fn toggle(&mut self, key: &str) {
        if !self.unfolded.remove(key) {
            self.unfolded.insert(key.to_owned());
        } else if key == closed::KEY {
            self.closed.fold();
        }
    }

    /// Unfolds a plan's children under its row, or folds them away.
    pub fn unfold(&mut self, plan: &Id) {
        if !self.plans.remove(plan) {
            self.plans.insert(plan.clone());
        }
    }
}

/// How far the open tasks are known. Until the board is complete a place counts nothing as
/// zero and says nothing is there: the tasks are still coming, or they cannot come because the
/// tasks topic failed or corrald is unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Board {
    Complete,
    Loading,
    Unavailable,
}

pub fn board(feed: &Feed) -> Board {
    if feed.board.complete {
        Board::Complete
    } else if feed.link == Link::Down || work::daemon_status(feed).banner.is_some() {
        Board::Unavailable
    } else {
        Board::Loading
    }
}

/// What the list says beside its rows: why it has none, or, while the board is not complete,
/// that the rows it holds are not every task.
pub fn notice(feed: &Feed, empty: bool, filtered: bool) -> Option<&'static str> {
    match board(feed) {
        Board::Loading => Some(LOADING),
        Board::Unavailable => Some(UNAVAILABLE),
        Board::Complete if !empty => None,
        Board::Complete if filtered => Some(NO_MATCH),
        Board::Complete => Some(EMPTY),
    }
}

/// What joins a child's id to its plan's in the row key of the child listed under the plan.
const UNDER: &str = "-in-";

/// The row key of `child` listed under its unfolded `plan`: another row than the child's own in
/// its status group, so each is selected on its own.
pub fn child_key(plan: &str, child: &str) -> Id {
    Id::from(format!("{child}{UNDER}{plan}"))
}

/// The task a row stands for: the child for a row under its plan, else the row's own id.
pub fn task_of(row: &Id) -> Id {
    match row.as_str().split_once(UNDER) {
        Some((child, _)) => Id::from(child),
        None => row.clone(),
    }
}

/// The task page's tab a key shows: `1` Overview, `2` Changes, `3` History.
pub fn page_tab(key: &str) -> Option<TaskTab> {
    let at = key.parse::<usize>().ok()?.checked_sub(1)?;
    TaskTab::ALL.get(at).copied()
}

/// The filter's words, each of which a task must contain in its id, title or project.
pub fn words(filter: &str) -> Vec<String> {
    filter.split_whitespace().map(str::to_lowercase).collect()
}

pub(crate) fn matches(task: &corral_types::Task, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let text = format!(
        "{} {} {}",
        task.id,
        task.title,
        task.project.as_deref().unwrap_or_default()
    )
    .to_lowercase();
    words.iter().all(|word| text.contains(word.as_str()))
}

fn in_scope<'a>(feed: &'a Feed, scope: &'a Scope) -> impl Iterator<Item = &'a Task> {
    feed.board
        .tasks()
        .iter()
        .filter(move |task| scope.matches(task.project()))
}

/// Whether a task is a plan: corrald lists it among the parents, or the board holds a child.
fn is_plan(feed: &Feed, id: &str) -> bool {
    feed.plans.contains_key(id) || feed.board.children(id).next().is_some()
}

/// The open children of a plan in scope, in the list's status order.
fn children<'a>(feed: &'a Feed, scope: &Scope, plan: &'a str) -> Vec<&'a Task> {
    let mut children: Vec<&Task> = feed
        .board
        .children(plan)
        .filter(|child| scope.matches(child.project()))
        .collect();
    children.sort_by_key(|child| OPEN.iter().position(|status| *status == child.status()));
    children
}

/// The list's sections: a group per open status that has a task matching the filter, each
/// unfolded plan followed by its children; then the Done and Dropped fold.
pub fn sections(feed: &Feed, scope: &Scope, state: &State) -> Vec<Section> {
    let words = words(&state.filter);
    let mut sections: Vec<Section> = OPEN
        .into_iter()
        .filter_map(|status| {
            let mut rows = Vec::new();
            let mut count = 0;
            let listed = in_scope(feed, scope)
                .filter(|task| task.status() == status && matches(task.task(), &words));
            for task in listed {
                count += 1;
                rows.push(task.id.clone());
                if state.plans.contains(&task.id) {
                    rows.extend(
                        children(feed, scope, task.id.as_str())
                            .into_iter()
                            .map(|child| child_key(task.id.as_str(), child.id.as_str())),
                    );
                }
            }
            (count > 0).then(|| Section {
                key: status.as_str().to_owned(),
                label: status_word(status).to_owned(),
                look: Some(status_look(status)),
                count,
                note: None,
                fold: None,
                rows,
            })
        })
        .collect();
    sections.extend(closed::section(
        feed,
        scope,
        &words,
        &state.closed,
        state.unfolded.contains(closed::KEY),
    ));
    sections
}

/// `22 open · slots 3 of 5 used` over the list: the open tasks in scope, and the running slots
/// corrald counts (a project with its own limit counts its own). The open count is unknown
/// until the board is complete.
pub fn summary(feed: &Feed, scope: &Scope) -> String {
    let open = if feed.board.complete {
        in_scope(feed, scope)
            .filter(|task| !folds(task.status()))
            .count()
            .to_string()
    } else {
        ABSENT.to_owned()
    };
    let slots = feed
        .health
        .as_ref()
        .and_then(|health| health.slots.as_ref());
    let count = slots.map(|slots| {
        scope
            .project()
            .and_then(|project| slots.projects.get(project.as_str()))
            .unwrap_or(&slots.global)
    });
    match count {
        Some(count) => format!("{open} open · slots {} of {} used", count.used, count.limit),
        None => format!("{open} open"),
    }
}

/// The task by id: its board row, else its row on a closed page, else its served detail.
pub fn task_at<'a>(feed: &'a Feed, id: &str) -> Option<Cow<'a, Task>> {
    if let Some(task) = feed.board.task(id) {
        return Some(Cow::Borrowed(task));
    }
    if let Some(detail) = work::loaded_task(feed, id) {
        return Some(Cow::Owned(work::detail_task(detail)));
    }
    closed::find(feed, id).map(|task| {
        Cow::Owned(Task::new(corral_protocol::rows::TaskRow {
            task: task.clone(),
            ..Default::default()
        }))
    })
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
    /// Whether the task is a plan, which unfolds its children in the list.
    pub plan: bool,
    /// What is happening, as one plain phrase.
    pub now: String,
    pub pipeline: [PipelineStep; 4],
    pub project: Option<String>,
    /// The run's profile, else the one dispatch would pick.
    pub profile: Option<String>,
    /// How long ago an open task last changed (`3h`); the date a closed one closed.
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

fn profile_of(task: &Task) -> Option<String> {
    task.row
        .run
        .as_ref()
        .map(|run| run.profile.clone())
        .or_else(|| task.row.resolved_profile.clone())
}

fn line_of(task: &Task, feed: &Feed) -> TaskLine {
    let status = task.status();
    TaskLine {
        id: task.id.clone(),
        status,
        title: task.title().to_owned(),
        priority: priority_tag(task.task().priority),
        parent: task.parent().map(str::to_owned),
        plan: is_plan(feed, task.id.as_str()),
        now: phrase(task, feed),
        pipeline: pipeline(task),
        project: task.project().map(str::to_owned),
        profile: profile_of(task),
        age: if folds(status) {
            work::stamp_at(task.task().updated, feed)
        } else {
            work::age(task.since(), feed.now)
        },
        next: next_step(task),
    }
}

/// The task's line, wherever the client holds the task.
pub fn line(feed: &Feed, id: &str) -> Option<TaskLine> {
    task_at(feed, id).map(|task| line_of(&task, feed))
}

/// What the inspector and the page add to the line.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskFacts {
    pub line: TaskLine,
    /// Labelled facts in display order; a fact corrald does not serve is absent.
    pub facts: Vec<(&'static str, String)>,
    /// The task body's Markdown, exactly as the engine stores it.
    pub body: Option<String>,
}

/// The facts the page's meta line shows, in order; the others are the inspector's.
const META: [&str; 7] = [
    "project", "priority", "kind", "size", "base", "profile", "round",
];

impl TaskFacts {
    /// The page's meta line: `project site · priority P1 · base main`.
    pub fn meta(&self) -> Vec<(&'static str, String)> {
        self.facts
            .iter()
            .filter(|(label, _)| META.contains(label))
            .cloned()
            .collect()
    }
}

/// `1 of 2`: the round the task is in against its limit, once it has had a run. The engine
/// counts from zero until its first send-back.
fn round(task: &Task) -> Option<String> {
    task.row.run.as_ref()?;
    let round = task.task().round.max(1);
    Some(match task.row.max_rounds {
        Some(max) => format!("{round} of {max}"),
        None => round.to_string(),
    })
}

/// The task's facts, wherever the client holds the task.
pub fn facts(feed: &Feed, id: &str) -> Option<TaskFacts> {
    let task = task_at(feed, id)?;
    let line = line_of(&task, feed);
    let inner = task.task();
    let run = task.row.run.as_ref();
    let mut facts = Vec::new();
    let mut fact = |label: &'static str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            facts.push((label, value));
        }
    };
    fact("project", line.project.clone());
    fact("priority", Some(priority_word(inner.priority).to_owned()));
    fact("kind", inner.kind.clone());
    fact("size", inner.size.clone());
    fact(
        "base",
        run.map(|run| run.base.clone())
            .or_else(|| inner.base.clone()),
    );
    fact(
        "profile",
        match (run, &task.row.resolved_profile) {
            (Some(run), _) => Some(run.profile.clone()),
            (None, Some(resolved)) => Some(format!("{resolved} (dispatch default)")),
            (None, None) => inner.profile.clone(),
        },
    );
    fact("round", round(&task));
    fact("plan", line.parent.clone());
    fact(
        "depends on",
        (!inner.depends_on.is_empty()).then(|| inner.depends_on.join(", ")),
    );
    fact("branch", run.map(|run| run.branch.clone()));
    fact(
        "changed",
        Some(work::when_at(inner.updated, feed)).filter(|when| !when.is_empty()),
    );
    Some(TaskFacts {
        body: Some(inner.body.clone()).filter(|body| !body.trim().is_empty()),
        facts,
        line,
    })
}

/// A task row's columns. Profile gives way first as the list narrows, then project, then
/// cost, then the phrase; with one project in scope the project column is left out.
pub fn columns(scope: &Scope) -> Vec<Column> {
    let mut columns = vec![
        Column::fixed("status", "", 14.0, 1.0),
        Column::fixed("id", "ID", 58.0, 6.0),
        Column::flex("title", "Title", 140.0, 16.0, 1.4),
        Column::flex("now", "Now", 110.0, 14.0, 1.0).gives_way(4),
        Column::fixed("pipeline", "run gate rev land", 104.0, 7.0).short("stages"),
    ];
    if scope.project().is_none() {
        columns.push(Column::fixed("project", "Project", 104.0, 12.0).gives_way(2));
    }
    columns.extend([
        Column::fixed("profile", "Profile", 92.0, 11.0).gives_way(1),
        Column::fixed("cost", "Cost", 50.0, 5.0).end().gives_way(3),
        Column::fixed("age", "Age", 50.0, 6.0).end(),
        Column::fixed("next", "Next step", 176.0, 21.0).end(),
    ]);
    columns
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::{bare_task, task_row};

    fn task(id: &str, project: &str, status: Status, parent: Option<&str>) -> Task {
        let mut task = bare_task(id, id, status, jiff::Timestamp::UNIX_EPOCH);
        task.project = Some(project.into());
        task.parent = parent.map(str::to_owned);
        Task::new(task_row(task))
    }

    fn feed() -> Feed {
        let mut feed = Feed::default();
        feed.board.apply_batch(
            1,
            vec![
                task("T-1", "site", Status::Idea, None),
                task("T-2", "site", Status::Blocked, Some("T-1")),
                task("T-3", "docs", Status::Ready, None),
                task("T-5", "site", Status::Blocked, None),
            ],
            &[],
        );
        feed
    }

    fn keys(sections: &[Section]) -> Vec<(String, usize, Vec<String>)> {
        sections
            .iter()
            .map(|section| {
                let rows = section.rows.iter().map(ToString::to_string).collect();
                (section.key.clone(), section.count, rows)
            })
            .collect()
    }

    /// The list read in id order mixed what waits on the owner with ideas: the groups come in
    /// the order the owner acts on them, and the scope and the filter narrow every group.
    #[test]
    fn sections_come_in_the_owners_order_narrowed_by_scope_and_filter() {
        let feed = feed();
        let mut state = State::default();
        let all = sections(&feed, &Scope::All, &state);
        let order: Vec<String> = all.iter().map(|section| section.key.clone()).collect();
        assert_eq!(order, ["blocked", "ready", "idea"]);
        let docs = sections(&feed, &Scope::Project("docs".into()), &state);
        assert_eq!(keys(&docs), [("ready".into(), 1, vec!["T-3".into()])]);
        state.filter = "t-5 SITE".into();
        assert_eq!(
            keys(&sections(&feed, &Scope::All, &state)),
            [("blocked".into(), 1, vec!["T-5".into()])]
        );
        assert_eq!(summary(&feed, &Scope::Project("site".into())), "3 open");
    }

    /// An unfolded plan listed its child only in the child's own group, so the plan read as
    /// empty: the child is listed under the plan as well, as its own row standing for the child.
    #[test]
    fn an_unfolded_plan_lists_its_children_under_it_as_rows_of_their_own() {
        let feed = feed();
        let mut state = State::default();
        state.unfold(&Id::from("T-1"));
        let idea = sections(&feed, &Scope::All, &state)
            .into_iter()
            .find(|section| section.key == "idea")
            .map(|section| section.rows);
        let under = child_key("T-1", "T-2");
        assert_eq!(idea, Some(vec![Id::from("T-1"), under.clone()]));
        assert_eq!(task_of(&under), Id::from("T-2"));
        assert_eq!(task_of(&Id::from("T-2")), Id::from("T-2"));
        assert!(line(&feed, "T-1").is_some_and(|line| line.plan));
    }

    /// A tasks topic that failed before the first board left Tasks loading forever, with no
    /// reason and no way to retry: the board reads as unavailable, and its banner offers a
    /// Retry that can be pressed.
    #[test]
    fn a_tasks_topic_failed_before_the_board_is_unavailable_with_retry() {
        use crate::model::daemon::{ClientError, TopicState};

        let mut feed = Feed::default();
        feed.topics.insert(
            "tasks".into(),
            TopicState {
                loading: false,
                stale: true,
                error: Some(ClientError {
                    code: "unavailable".into(),
                    message: "board unavailable".into(),
                }),
            },
        );
        assert_eq!(board(&feed), Board::Unavailable);
        assert_ne!(notice(&feed, true, false), Some(LOADING));
        let retry = work::daemon_status(&feed)
            .banner
            .and_then(|banner| banner.actions.into_iter().next());
        assert!(retry.is_some_and(|button| {
            button.enabled && button.action == Some(work::Press::RetryBoard)
        }));
    }

    /// With one project in scope a Project column would repeat it on every row.
    #[test]
    fn the_project_column_is_left_out_under_a_project_scope() {
        let has = |scope: &Scope| columns(scope).iter().any(|column| column.key == "project");
        assert!(has(&Scope::All));
        assert!(!has(&Scope::Project("site".into())));
    }
}
