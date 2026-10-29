use anyhow::{Context, Result, ensure};
use corral_core::{Run, Status, Store, Task, TaskRef};

use crate::{Ctx, review_budget::ReviewLaunch, verification};

pub(super) fn submit<R>(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    launch: &mut ReviewLaunch<'_>,
    pending_reply: Option<&str>,
    send: impl FnOnce() -> Result<R>,
) -> Result<R> {
    launch.submit_head(
        |store, current| {
            ensure!(
                current.id == task.id && current.status == Status::Review,
                "review task changed before submission"
            );
            let parent = current
                .current_run_head()?
                .filter(|r| r.id == run.id)
                .context("review parent changed before submission")?;
            ensure!(
                parent
                    .current_child
                    .as_deref()
                    .and_then(|r| r.pending_reply.as_deref())
                    == pending_reply,
                "review transport request changed before submission"
            );
            current_assessment_ref(ctx, store, &*current, &run.id)
        },
        send,
    )
}

/// Every reviewer submission, new or resumed, re-checks the candidate, the recorded route
/// and reviewer family against the fresh store under the same mutation boundary.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "kept full form (plan T-253 section 4): automatic actions use the head form"
    )
)]
pub(crate) fn current_assessment(
    ctx: &Ctx,
    store: &Store,
    task: &Task,
    parent: &Run,
) -> Result<()> {
    assess(ctx, store, task, parent, |at| {
        Ok(parent
            .review_runs
            .iter()
            .rev()
            .find(|child| child.review.as_ref().is_some_and(|r| r.at == at))
            .and_then(|child| child.vendor.clone()))
    })
}

/// [`current_assessment`] over a task form: the current run comes from the shell of the
/// head fields and current containers, the reviewing child by its review time.
pub(crate) fn current_assessment_ref<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    store: &Store,
    task: &T,
    parent: &str,
) -> Result<()> {
    let shell = super::review_shell(task)?;
    let run = shell
        .runs
        .last()
        .filter(|run| run.id == parent)
        .context("review parent changed before submission")?;
    assess(ctx, store, &shell, run, |at| {
        Ok(task
            .review_child_by_review_at(parent, at)?
            .and_then(|child| child.vendor))
    })
}

fn assess(
    ctx: &Ctx,
    store: &Store,
    task: &Task,
    parent: &Run,
    vendor_at: impl FnOnce(jiff::Timestamp) -> Result<Option<String>>,
) -> Result<()> {
    let record = parent
        .review
        .as_ref()
        .context("review record disappeared")?;
    let mut fresh = ctx.clone();
    fresh.store = store.clone();
    ensure!(
        verification::review_changes(&fresh, task, parent, record)?.is_empty()
            && corral_git::is_clean(&parent.worktree)?,
        "review candidate or evidence changed before submission"
    );
    if let Some(route) = record.route.as_deref() {
        let changes =
            crate::review_packet::route_changes(&fresh, task, parent, record, route, false)?;
        ensure!(
            changes.is_empty(),
            "review assessment changed before submission: {}",
            serde_json::to_string(&changes)?
        );
    }
    let mut policy = store.config.task_review_policy(task);
    if crate::review_routing::choice(&store.config, task, None)
        == corral_core::Reviewer::CrossVendor
    {
        policy.require_cross_family = true;
    }
    let (_, resolved) = crate::review_routing::resolve(
        &store.config,
        &if record.reviewer == "pm" {
            corral_core::Reviewer::Pm
        } else {
            corral_core::Reviewer::Profile(record.reviewer.clone())
        },
        parent.vendor.as_deref(),
    )?;
    // An availability fallback reviews on its own model; check the family that actually ran.
    let actual = vendor_at(record.at)?.or(resolved.vendor);
    policy.check_reviewer_family(parent.vendor.as_deref(), actual.as_deref())
}

#[cfg(all(test, unix))]
mod assessment_tests {
    use corral_core::Status;

    use crate::submission_paths_tests::fixture;

    /// `current_assessment` and its task-form counterpart on the same reviewer state.
    fn assessments(fallback: bool, stale: bool) -> (Result<(), String>, Result<(), String>) {
        let fx = fixture::Fx::new();
        let id = fx.task("assessment", Status::Ready).id;
        fx.start(&id);
        fx.done(&id, "candidate.txt");
        crate::gate_start(&fx.ctx, &id).unwrap();
        assert!(crate::gate_collect(&fx.ctx, &id).unwrap().is_some());
        let task = fx.ctx.store.get(&id).unwrap();
        let run = task.current_run().unwrap();
        let gate = run.gate.as_ref().unwrap().identity.as_ref().unwrap();
        let candidate = crate::verification::identity(&fx.ctx, &task, run, gate).unwrap();
        fx.ctx
            .store
            .update(&id, |task| {
                let run = task.current_run_mut().unwrap();
                let digest = if stale {
                    "stale".to_owned()
                } else {
                    crate::verification::evidence_digest(run)?
                };
                let review = corral_core::ReviewRecord {
                    route: None,
                    identity: Some(candidate.clone()),
                    evidence_digest: Some(digest),
                    verdict: None,
                    process_lost: false,
                    reviewer: "rev".into(),
                    notes: None,
                    sha: candidate.head.clone(),
                    at: fixture::now(),
                    pane: None,
                };
                let mut child = run.clone();
                child.id = "R1-review1".into();
                child.agent = "rev".into();
                child.review = Some(review.clone());
                child.review_runs.clear();
                run.review = Some(review);
                if fallback {
                    let mut second = child.clone();
                    second.id = "R1-review2".into();
                    second.vendor = Some("other-vendor".into());
                    run.review_runs.push(child);
                    run.review_runs.push(second);
                } else {
                    run.review_runs.push(child);
                }
                Ok(())
            })
            .unwrap();
        let store = &fx.ctx.store;
        let task = store.get(&id).unwrap();
        let run = task.current_run().unwrap();
        let full =
            super::current_assessment(&fx.ctx, store, &task, run).map_err(|e| format!("{e:#}"));
        let head = store.head(&id).unwrap();
        let headed = super::current_assessment_ref(&fx.ctx, store, &head, &run.id)
            .map_err(|e| format!("{e:#}"));
        (full, headed)
    }

    #[test]
    fn current_assessment_task_form_equals_the_full_form() {
        let mut outcomes = std::collections::BTreeSet::new();
        for fallback in [false, true] {
            for stale in [false, true] {
                let (full, headed) = assessments(fallback, stale);
                assert_eq!(full, headed, "fallback={fallback} stale={stale}");
                outcomes.insert(full.is_ok());
            }
        }
        assert!(outcomes.contains(&false), "no variant refused");
    }
}
