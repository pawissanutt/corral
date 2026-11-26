//! Where a task stands in run, gate, review and land: four marks, each with the sentence that
//! says it, and the plain phrase a row shows ("gate passed · review round 1 of 2"). Both read
//! the served row and the rail `daemon_board` derives from it; nothing here is guessed.

use corral_types::{CheckResult, GateAttemptState, Status};

use crate::model::board::Mark;
use crate::model::daemon::{Feed, Link, Task};
use crate::model::gate::{self, GateOpen};
use crate::model::status::{Stage, status_word};

/// The stages' names, in order.
pub const STAGES: [&str; 4] = ["Run", "Gate", "Review", "Land"];

/// One stage of a task's pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineStep {
    /// `Run`, `Gate`, `Review`, `Land`.
    pub name: &'static str,
    pub stage: Stage,
    /// What the mark stands for, as its hint and as the inspector spells it; empty where the
    /// stage is not part of where the task is.
    pub sentence: String,
    /// The Gate view this stage opens: only the gate, and only once corrald reports an attempt.
    pub open: Option<GateOpen>,
}

impl PipelineStep {
    /// What a screen reader hears for the mark: `Gate passed: passed on c0ffee1`.
    pub fn spoken(&self) -> String {
        if self.sentence.is_empty() {
            format!("{} {}", self.name, self.stage.word())
        } else {
            format!("{} {}: {}", self.name, self.stage.word(), self.sentence)
        }
    }
}

/// `round 1 of 2`, or `round 1` where the row names no limit. The engine counts rounds from
/// zero until its first send-back.
fn round(task: &Task) -> String {
    let round = task.task().round.max(1);
    match task.row.max_rounds {
        Some(max) => format!("round {round} of {max}"),
        None => format!("round {round}"),
    }
}

/// The task's run, gate, review and land.
pub fn pipeline(task: &Task) -> [PipelineStep; 4] {
    let row = &task.row;
    let status = task.status();
    let in_review = status == Status::Review;
    let gate_failed = row
        .gate
        .as_ref()
        .is_some_and(|gate| gate.result == CheckResult::Fail);
    let sha = |at: usize| task.rail[at].sha7.clone().unwrap_or_default();
    let on = |words: &str, sha: String| {
        if sha.is_empty() {
            words.to_owned()
        } else {
            format!("{words} on {sha}")
        }
    };
    let run = match (&row.run, task.rail[0].mark) {
        (None, _) | (_, Mark::Next | Mark::Na | Mark::Stale) => (Stage::None, String::new()),
        (Some(run), Mark::Active) => (
            Stage::Working,
            format!("worker {} · {} working", run.id, run.profile),
        ),
        // A task blocked on its failed gate has a finished run; any other block is the run's.
        (Some(_), Mark::Fail) if !gate_failed => (
            Stage::You,
            task.blocked()
                .map_or_else(|| "blocked".to_owned(), |blocked| blocked.reason.clone()),
        ),
        (Some(run), Mark::Fail | Mark::Done) => (Stage::Ok, format!("worker {} finished", run.id)),
    };
    // Only an observed attempt says the gate runs; a check of the candidate speaks for the
    // finished gate, and an attempt without one for how it ended.
    let attempt = gate::current(row);
    let gate = attempt.map_or_else(
        || match task.rail[1].mark {
            Mark::Done => (Stage::Ok, on("passed", sha(1))),
            Mark::Fail => (Stage::Fail, on("failed", sha(1))),
            Mark::Next if in_review => (Stage::Wait, "no gate result yet".to_owned()),
            Mark::Active | Mark::Next | Mark::Na | Mark::Stale => (Stage::None, String::new()),
        },
        gate::stage,
    );
    let review = match task.rail[2].mark {
        Mark::Done => (Stage::Ok, format!("approved in {}", round(task))),
        Mark::Fail => (Stage::Fail, format!("changes requested in {}", round(task))),
        Mark::Active if row.reviewer.is_some() => {
            (Stage::Now, format!("{}, reviewer working", round(task)))
        }
        Mark::Active => (Stage::Now, round(task)),
        Mark::Next if in_review => (Stage::Wait, "waits for the gate".to_owned()),
        Mark::Next | Mark::Na | Mark::Stale => (Stage::None, String::new()),
    };
    let land = match status {
        Status::Done => (Stage::Ok, "landed".to_owned()),
        Status::Review if row.ready => (Stage::You, "ready, waits on your approve".to_owned()),
        Status::Review => (Stage::Wait, "after gate and review".to_owned()),
        _ => (Stage::None, String::new()),
    };
    let step = |at: usize, (stage, sentence): (Stage, String)| PipelineStep {
        name: STAGES[at],
        stage,
        sentence,
        open: (at == 1).then(|| gate::open_of(row)).flatten(),
    };
    [step(0, run), step(1, gate), step(2, review), step(3, land)]
}

