//! Bounded infrastructure waits and the autopilot's source-owned notification claims.

use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use corral_core::attention::{NoticeClaim, NoticeCoverage, NoticeFact};
use corral_core::{InfraWait, InfraWaitClass};

use super::*;
use crate::owner_notice;

pub(super) fn classified(error: &anyhow::Error) -> bool {
    error.is::<corral_git::GitLockContention>() || error.is::<corral_git::GitOperationOngoing>()
}

fn observation(error: &anyhow::Error, now: Timestamp) -> Result<InfraWait> {
    let (class, marker, path, first_mtime) =
        if let Some(lock) = error.downcast_ref::<corral_git::GitLockContention>() {
            (
                InfraWaitClass::Contention,
                lock.marker.clone(),
                lock.path.clone(),
                Some(Timestamp::try_from(lock.mtime).context("converting Git lock mtime")?),
            )
        } else {
            let operation = error
                .downcast_ref::<corral_git::GitOperationOngoing>()
                .context("unclassified infrastructure wait")?;
            (
                InfraWaitClass::Operation,
                operation.marker.clone(),
                operation.path.clone(),
                None,
            )
        };
    Ok(InfraWait {
        version: 1,
        class,
        marker,
        path,
        first_seen: now,
        first_mtime,
        held_at: None,
    })
}

/// Resolve only the already-validated linked checkout's administrative pointer.
/// Normal ownership validation runs again after the markers clear.
fn roots(checkout: &Path) -> Result<(PathBuf, PathBuf)> {
    let pointer = std::fs::read_to_string(checkout.join(".git"))
        .context("reading waiting target's Git administrative pointer")?;
    let admin = PathBuf::from(
        pointer
            .trim_end()
            .strip_prefix("gitdir: ")
            .context("waiting target is not a linked worktree")?,
    );
    ensure!(
        admin.is_absolute(),
        "waiting target Git directory is not absolute"
    );
    let common = std::fs::read_to_string(admin.join("commondir"))
        .context("reading waiting target's common directory pointer")?;
    let common = admin
        .join(common.trim_end())
        .canonicalize()
        .context("resolving waiting target's common directory")?;
    Ok((admin, common))
}

