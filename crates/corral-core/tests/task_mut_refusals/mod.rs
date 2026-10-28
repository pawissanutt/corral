//! Edits every `TaskMut` form must refuse with nothing written, for `task_mut_sequences`.

use super::*;

/// An edit every form must refuse with nothing written.
#[derive(Debug, Clone)]
pub(crate) enum Refusal {
    RunId(String),
    RunStarted(String),
    RunEnded(String),
    RunUsage(String),
    PlanRunId(String),
    PlanRunEnded(String),
    PlanId(String),
    AttemptId(String),
    AttemptStarted(String),
    ChildId(String, String),
    ChildStarted(String, String),
    PlanReviewId(String, String),
    PlanReviewInbox(String, String),
    AggregateReviewId(String, String),
    WaitId(String, String),
    DecisionId(String, String),
    DuplicateChild(String, Box<Run>),
    DuplicateWait(String, Box<WaitRecord>),
}

pub(crate) fn refuse<M: TaskMut>(task: &mut M, refusal: Refusal) -> Result<()> {
    match refusal {
        Refusal::RunId(id) => task.edit_run(&id, |r| {
            r.id = "renamed".into();
            Ok(())
        }),
        Refusal::RunStarted(id) => task.edit_run(&id, |r| {
            r.started = at(-5);
            Ok(())
        }),
        Refusal::RunEnded(id) => task.edit_run(&id, |r| {
            r.ended = Some(at(-5));
            Ok(())
        }),
        Refusal::RunUsage(id) => task.edit_run(&id, |r| {
            r.usage = None;
            Ok(())
        }),
        Refusal::PlanRunId(id) => task.edit_plan_run(&id, |r| {
            r.id = "renamed".into();
            Ok(())
        }),
        Refusal::PlanRunEnded(id) => task.edit_plan_run(&id, |r| {
            r.ended = Some(at(-5));
            Ok(())
        }),
        Refusal::PlanId(id) => task.edit_plan(&id, |p| {
            p.id = "renamed".into();
            Ok(())
        }),
        Refusal::AttemptId(id) => task.edit_aggregate_attempt(&id, |a| {
            a.id = "renamed".into();
            Ok(())
        }),
        Refusal::AttemptStarted(id) => task.edit_aggregate_attempt(&id, |a| {
            a.started = at(-5);
            Ok(())
        }),
        Refusal::ChildId(run, id) => task.edit_review_child(&run, &id, |c| {
            c.id = "renamed".into();
            Ok(())
        }),
        Refusal::ChildStarted(run, id) => task.edit_review_child(&run, &id, |c| {
            c.started = at(-5);
            Ok(())
        }),
        Refusal::PlanReviewId(plan, id) => task.edit_plan_review(&plan, &id, |r| {
            r.id = "renamed".into();
            Ok(())
        }),
        Refusal::PlanReviewInbox(plan, id) => task.edit_plan_review(&plan, &id, |r| {
            r.inbox.next_seq += 1;
            Ok(())
        }),
        Refusal::AggregateReviewId(attempt, id) => task.edit_aggregate_review(&attempt, &id, |r| {
            r.id = "renamed".into();
            Ok(())
        }),
        Refusal::WaitId(run, id) => task.edit_wait(&run, &id, |w| {
            w.request.id = "renamed".into();
            Ok(())
        }),
        Refusal::DecisionId(run, id) => task.edit_decision(&run, &id, |d| {
            d.request.id = "renamed".into();
            Ok(())
        }),
        Refusal::DuplicateChild(run, child) => task.push_review_child(&run, *child),
        Refusal::DuplicateWait(run, wait) => task.push_wait(&run, *wait),
    }
}

pub(crate) fn refusal(task: &Task, rng: &mut Rng) -> Refusal {
    let run = pick(rng, &task.runs).id.clone();
    let current = task.runs.last().unwrap();
    let plan = task.plans.last().unwrap();
    let attempt = task.aggregate_attempts.last().unwrap();
    let child = || current.review_runs.first().map(|c| c.id.clone());
    match rng.below(18) {
        0 => Refusal::RunId(run),
        1 => Refusal::RunStarted(run),
        2 => Refusal::RunEnded(run),
        3 => Refusal::RunUsage(run),
        4 => Refusal::PlanRunId(pick(rng, &task.plan_runs).id.clone()),
        5 => Refusal::PlanRunEnded(pick(rng, &task.plan_runs).id.clone()),
        6 => Refusal::PlanId(pick(rng, &task.plans).id.clone()),
        7 => Refusal::AttemptId(pick(rng, &task.aggregate_attempts).id.clone()),
        8 => Refusal::AttemptStarted(pick(rng, &task.aggregate_attempts).id.clone()),
        9 if child().is_some() => Refusal::ChildId(current.id.clone(), child().unwrap()),
        10 if child().is_some() => Refusal::ChildStarted(current.id.clone(), child().unwrap()),
        11 if !plan.reviews.is_empty() => {
            Refusal::PlanReviewId(plan.id.clone(), pick(rng, &plan.reviews).id.clone())
        }
        12 if !plan.reviews.is_empty() => {
            Refusal::PlanReviewInbox(plan.id.clone(), pick(rng, &plan.reviews).id.clone())
        }
        13 if !attempt.reviews.is_empty() => {
            Refusal::AggregateReviewId(attempt.id.clone(), pick(rng, &attempt.reviews).id.clone())
        }
        14 if !current.obligations.waits.is_empty() => Refusal::WaitId(
            current.id.clone(),
            pick(rng, &current.obligations.waits).request.id.clone(),
        ),
        15 if !current.obligations.decisions.is_empty() => Refusal::DecisionId(
            current.id.clone(),
            pick(rng, &current.obligations.decisions).request.id.clone(),
        ),
        16 if !current.review_runs.is_empty() => {
            Refusal::DuplicateChild(current.id.clone(), Box::new(current.review_runs[0].clone()))
        }
        17 if !current.obligations.waits.is_empty() => Refusal::DuplicateWait(
            current.id.clone(),
            Box::new(current.obligations.waits[0].clone()),
        ),
        _ => Refusal::RunStarted(run),
    }
}

pub(crate) fn refusal_name(refusal: &Refusal) -> &'static str {
    match refusal {
        Refusal::RunId(_) => "refused run id",
        Refusal::RunStarted(_) => "refused run started",
        Refusal::RunEnded(_) => "refused run ended",
        Refusal::RunUsage(_) => "refused run usage",
        Refusal::PlanRunId(_) => "refused plan run id",
        Refusal::PlanRunEnded(_) => "refused plan run ended",
        Refusal::PlanId(_) => "refused plan id",
        Refusal::AttemptId(_) => "refused attempt id",
        Refusal::AttemptStarted(_) => "refused attempt started",
        Refusal::ChildId(..) => "refused child id",
        Refusal::ChildStarted(..) => "refused child started",
        Refusal::PlanReviewId(..) => "refused plan review id",
        Refusal::PlanReviewInbox(..) => "refused plan review inbox",
        Refusal::AggregateReviewId(..) => "refused aggregate review id",
        Refusal::WaitId(..) => "refused wait id",
        Refusal::DecisionId(..) => "refused decision id",
        Refusal::DuplicateChild(..) => "refused duplicate child",
        Refusal::DuplicateWait(..) => "refused duplicate wait",
    }
}