/// On a disconnected board the last live observation cannot claim a runner is still alive.
pub fn observed_pipeline(task: &Task, feed: &Feed) -> [PipelineStep; 4] {
    let mut steps = pipeline(task);
    if feed.link != Link::Up && task.gate_attempt().is_some_and(gate::live) {
        steps[1].stage = Stage::Stale;
        steps[1].sentence = "runner liveness unknown offline".to_owned();
    }
    steps
}

/// What is happening to the task, as one plain phrase from its own facts; empty where the
/// status says it all.
pub fn phrase(task: &Task, feed: &Feed) -> String {
    let row = &task.row;
    match task.status() {
        Status::Blocked => task
            .blocked()
            .map(|blocked| blocked.reason.clone())
            .unwrap_or_default(),
        Status::Review => {
            let [_, gate, review, _] = pipeline(task);
            let mut parts = Vec::new();
            if row.ready {
                parts.push(match row.run.as_ref() {
                    Some(run) => format!("ready to land onto {}", run.base),
                    None => "ready to land".to_owned(),
                });
            }
            if let Some(live) = task
                .gate_attempt()
                .and_then(|attempt| gate::phrase(attempt, feed))
            {
                parts.push(live);
            } else {
                match gate.stage {
                    Stage::Ok => parts.push("gate passed".to_owned()),
                    Stage::Fail => parts.push(
                        match task.gate_attempt().map(|attempt| attempt.state) {
                            Some(GateAttemptState::Interrupted) => "gate interrupted",
                            Some(GateAttemptState::Error) => "gate could not run",
                            _ => "gate failed",
                        }
                        .to_owned(),
                    ),
                    Stage::Wait => parts.push("no gate result yet".to_owned()),
                    _ => {}
                }
            }
            if matches!(review.stage, Stage::Ok | Stage::Fail | Stage::Now) {
                parts.push(format!("review {}", review.sentence));
            }
            parts.join(" · ")
        }
        Status::Running => {
            let worker = match &row.run {
                Some(run) => format!("worker {} · {}", run.id, run.profile),
                None => "no worker session".to_owned(),
            };
            // An explicit gate on a running task is reported as its runner reports it.
            match task
                .gate_attempt()
                .and_then(|attempt| gate::phrase(attempt, feed))
            {
                Some(live) => format!("{worker} · {live}"),
                None => worker,
            }
        }
        Status::Ready => row
            .dependencies
            .iter()
            .flatten()
            .find(|dependency| dependency.status != Some(Status::Done))
            .map(|dependency| match dependency.status {
                Some(status) => format!("depends on {} ({})", dependency.id, status_word(status)),
                None => format!("depends on {}", dependency.id),
            })
            .unwrap_or_default(),
        Status::Idea | Status::Backlog | Status::Done | Status::Dropped => children(task, feed),
    }
}

/// The compact row leads with the alert, retaining the ordinary status phrase after it.
pub fn row_phrase(task: &Task, feed: &Feed) -> String {
    let now = phrase(task, feed);
    match task.attention() {
        Some(reason) if !now.is_empty() => format!("{reason} · {now}"),
        Some(reason) => reason.to_owned(),
        None => now,
    }
}

/// `plan · 2 of 5 children done` for a task with children.
fn children(task: &Task, feed: &Feed) -> String {
    let Some(plan) = feed.plans.get(task.id.as_str()) else {
        return String::new();
    };
    let total: u64 = plan.child_counts.iter().map(|count| count.count).sum();
    if total == 0 {
        return String::new();
    }
    let done: u64 = plan
        .child_counts
        .iter()
        .filter(|count| count.status == Status::Done)
        .map(|count| count.count)
        .sum();
    format!("plan · {done} of {total} children done")
}

