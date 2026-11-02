//! Common progress validation and the native head-only final transaction.
use super::*;

pub(super) fn next_progress<T: TaskRef + ?Sized>(
    task: &T,
    input: &ProgressUpdate,
) -> Result<RunProgress> {
    identity(task, &input.task)?;
    let run = task.current_run_head()?.context("missing_run")?;
    if run.id != input.run {
        bail!("stale_run: expected {}, received {}", run.id, input.run);
    }
    if !matches!(task.status(), Status::Running | Status::NeedsMe) || run.outcome.is_some() {
        bail!("run_closed: {}", run.id);
    }
    if progress_spec_revision(task)? != input.spec_revision {
        bail!("stale_spec");
    }
    let revision = run.progress.as_ref().map_or(0, |p| p.revision);
    if revision != input.expected_revision {
        bail!("stale_progress: expected {revision}");
    }
    unique(input.phases.iter().map(|p| p.id.as_str()))?;
    let steps: Vec<_> = input.phases.iter().flat_map(|p| &p.steps).collect();
    unique(steps.iter().map(|s| s.id.as_str()))?;
    if input.phases.iter().any(|p| p.label.trim().is_empty())
        || steps.iter().any(|s| s.label.trim().is_empty())
    {
        bail!("empty_progress_label");
    }
    if steps
        .iter()
        .filter(|s| s.status == StepState::InProgress)
        .count()
        > 1
    {
        bail!("multiple_current_steps");
    }
    for step in &steps {
        unique(step.criteria.iter().map(String::as_str))?;
        for id in &step.criteria {
            if !task
                .completion()
                .as_ref()
                .is_some_and(|m| m.criteria.iter().any(|c| &c.id == id))
            {
                bail!("unknown_criterion: {id}");
            }
        }
        if let StepState::Dropped { reason } = &step.status
            && reason.trim().is_empty()
        {
            bail!("drop_reason_required: {}", step.id);
        }
    }
    if let Some(previous) = &run.progress {
        for phase in &previous.phases {
            if !input.phases.iter().any(|p| p.id == phase.id) {
                bail!("phase_removed: {}; retain its identity", phase.id);
            }
        }
        for step in previous.steps() {
            let next = steps.iter().find(|s| s.id == step.id).with_context(|| {
                format!("step_removed: {}; drop with a reason instead", step.id)
            })?;
            match &step.status {
                StepState::Done | StepState::Dropped { .. } if next.status != step.status => {
                    bail!("terminal_step_changed: {}", step.id)
                }
                StepState::Pending
                | StepState::InProgress
                | StepState::Done
                | StepState::Dropped { .. } => {}
            }
        }
    }
    Ok(RunProgress {
        revision: revision
            .checked_add(1)
            .context("progress_revision_exhausted")?,
        spec_revision: input.spec_revision.clone(),
        phases: input.phases.clone(),
        plan: None,
        waits: Vec::new(),
    })
}

pub(super) fn update_progress(
    ctx: &Ctx,
    id: &str,
    input: &ProgressUpdate,
    origin: &RequestOrigin,
) -> Result<Receipted> {
    effects::swept(ctx, origin, effects::Home::of(origin), || {
        let mut receipt = None;
        ctx.store.update_head(id, |task| {
            effects::recheck_head(task, origin)?;
            let progress = next_progress(task, input)?;
            let revision = progress.revision;
            let run = task.current_run_mut().context("missing_run")?;
            run.progress = Some(progress);
            run.checkpoint_required = true;
            let detail = json!({"run": input.run, "revision": revision});
            let event = effects::event("progress_updated", detail, origin);
            receipt = Some(effects::record_head(
                task,
                origin,
                effects::Commit {
                    result: json!({"run":input.run,"revision":revision,"event":event}),
                    effect: Some(("progress", revision.to_string())),
                    owed: vec!["effect:event"],
                },
            )?);
            Ok(())
        })?;
        Ok(receipt)
    })
}
