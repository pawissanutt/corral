//! Filed board-plan parents: summaries of their children, never work of their own. corrald
//! marks one by serving its children's counts on its row; its shown state follows them, not
//! its own stored status. Active plans list together after the open groups; Complete and
//! Closed ones fold into Completed plans, and a reopened child brings its plan back.

use corral_types::Status;

use crate::model::Id;
use crate::model::daemon::{Feed, Task};
use crate::model::list::Section;
use crate::model::projects::Scope;

use super::Filter;

/// The section of plans with a child still open.
pub const ACTIVE_KEY: &str = "plans";
pub const ACTIVE_LABEL: &str = "Plans";
/// The fold of plans whose children have all closed.
pub const COMPLETED_KEY: &str = "completed-plans";
pub const COMPLETED_LABEL: &str = "Completed plans";

/// A filed plan's state as its children make it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanState {
    /// At least one child is neither Done nor Dropped (or it has none yet).
    Active { done: u64, total: u64 },
    /// Every child is Done.
    Complete { done: u64 },
    /// Every child is Done or Dropped, and at least one was Dropped.
    Closed { done: u64, dropped: u64 },
}

impl PlanState {
    pub fn completed(self) -> bool {
        !matches!(self, PlanState::Active { .. })
    }

    /// The status whose mark the row shows, whatever the parent's stored one: Done and
    /// Dropped stay distinct among completed plans; an active one shows open work's mark.
    pub fn shown(self) -> Status {
        match self {
            PlanState::Active { .. } => Status::Backlog,
            PlanState::Complete { .. } => Status::Done,
            PlanState::Closed { .. } => Status::Dropped,
        }
    }

    /// `Plan · 2 of 5 done`, `Plan complete · 5 done`, `Plan closed · 3 done, 2 dropped`.
    pub fn phrase(self) -> String {
        match self {
            PlanState::Active { done, total } => format!("Plan · {done} of {total} done"),
            PlanState::Complete { done } => format!("Plan complete · {done} done"),
            PlanState::Closed { done, dropped } => {
                format!("Plan closed · {done} done, {dropped} dropped")
            }
        }
    }
}

/// Whether the task is a filed plan parent, which is a summary rather than work.
pub fn is_summary(task: &Task) -> bool {
    task.row.plan_children.is_some()
}

/// The plan's state from its children's counts; none for a task that is not a filed plan.
pub fn state(task: &Task) -> Option<PlanState> {
    let counts = task.row.plan_children.as_ref()?;
    let count = |status: Status| {
        counts
            .iter()
            .filter(|count| count.status == status)
            .map(|count| count.count)
            .sum::<u64>()
    };
    let total: u64 = counts.iter().map(|count| count.count).sum();
    let (done, dropped) = (count(Status::Done), count(Status::Dropped));
    Some(if total == 0 || done + dropped < total {
        PlanState::Active { done, total }
    } else if dropped == 0 {
        PlanState::Complete { done }
    } else {
        PlanState::Closed { done, dropped }
    })
}

/// The Plans section and the Completed plans fold, each left out when no plan belongs there.
pub fn sections(feed: &Feed, scope: &Scope, filter: &Filter, completed_open: bool) -> Vec<Section> {
    let (mut active, mut completed) = (Vec::new(), Vec::new());
    for task in feed.board.tasks() {
        // A parent's own stored status, even one closed by hand, does not decide its place.
        if !scope.matches(task.project()) || !filter.matches(task.task()) {
            continue;
        }
        match state(task) {
            Some(state) if state.completed() => completed.push(task.id.clone()),
            Some(_) => active.push(task.id.clone()),
            None => {}
        }
    }
    let section = |key: &str, label: &str, rows: Vec<Id>, fold: Option<bool>| Section {
        key: key.to_owned(),
        label: label.to_owned(),
        look: None,
        count: rows.len(),
        note: None,
        rows: if fold == Some(false) {
            Vec::new()
        } else {
            rows
        },
        fold,
    };
    let mut sections = Vec::new();
    if !active.is_empty() {
        sections.push(section(ACTIVE_KEY, ACTIVE_LABEL, active, None));
    }
    if !completed.is_empty() {
        sections.push(section(
            COMPLETED_KEY,
            COMPLETED_LABEL,
            completed,
            Some(completed_open),
        ));
    }
    sections
}

