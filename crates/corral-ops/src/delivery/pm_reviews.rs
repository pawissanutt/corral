//! Deferred shared-PM requests bind their exact review to the real PM at delivery,
//! never to a fabricated pane during launch preparation.
use anyhow::{Context, Result};
use corral_core::inbox::{Envelope, Inbox, Recipient};
use corral_core::{HerdrRef, TaskHeadMut};
use serde::{Deserialize, Serialize};

use crate::Ctx;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PmReviewTarget {
    Worker {
        task: String,
        run: String,
        review: String,
    },
    Plan {
        task: String,
        plan: String,
        review: String,
    },
    Aggregate {
        task: String,
        attempt: String,
        review: String,
    },
}

impl PmReviewTarget {
    pub(crate) fn task(&self) -> &str {
        match self {
            Self::Worker { task, .. } | Self::Plan { task, .. } | Self::Aggregate { task, .. } => {
                task
            }
        }
    }

    fn input_recipients(&self, ctx: &Ctx) -> Result<Option<[Option<Recipient>; 2]>> {
        Ok(Some(match self {
            Self::Worker { task, run, review } => {
                if ctx
                    .store
                    .head(task)?
                    .run_head(run)?
                    .is_some_and(|source| source.outcome == Some(corral_core::RunOutcome::Stopped))
                {
                    return Ok(None);
                }
                [
                    Some(Recipient::Run {
                        task: task.clone(),
                        run: run.clone(),
                    }),
                    Some(Recipient::ReviewRun {
                        task: task.clone(),
                        run: run.clone(),
                        review: review.clone(),
                    }),
                ]
            }
            Self::Plan { task, plan, review } => {
                let planner = ctx
                    .store
                    .head(task)?
                    .last_plan
                    .as_ref()
                    .filter(|source| source.id == *plan)
                    .and_then(|source| source.run.as_ref())
                    .map(|run| Recipient::PlanRun {
                        task: task.clone(),
                        plan_run: run.clone(),
                    });
                [
                    planner,
                    Some(Recipient::PlanReview {
                        task: task.clone(),
                        plan: plan.clone(),
                        review: review.clone(),
                    }),
                ]
            }
            Self::Aggregate { .. } => [None, None],
        }))
    }

    fn bind(&self, task: &mut TaskHeadMut, herdr: &HerdrRef) -> Result<bool> {
        match self {
            Self::Worker { run, review, .. } => {
                let Some(parent) = task.current_run_mut().filter(|parent| parent.id == *run) else {
                    return Ok(false);
                };
                let Some(child) = parent
                    .current_child
                    .as_deref_mut()
                    .filter(|child| child.id == *review && child.outcome.is_none())
                else {
                    return Ok(false);
                };
                let Some(record) = parent
                    .review
                    .as_mut()
                    .filter(|record| record.reviewer == "pm" && record.verdict.is_none())
                else {
                    return Ok(false);
                };
                if child.herdr.as_ref() == Some(herdr) && child.pending_reply.is_none() {
                    return Ok(false);
                }
                record.pane = Some(herdr.pane_id.clone());
                child.herdr = Some(herdr.clone());
                child.review = Some(record.clone());
                child.pending_reply = None;
                Ok(true)
            }
            Self::Plan { plan, review, .. } => {
                let Some(record) = task
                    .last_plan_mut()
                    .filter(|stored| stored.id == *plan)
                    .and_then(|stored| stored.current_review.as_mut())
                    .filter(|stored| {
                        stored.id == *review
                            && stored.reviewer == "pm"
                            && stored.verdict.is_none()
                            && !stored.cancelled
                    })
                else {
                    return Ok(false);
                };
                if record.herdr.as_ref() == Some(herdr) && record.pending_reply.is_none() {
                    return Ok(false);
                }
                record.herdr = Some(herdr.clone());
                record.pending_reply = None;
                Ok(true)
            }
            Self::Aggregate {
                attempt, review, ..
            } => {
                let Some(record) = task
                    .last_aggregate_attempt_mut()
                    .filter(|stored| stored.id == *attempt)
                    .and_then(|stored| stored.current_review.as_mut())
                    .filter(|stored| {
                        stored.id == *review && stored.reviewer == "pm" && stored.pending()
                    })
                else {
                    return Ok(false);
                };
                if record.herdr.as_ref() == Some(herdr) {
                    return Ok(false);
                }
                record.herdr = Some(herdr.clone());
                Ok(true)
            }
        }
    }
}

/// The caller owns deliver-pm and holds no inbox/dispatch lock. Bind only messages in
/// the selected envelope, immediately before its send attempt; old callbacks cannot
/// accidentally attach a superseding review with the same task id.
pub(super) fn bind(
    ctx: &Ctx,
    inbox: &Inbox<'_>,
    envelope: &Envelope,
) -> Result<Option<Vec<corral_core::StoreLock>>> {
    let mut targets = Vec::new();
    let mut inputs: Vec<corral_core::StoreLock> = Vec::new();
    for seq in &envelope.members {
        let message = inbox.message(*seq)?;
        if let Some(value) = message
            .subject
            .as_ref()
            .and_then(|subject| subject.get("pm_review"))
        {
            targets.push(
                serde_json::from_value::<PmReviewTarget>(value.clone())
                    .context("reading deferred PM review identity")?,
            );
        }
    }
    if targets.is_empty() {
        return Ok(Some(inputs));
    }
    // PM is already claimed. Only try source claims: a queued review may not
    // bypass a later source-session pause/stop, nor invert another delivery lock.
    for target in &targets {
        let Some(recipients) = target.input_recipients(ctx)? else {
            return Ok(None);
        };
        for recipient in recipients.into_iter().flatten() {
            let name = format!("deliver-{}", recipient.key()?);
            let path = ctx.store.dir().join("lock").join(format!("{name}.lock"));
            if !inputs.iter().any(|claim| claim.exclusively_protects(&path)) {
                let Some(claim) = ctx.store.try_lock(&name)? else {
                    return Ok(None);
                };
                inputs.push(claim);
            }
            if !crate::session_control::automation_may_start(ctx, &recipient)? {
                return Ok(None);
            }
        }
    }
    let pane = crate::pm::live_pm_pane(ctx)?.context("deferred PM review has no live PM")?;
    let info = ctx
        .herdr
        .pane_get(&pane)?
        .context("PM pane disappeared before deferred review delivery")?;
    let herdr = HerdrRef {
        workspace_id: info.workspace_id,
        pane_id: pane,
        socket: Some(ctx.herdr.socket_path()?),
    };
    for target in targets {
        ctx.store
            .try_update_head(target.task(), |task| {
                if !target.bind(task, &herdr)? {
                    return Ok(false);
                }
                if let PmReviewTarget::Worker { run, .. } = &target {
                    crate::role_session::publish_reviewer(ctx, task, run)?;
                }
                ctx.store.log_event(
                    Some(target.task()),
                    "pm_review_bound",
                    serde_json::json!({"review": target, "pane": herdr.pane_id}),
                )?;
                Ok(true)
            })
            .context("binding deferred review to PM delivery")?;
    }
    Ok(Some(inputs))
}
