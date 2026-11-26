//! The one next step of a task or a session: the single press that moves it, picked from its
//! status. Every other operation is under More, where the next step is marked. The app only
//! suggests: the step is always offered and the engine decides, as with every operation.

use corral_types::{Op, Status};

use crate::model::Live;
use crate::model::agents::SessionLine;
use crate::model::daemon::Task;
use crate::model::status::SessionState;
use crate::model::work::{Press, Step, Verb};

/// The one next step: its words and the press that runs it.
#[derive(Debug, Clone, PartialEq)]
pub struct NextStep {
    /// `Move to Backlog`, `Dispatch`, `Open session`, `Approve`.
    pub label: String,
    /// What the step does when pressed, through `work::State::press`.
    pub press: Press,
    /// The More menu row this step is (`btn-<slug>`), marked there; none for a step that
    /// opens a page.
    pub verb: Option<&'static str>,
    /// Whether the step is the owner's move (Blocked, ready to land): the filled button.
    pub yours: bool,
}

impl NextStep {
    fn step(label: impl Into<String>, task: &Task, step: Step, verb: Verb) -> NextStep {
        NextStep {
            label: label.into(),
            press: Press::Step {
                task: task.id.clone(),
                step,
            },
            verb: Some(verb.slug()),
            yours: step == Step::Approve,
        }
    }

    /// The engine operation the step submits on one press; none for a step that opens a page
    /// or runs a Blocked card's entry.
    pub fn op(&self) -> Option<Op> {
        match &self.press {
            Press::Step { task, step } => Some(step.op(task)),
            _ => None,
        }
    }

    /// What the button says while it must not offer its operation again. Sent and unanswered,
    /// it names the operation under way (`Moving to Ready…`). Answered while the board has not
    /// caught up, it says what is known: corrald moved the task, or the outcome is unknown.
    /// Neither claims the new status before the board shows it.
    pub fn pending_label(&self, pending: Pending) -> Option<String> {
        let Press::Step { step, .. } = &self.press else {
            return None;
        };
        Some(match (pending, step) {
            (Pending::Sending, Step::Move(status)) => {
                format!("Moving to {}…", crate::model::status::status_word(*status))
            }
            (Pending::Sending, Step::Dispatch) => "Dispatching…".into(),
            (Pending::Sending, Step::Approve) => "Landing…".into(),
            (Pending::Confirmed, _) => "Done · updating the board…".into(),
            (Pending::Unknown, _) => "Outcome unknown · checking the board…".into(),
        })
    }
}

/// Why a step's button holds its operation back (`AppState::op_pending`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    /// Sent; corrald has not answered.
    Sending,
    /// corrald applied it; the board does not show a walk taken since.
    Confirmed,
    /// Its reply was lost; the board does not show a walk taken since.
    Unknown,
}

/// The test id of a record's next-step button: `next-T-12`, `next-B-k3x9`.
pub fn next_id(record: &str) -> String {
    format!("next-{record}")
}

/// The run's session id as the app keys it: `<task>_<run>`.
fn run_session(task: &Task) -> Option<crate::model::Id> {
    task.row
        .run
        .as_ref()
        .map(|run| crate::model::Id::from(format!("{}_{}", task.id, run.id)))
}

/// The task's one next step. Idea: Move to Backlog. Backlog: Move to Ready. Ready: Dispatch.
/// Running: Open session. Blocked: the engine's first `next` entry. Review the engine marked
/// `ready`: Approve. Review while the gate or the reviewer runs, Done and Dropped: none
/// (Reopen is under More).
/// A filed plan parent is a summary whatever its stored status: View plan.
pub fn next_step(task: &Task) -> Option<NextStep> {
    if crate::model::tasks::plans::is_summary(task) {
        return Some(NextStep {
            label: "View plan".into(),
            press: Press::Open(Live::Plan(task.id.clone())),
            verb: None,
            yours: false,
        });
    }
    let moved = |status: Status| {
        NextStep::step(
            format!("Move to {}", crate::model::status::status_word(status)),
            task,
            Step::Move(status),
            Verb::Move,
        )
    };
    match task.status() {
        Status::Idea => Some(moved(Status::Backlog)),
        Status::Backlog => Some(moved(Status::Ready)),
        Status::Ready => Some(NextStep::step(
            "Dispatch",
            task,
            Step::Dispatch,
            Verb::Dispatch,
        )),
        Status::Running => run_session(task).map(|session| NextStep {
            label: "Open session".into(),
            press: Press::Open(Live::Session(session)),
            verb: None,
            yours: false,
        }),
        Status::Blocked => task.blocked()?.next.first().map(|next| {
            let (label, press) = crate::model::work::blocked_step(task, 0, next);
            let verb = matches!(press, Press::Next { .. })
                .then(|| op_verb(&next.op).map(Verb::slug))
                .flatten();
            NextStep {
                label,
                press,
                verb,
                yours: true,
            }
        }),
        Status::Review if task.row.ready => Some(NextStep::step(
            "Approve",
            task,
            Step::Approve,
            Verb::Approve,
        )),
        Status::Review | Status::Done | Status::Dropped => None,
    }
}