#[cfg(test)]
mod tests {
    use corral_protocol::rows::StatusCount;

    use super::*;
    use crate::model::daemon::{bare_task, task_row};
    use crate::model::tasks::{State, line, summary};

    fn task(id: &str, status: Status, children: Option<&[(Status, u64)]>) -> Task {
        let mut row = task_row(bare_task(id, id, status, jiff::Timestamp::UNIX_EPOCH));
        row.plan_children = children.map(|counts| {
            counts
                .iter()
                .map(|&(status, count)| StatusCount { status, count })
                .collect()
        });
        Task::new(row)
    }

    fn board(tasks: Vec<Task>) -> Feed {
        let mut feed = Feed::default();
        feed.board.apply_batch(1, tasks, &[]);
        feed.board.complete = true;
        feed
    }

    fn keys(feed: &Feed, state: &State) -> Vec<(String, usize, Vec<String>)> {
        super::super::sections(feed, &Scope::All, state)
            .into_iter()
            .filter(|section| section.key != super::super::closed::KEY)
            .map(|section| {
                let rows = section.rows.iter().map(ToString::to_string).collect();
                (section.key, section.count, rows)
            })
            .collect()
    }

    /// A filed parent whose children were all Done still sat in Backlog with Move to Ready:
    /// it folds into Completed plans with View plan, and counts as no open work.
    #[test]
    fn a_completed_plan_folds_away_and_is_not_backlog_work() {
        let feed = board(vec![
            task("T-1", Status::Backlog, Some(&[(Status::Done, 3)])),
            task("T-4", Status::Backlog, None),
        ]);
        let mut state = State::default();
        assert_eq!(
            keys(&feed, &state),
            [
                ("backlog".into(), 1, vec!["T-4".into()]),
                (COMPLETED_KEY.into(), 1, Vec::new()),
            ]
        );
        state.toggle(COMPLETED_KEY);
        assert_eq!(keys(&feed, &state)[1].2, ["T-1"]);
        let plan = line(&feed, "T-1").unwrap();
        assert_eq!(plan.plan, Some(PlanState::Complete { done: 3 }));
        assert_eq!(plan.status, Status::Done);
        let next = plan.next.unwrap();
        assert_eq!(next.label, "View plan");
        assert_eq!(
            next.press,
            crate::model::work::Press::Open(crate::model::Live::Plan(Id::from("T-1")))
        );
        assert!(summary(&feed, &Scope::All).starts_with("1 open"));
        assert_eq!(
            feed.board.task("T-1").unwrap().group,
            crate::model::daemon::Group::Plan
        );
    }

    /// Dropped work stays distinct among completed plans, and a reopened child returns its
    /// plan to the active list with its progress.
    #[test]
    fn dropped_children_close_a_plan_and_a_reopened_child_reactivates_it() {
        let mut feed = board(vec![task(
            "T-1",
            Status::Backlog,
            Some(&[(Status::Done, 1), (Status::Dropped, 1)]),
        )]);
        let closed = line(&feed, "T-1").unwrap();
        assert_eq!(
            closed.plan,
            Some(PlanState::Closed {
                done: 1,
                dropped: 1
            })
        );
        assert_eq!(closed.status, Status::Dropped);
        assert_eq!(closed.now, "Plan closed · 1 done, 1 dropped");
        assert_eq!(keys(&feed, &State::default())[0].0, COMPLETED_KEY);

        // Even a parent dropped by hand returns, and is not listed among closed tasks.
        feed.board.apply_batch(
            1,
            vec![task(
                "T-1",
                Status::Dropped,
                Some(&[(Status::Done, 1), (Status::Ready, 1)]),
            )],
            &[],
        );
        let mut state = State::default();
        state.toggle(super::super::closed::KEY);
        assert_eq!(
            keys(&feed, &state),
            [(ACTIVE_KEY.into(), 1, vec!["T-1".into()])]
        );
        assert!(
            super::super::sections(&feed, &Scope::All, &state)
                .iter()
                .all(
                    |section| !section.rows.contains(&Id::from("T-1")) || section.key == ACTIVE_KEY
                )
        );
        let active = line(&feed, "T-1").unwrap();
        assert_eq!(active.status, Status::Backlog);
        assert_eq!(active.now, "Plan · 1 of 2 done");
        assert_eq!(active.next.unwrap().label, "View plan");
    }
}
