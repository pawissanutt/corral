//! Growth variants use the tracked core fixture, with old work closed and zero-cost.
#[path = "../../../corral-core/tests/store_support/mod.rs"]
mod support;

pub(super) use support::at;

pub(super) fn grow(task: &mut corral_core::Task, history: usize, nested: usize, open: bool) {
    support::grow(task, history, nested + 1, history, nested);
    let current = task.runs.len() - 1;
    for (i, run) in task.runs.iter_mut().enumerate() {
        if i != current {
            run.usage = None;
            for child in &mut run.review_runs {
                child.usage = None;
            }
        }
    }
    if !open {
        let run = task.runs.last_mut().unwrap();
        run.ended = Some(at(1000));
        run.outcome = Some(corral_core::RunOutcome::Done);
    }
    let current = task.plans.len() - 1;
    for (i, plan) in task.plans.iter_mut().enumerate() {
        if i != current {
            for review in &mut plan.reviews {
                review.verdict = Some(corral_core::Verdict::Approve);
            }
        }
    }
}
