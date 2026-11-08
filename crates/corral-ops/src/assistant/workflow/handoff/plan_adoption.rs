//! Exact plan supersession/provenance uses the existing transfer and submission transaction.
use super::{Prepared, lifecycle};
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::{PlanRun, PlanRunState, Status, Task, TaskRef};

/// Bound only the current submission/review/producer, not accumulated history bytes.
fn pin(task: &Task) -> Result<String> {
    Ok(crate::verification::digest(&serde_json::to_vec(&(
        task.plans.len(),
        task.plans.last().map(|p| {
            (
                &p.id,
                &p.artifact.sha256,
                &p.spec_revision,
                &p.run,
                p.reviews.len(),
                p.reviews.last(),
                &p.approval,
            )
        }),
        task.plan_runs.len(),
        task.plan_runs.last().map(|r| {
            (
                &r.id,
                &r.profile,
                &r.spec_revision,
                &r.state,
                &r.submitted,
                &r.ended,
                r.rounds,
                &r.herdr,
                &r.output_dir,
                r.agent_host.as_ref().map(|s| {
                    (
                        &s.binding,
                        s.current_generation().map(|g| (&g.bound, &g.fenced)),
                    )
                }),
            )
        }),
    ))?))
}

fn check(ctx: &Ctx, task: &Task, prepared: &Prepared, previous: &str) -> Result<()> {
    let journal = super::read_journal(&ctx.store, prepared.id())?.context("transfer missing")?;
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    ensure!(
        task.dispatch_hold
            && !transfer.ownership_committed
            && transfer.task == task.id
            && transfer.contract_revision == task.contract_revision()?
            && pin(task)? == previous,
        "handoff_plan_changed: held task/producer/findings changed during exact adoption"
    );
    let source = ctx.store.get_assistant_container(&journal.assistant)?;
    journal
        .source_boundary
        .as_ref()
        .context("source drain proof missing")?
        .require_current(&ctx.store, &source, &journal)
}

pub(super) fn adopt(ctx: &Ctx, task: &Task, prepared: &Prepared) -> Result<()> {
    let Some(plan) = &prepared.package().plan else {
        return Ok(());
    };
    let bytes = super::super::publication::load_artifact(&ctx.store, &plan.artifact)?;
    let previous = pin(task)?;
    if corral_core::PlanDocument::parse(std::str::from_utf8(&bytes)?).is_err() {
        // A newer raw draft is prompt input, never a submitted/approved replacement.
        // Keep the old plan immutable, but its stopping state cannot authorize this draft.
        ctx.store.update(&task.id, |task| {
            check(ctx, task, prepared, &previous)?;
            let mut journal =
                super::read_journal(&ctx.store, prepared.id())?.context("transfer missing")?;
            let transfer = journal.transfer.as_mut().context("transfer missing")?;
            let predecessor = task.plans.last().map(|p| p.id.clone());
            ensure!(
                transfer
                    .draft_predecessor
                    .as_ref()
                    .is_none_or(|p| Some(p) == predecessor.as_ref()),
                "handoff_plan_changed: draft predecessor changed"
            );
            transfer.draft_predecessor = predecessor;
            task.status = Status::Ready;
            super::recovery::save(&ctx.store, &journal)
        })?;
        return Ok(());
    }
    if task.plans.last().is_some_and(|p| {
        p.artifact.sha256 == plan.artifact.blob.hash().as_str()
            && crate::plan_spec_revision(task).is_ok_and(|spec| p.spec_revision == spec)
    }) {
        // Existing same-task acceptance is history, not imported source approval.
        return Ok(());
    }
    let assistant = ctx.store.get_assistant(&prepared.package().assistant)?;
    let temp = tempfile::tempdir().context("prepare exact plan submission")?;
    let path = temp.path().join("plan.md");
    std::fs::write(&path, &bytes).context("write exact plan snapshot")?;
    let submitted = crate::plan::plan_submit_prepared(
        ctx,
        &task.id,
        &path,
        &|task| check(ctx, task, prepared, &previous),
        &|task| {
            let mut journal =
                super::read_journal(&ctx.store, prepared.id())?.context("transfer missing")?;
            let transfer = journal.transfer.as_mut().context("transfer missing")?;
            if let Some(id) = &transfer.imported_plan_run {
                // Recover an older interrupted provenance preparation without another D ID.
                let run = task.plan_runs.last().context("imported producer missing")?;
                ensure!(
                    &run.id == id
                        && run.profile == assistant.profile
                        && run.submitted.is_none()
                        && run.state == PlanRunState::Planning
                        && run.agent_host.is_none()
                        && run.herdr.is_none()
                        && run.ended.is_some()
                        && run.inbox.settled.is_some()
                        && run.spec_revision == crate::plan_spec_revision(task)?,
                    "handoff_plan_changed: imported producer identity changed"
                );
                return Ok(());
            }
            ensure!(
                task.plan_runs.last().is_none_or(|r| !r.holds_slot()),
                "handoff_plan_busy: prior producer must settle"
            );
            let now = corral_core::clock::now();
            let id = format!("D{}", task.plan_runs.len() + 1);
            task.plan_runs.push(PlanRun {
                agent_host: None,
                execution: None,
                route: None,
                delivery: Default::default(),
                inbox: Default::default(),
                id: id.clone(),
                profile: assistant.profile.clone(),
                harness: "corral-pi".into(),
                model: None,
                vendor: None,
                effort: None,
                spec_revision: crate::plan_spec_revision(task)?,
                started: now,
                ended: Some(now),
                herdr: None,
                output_dir: ctx
                    .store
                    .dir()
                    .join("handoff-plan")
                    .join(&task.id)
                    .join(&id),
                submitted: None,
                rounds: 0,
                state: PlanRunState::Planning,
                pending_reply: None,
            });
            // Provenance only; never a launched/unfinished planner. Submission and this
            // companion record commit together, so no synthetic active slot is published.
            task.plan_runs
                .last_mut()
                .context("producer missing")?
                .inbox
                .settled = Some(now);
            transfer.imported_plan_run = Some(id);
            super::recovery::save(&ctx.store, &journal)
        },
    )?;
    ensure!(
        submitted.artifact.sha256 == plan.artifact.blob.hash().as_str(),
        "handoff_plan_changed: submitted bytes differ"
    );
    Ok(())
}

pub(crate) fn draft_pending<T: TaskRef + ?Sized>(
    task: &T,
    transfer: &lifecycle::Transfer,
) -> Result<bool> {
    let Some(id) = &transfer.draft_predecessor else {
        return Ok(false);
    };
    Ok(task.last_plan_head()?.is_some_and(|p| &p.id == id))
}
