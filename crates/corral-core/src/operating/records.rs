//! Serialize preparation of a keyed executor authorization, not the board's history.
use anyhow::{Context, Result, ensure};

use crate::store::StoreLock;
use crate::{ActionAuthorization, AuthorizedSubmission, Config, OperatingOrigin, Store};

impl Store {
    pub fn authorized_submission(&self, id: &str) -> Result<AuthorizedSubmission> {
        self.submission_record(id)?
            .with_context(|| format!("authorization {id} not found"))
    }

    pub(crate) fn save_authorized_submission(&self, record: &AuthorizedSubmission) -> Result<()> {
        self.save_submission_record(record)
    }

    pub(crate) fn validate_operating_action<T: crate::TaskRef + ?Sized>(
        &self,
        config: &Config,
        task: &T,
        authorization: &ActionAuthorization,
    ) -> Result<()> {
        ensure!(
            authorization.workspace == self.root() && authorization.action.task == *task.id(),
            "action workspace/task identity changed"
        );
        ensure!(
            authorization.member == self.operating_member(config, task)?,
            "action task contract or project identity changed"
        );
        ensure!(
            authorization.policy_revision == self.policy_revision(config)?,
            "action policy revision changed"
        );
        match &authorization.origin {
            OperatingOrigin::Automatic {
                authorization: grant,
                revision,
            } => {
                let effective = self.resolve_operating_policy(config)?;
                ensure!(
                    effective.authorization.as_ref() == Some(grant)
                        && *revision == config.automation.revision,
                    "automatic grant was revoked"
                );
                ensure!(
                    effective.allows(task.id(), authorization.action.kind),
                    "automatic action is not authorized: {:?}",
                    effective.tasks.get(task.id()).map(|task| &task.refusals)
                );
            }
            OperatingOrigin::ExplicitOwner {
                approval,
                confirmed_by,
                ..
            } => {
                ensure!(
                    !approval.trim().is_empty() && !confirmed_by.trim().is_empty(),
                    "explicit owner approval is missing"
                );
            }
        }
        if let Some(candidate) = &authorization.action.candidate {
            ensure!(
                candidate.task == *task.id() && candidate.run == authorization.action.run,
                "candidate does not match action task/run"
            );
        }
        Ok(())
    }

    /// Hold through reading, preparing and publishing one executor claim.
    ///
    /// The lease prevents competing owner approvals from reaching authorization
    /// preparation together; record version preconditions still protect publication.
    pub fn operating_claim_lock(&self) -> Result<StoreLock> {
        self.lock("operating-claims")
    }
}