impl Pilot<'_> {
    pub(super) fn record_infra(&self, error: &anyhow::Error) -> Result<Step> {
        let wait = observation(error, self.now)?;
        self.ctx.store.try_update(self.id(), |task| {
            if task.status != Status::Review
                || task.current_run() != Some(self.run)
                || task.infra_wait.is_some()
            {
                return Ok(false);
            }
            task.infra_wait = Some(wait);
            Ok(true)
        })?;
        Ok(Step::Wait)
    }

    pub(super) fn infra_guard(&self) -> Result<Option<Step>> {
        let wait = self
            .task
            .infra_wait
            .as_ref()
            .context("missing infra wait")?;
        let target = self
            .run
            .plan_target
            .as_ref()
            .context("waiting target binding missing")?;
        let (admin, common) = roots(&target.checkout)?;
        match corral_git::check_target_markers(&admin, &common, &target.branch) {
            Err(error) if classified(&error) => {
                let current = observation(&error, self.now)?;
                // One uninterrupted marker episode keeps its original age even if another
                // marker takes precedence. The complete fixed set must clear to resume.
                if wait.held_at.is_none()
                    && self.now.duration_since(wait.first_seen).as_secs()
                        >= i64::try_from(self.config.review_infra_wait_secs())?
                {
                    self.hold_infra(wait, &current)?;
                }
                Ok(Some(Step::Wait))
            }
            Err(error) => Err(error),
            Ok(()) => {
                let written = self.ctx.store.try_update(self.id(), |task| {
                    if task.status != Status::Review || task.infra_wait.as_ref() != Some(wait) {
                        return Ok(false);
                    }
                    task.infra_wait = None;
                    Ok(true)
                })?;
                if written.is_some() && wait.held_at.is_some() {
                    self.ctx.store.log_event(
                        Some(self.id()),
                        "infra_hold_cleared",
                        json!({"run": self.run.id, "first_seen": wait.first_seen,
                            "duration_secs": self.now.duration_since(wait.first_seen).as_secs()}),
                    )?;
                }
                Ok(None)
            }
        }
    }

    fn hold_infra(&self, wait: &InfraWait, current: &InfraWait) -> Result<()> {
        let source = format!("{}/infra_hold/{}", self.run.id, wait.first_seen);
        let mut claim = owner_notice::prepare(
            self.ctx,
            self.task,
            "infra_hold",
            &source,
            self.now,
            vec![NoticeCoverage::Exact {
                condition_key: format!("{}/infra_hold/{}", self.id(), wait.first_seen),
            }],
        )?;
        claim.pending.insert(
            0,
            NoticeFact {
                fact_id: format!("{}/infra_hold", claim.batch_id),
                kind: "infra_hold".into(),
                detail: json!({"run":self.run.id, "class":wait.class, "marker":wait.marker,
                "path":wait.path, "first_seen":wait.first_seen,
                "age_secs":self.now.duration_since(wait.first_seen).as_secs(),
                "mtime_changed":wait.first_mtime != current.first_mtime}),
            },
        );
        let committed = owner_notice::commit(self.ctx, self.id(), |_, task| {
            if task.status != Status::Review
                || task.current_run() != Some(self.run)
                || task.infra_wait.as_ref() != Some(wait)
            {
                return Ok(None);
            }
            task.infra_wait
                .as_mut()
                .context("infra wait disappeared")?
                .held_at = Some(self.now);
            Ok(Some(claim))
        })?;
        if let Some(committed) = committed {
            owner_notice::notify(
                self.ctx,
                committed,
                &format!("{} integration infrastructure held", self.id()),
                &format!(
                    "{}: {} ({})",
                    wait.class.as_str(),
                    wait.marker,
                    wait.path.display()
                ),
                true,
            )?;
        }
        Ok(())
    }

    fn stage_notice(&self, producer: &str, stage: AutoStage) -> Result<NoticeClaim> {
        owner_notice::prepare(
            self.ctx,
            self.task,
            producer,
            &format!("{}/{}/{}", self.run.id, stage.as_str(), self.now),
            self.now,
            vec![NoticeCoverage::Episode {
                task: self.id().into(),
                run: Some(self.run.id.clone()),
                stage: stage.as_str().into(),
                at: self.now,
            }],
        )
    }

    pub(super) fn escalate(&self, reason: &str) -> Result<Step> {
        let state = AutopilotState {
            stage: AutoStage::Escalated,
            reason: Some(reason.into()),
            sha: worktree_head(self.run),
            at: self.now,
        };
        let claim = self.stage_notice("autopilot_escalate", AutoStage::Escalated)?;
        let committed = owner_notice::commit(self.ctx, self.id(), |store, task| {
            if !self.transition(store, task, self.stage(), state, Status::NeedsMe)? {
                return Ok(None);
            }
            Ok(Some(claim))
        })?;
        let Some(committed) = committed else {
            return Ok(Step::Wait);
        };
        cleanup(self.ctx, self.task, Cleanup::Round);
        owner_notice::notify(
            self.ctx,
            committed,
            &format!("▲ {} needs you", self.id()),
            &format!("autopilot: {reason}"),
            true,
        )?;
        self.log(
            AutoStage::Escalated,
            self.task.rounds,
            None,
            Some(reason),
            "handed to the owner",
        )?;
        Ok(Step::Moved(Change {
            task: self.id().into(),
            from: Status::Review,
            to: Status::NeedsMe,
            note: format!("autopilot escalated: {reason}"),
            stage: Some(AutoStage::Escalated),
        }))
    }

    pub(super) fn block(&self, reason: &str) -> Result<Step> {
        if let Some(state) = self
            .state()
            .filter(|state| state.stage == AutoStage::Blocked)
        {
            if state.reason.as_deref() != Some(reason) {
                self.write(
                    Some(AutoStage::Blocked),
                    AutopilotState {
                        reason: Some(reason.into()),
                        ..state.clone()
                    },
                    Status::Review,
                )?;
            }
            return Ok(Step::Wait);
        }
        let sha = self.state().and_then(|state| state.sha.clone());
        let state = AutopilotState {
            stage: AutoStage::Blocked,
            reason: Some(reason.into()),
            sha: sha.clone(),
            at: self.now,
        };
        let claim = self.stage_notice("landing_blocked", AutoStage::Blocked)?;
        let committed = owner_notice::commit(self.ctx, self.id(), |store, task| {
            if !self.transition(store, task, self.stage(), state, Status::Review)? {
                return Ok(None);
            }
            Ok(Some(claim))
        })?;
        let Some(committed) = committed else {
            return Ok(Step::Wait);
        };
        self.log(
            AutoStage::Blocked,
            self.task.rounds,
            sha.as_deref(),
            Some(reason),
            "landing blocked; retried on later syncs",
        )?;
        owner_notice::notify(
            self.ctx,
            committed,
            &format!("⏸ {} cannot land", self.id()),
            reason,
            false,
        )?;
        Ok(Step::Moved(Change {
            task: self.id().into(),
            from: Status::Review,
            to: Status::Review,
            note: "autopilot blocked: landing blocked; retried on later syncs".into(),
            stage: Some(AutoStage::Blocked),
        }))
    }

    fn transition(
        &self,
        store: &corral_core::Store,
        task: &mut Task,
        from: Option<AutoStage>,
        state: AutopilotState,
        status: Status,
    ) -> Result<bool> {
        let current = Ctx {
            store: store.clone(),
            ..self.ctx.clone()
        };
        let Some(kind) = crate::automation_status::next_action(&current, task)? else {
            return Ok(false);
        };
        if crate::operating::require(store, task, kind).is_err() {
            return Ok(false);
        }
        let Some(run) = task.current_run_mut().filter(|run| {
            run.id == self.run.id
                && run.outcome == Some(RunOutcome::Done)
                && run.autopilot.as_ref().map(|pilot| pilot.stage) == from
        }) else {
            return Ok(false);
        };
        run.autopilot = Some(state);
        task.status = status;
        Ok(true)
    }

    pub(super) fn write(
        &self,
        from: Option<AutoStage>,
        state: AutopilotState,
        status: Status,
    ) -> Result<bool> {
        let (_, written) = self.ctx.store.dispatch_update(self.id(), |store, task| {
            self.transition(store, task, from, state, status)
        })?;
        Ok(written)
    }
}
