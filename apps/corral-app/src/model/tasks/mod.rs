//! Tasks: one list of every task in scope, grouped by status in the order Blocked, Review,
//! Running, Ready, Backlog, Idea, with Done and Dropped folded at the bottom and read a served
//! page at a time. A plan unfolds children in its own status group; children in other groups
//! are not repeated under it. A task's row, inspector and page use its served row or detail.

pub mod band;
pub mod closed;
pub mod columns;
pub mod meta;
pub mod page;

use std::borrow::Cow;
use std::collections::BTreeSet;

use corral_types::{Priority, Status, TaskDifficulty};

use crate::model::daemon::{Feed, Link, Task};
use crate::model::list::{Column, Section};
use crate::model::next::{NextStep, next_step};
use crate::model::pipeline::{PipelineStep, observed_pipeline, row_phrase};
use crate::model::projects::Scope;
use crate::model::status::{OPEN, folds, status_look, status_word};
use crate::model::{Id, TaskTab, work};

pub use closed::Closed;
pub use meta::Filter;

pub const TITLE: &str = "Tasks";
/// What the list says while the open tasks have not arrived.
pub const LOADING: &str = "Loading tasks…";
/// What the list says when the open tasks cannot arrive; the board's banner says why.
pub const UNAVAILABLE: &str = "Tasks are unavailable until corrald serves the board.";
/// What the list says when the scope holds no task.
pub const EMPTY: &str = "No tasks in this scope.";
/// What the list says when the filter's words match no task.
pub const NO_MATCH: &str = "No task matches this filter.";
/// The filter field's words before anything is typed, and its spoken name.
pub const FILTER: &str = "Filter tasks…";
pub const FILTER_LABEL: &str =
    "Filter tasks by id, title, project or tag words, or size:, difficulty: and tag: exactly";
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

    /// Unfolds a plan's children in the same status group, or folds them away.
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

/// Keep the task selected when its inline row disappears but its canonical row remains.
pub fn settle_selection(order: &[Id], selected: Option<&Id>) -> Option<Id> {
    selected
        .and_then(|row| {
            if order.contains(row) {
                Some(row.clone())
            } else {
                let task = task_of(row);
                order.contains(&task).then_some(task)
            }
        })
        .or_else(|| order.first().cloned())
}

/// The task page's tab a key shows: `1` Overview, `2` Changes, `3` History.
pub fn page_tab(key: &str) -> Option<TaskTab> {
    let at = key.parse::<usize>().ok()?.checked_sub(1)?;
    TaskTab::ALL.get(at).copied()
}

fn in_scope<'a>(feed: &'a Feed, scope: &'a Scope) -> impl Iterator<Item = &'a Task> {
    feed.board
        .tasks()
        .iter()
        .filter(move |task| scope.matches(task.project()))
}

/// Whether corrald lists the task as a plan or the board holds one of its children.
fn is_plan(feed: &Feed, id: &str) -> bool {
    feed.plans.contains_key(id) || feed.board.children(id).next().is_some()
}

/// Only children in the plan's status group belong under its row. Children in other groups
/// already have their own rows and remain linked from the plan's detail.
fn inline_children<'a>(
    feed: &'a Feed,
    scope: &'a Scope,
    filter: &'a Filter,
    plan: &'a Task,
) -> impl Iterator<Item = &'a Task> {
    feed.board.children(plan.id.as_str()).filter(move |child| {
        child.status() == plan.status()
            && scope.matches(child.project())
            && filter.matches(child.task())
    })
}

/// Whether the plan's caret would reveal any children with the current scope and filter.
pub fn has_inline_children(feed: &Feed, scope: &Scope, state: &State, plan: &str) -> bool {
    feed.board.task(plan).is_some_and(|plan| {
        !folds(plan.status())
            && inline_children(feed, scope, &Filter::parse(&state.filter), plan)
                .next()
                .is_some()
    })
}

