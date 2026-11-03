//! Co-work keeps its assistant as the repair destination; it never invents a worker session.
use anyhow::Context;
use corral_core::TaskFieldsRef as _;
use corral_core::inbox::{Inbox, MessageBinding, Post, Recipient, Urgency};

use super::*;

#[cfg(test)]
#[path = "cowork_tests.rs"]
mod tests;

/// Whether a repair can be queued, and whether its originating assistant is live.
fn destination(ctx: &Ctx, session: &str) -> (bool, bool) {
    let open = ctx
        .store
        .get_assistant(session)
        .is_ok_and(|s| s.ended.is_none());
    let live = open
        && ctx
            .store
            .get_assistant_container(session)
            .is_ok_and(|container| crate::agent::pi::host_alive(&container.state).unwrap_or(false));
    (open, live)
}

/// A resumed/busy assistant still receives its queued repair on subsequent syncs. Native
/// delivery owns replay suppression; do not re-post or send anything into the owner's pane.
pub(super) fn deliver_pending(ctx: &Ctx) {
    let sweep = || -> Result<()> {
        for head in ctx
            .store
            .heads(&corral_core::backend::HeadQuery::Open { project: None })?
        {
            let Some(cowork) = head.cowork_run() else {
                continue;
            };
            if !head
                .current_run
                .as_ref()
                .and_then(|r| r.autopilot.as_ref())
                .is_some_and(|state| state.stage == AutoStage::Changes)
            {
                continue;
            }
            let (open, live) = destination(ctx, &cowork.session);
            if head.status == Status::Review && !live {
                let previous = head
                    .current_run
                    .as_ref()
                    .and_then(|r| r.autopilot.as_ref())
                    .and_then(|state| state.reason.as_deref())
                    .unwrap_or("co-work repair pending");
                owner_wait(
                    ctx,
                    &head,
                    &format!(
                        "{previous}\nAssistant {} stopped before repair completed. Owner: run `corral pi --resume {}` if open, or finish the named scratch from your shell if ended.",
                        cowork.session, cowork.session
                    ),
                )?;
            }
            if open {
                let recipient = Recipient::Assistant {
                    session: cowork.session.clone(),
                };
                crate::delivery::deliver(
                    ctx,
                    &recipient,
                    &crate::agent::delivery::CorralPiAdapter,
                    None,
                )?;
            }
        }
        Ok(())
    };
    if let Err(error) = sweep() {
        crate::messages::report(
            None,
            "cowork",
            format!("assistant repair delivery deferred: {error:#}"),
        );
    }
}

/// Keep the repair stage resumable while making failed/offline delivery actionable to the owner.
fn owner_wait(ctx: &Ctx, head: &corral_core::TaskHead, reason: &str) -> Result<()> {
    let run = head.current_run.as_ref().context("co-work run missing")?;
    let state = run.autopilot.as_ref().context("co-work repair missing")?;
    let claim = crate::owner_notice::prepare(
        ctx,
        head,
        "cowork_repair",
        &format!("{}/{}", run.id, state.at),
        ctx.store.now(),
        vec![],
    )?;
    if let Some(committed) = crate::owner_notice::commit_head(ctx, &head.id, |_, current| {
        let Some(current_run) = current.current_run_mut().filter(|r| {
            r.id == run.id
                && r.autopilot
                    .as_ref()
                    .is_some_and(|s| s.stage == AutoStage::Changes)
        }) else {
            return Ok(None);
        };
        current_run
            .autopilot
            .as_mut()
            .context("co-work repair missing")?
            .reason = Some(reason.into());
        current.status = Status::NeedsMe;
        Ok(Some(claim))
    })? {
        crate::owner_notice::notify(
            ctx,
            committed,
            &format!("{} needs co-work repair", head.id),
            reason,
            true,
        )?;
    }
    Ok(())
}

