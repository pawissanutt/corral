//! Re-admit an exact never-submitted attempt, not a replacement gate or review child.
use anyhow::{Context, Result, ensure};
use corral_core::{OperatingOrigin, ProfileChoice};
use jiff::Timestamp;
use serde_json::json;

use super::{Kind, Pending, ReviewLaunch, submission};
use crate::{Ctx, operating, verification};

pub(super) fn prepare(
    ctx: &Ctx,
    mut pending: Pending,
    origin: operating::Origin,
) -> Result<Pending> {
    let authorization = pending
        .authorization
        .as_ref()
        .context("prepared action has no origin")?;
    ensure!(
        operating::is_automatic(authorization) == (origin == operating::Origin::Automatic),
        "cannot change the prepared action's origin"
    );
    ensure!(
        pending.submitted == Some(false),
        "cannot replay an uncertain auxiliary launch"
    );
    let task = ctx.store.get(&pending.task)?;
    let run = task
        .current_run()
        .filter(|run| run.id == pending.run)
        .context("prepared parent run changed")?;
    let candidate = authorization
        .action
        .candidate
        .as_ref()
        .context("prepared candidate missing")?;
    ensure!(
        verification::identity(ctx, &task, run, candidate)? == *candidate
            && corral_git::is_clean(&run.worktree)?,
        "prepared candidate changed; retained evidence needs owner disposition"
    );
    if matches!(authorization.origin, OperatingOrigin::Automatic { .. }) {
        pending.authorization = Some(operating::authorize(
            ctx,
            authorization.action.clone(),
            operating::Origin::Automatic,
        )?);
    }
    pending.refused = None;
    Ok(pending)
}

impl ReviewLaunch<'_> {
    pub(crate) fn restored(&self) -> bool {
        self.pending.prepared_gate.is_some() || self.pending.prepared_review.is_some()
    }

    pub(crate) fn resume(&mut self) -> Result<()> {
        let ctx = self.ctx;
        let authorization = self
            .pending
            .authorization
            .clone()
            .context("prepared authority missing")?;
        let task = ctx.store.get(&self.pending.task)?;
        let run = task.current_run().context("prepared run missing")?;
        let pane = match &self.pending.pane {
            Some(pane) => pane.clone(),
            None => {
                ensure!(
                    self.pending.kind == Kind::Reviewer && self.pending.close_pane,
                    "prepared shared pane requires owner disposition"
                );
                let herdr = crate::gate::open_run_pane(ctx, &task, run, "review")?;
                self.attach(&herdr.pane_id, true)?;
                ctx.store.update(&task.id, |task| {
                    let run = task
                        .current_run_mut()
                        .context("prepared parent disappeared")?;
                    let review = run.review.as_mut().context("prepared review missing")?;
                    review.pane = Some(herdr.pane_id.clone());
                    let child = run
                        .review_runs
                        .last_mut()
                        .context("prepared child missing")?;
                    child.herdr = Some(herdr.clone());
                    child.review = Some(review.clone());
                    Ok(())
                })?;
                herdr.pane_id
            }
        };
        ensure!(
            ctx.herdr.pane_get(&pane)?.is_some(),
            "prepared pane disappeared"
        );
        let command = match self.pending.kind {
            Kind::Gate => {
                let paths = crate::gate::gate_paths(ctx, &task.id, &run.id);
                ensure!(paths.script.is_file(), "prepared gate script missing");
                format!("bash {} </dev/null", crate::gate_script::qp(&paths.script))
            }
            Kind::Reviewer => {
                let child = run
                    .review_runs
                    .last()
                    .context("prepared reviewer child missing")?;
                let prompt = ctx
                    .store
                    .prompts_dir()
                    .join(format!("{}-{}-review.md", task.id, run.id));
                ensure!(prompt.is_file(), "prepared review prompt missing");
                let profile = ctx.store.config.resolve_profile(
                    &ProfileChoice {
                        profile: Some(child.agent.clone()),
                        model: child.model.clone(),
                        effort: child.effort.clone(),
                    },
                    None,
                    None,
                )?;
                let command = crate::prompt::render_template(
                    &profile,
                    &[
                        ("prompt_file", prompt.display().to_string()),
                        ("name", format!("{} review", task.id)),
                        ("task", task.id.clone()),
                        ("worktree", run.worktree.display().to_string()),
                    ],
                )?;
                let attempt = format!("{}-{}", run.id, child.id);
                crate::launch_exit::arm(
                    &command,
                    &crate::launch_exit::receipt(ctx, &task.id, &attempt),
                )?
            }
        };
        ctx.herdr.pane_report_metadata(
            &pane,
            &format!("{} prepared {:?}", task.id, self.pending.kind),
            &[("task", &task.id), ("run", &run.id)],
        )?;
        let pm = !self.pending.close_pane;
        let reviewer = self.pending.kind == Kind::Reviewer;
        let message = run
            .review_runs
            .last()
            .and_then(|child| child.pending_reply.clone());
        self.submit(
            |store, task| {
                let fresh = Ctx {
                    store: store.clone(),
                    ..ctx.clone()
                };
                let run = task.current_run().context("prepared run disappeared")?;
                let candidate = authorization
                    .action
                    .candidate
                    .as_ref()
                    .context("candidate missing")?;
                ensure!(
                    verification::identity(&fresh, task, run, candidate)? == *candidate
                        && corral_git::is_clean(&run.worktree)?,
                    "prepared candidate changed before resubmission"
                );
                if reviewer && run.review.is_some() {
                    crate::reviewer::current_assessment(ctx, store, task, run)?;
                }
                Ok(())
            },
            || {
                if pm {
                    let delivery = ctx.herdr.agent_prompt(
                        &pane,
                        message.as_deref().context("prepared PM request missing")?,
                    )?;
                    ensure!(delivery.submitted, "prepared PM delivery is unconfirmed");
                    Ok(())
                } else {
                    ctx.herdr.pane_run(&pane, &command)
                }
            },
        )?;
        if self.pending.kind == Kind::Reviewer {
            ctx.store.update(&task.id, |task| {
                let child = task
                    .current_run_mut()
                    .context("parent missing")?
                    .review_runs
                    .last_mut()
                    .context("child missing")?;
                child.started = Timestamp::now();
                child.pending_reply = None;
                Ok(())
            })?;
        }
        ctx.store.log_event(
            Some(&task.id),
            match self.pending.kind {
                Kind::Gate => "gate_started",
                Kind::Reviewer => "review_started",
            },
            json!({"run": run.id, "authorization": authorization, "recovered":true, "pane":pane}),
        )
    }
}

pub(super) fn clear_retained(ctx: &Ctx, pending: &Pending) -> Result<()> {
    let path = submission::retained_path(ctx, &pending.task, &pending.run, pending.kind);
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("clearing submitted auxiliary preparation"),
    }
}
