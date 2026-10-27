use super::*;
use corral_herdr::AgentStatus;
use serde_json::json;

pub(super) fn record(
    ctx: &Ctx,
    receipt: &StateRecord<'_>,
    state: &mut State,
    i: usize,
    outcome: DeliveryState,
    detail: &str,
) -> Result<()> {
    if state.notices[i].state == outcome && state.notices[i].detail.as_deref() == Some(detail) {
        return Ok(());
    }
    let notice = &mut state.notices[i];
    notice.state = outcome;
    notice.detail = Some(detail.to_string());
    if state.notices[i].state == DeliveryState::Sending {
        // The transport can succeed even if the surrounding callback later refuses.
        RECORDS.save_intent(&ctx.store, receipt.key, state)?;
    } else {
        save(receipt, state)?;
    }
    let notice = &state.notices[i];
    ctx.store.log_event(
        Some(&notice.target.task),
        "main_movement_notice",
        json!({
            "id":notice.id, "run":notice.target.run, "pane":notice.target.herdr,
            "state":notice.state, "detail":notice.detail, "delivery":notice.delivery,
            "overlapping_paths":notice.overlapping_paths, "landings":notice.landings,
            "context":notice.artifact, "receipt":format!("main-movement/{}", receipt.key),
        }),
    )?;
    if notice.state != DeliveryState::Delivered && notice.state != DeliveryState::Sending {
        crate::messages::report(
            Some(&notice.target.task),
            "main movement",
            format!(
                "{} main-movement notice {:?}: {detail}; receipt main-movement/{}",
                notice.target.task, notice.state, receipt.key
            ),
        );
    }
    Ok(())
}

