use super::*;

fn deliver(
    ctx: &Ctx,
    id: &str,
    pane: &str,
    text: &str,
    kind: &str,
    by: Option<&str>,
) -> Result<()> {
    let delivery = ctx.herdr.agent_prompt(pane, text)?;
    ctx.store.log_event(
        Some(id),
        kind,
        json!({
            "pane": pane, "text": text, "by": by,
            "via": delivery.via, "submitted": delivery.submitted,
            "attempts": delivery.attempts, "error": delivery.error,
        }),
    )?;
    if !delivery.submitted {
        bail!(
            "{id}: reply not delivered ({}); pending, retry with `corral reply {id} --resend`",
            delivery
                .error
                .as_deref()
                .unwrap_or("submission was not observed")
        );
    }
    Ok(())
}

/// From Review (or Needs me after an autopilot escalation): sends the change request to the same run's pane
/// (agent_prompt), clears the run's outcome/ended/summary, status Running, logs "changes_requested". The owner
/// redirecting the task resets the autopilot's rounds and its record on the run. Error when the pane is gone.
pub fn request_changes(ctx: &Ctx, id: &str, text: &str) -> Result<Task> {
    send_changes(ctx, id, text, "owner", None, |t| {
        t.rounds = 0;
        if let Some(run) = t.current_run_mut() {
            run.autopilot = None;
        }
    })
}

/// The autopilot's request-changes: like `request_changes`, but records round `round` and `state` on the run.
pub(crate) fn auto_request_changes(
    ctx: &Ctx,
    id: &str,
    text: &str,
    round: u32,
    state: AutopilotState,
    expected: &RepairKey,
) -> Result<Task> {
    send_changes(ctx, id, text, "autopilot", Some(expected), |t| {
        t.rounds = round;
        if let Some(run) = t.current_run_mut() {
            run.autopilot = Some(state);
        }
    })
}

fn send_changes(
    ctx: &Ctx,
    id: &str,
    text: &str,
    by: &str,
    expected: Option<&RepairKey>,
    mark: impl FnOnce(&mut Task),
) -> Result<Task> {
    let mut fresh = ctx.clone();
    fresh.store.reload_config_if_changed()?;
    let ctx = &fresh;
    let id = corral_core::Store::normalize_id(id)?;
    let _owner = ctx
        .store
        .try_lock(&format!("launch-{id}"))?
        .context("dispatch is already in progress for this task")?;
    let _delivery = ctx
        .store
        .try_lock(&format!("delivery-{id}"))?
        .context("another reply is being delivered; try again")?;
    let task = ctx.store.get(&id)?;
    if let Some(expected) = expected {
        expected.check(&task)?;
    }
    if !task.awaiting_owner() {
        bail!(
            "{} is {}; changes can only be requested in Review",
            task.id,
            task.status.label()
        );
    }
    let previous = task.current_run().context("task has no run")?;
    let pane = previous
        .herdr
        .as_ref()
        .context("task has no run attached to herdr")?
        .pane_id
        .clone();
    if ctx.herdr.pane_get(&pane)?.is_none() {
        bail!("{}: agent pane {pane} is gone; use retry instead", task.id);
    }
    if let Some(expected) = expected {
        expected.check(&ctx.store.get(&task.id)?)?;
    }
    // Reopening Review reserves shared capacity before contacting the composer.
    // Pending text and Running status survive a launcher crash without freeing its slot.
    let prepare = |store: &corral_core::Store, current: &mut Task| {
        crate::queue::check_snapshot(ctx, store, &task, current)?;
        if let Some(reason) = crate::queue::blocked_reason(ctx, store, current, false)? {
            bail!("cannot request changes for {}: {reason}", current.id);
        }
        if let Some(expected) = expected {
            expected.check(current)?;
        }
        let run = current
            .current_run_mut()
            .context("run disappeared before changes")?;
        if run
            .pending_reply
            .as_deref()
            .is_some_and(|pending| pending != text)
        {
            bail!(
                "reply not delivered; resend it with corral reply {} --resend before sending another",
                task.id
            );
        }
        run.pending_reply = Some(text.to_string());
        run.outcome = None;
        run.ended = None;
        run.summary = None;
        current.status = Status::Running;
        Ok(())
    };
    let delivery = if let Some(expected) = expected {
        let attempt = format!("repair-{}", expected.digest()?);
        let mut action = crate::operating::identity(
            &task.id,
            &previous.id,
            corral_core::OperatingActionKind::Repair,
            &attempt,
        );
        action.candidate = Some(crate::verification::capture_identity(
            ctx, &task, previous, &attempt,
        )?);
        let authorization =
            crate::operating::authorize(ctx, action, crate::operating::Origin::Automatic)?;
        crate::operating::submit(ctx, &authorization, prepare, || {
            deliver(ctx, &task.id, &pane, text, "changes_requested", Some(by))
        })
    } else {
        ctx.store.dispatch_update(&task.id, prepare)?;
        deliver(ctx, &task.id, &pane, text, "changes_requested", Some(by))
    };
    if let Err(error) = delivery {
        if crate::operating::refused(&error) {
            return Err(error);
        }
        // Keep T-051's failed-delivery contract: preserve the pending request in Review.
        // Do not overwrite a worker that reported completion in the meantime.
        ctx.store.update(&task.id, |current| {
            if current.status == Status::Running
                && let Some(run) = current
                    .current_run_mut()
                    .filter(|r| r.id == previous.id && r.outcome.is_none())
            {
                run.outcome = previous.outcome;
                run.ended = previous.ended;
                run.summary = previous.summary.clone();
                current.status = task.status;
            }
            Ok(())
        })?;
        return Err(error);
    }
    crate::watch::capture_review(ctx, &task, previous);
    let task = ctx.store.update(&task.id, |current| {
        if current.current_run().is_none_or(|r| r.id != previous.id) {
            bail!("run changed after submitting changes");
        }
        if let Some(run) = current.current_run_mut() {
            crate::reviewer::supersede_review(ctx, run, Timestamp::now());
            if run.pending_reply.as_deref() == Some(text) {
                run.pending_reply = None;
            }
            run.blocked_excerpt = None;
            run.seen_working = false;
            run.idle_since = None;
            run.idle_no_report = false;
        }
        mark(current);
        Ok(())
    })?;
    cleanup(ctx, &task, Cleanup::Round);
    Ok(task)
}
