//! Read the submitted plan independently of unrelated later planner failures.
use anyhow::Result;
use corral_core::{PlanRunState, Status, Task, Verdict};

use super::PmObservation;
use crate::{Ctx, plan};

pub(super) fn observation(ctx: &Ctx, task: &Task) -> Result<Option<PmObservation>> {
    // Implementation keeps its existing question, gate and review precedence.
    if task.current_run().is_some_and(|r| r.outcome.is_none()) {
        return Ok(None);
    }
    let run = task.plan_runs.last();
    if let Some(run) = run.filter(|r| r.holds_slot()) {
        // A real replacement/revision is active, unlike a terminal failed D1.
        return Ok(
            (task.status == Status::NeedsMe).then(|| PmObservation::Owner {
                task: task.id.clone(),
                reason: format!(
                    "Planner {} ({:?}): {}",
                    run.id,
                    run.state,
                    run.pending_reply
                        .as_deref()
                        .unwrap_or("Planner needs owner input.")
                ),
            }),
        );
    }
    if let Some(selected) = task.plans.last().filter(|p| p.approval.is_none())
        && selected.spec_revision == plan::plan_spec_revision(task)?
    {
        let review = selected.reviews.last();
        if let Some(review) = review.filter(|r| r.verdict.is_none()) {
            return Ok(Some(PmObservation::ReviewInFlight {
                task: task.id.clone(),
                reviews: vec![format!("{} {}/{}", task.id, selected.id, review.id)],
            }));
        }
        let reason = match review.and_then(|r| r.verdict) {
            Some(Verdict::Escalate) => format!(
                "Plan {} escalated: {}",
                selected.id,
                review
                    .and_then(|r| r.notes.as_deref())
                    .unwrap_or("Owner decision required.")
            ),
            Some(Verdict::Approve) => format!(
                "Plan {} has review approval but no final owner acceptance.",
                selected.id
            ),
            Some(Verdict::Changes) => format!(
                "Plan {} needs revisions: {}",
                selected.id,
                review
                    .and_then(|r| r.notes.as_deref())
                    .unwrap_or("See latest review.")
            ),
            None => format!("Plan {} awaits independent review.", selected.id),
        };
        let mut evidence = vec![
            format!(
                "plan {}; spec {}; snapshot {}; SHA-256 {}; final acceptance: none",
                selected.id,
                selected.spec_revision,
                ctx.store.dir().join(&selected.artifact.path).display(),
                selected.artifact.sha256
            ),
            format!(
                "owner decisions: {}",
                serde_json::to_string(
                    &selected
                        .document
                        .owner_decisions
                        .iter()
                        .chain(task.completion.iter().flat_map(|m| &m.owner_decisions))
                        .collect::<Vec<_>>()
                )?
            ),
        ];
        if let Some(review) = review {
            evidence.push(format!(
                "plan {}/{}; reviewer {}; verdict {:?}; policy {}; notes: {}",
                selected.id,
                review.id,
                review.reviewer,
                review.verdict,
                review.policy_digest,
                review.notes.as_deref().unwrap_or("")
            ));
        }
        if let Some(run) = run {
            evidence.push(format!(
                "latest planner {}; state {:?}; submitted {:?}; reason: {}",
                run.id,
                run.state,
                run.submitted,
                run.pending_reply.as_deref().unwrap_or("none recorded")
            ));
        }
        // Read current policy and exact bytes. Historical mismatch prose is
        // evidence, not proof of either a current routing defect or approval.
        let readiness = (|| -> Result<(bool, bool)> {
            plan::fresh(ctx, task, selected)?;
            let config = plan::config(ctx)?;
            let expected = plan::expected_reviewer(&config, task)?;
            let digest = plan::policy_digest(&config, task)?;
            let approved = review.is_some_and(|r| {
                r.verdict == Some(Verdict::Approve)
                    && r.reviewer == expected
                    && r.policy_digest == digest
            });
            evidence.push(format!(
                "current reviewer: {expected}; current policy: {digest}; independent approval current: {approved}"
            ));
            Ok((expected != "owner", approved || expected == "owner"))
        })();
        let (review_ready, approval_ready) = match readiness {
            Ok(ready) => ready,
            Err(error) => {
                evidence.push(format!("current plan validation: {error:#}"));
                (false, false)
            }
        };
        return Ok(Some(PmObservation::PlanDecision {
            task: task.id.clone(),
            plan: selected.id.clone(),
            reason,
            evidence,
            review_ready,
            approval_ready,
            notes_file: ctx
                .store
                .state_dir()
                .join(format!("{}-{}-owner-plan-notes.md", task.id, selected.id))
                .display()
                .to_string(),
        }));
    }
    Ok(run
        .filter(|r| {
            task.status == Status::NeedsMe
                && matches!(
                    r.state,
                    PlanRunState::Failed | PlanRunState::Escalated | PlanRunState::Cancelled
                )
        })
        .map(|run| PmObservation::Owner {
            task: task.id.clone(),
            reason: format!(
                "Planner {} ({:?}): {}",
                run.id,
                run.state,
                run.pending_reply
                    .as_deref()
                    .unwrap_or("Inspect plan run history before deciding recovery.")
            ),
        }))
}