pub(super) fn deliver(
    ctx: &Ctx,
    receipt: &StateRecord<'_>,
    state: &mut State,
    i: usize,
    available: bool,
) -> Result<()> {
    if state.notices[i].state.terminal() {
        return Ok(());
    }
    // No transport provides a transactional send + local receipt. Never blindly replay a send
    // interrupted by a process crash: it may already have reached the worker.
    if state.notices[i].state == DeliveryState::Sending {
        return record(
            ctx,
            receipt,
            state,
            i,
            DeliveryState::Uncertain,
            "interrupted submission; inspect pane before any manual delivery",
        );
    }
    let target = state.notices[i].target.clone();
    let Some(_delivery_lock) = ctx.store.try_lock(&format!("delivery-{}", target.task))? else {
        return record(
            ctx,
            receipt,
            state,
            i,
            DeliveryState::Deferred,
            "another reply delivery holds the pane",
        );
    };
    if !target.matches(&ctx.store.get(&target.task)?) {
        return record(
            ctx,
            receipt,
            state,
            i,
            DeliveryState::Cancelled,
            "run finished or identity changed",
        );
    }
    if state.notices[i].artifact.is_none() {
        state.notices[i].artifact = Some(attach(ctx, &state.notices[i])?);
        let notice = &state.notices[i];
        let tasks: Vec<_> = notice
            .landings
            .iter()
            .map(|l| format!("{}@{}", l.task, l.sha))
            .collect();
        state.notices[i].message = Some(format!(
            "[corral] Main movement for {}:{}: {}. Overlapping paths: {}. Source headroom (effective extension ceilings): {}. Full context: {}. Informational only: choose when to integrate; no rebase or change round was performed. Path disjointness does not prove semantic independence; the reviewer checks composition and the owner decides conflicting requirements.",
            target.task,
            target.run,
            tasks.join(", "),
            notice.overlapping_paths.join(", "),
            notice
                .rust_headroom
                .iter()
                .map(|h| format!("{} candidate={:?} main={:?}", h.path, h.candidate, h.main))
                .collect::<Vec<_>>()
                .join("; "),
            notice.artifact.as_deref().unwrap_or_default(),
        ));
        save(receipt, state)?;
    }
    if !available {
        return record(
            ctx,
            receipt,
            state,
            i,
            DeliveryState::Deferred,
            "herdr unavailable",
        );
    }
    // Hold the record lock through the external send, so done/retry cannot replace this run
    // between its final identity check and submission. No task fields are changed here.
    ctx.store.read_locked(&target.task, |task| {
        if !target.matches(task) {
            record(
                ctx,
                receipt,
                state,
                i,
                DeliveryState::Cancelled,
                "run finished or identity changed",
            )?;
            return Ok(());
        }
        let run = task.current_run().context("live run disappeared")?;
        let deferred = if run.stuck.is_some() {
            Some("worker is stuck")
        } else if run.pending_reply.is_some() {
            Some("worker has a pending reply")
        } else if run.obligations.pending_decision().is_some()
            || task.status == corral_core::Status::NeedsMe
        {
            Some("worker awaits the owner")
        } else {
            None
        };
        if let Some(reason) = deferred {
            record(ctx, receipt, state, i, DeliveryState::Deferred, reason)?;
            return Ok(());
        }
        let Some(reference) = &target.herdr else {
            record(
                ctx,
                receipt,
                state,
                i,
                DeliveryState::Deferred,
                "run has no pane",
            )?;
            return Ok(());
        };
        let pane = match ctx.herdr.pane_get(&reference.pane_id) {
            Ok(Some(pane)) => pane,
            Ok(None) => {
                record(
                    ctx,
                    receipt,
                    state,
                    i,
                    DeliveryState::Deferred,
                    "agent pane is missing",
                )?;
                return Ok(());
            }
            Err(error) => {
                record(
                    ctx,
                    receipt,
                    state,
                    i,
                    DeliveryState::Deferred,
                    &format!("pane lookup failed: {error:#}"),
                )?;
                return Ok(());
            }
        };
        if pane.pane_id != reference.pane_id
            || pane.workspace_id != reference.workspace_id
            || pane.cwd.as_ref() != Some(&target.worktree)
            || corral_git::current_branch(&target.worktree)? != target.branch
        {
            record(
                ctx,
                receipt,
                state,
                i,
                DeliveryState::Cancelled,
                "pane workspace, cwd or branch identity changed",
            )?;
            return Ok(());
        }
        if pane.status == AgentStatus::Blocked || pane.agent.is_none() {
            record(
                ctx,
                receipt,
                state,
                i,
                DeliveryState::Deferred,
                "pane has no receptive worker",
            )?;
            return Ok(());
        }
        let process = match ctx.herdr.pane_process_info(&reference.pane_id) {
            Ok(info) => info,
            Err(error) => {
                record(
                    ctx,
                    receipt,
                    state,
                    i,
                    DeliveryState::Deferred,
                    &format!("pane process lookup failed: {error:#}"),
                )?;
                return Ok(());
            }
        };
        if state.notices[i]
            .shell_pid
            .is_some_and(|pid| pid != process.shell_pid)
        {
            record(
                ctx,
                receipt,
                state,
                i,
                DeliveryState::Cancelled,
                "pane shell identity changed",
            )?;
            return Ok(());
        }
        state.notices[i].shell_pid = Some(process.shell_pid);
        record(
            ctx,
            receipt,
            state,
            i,
            DeliveryState::Sending,
            "submitting informational notice",
        )?;
        let text = state.notices[i]
            .message
            .as_deref()
            .context("notice has no message")?;
        match ctx.herdr.agent_prompt(&reference.pane_id, text) {
            Ok(delivery) => {
                let submitted = delivery.submitted;
                let error = delivery
                    .error
                    .clone()
                    .unwrap_or_else(|| "submission was not observed".into());
                state.notices[i].delivery = Some(delivery);
                if submitted {
                    record(
                        ctx,
                        receipt,
                        state,
                        i,
                        DeliveryState::Delivered,
                        "submission confirmed",
                    )?;
                } else {
                    record(ctx, receipt, state, i, DeliveryState::Failed, &error)?;
                }
            }
            Err(error) => record(
                ctx,
                receipt,
                state,
                i,
                DeliveryState::Uncertain,
                &format!("submission outcome unknown: {error:#}"),
            )?,
        }
        Ok(())
    })?;
    Ok(())
}
