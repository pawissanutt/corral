//! Audited acknowledgement of a reply already delivered outside corral.
use anyhow::{Context, Result, ensure};
use corral_core::{CommentAuthor, ReceiptOutcome, ResumeDelivery, ResumeReceipt, Status, Task};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

#[cfg(test)]
#[path = "reply_ack_tests.rs"]
mod tests;

/// Records an operator's delivery acknowledgement without contacting the worker.
///
/// # Errors
/// Rejects an empty reason, concurrent delivery, or anything other than an exact
/// pending reply on the current open coding run. Finished change requests must
/// use the changes path, which owns review and round policy.
pub fn mark_reply_delivered(
    ctx: &Ctx,
    id: &str,
    reason: &str,
    author: CommentAuthor,
) -> Result<Task> {
    ensure!(
        !reason.trim().is_empty(),
        "delivery acknowledgement reason is empty"
    );
    let id = corral_core::Store::normalize_id(id)?;
    let _delivery = ctx
        .store
        .try_lock(&format!("delivery-{id}"))?
        .context("another reply is being delivered; try again")?;
    let task = ctx.store.get(&id)?;
    if let Some(recipient) = crate::delivery::owner::live_run(&task) {
        // Inbox runs settle through identity-bound receipts and fenced effects, never by text.
        let waiting = crate::delivery::owner::unobserved_owner(ctx, &recipient)?;
        anyhow::bail!(
            "{id} uses inbox delivery; acknowledge the exact message with `corral inbox mark <id> --observed --reason <text>` (unsettled: {})",
            if waiting.is_empty() {
                "none".to_string()
            } else {
                waiting.join(", ")
            }
        );
    }
    let (run, pane, text) = pending(&task)?;
    let control = ctx
        .store
        .events_shared()?
        .iter()
        .rev()
        .find(|event| {
            event.task.as_deref() == Some(id.as_str())
                && event.detail["pane"].as_str() == Some(pane)
                && event.detail["text"].as_str() == Some(text)
                && event.detail["run"].as_str().is_none_or(|id| id == run)
        })
        .is_some_and(|event| event.kind == "checkpoint_requested");
    acknowledge(ctx, &task, reason, author, control)
}

fn pending(task: &Task) -> Result<(&str, &str, &str)> {
    ensure!(
        !task.awaiting_owner() && matches!(task.status, Status::Running | Status::NeedsMe),
        "mark-delivered requires a live coding run; pending review changes require the changes path"
    );
    let run = task
        .current_run()
        .filter(|run| run.outcome.is_none() && run.ended.is_none())
        .context("task has no live coding run")?;
    ensure!(
        run.delivery == corral_core::inbox::DeliveryMode::Legacy,
        "mark-delivered applies to legacy delivery only; use `corral inbox mark` for inbox runs"
    );
    let pane = &run
        .herdr
        .as_ref()
        .context("run has no worker pane")?
        .pane_id;
    let text = run
        .pending_reply
        .as_deref()
        .context("no pending reply to mark delivered")?;
    ensure!(!text.trim().is_empty(), "pending reply is empty");
    Ok((&run.id, pane, text))
}

fn acknowledge(
    ctx: &Ctx,
    expected: &Task,
    reason: &str,
    author: CommentAuthor,
    control: bool,
) -> Result<Task> {
    let identity = pending(expected)?;
    ctx.store.update(&expected.id, |task| {
        ensure!(pending(task)? == identity, "pending reply or run changed before acknowledgement");
        let (run_id, pane, text) = identity;
        // The audit records whether this answered a decision and resumed the run,
        // so phase timing can close the owner's wait at the acknowledgement.
        let pending: Vec<_> = task
            .current_run()
            .context("run disappeared")?
            .obligations
            .decisions
            .iter()
            .filter(|d| d.pending())
            .map(|d| d.request.id.clone())
            .collect();
        let decision = pending.first().filter(|_| !control);
        let resumes = !control && pending.len() <= 1;
        // Persist the audit before clearing anything, inside the guarded task update.
        ctx.store.log_event(Some(&task.id), "reply_marked_delivered", json!({
            "task": task.id, "run": run_id, "pane": pane, "text": text,
            "by": author.as_str(), "reason": reason, "submitted": true, "attempts": 0,
            "control": control, "decision": decision, "resumes": resumes,
        }))?;
        let run = task.current_run_mut().context("run disappeared")?;
        run.pending_reply = None;
        if !control {
            run.blocked_excerpt = None;
            run.seen_working = false;
            run.idle_since = None;
            run.idle_no_report = false;
            if let Some(decision) = run.obligations.decisions.iter_mut().find(|d| d.pending()) {
                let now = Timestamp::now();
                let receipt = ResumeReceipt {
                    version: 1,
                    id: decision.request.id.clone(),
                    run: run_id.to_string(),
                    outcome: ReceiptOutcome::Answered,
                    delivery: ResumeDelivery::Acknowledged,
                    observed_at: now,
                    answer: Some(text.to_string()),
                    data: json!({"via":"corral_reply_mark_delivered","by":author.as_str(),"reason":reason}),
                };
                decision.observation = crate::obligations::receipt_observation(&receipt, now);
                decision.receipt = Some(receipt);
            }
            if let Some(decision) = run.obligations.pending_decision() {
                run.blocked_excerpt = Some(format!(
                    "{}\nAction: {}", decision.request.question, decision.request.requested_action
                ));
                task.status = Status::NeedsMe;
            } else if task.status == Status::NeedsMe {
                task.status = Status::Running;
            }
        }
        Ok(())
    })
}