#[cfg(test)]
mod tests {
    use corral_types::{Actor, Check, CheckKind};

    use super::*;
    use crate::model::daemon::{bare_task, task_row};

    fn check(kind: CheckKind, result: CheckResult) -> Check {
        Check {
            task: "T-1".into(),
            kind,
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

    fn review_task(ready: bool, gate: Option<CheckResult>, review: Option<CheckResult>) -> Task {
        let mut row = task_row(bare_task(
            "T-1",
            "t",
            Status::Review,
            jiff::Timestamp::UNIX_EPOCH,
        ));
        row.task.round = 1;
        row.max_rounds = Some(2);
        row.ready = ready;
        row.gate = gate.map(|result| check(CheckKind::Gate, result));
        row.review = review.map(|result| check(CheckKind::Review, result));
        Task::new(row)
    }

    /// The row's phrase said "in review" while the gate had not passed, so a task that waited
    /// on its gate read as if a reviewer had it.
    #[test]
    fn a_review_phrase_names_the_gate_and_the_round_it_is_in() {
        let feed = Feed::default();
        let waiting = review_task(false, None, None);
        assert_eq!(phrase(&waiting, &feed), "no gate result yet");
        assert_eq!(pipeline(&waiting)[2].stage, Stage::Wait);
        let reviewing = review_task(false, Some(CheckResult::Pass), None);
        assert_eq!(
            phrase(&reviewing, &feed),
            "gate passed · review round 1 of 2"
        );
        let ready = review_task(true, Some(CheckResult::Pass), Some(CheckResult::Pass));
        assert_eq!(
            phrase(&ready, &feed),
            "ready to land · gate passed · review approved in round 1 of 2"
        );
        let stages: Vec<Stage> = pipeline(&ready).iter().map(|step| step.stage).collect();
        assert_eq!(stages, [Stage::None, Stage::Ok, Stage::Ok, Stage::You]);
        assert_eq!(pipeline(&ready)[1].sentence, "passed on c0ffee1");
    }

    /// The Review rail showed the gate running from the task's status alone, with no runner
    /// behind it; and a runner that vanished stayed "running" or read as a pass.
    #[test]
    fn only_an_observed_attempt_runs_the_gate_and_an_interrupted_one_does_not_pass() {
        let mut feed = Feed {
            link: crate::model::daemon::Link::Up,
            ..Feed::default()
        };
        let waiting = review_task(false, None, None);
        assert_eq!(pipeline(&waiting)[1].stage, Stage::Wait);
        assert!(pipeline(&waiting)[1].open.is_none());
        let observed = |state| {
            let mut row = review_task(false, None, None).row;
            row.gate_attempt = Some(crate::model::gate::tests::attempt(state));
            Task::new(row)
        };
        let running = observed(GateAttemptState::Running);
        let [_, gate, _, _] = pipeline(&running);
        assert_eq!(gate.stage, Stage::Now);
        assert_eq!(
            gate.open.as_ref().map(|open| open.attempt.as_str()),
            running.gate_attempt().map(|attempt| attempt.id.as_str())
        );
        assert_eq!(phrase(&running, &feed), "gate running · step 2 of 5");
        let stale = observed(GateAttemptState::Stale);
        assert_eq!(pipeline(&stale)[1].stage, Stage::Stale);
        let interrupted = observed(GateAttemptState::Interrupted);
        assert_eq!(pipeline(&interrupted)[1].stage, Stage::Fail);
        assert_eq!(interrupted.rail[1].mark, Mark::Fail);
        assert_eq!(phrase(&interrupted, &feed), "gate interrupted");
        feed.link = crate::model::daemon::Link::Down;
        assert_eq!(phrase(&running, &feed), "gate liveness unknown offline");
    }

    /// A stage that is not part of where the task is must not read as pending work.
    #[test]
    fn an_open_task_before_its_run_has_no_stage_and_no_phrase() {
        let task = Task::new(task_row(bare_task(
            "T-2",
            "t",
            Status::Backlog,
            jiff::Timestamp::UNIX_EPOCH,
        )));
        assert!(pipeline(&task).iter().all(|step| step.stage == Stage::None));
        assert_eq!(phrase(&task, &Feed::default()), "");
    }
}
