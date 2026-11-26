//! The decision band at the top of a task's page and of its inspector: what is happening in one
//! line, the plain phrase under it, and what the owner can do there. A Blocked task holds the
//! engine's reason, detail, reply box and next steps (`work::BlockedCard`); a review the owner
//! decides holds the review's decide bar (Approve onto its target, Request changes, Land
//! anyway). A Running task with a current failed gate shows its receipt and a retry; anything
//! else shows its one next step, or nothing while the engine works.

use corral_types::{CheckResult, Role, SessionStatus, Status};

use crate::model::Id;
use crate::model::daemon::{Feed, Task};
use crate::model::next::{NextStep, next_step};
use crate::model::pipeline::{phrase, pipeline};
use crate::model::status::{Stage, status_word};
use crate::model::work::{self, BlockedCard, loaded_task};

use super::task_at;

/// What the band holds under its words.
#[derive(Debug, Clone, PartialEq)]
pub enum Hold {
    /// The engine's reason as the band's words, then its detail, reply box and next steps.
    Blocked(BlockedCard),
    /// The review's decide bar and its Onto target.
    Decide,
    /// The current candidate's failed gate, while the task itself is still Running.
    GateFailed(GateFailure),
    /// The one next step; none while the engine has the task or it is closed.
    Step(Option<NextStep>),
}

/// The facts of a failed gate and what the owner can do from the band.
#[derive(Debug, Clone, PartialEq)]
pub struct GateFailure {
    pub command: Option<String>,
    pub log: Option<String>,
    pub retry: work::Button,
    pub session: Option<NextStep>,
}

/// The band of one task.
#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    pub id: Id,
    pub status: Status,
    /// `Blocked: no free slot`, `Ready to land onto main`, `In review: nothing for you yet`.
    pub what: String,
    /// The plain phrase under it; empty where `what` says it all.
    pub how: String,
    pub attention: Option<String>,
    pub hold: Hold,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictNotice {
    pub title: String,
    pub reason: String,
    pub command: String,
}

pub const VERDICT_RETRY: &str = "Retry from the workspace root:";

pub fn can_refresh_verdict(feed: &Feed, id: &str) -> bool {
    loaded_task(feed, id).is_some_and(|detail| {
        matches!(detail.task.status, Status::Review | Status::Blocked)
            && detail
                .runs
                .iter()
                .rev()
                .find(|run| run.role == Role::Reviewer)
                .is_some_and(|run| run.harness == "pi")
    })
}

pub fn pending_verdict(feed: &Feed, id: &str) -> Option<VerdictNotice> {
    let verdict = loaded_task(feed, id)?.pending_review_verdict.as_ref()?;
    let [task, run, request] = [&verdict.task, &verdict.run, &verdict.request]
        .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")));
    Some(VerdictNotice {
        title: format!(
            "{} ({} · {})",
            verdict.message, verdict.run, verdict.verdict
        ),
        reason: verdict.reason.clone(),
        command: format!("corral-next review-verdict {task} {run} {request}"),
    })
}

/// The base a review lands onto: the run's, else the task's.
fn base(task: &Task) -> Option<&str> {
    task.row
        .run
        .as_ref()
        .map(|run| run.base.as_str())
        .or(task.task().base.as_deref())
}

