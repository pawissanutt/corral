//! Open tasks as the `tasks` topic serves them, with the groups and rail the app derives.
use std::cmp::Ordering;
use std::collections::HashSet;

use corral_protocol::rows::TaskRow;
use corral_types::{Blocked, CheckResult, Status};

use crate::model::Id;
use crate::model::board::Mark;

/// Who moves a task next, in Now's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// Blocked, or in Review with `ready` appended: the owner moves next.
    NeedsYou,
    /// In Review while the gate or the reviewer runs.
    Verifying,
    /// Running.
    Producing,
    /// Ready.
    Queued,
    Backlog,
    Ideas,
    /// Done or Dropped; the open-task topic does not normally carry these.
    Other,
}

impl Group {
    pub fn of(row: &TaskRow) -> Group {
        match row.task.status {
            Status::Blocked => Group::NeedsYou,
            Status::Review if row.ready => Group::NeedsYou,
            Status::Review => Group::Verifying,
            Status::Running => Group::Producing,
            Status::Ready => Group::Queued,
            Status::Backlog => Group::Backlog,
            Status::Idea => Group::Ideas,
            Status::Done | Status::Dropped => Group::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub mark: Mark,
    pub sha7: Option<String>,
    /// Unix seconds.
    pub at: Option<u64>,
}

/// One open task: the row exactly as served, and what the app derives from it.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: Id,
    pub row: TaskRow,
    pub group: Group,
    /// Run, gate, review, land.
    pub rail: [Step; 4],
}

impl Task {
    pub fn new(row: TaskRow) -> Task {
        Task {
            id: Id::new(&row.task.id),
            group: Group::of(&row),
            rail: rail(&row),
            row,
        }
    }

    pub fn task(&self) -> &corral_types::Task {
        &self.row.task
    }

    pub fn title(&self) -> &str {
        &self.row.task.title
    }

    pub fn project(&self) -> Option<&str> {
        self.row.task.project.as_deref()
    }

    pub fn parent(&self) -> Option<&str> {
        self.row.task.parent.as_deref()
    }

    pub fn status(&self) -> Status {
        self.row.task.status
    }

    pub fn blocked(&self) -> Option<&Blocked> {
        self.row.task.blocked.as_ref()
    }

    /// One line about where the task stands: a Blocked card's reason, else its status.
    pub fn now(&self) -> String {
        if let Some(blocked) = self.blocked() {
            return blocked.reason.clone();
        }
        match self.status() {
            Status::Review if self.row.ready => "ready to land".into(),
            Status::Review => match (&self.row.gate, &self.row.review) {
                (Some(gate), _) if gate.result == CheckResult::Fail => "gate failed".into(),
                (None, _) => "gate running".into(),
                (Some(_), None) => "in review".into(),
                (Some(_), Some(_)) => "reviewed".into(),
            },
            Status::Running => match &self.row.run {
                Some(run) => format!("{} running", run.profile),
                None => "running".into(),
            },
            status => status.as_str().into(),
        }
    }

    /// Unix seconds of the task's last change.
    pub fn since(&self) -> Option<u64> {
        seconds(self.row.task.updated)
    }

