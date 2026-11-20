//! The decision band at the top of a task's page and of its inspector: what is happening in one
//! line, the plain phrase under it, and what the owner can do there. A Blocked task holds the
//! engine's reason, detail, reply box and next steps (`work::BlockedCard`); a review the owner
//! decides holds the review's decide bar (Approve onto its target, Request changes, Land
//! anyway); anything else its one next step, or nothing while the engine works.

use corral_types::Status;

use crate::model::Id;
use crate::model::daemon::{Feed, Task};
use crate::model::next::{NextStep, next_step};
use crate::model::pipeline::{phrase, pipeline};
use crate::model::status::{Stage, status_word};
use crate::model::work::{self, BlockedCard};

use super::task_at;

/// What the band holds under its words.
#[derive(Debug, Clone, PartialEq)]
pub enum Hold {
    /// The engine's reason as the band's words, then its detail, reply box and next steps.
    Blocked(BlockedCard),
    /// The review's decide bar and its Onto target.
    Decide,
    /// The one next step; none while the engine has the task or it is closed.
    Step(Option<NextStep>),
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
    pub hold: Hold,
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
        Status::Running => match &task.row.run {
            Some(run) => (
                format!("Running: worker {}", run.id),
                format!("profile {} · on {}", run.profile, run.branch),
                step(),
            ),
            None => (
                "Running, with no worker session".to_owned(),
                "Moved here by force; move it again from More.".to_owned(),
                step(),
            ),
        },
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
        hold,
    }
}

/// The task's band, wherever the client holds the task.
pub fn band(feed: &Feed, id: &str) -> Option<Band> {
    task_at(feed, id).map(|task| of(feed, &task))
}

#[cfg(test)]
mod tests {
    use corral_types::{Actor, Check, CheckKind, CheckResult};

    use super::*;
    use crate::model::daemon::{bare_task, task_row};

    fn review_task(ready: bool, gate: CheckResult) -> Feed {
        let mut row = task_row(bare_task(
            "T-1",
            "t",
            Status::Review,
            jiff::Timestamp::UNIX_EPOCH,
        ));
        row.ready = ready;
        row.gate = Some(Check {
            task: "T-1".into(),
            kind: CheckKind::Gate,
            key: "c0ffee1234".into(),
            result: gate,
            notes: None,
            log: None,
            by: Actor::Auto,
            at: jiff::Timestamp::UNIX_EPOCH,
            round: None,
            run: None,
        });
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
}