/// The list's sections: a group per open status that has a task matching the filter, each
/// unfolded plan followed by its same-status children; then the Done and Dropped fold.
pub fn sections(feed: &Feed, scope: &Scope, state: &State) -> Vec<Section> {
    let filter = Filter::parse(&state.filter);
    let mut sections: Vec<Section> = OPEN
        .into_iter()
        .filter_map(|status| {
            let mut rows = Vec::new();
            let mut count = 0;
            let listed = in_scope(feed, scope)
                .filter(|task| task.status() == status && filter.matches(task.task()));
            for task in listed {
                count += 1;
                rows.push(task.id.clone());
                if state.plans.contains(&task.id) {
                    rows.extend(
                        inline_children(feed, scope, &filter, task)
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
        &filter,
        &state.closed,
        state.unfolded.contains(closed::KEY),
    ));
    sections
}

/// The open tasks in scope and the dispatch slots corrald counts (a project with its own
/// limit counts its own). The open count is unknown until the board is complete.
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
        Some(count) => format!(
            "{open} open · slots {}/{} incl. Review",
            count.used, count.limit
        ),
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
    /// `P0`: a priority other than the default, as a tag on the title while the Priority
    /// column is hidden.
    pub priority: Option<&'static str>,
    /// `P2`: the priority, default included, as the Priority column shows it.
    pub rank: &'static str,
    /// The plan the task is a child of.
    pub parent: Option<String>,
    /// What is happening, as one plain phrase.
    pub now: String,

    pub pipeline: [PipelineStep; 4],
    pub project: Option<String>,
    /// The run's profile, else the one dispatch would pick.
    pub profile: Option<String>,
    /// How much work, exactly as stored (an older task may carry a word off the scale).
    pub size: Option<String>,
    /// How complex or uncertain, apart from its size.
    pub difficulty: Option<TaskDifficulty>,
    /// Exact labels, in the task's order.
    pub tags: Vec<String>,
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
        spoken.push_str(&format!(
            ", size {}, difficulty {}",
            meta::size_word(self.size.as_deref()),
            meta::difficulty_word(self.difficulty)
        ));
        if !self.tags.is_empty() {
            spoken.push_str(", tags ");
            spoken.push_str(&self.tags.join(", "));
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
        rank: priority_word(task.task().priority),
        parent: task.parent().map(str::to_owned),
        now: row_phrase(task, feed),
        pipeline: observed_pipeline(task, feed),
        project: task.project().map(str::to_owned),
        profile: profile_of(task),
        size: task.task().size.clone(),
        difficulty: task.task().difficulty,
        tags: task.task().tags.clone(),
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
const META: [&str; 6] = ["project", "priority", "kind", "base", "profile", "round"];

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
    fact("size", Some(meta::size_word(inner.size.as_deref())));
    fact("difficulty", Some(meta::difficulty_word(inner.difficulty)));
    fact("tags", Some(meta::tags_word(&inner.tags)));
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

/// A task row's columns, every optional one included ([`columns::Chosen`] says which show).
/// Profile, project, cost and the phrase give way before the planning estimates. A chosen
/// Priority column never does: it is three Cells, and while it is chosen the title carries no
/// priority tag, so its giving way would leave the row without one. With one project in scope the project column is left out. The title and phrase stop
/// growing at a readable line.
pub fn columns(scope: &Scope) -> Vec<Column> {
    let mut columns = vec![
        Column::fixed("status", "", 14.0, 1.0),
        Column::fixed("id", "ID", 58.0, 6.0),
        Column::fixed("priority", "Pri", 30.0, 3.0),
        Column::flex("title", "Title", 140.0, 16.0, 1.4).most(480.0, 56.0),
        Column::flex("now", "Now", 110.0, 14.0, 1.0)
            .most(360.0, 40.0)
            .gives_way(4),
        Column::fixed("pipeline", "run gate rev land", 104.0, 7.0).short("stages"),
        Column::fixed("size", "Size", 40.0, 5.0).gives_way(6),
        Column::fixed("difficulty", "Difficulty", 70.0, 9.0)
            .short("Diff.")
            .gives_way(5),
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

    #[test]
    fn a_dispatched_child_leaves_its_unfolded_plans_backlog_group() {
        let mut feed = Feed::default();
        feed.board.apply_batch(
            1,
            vec![
                task("T-1", "site", Status::Backlog, None),
                task("T-2", "site", Status::Running, None),
                task("T-3", "site", Status::Backlog, Some("T-1")),
            ],
            &[],
        );
        let mut state = State::default();
        state.unfold(&Id::from("T-1"));
        assert!(has_inline_children(&feed, &Scope::All, &state, "T-1"));
        let under = child_key("T-1", "T-3");
        let before = sections(&feed, &Scope::All, &state);
        assert_eq!(
            keys(&before),
            [
                ("running".into(), 1, vec!["T-2".into()]),
                (
                    "backlog".into(),
                    2,
                    vec!["T-1".into(), under.to_string(), "T-3".into()]
                ),
            ]
        );
        assert_eq!(
            settle_selection(&crate::model::list::order(&before), Some(&under)),
            Some(under.clone())
        );

        feed.board.apply_batch(
            1,
            vec![task("T-3", "site", Status::Running, Some("T-1"))],
            &[],
        );
        let after = sections(&feed, &Scope::All, &state);
        assert_eq!(
            keys(&after),
            [
                ("running".into(), 2, vec!["T-2".into(), "T-3".into()]),
                ("backlog".into(), 1, vec!["T-1".into()]),
            ]
        );
        assert_eq!(
            settle_selection(&crate::model::list::order(&after), Some(&under)),
            Some(Id::from("T-3"))
        );
        assert!(!has_inline_children(&feed, &Scope::All, &state, "T-1"));
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

    /// The default P2 said nothing on a row: the Priority column names every priority, while
    /// the title's tag stays for a priority other than the default.
    #[test]
    fn the_priority_column_names_the_default_priority_too() {
        let mut urgent = bare_task("T-7", "T-7", Status::Ready, jiff::Timestamp::UNIX_EPOCH);
        urgent.priority = Priority::P0;
        let mut feed = feed();
        feed.board
            .apply_batch(1, vec![Task::new(task_row(urgent))], &[]);
        let ranks = |id: &str| line(&feed, id).map(|line| (line.rank, line.priority));
        assert_eq!(ranks("T-3"), Some(("P2", None)));
        assert_eq!(ranks("T-7"), Some(("P0", Some("P0"))));
        assert!(columns(&Scope::All).iter().any(|c| c.key == "priority"));
    }

    /// With one project in scope a Project column would repeat it on every row.
    #[test]
    fn the_project_column_is_left_out_under_a_project_scope() {
        let has = |scope: &Scope| columns(scope).iter().any(|column| column.key == "project");
        assert!(has(&Scope::All));
        assert!(!has(&Scope::Project("site".into())));
    }
}