/// A session's one next step: Blocked on its Pi host's card, Answer opens its page with that
/// card open; Blocked otherwise (another harness's prompt, a host error), Working or Idle, open
/// it; a Gone one has none (Relaunch is under More).
pub fn session_next_step(session: &SessionLine) -> Option<NextStep> {
    let open = Press::Open(Live::Session(session.id.clone()));
    let (label, yours, press) = match (session.state, &session.asked) {
        (SessionState::Blocked, Some(asked)) => (
            "Answer",
            true,
            Press::Question {
                session: session.id.clone(),
                asked: asked.clone(),
            },
        ),
        (SessionState::Blocked, None) => ("Open", true, open),
        (SessionState::Working | SessionState::Idle | SessionState::Unavailable, _) => {
            ("Open", false, open)
        }
        (SessionState::Gone, _) => return None,
    };
    Some(NextStep {
        label: label.into(),
        press,
        verb: None,
        yours,
    })
}

/// The More menu row (`btn-<slug>`) that is the record's next step, marked there; none for a
/// session, whose next step opens its page, and for a task without one.
pub fn next_verb(
    feed: &crate::model::daemon::Feed,
    record: &crate::model::work::Record,
) -> Option<&'static str> {
    match record {
        crate::model::work::Record::Task(id) => feed
            .board
            .task(id)
            .and_then(next_step)
            .and_then(|step| step.verb),
        crate::model::work::Record::Session(_) | crate::model::work::Record::Workspace => None,
    }
}

/// The More menu row an operation is.
fn op_verb(op: &Op) -> Option<Verb> {
    Some(match op {
        Op::Edit { .. } => Verb::Edit,
        Op::Comment { .. } => Verb::Comment,
        Op::Move { .. } => Verb::Move,
        Op::Dispatch { .. } => Verb::Dispatch,
        Op::Reply { .. } => Verb::Reply,
        Op::Done { .. } => Verb::Done,
        Op::Gate { .. } => Verb::Gate,
        Op::Review { .. } => Verb::Review,
        Op::Verdict { .. } => Verb::Verdict,
        Op::Changes { .. } => Verb::Changes,
        Op::Approve { .. } => Verb::Approve,
        Op::Retry { .. } => Verb::Retry,
        Op::Stop { .. } => Verb::Stop,
        Op::Drop { .. } => Verb::Drop,
        Op::Reopen { .. } => Verb::Reopen,
        Op::New { .. }
        | Op::LensDraft { .. }
        | Op::LensRepair { .. }
        | Op::ArtifactAdd { .. }
        | Op::UsageAdd { .. }
        | Op::Sync => return None,
    })
}

#[cfg(test)]
mod tests {
    use corral_types::{Blocked, Next};

    use super::*;
    use crate::model::daemon::{bare_task, task_row};

    fn task(status: Status) -> Task {
        Task::new(task_row(bare_task(
            "T-7",
            "t",
            status,
            jiff::Timestamp::UNIX_EPOCH,
        )))
    }

    fn label(task: &Task) -> Option<String> {
        next_step(task).map(|step| step.label)
    }

    /// Every row once offered all fifteen operations with none standing out; a status whose
    /// step is wrong or missing sends the owner back to hunting through More.
    #[test]
    fn each_status_offers_the_one_step_that_moves_it() {
        assert_eq!(
            label(&task(Status::Idea)).as_deref(),
            Some("Move to Backlog")
        );
        assert_eq!(
            label(&task(Status::Backlog)).as_deref(),
            Some("Move to Ready")
        );
        assert_eq!(label(&task(Status::Ready)).as_deref(), Some("Dispatch"));
        // Nothing to press while the gate or the reviewer runs, and none on a closed task.
        for status in [
            Status::Review,
            Status::Done,
            Status::Dropped,
            Status::Running,
        ] {
            assert_eq!(label(&task(status)), None, "{status:?}");
        }
        let mut ready = task_row(bare_task(
            "T-7",
            "t",
            Status::Review,
            jiff::Timestamp::UNIX_EPOCH,
        ));
        ready.ready = true;
        let step = next_step(&Task::new(ready)).expect("a ready review has a step");
        assert_eq!(
            (step.label.as_str(), step.verb),
            ("Approve", Some("approve"))
        );
        assert!(step.yours);
    }

    /// A Blocked task's step is the engine's own first entry, forced form included: the app
    /// must not substitute a step the engine did not name.
    #[test]
    fn a_blocked_task_takes_the_engines_first_next_entry() {
        let mut row = task_row(bare_task(
            "T-7",
            "t",
            Status::Blocked,
            jiff::Timestamp::UNIX_EPOCH,
        ));
        row.task.blocked = Some(Blocked {
            reason: "no free slot".into(),
            detail: None,
            next: vec![
                Next::force(Op::Dispatch {
                    id: "T-7".into(),
                    profile: None,
                    base: None,
                }),
                Next::run(Op::Drop { id: "T-7".into() }),
            ],
        });
        let step = next_step(&Task::new(row)).expect("a blocked task with next entries");
        assert_eq!(step.label, "Dispatch anyway");
        assert_eq!(
            step.press,
            Press::Next {
                task: "T-7".into(),
                index: 0
            }
        );
        assert_eq!(step.verb, Some("dispatch"));
        // The engine named no step: none is invented.
        let mut bare = task_row(bare_task(
            "T-7",
            "t",
            Status::Blocked,
            jiff::Timestamp::UNIX_EPOCH,
        ));
        bare.task.blocked = Some(Blocked {
            reason: "stopped".into(),
            detail: None,
            next: Vec::new(),
        });
        assert_eq!(next_step(&Task::new(bare)), None);
    }
}