impl Pilot<'_> {
    /// Do not rewrite/re-gate a candidate while something other than verification blocks it.
    pub(super) fn cowork_wait(&self) -> Result<Option<Step>> {
        if self.head.review_wait == Some(corral_core::ReviewWork::Reviewer)
            && (crate::operating::require(&self.ctx.store, self.head, OperatingActionKind::Review)
                .is_err()
                || crate::review_budget::reviewer_waiting(self.ctx, self.id())?)
        {
            return Ok(Some(Step::Wait));
        }
        match self.stage() {
            Some(AutoStage::Changes) => return Ok(Some(Step::Wait)),
            Some(AutoStage::Reviewed)
                if crate::operating::require(
                    &self.ctx.store,
                    self.head,
                    OperatingActionKind::Accept,
                )
                .is_err() =>
            {
                return Ok(Some(Step::Wait));
            }
            Some(AutoStage::Reviewing)
                if self
                    .run
                    .review
                    .as_ref()
                    .is_some_and(|r| r.verdict.is_none()) =>
            {
                return self.reviewing().map(Some);
            }
            Some(AutoStage::Blocked) => {
                let repo = main_repo(self.ctx, self.head, self.run)?;
                if corral_git::current_branch(&repo).ok().as_deref() != Some(&self.run.base)
                    || has_tracked_changes(&repo)?
                {
                    return Ok(Some(Step::Wait));
                }
            }
            _ => {}
        }
        if self.policy.reviewer == Reviewer::None
            && self.head.cowork_run().is_some_and(|c| c.owner_confirmed)
        {
            return self.escalate(&format!(
                "co-work finish needs an independent reviewer; configure the project reviewer, then run `corral review-run {}`",
                self.id()
            )).map(Some);
        }
        Ok(None)
    }

    pub(super) fn cowork_changes(
        &self,
        cowork: &corral_core::Cowork,
        message: &str,
        summary: &str,
        mut state: AutopilotState,
        key: &RepairKey,
        round: u32,
    ) -> Result<Step> {
        let session = &cowork.session;
        let resume = format!("corral pi --resume {session}");
        let finish = format!(
            "Fix and commit in {}, then use the original owner finish instruction with \
             `corral_owner_action` (kind finish, target {}, args --into {} --expected-head <new HEAD>). \
             From the owner's shell: `corral assistant scratch finish {} --session {session} --into {} --expected-head <new HEAD>`. \
             This resubmits the same work for gate and independent review; do not dispatch a worker.",
            self.run.worktree.display(),
            cowork.scratch,
            self.id(),
            cowork.scratch,
            self.id()
        );
        let message = format!("{message}\n\nCo-work repair for assistant {session}: {finish}");
        let (open, live) = destination(self.ctx, session);
        let recipient = Recipient::Assistant {
            session: session.clone(),
        };
        let reason = if live {
            format!("{summary}; changes returned to assistant {session}. {finish}")
        } else if open {
            format!(
                "{summary}; assistant {session} is offline. Owner: run `{resume}` to receive the queued repair. {finish}"
            )
        } else {
            format!(
                "{summary}; assistant {session} ended or is unavailable. Owner: reopen the scratch in an assistant and finish it from your shell. {finish}"
            )
        };
        state.reason = Some(reason.clone());
        let status = if live {
            Status::Review
        } else {
            Status::NeedsMe
        };
        let attempt = format!("cowork-repair-{}", key.digest()?);
        let mut action = crate::operating::identity(
            self.id(),
            &self.run.id,
            OperatingActionKind::Repair,
            &attempt,
        );
        action.candidate = Some(verification::capture_identity(
            self.ctx, self.head, self.run, &attempt,
        )?);
        let authorization =
            crate::operating::authorize(self.ctx, action, crate::operating::Origin::Automatic)?;
        let submitted = crate::operating::submit_head(
            self.ctx,
            &authorization,
            |store, current| {
                key.check(&*current)?;
                crate::operating::require(store, &*current, OperatingActionKind::Repair)?;
                crate::reviewer::supersede_review_head(current, self.now)?;
                current
                    .current_run_mut()
                    .context("co-work run disappeared")?
                    .autopilot = Some(state);
                current.rounds = round;
                current.status = status;
                Ok(())
            },
            || {
                if open {
                    Inbox::new(&self.ctx.store, recipient.clone())?.post(Post {
                        kind: "changes_requested",
                        urgency: Urgency::Wake,
                        author: "autopilot",
                        body: &message,
                        binding: MessageBinding::default(),
                        subject: Some(
                            json!({"key": attempt, "task": self.id(), "run": self.run.id}),
                        ),
                        max_payload: corral_core::inbox::MAX_PAYLOAD,
                    })?;
                }
                Ok(())
            },
        );
        if let Err(error) = submitted {
            if crate::operating::transient_refusal(&error) {
                return Ok(Step::Wait);
            }
            let current = self.ctx.store.head(self.id())?;
            if current.current_run.as_ref().is_some_and(|r| {
                r.id == self.run.id
                    && r.autopilot
                        .as_ref()
                        .is_some_and(|s| s.stage == AutoStage::Changes)
            }) {
                let reason = format!(
                    "{reason}\nAssistant repair delivery failed: {error:#}. Owner: {finish}"
                );
                owner_wait(self.ctx, &current, &reason)?;
                return Ok(Step::Moved(Change {
                    task: self.id().into(),
                    from: Status::Review,
                    to: Status::NeedsMe,
                    note: reason,
                    stage: Some(AutoStage::Changes),
                }));
            }
            return Err(error);
        }
        crate::cleanup::cleanup_head(
            self.ctx,
            &self.ctx.store.head(self.id())?,
            crate::cleanup::Cleanup::Round,
        );
        if open
            && let Err(error) = crate::delivery::deliver(
                self.ctx,
                &recipient,
                &crate::agent::delivery::CorralPiAdapter,
                None,
            )
        {
            crate::messages::report(
                Some(self.id()),
                "cowork",
                format!("{session}: repair queued; delivery deferred: {error:#}"),
            );
        }
        if !live {
            // Keep the Changes stage so the owner's finish --into can resubmit the attached
            // scratch. Needs me supplies the ordinary owner attention, not worker recovery.
            owner_wait(self.ctx, &self.ctx.store.head(self.id())?, &reason)?;
        }
        self.log(
            AutoStage::Changes,
            round,
            None,
            Some(&reason),
            "returned to originating assistant",
        )?;
        Ok(Step::Moved(Change {
            task: self.id().into(),
            from: Status::Review,
            to: status,
            note: reason,
            stage: Some(AutoStage::Changes),
        }))
    }
}