    /// Unix seconds of the task's creation.
    pub fn age_since(&self) -> Option<u64> {
        seconds(self.row.task.created)
    }
}

pub fn seconds(at: jiff::Timestamp) -> Option<u64> {
    u64::try_from(at.as_second()).ok()
}

fn step(mark: Mark, check: Option<&corral_types::Check>) -> Step {
    Step {
        mark,
        sha7: check.map(|check| check.key.chars().take(7).collect()),
        at: check.and_then(|check| seconds(check.at)),
    }
}

fn rail(row: &TaskRow) -> [Step; 4] {
    let status = row.task.status;
    let checked = |check: Option<&corral_types::Check>, waiting: bool| match check {
        Some(check) if check.result == CheckResult::Pass => Mark::Done,
        Some(_) => Mark::Fail,
        None if waiting => Mark::Active,
        None => Mark::Next,
    };
    let run = match status {
        Status::Running => Mark::Active,
        Status::Blocked => Mark::Fail,
        Status::Review | Status::Done => Mark::Done,
        _ => Mark::Next,
    };
    let in_review = status == Status::Review;
    let gate_passed = row
        .gate
        .as_ref()
        .is_some_and(|gate| gate.result == CheckResult::Pass);
    let land = match status {
        Status::Done => Mark::Done,
        Status::Dropped => Mark::Na,
        _ => Mark::Next,
    };
    [
        Step {
            mark: run,
            sha7: row
                .run
                .as_ref()
                .and_then(|run| run.head_sha.as_ref())
                .map(|sha| sha.chars().take(7).collect()),
            at: row.run.as_ref().and_then(|run| seconds(run.started)),
        },
        step(checked(row.gate.as_ref(), in_review), row.gate.as_ref()),
        step(
            checked(row.review.as_ref(), in_review && gate_passed),
            row.review.as_ref(),
        ),
        step(land, None),
    ]
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Board {
    pub epoch: u64,
    /// False until one complete snapshot has been applied since app startup.
    pub complete: bool,
    rows: Vec<Task>,
    pending_epoch: Option<u64>,
}

impl Board {
    pub fn tasks(&self) -> &[Task] {
        &self.rows
    }

    pub fn task(&self, id: &str) -> Option<&Task> {
        self.rows
            .binary_search_by(|task| order(&task.id, id))
            .ok()
            .map(|index| &self.rows[index])
    }

    pub fn in_group(&self, group: Group) -> impl Iterator<Item = &Task> {
        self.rows.iter().filter(move |task| task.group == group)
    }

    pub fn count(&self, group: Group) -> usize {
        self.in_group(group).count()
    }

    /// The open children of a parent task, in id order.
    pub fn children<'a>(&'a self, parent: &'a str) -> impl Iterator<Item = &'a Task> {
        self.rows
            .iter()
            .filter(move |task| task.parent() == Some(parent))
    }

    /// Keep the previous snapshot visible until the replacement is complete.
    pub fn begin_epoch(&mut self, epoch: u64) {
        self.pending_epoch = Some(epoch);
    }

    /// Apply only complete batches: page accumulation belongs to the session.
    pub fn apply_batch(&mut self, epoch: u64, rows: Vec<Task>, left: &[String]) {
        if !self.complete || self.epoch != epoch || self.pending_epoch == Some(epoch) {
            self.rows.clear();
        }
        for row in rows {
            match self
                .rows
                .binary_search_by(|existing| order(&existing.id, &row.id))
            {
                Ok(index) => self.rows[index] = row,
                Err(index) => self.rows.insert(index, row),
            }
        }
        if !left.is_empty() {
            let left: HashSet<&str> = left.iter().map(String::as_str).collect();
            self.rows.retain(|row| !left.contains(row.id.as_str()));
        }
        self.epoch = epoch;
        self.complete = true;
        self.pending_epoch = None;
    }
}

fn order(a: &str, b: &str) -> Ordering {
    fn number(id: &str) -> Option<u64> {
        id.strip_prefix("T-")?.parse().ok()
    }
    match (number(a), number(b)) {
        (Some(a_num), Some(b_num)) => a_num.cmp(&b_num).then_with(|| a.cmp(b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

/// A served task row for tests and the sample preview.
pub fn task_row(task: corral_types::Task) -> TaskRow {
    TaskRow {
        task,
        run: None,
        gate: None,
        review: None,
        ready: false,
    }
}

/// A task with only the fields every row needs, for tests and the sample preview.
pub fn bare_task(id: &str, title: &str, status: Status, at: jiff::Timestamp) -> corral_types::Task {
    corral_types::Task {
        id: id.into(),
        title: title.into(),
        body: String::new(),
        status,
        project: None,
        priority: corral_types::Priority::P2,
        kind: None,
        size: None,
        profile: None,
        base: None,
        parent: None,
        depends_on: Vec::new(),
        blocked: None,
        round: 0,
        created: at,
        updated: at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str) -> Task {
        Task::new(task_row(bare_task(
            id,
            id,
            Status::Ready,
            jiff::Timestamp::UNIX_EPOCH,
        )))
    }

    #[test]
    fn board_batch_removes_left_and_keeps_numeric_order() {
        let mut board = Board::default();
        board.apply_batch(1, vec![task("T-10"), task("T-9"), task("other")], &[]);
        board.apply_batch(1, vec![task("T-2")], &["T-10".into()]);
        assert_eq!(
            board
                .tasks()
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["T-2", "T-9", "other"]
        );
        assert!(board.task("T-10").is_none());
    }

    #[test]
    fn review_with_ready_needs_the_owner_and_blocked_rows_fail_the_run() {
        let mut row = task_row(bare_task("T-1", "t", Status::Review, jiff::Timestamp::UNIX_EPOCH));
        assert_eq!(Task::new(row.clone()).group, Group::Verifying);
        row.ready = true;
        assert_eq!(Task::new(row.clone()).group, Group::NeedsYou);
        row.task.status = Status::Blocked;
        assert_eq!(Task::new(row).rail[0].mark, Mark::Fail);
    }
}
