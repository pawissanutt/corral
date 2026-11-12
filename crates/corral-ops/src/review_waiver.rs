//! The owner lands one confirmed co-work finish after its gate without independent review.
//! The gate still runs; the waiver binds the current run and is recorded on it, in history and
//! in the landing report. Agents, assistants and the PM cannot waive.
use anyhow::{Context, Result, bail, ensure};
use corral_core::{
    AutoStage, ReviewWaiver, ReviewWork, RunOutcome, Status, Store, Task, TaskFieldsRef as _,
    Verdict,
};
use serde_json::json;

use crate::Ctx;

#[cfg(test)]
#[path = "review_waiver_tests.rs"]
mod tests;

/// Whether the owner may waive review for this card now; the board offers the action only then.
pub fn eligible(task: &Task) -> Result<()> {
    ensure!(
        task.status == Status::Review,
        "waive_review: {} is {}, not Review",
        task.id,
        task.status.label()
    );
    let cowork = task
        .cowork_run()
        .context("waive_review: only a confirmed co-work finish can skip review")?;
    ensure!(
        cowork.shipping() && cowork.completion.is_some() && cowork.assistant_driver().is_some(),
        "waive_review: needs a confirmed Ship finish still driven by its assistant"
    );
    let run = task.current_run().context("waive_review: no run")?;
    ensure!(
        run.id == cowork.run && run.outcome == Some(RunOutcome::Done),
        "waive_review: the finished co-work run is not the current run"
    );
    ensure!(
        !task.review_waived(),
        "waive_review: review is already waived for {}",
        run.id
    );
    match run.review.as_ref().map(|r| r.verdict) {
        None | Some(Some(Verdict::Approve)) => {}
        Some(None) => bail!("waive_review: a review is running; wait for its verdict"),
        Some(Some(_)) => bail!(
            "waive_review: the reviewer did not approve; overriding a verdict is `corral recover {} --action accept-code`",
            task.id
        ),
    }
    match run.autopilot.as_ref().map(|state| state.stage) {
        None | Some(AutoStage::Gating | AutoStage::Reviewed) => Ok(()),
        Some(AutoStage::Reviewing) if run.review.is_none() => Ok(()),
        Some(stage) => bail!(
            "waive_review: autopilot is {}; review can be skipped while gating or before a reviewer starts",
            stage.as_str()
        ),
    }
}

/// Owner entry point for the CLI and the board.
pub fn waive(ctx: &Ctx, id: &str, reason: &str) -> Result<()> {
    crate::automation_controls::require_automation_owner(&ctx.store)?;
    apply(&ctx.store, id, reason)
}

pub(crate) fn apply(store: &Store, id: &str, reason: &str) -> Result<()> {
    let reason = reason.trim();
    ensure!(
        !reason.is_empty(),
        "waive_review: give a reason; it is recorded with the landing"
    );
    let config = store.reopen()?.config;
    let at = store.now();
    store.dispatch_update(id, |store, task| {
        eligible(task)?;
        ensure!(
            !config.task_review_policy(&*task).gate.is_empty(),
            "waive_review: {} has no gate; landing without review still requires a passing gate",
            task.id
        );
        if task.review_wait == Some(ReviewWork::Reviewer) {
            task.review_wait = None;
        }
        let run = task.current_run_mut().context("waive_review: no run")?;
        // No reviewer launched yet: return to gating, which lands after a passing gate.
        if let Some(state) = run
            .autopilot
            .as_mut()
            .filter(|state| state.stage == AutoStage::Reviewing)
        {
            state.stage = AutoStage::Gating;
        }
        let waived = run.id.clone();
        task.cowork
            .as_mut()
            .context("waive_review: no co-work")?
            .review_waiver = Some(ReviewWaiver {
            run: waived.clone(),
            reason: reason.into(),
            at,
        });
        store.log_event(
            Some(id),
            "review_waived",
            json!({"actor": "owner", "run": waived, "reason": reason}),
        )
    })?;
    Ok(())
}