/// `gate passed on c0ffee1 · review approved in round 1 of 2`: the gate and review as the
/// pipeline spells them.
fn progress(task: &Task) -> String {
    let [_, gate, review, _] = pipeline(task);
    [gate, review]
        .iter()
        .filter(|step| !step.sentence.is_empty())
        .map(|step| format!("{} {}", step.name.to_lowercase(), step.sentence))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn review(task: &Task) -> (String, String, Hold) {
    let [_, gate, _, land] = pipeline(task);
    let decide = land.stage == Stage::You || gate.stage == Stage::Fail;
    let what = if land.stage == Stage::You {
        match base(task) {
            Some(base) => format!("Ready to land onto {base}"),
            None => "Ready to land".to_owned(),
        }
    } else if gate.stage == Stage::Fail {
        format!("Gate {}", gate.sentence)
    } else if task.attention().is_some() {
        "In review".to_owned()
    } else {
        "In review: nothing for you yet".to_owned()
    };
    let hold = if decide {
        Hold::Decide
    } else {
        Hold::Step(None)
    };
    (what, progress(task), hold)
}

fn running(feed: &Feed, task: &Task) -> (String, String, Hold) {
    let worker = task.row.run.as_ref().filter(|run| run.role == Role::Worker);
    let (what, how) = match worker {
        Some(run) => {
            let session = format!("{}_{}", task.id, run.id);
            let activity = match feed.agents.get(session.as_str()).map(|row| &row.status) {
                Some(SessionStatus::Working) => " · working",
                Some(SessionStatus::Idle) => " · idle",
                _ => "",
            };
            (
                format!("Running: worker {}{activity}", run.id),
                format!("profile {} · on {}", run.profile, run.branch),
            )
        }
        None => ("Running, with no worker session".to_owned(), String::new()),
    };
    let session = next_step(task);
    // The daemon verified the current gate; the last submitted SHA can name a different HEAD.
    let Some(gate) = task
        .row
        .gate
        .as_ref()
        .filter(|gate| gate.result == CheckResult::Fail)
    else {
        return (what, how, Hold::Step(session));
    };
    (
        "Gate failed".to_owned(),
        if how.is_empty() {
            what
        } else {
            format!("{what} · {how}")
        },
        Hold::GateFailed(GateFailure {
            // The gate records its failed shell command, not a parsed test name.
            command: gate.notes.clone().filter(|notes| !notes.trim().is_empty()),
            log: gate.log.clone(),
            retry: work::Button::new(
                "Retry gate",
                work::Press::Verb {
                    record: work::Record::Task(task.id.clone()),
                    verb: work::Verb::Gate,
                },
                work::ButtonKind::Primary,
            ),
            session,
        }),
    )
}

fn of(feed: &Feed, task: &Task) -> Band {
    let status = task.status();
    let step = || Hold::Step(next_step(task));
    let (what, how, hold) = match status {
        Status::Blocked => match work::blocked_card(task) {
            Some(card) => (
                format!("Blocked: {}", card.reason),
                String::new(),
                Hold::Blocked(card),
            ),
            None => ("Blocked".to_owned(), phrase(task, feed), step()),
        },
        Status::Review => review(task),
        Status::Running => running(feed, task),
        Status::Ready => ("Ready to dispatch".to_owned(), phrase(task, feed), step()),
        Status::Idea | Status::Backlog => {
            let how = phrase(task, feed);
            let how = if how.is_empty() {
                "Not started.".to_owned()
            } else {
                how
            };
            (status_word(status).to_owned(), how, step())
        }
        Status::Done | Status::Dropped => {
            let when = work::when_at(task.task().updated, feed);
            let how = match (status, base(task)) {
                (Status::Done, Some(base)) => format!("landed onto {base} · {when}"),
                _ => when,
            };
            (status_word(status).to_owned(), how, Hold::Step(None))
        }
    };
    Band {
        id: task.id.clone(),
        status,
        what,
        how,
        attention: task.attention().map(str::to_owned),
        hold,
    }
}

/// The task's band, wherever the client holds the task.
pub fn band(feed: &Feed, id: &str) -> Option<Band> {
    task_at(feed, id).map(|task| of(feed, &task))
}

#[cfg(test)]
mod tests {
    use corral_types::{Actor, Check, CheckKind, Op};

    use super::*;
    use crate::model::agents::source_tests::agent;
    use crate::model::daemon::{bare_task, task_row};
    use crate::model::effect::{Conn, Effect, Env};
    use crate::model::{Form, Live};

    fn gate(result: CheckResult) -> Check {
        Check {
            task: "T-1".into(),
            kind: CheckKind::Gate,
            key: "c0ffee1234".into(),
            result,
            notes: None,
            log: None,
            by: Actor::Auto,
            at: jiff::Timestamp::UNIX_EPOCH,
            round: None,
            run: None,
        }
    }

    fn review_task(ready: bool, result: CheckResult) -> Feed {
        let mut row = task_row(bare_task(
            "T-1",
            "t",
            Status::Review,
            jiff::Timestamp::UNIX_EPOCH,
        ));
        row.ready = ready;
        row.gate = Some(gate(result));
        let mut feed = Feed::default();
        feed.board.apply_batch(1, vec![Task::new(row)], &[]);
        feed
    }

    /// A review the owner can decide showed nothing to press on its page, and a running one
    /// offered Approve the engine would only refuse: the decide bar stands for a ready review
    /// and a red gate (Land anyway), and a review still in the engine's hands holds nothing.
    #[test]
    fn the_band_decides_a_ready_review_or_a_red_gate_and_waits_otherwise() {
        let hold = |feed: &Feed| band(feed, "T-1").map(|band| band.hold);
        assert_eq!(
            hold(&review_task(true, CheckResult::Pass)),
            Some(Hold::Decide)
        );
        assert_eq!(
            hold(&review_task(false, CheckResult::Fail)),
            Some(Hold::Decide)
        );
        assert_eq!(
            hold(&review_task(false, CheckResult::Pass)),
            Some(Hold::Step(None))
        );
    }

    #[test]
    fn a_running_gate_failure_uses_the_served_receipt_and_retries_only_the_gate() {
        let mut worker = agent("T-1", "R1", Role::Worker);
        worker.run.head_sha = Some("last-submission".into());
        let mut row = task_row(bare_task(
            "T-1",
            "t",
            Status::Running,
            jiff::Timestamp::UNIX_EPOCH,
        ));
        row.run = Some(worker.run);
        row.gate = Some(Check {
            notes: Some("cargo test --locked".into()),
            log: Some("T-1:2".into()),
            ..gate(CheckResult::Fail)
        });
        let mut feed = Feed::default();
        feed.board.apply_batch(1, vec![Task::new(row.clone())], &[]);

        let shown = band(&feed, "T-1").expect("the Running task's band");
        assert_eq!(shown.status, Status::Running);
        let Hold::GateFailed(failure) = shown.hold else {
            panic!("the served failure should be shown despite a different submitted SHA");
        };
        assert_eq!(failure.command.as_deref(), Some("cargo test --locked"));
        assert_eq!(failure.log.as_deref(), Some("T-1:2"));
        assert_eq!(
            failure.session.map(|step| step.press),
            Some(work::Press::Open(Live::Session(Id::from("T-1_R1"))))
        );
        let effects = work::State::default().press(
            failure.retry.action.expect("enabled gate retry"),
            &feed,
            Env {
                conn: Conn::Healthy,
                form: Form::Desktop,
            },
        );
        assert!(matches!(
            effects.as_slice(),
            [Effect::Act(act)] if act.op == Op::Gate { id: "T-1".into() } && !act.force
        ));

        for (rev, current) in [(2, Some(gate(CheckResult::Pass))), (3, None)] {
            row.gate = current;
            feed.board
                .apply_batch(rev, vec![Task::new(row.clone())], &[]);
            assert!(matches!(
                band(&feed, "T-1").map(|band| band.hold),
                Some(Hold::Step(Some(_)))
            ));
        }
    }

    #[test]
    fn the_running_band_reflects_worker_activity_without_treating_idle_as_failure() {
        let mut worker = agent("T-1", "R1", Role::Worker);
        let mut row = task_row(bare_task(
            "T-1",
            "t",
            Status::Running,
            jiff::Timestamp::UNIX_EPOCH,
        ));
        row.run = Some(worker.run.clone());
        let mut feed = Feed::default();
        feed.board.apply_batch(1, vec![Task::new(row)], &[]);
        let unknown = band(&feed, "T-1").expect("unobserved worker");
        feed.agents.insert(Id::from("T-1_R1"), worker.clone());
        let working = band(&feed, "T-1").expect("working worker");
        worker.status = SessionStatus::Idle;
        feed.agents.insert(Id::from("T-1_R1"), worker);
        let idle = band(&feed, "T-1").expect("idle worker");

        assert_ne!(working.what, unknown.what);
        assert_ne!(idle.what, unknown.what);
        assert_ne!(idle.what, working.what);
        assert!(matches!(idle.hold, Hold::Step(Some(_))));
    }
}
