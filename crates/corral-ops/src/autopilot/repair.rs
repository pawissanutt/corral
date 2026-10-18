use super::*;
use corral_git::MergePreflight;

impl Pilot<'_> {
    /// Re-evaluate exact SHAs before spending on gates/review, and again if main moves.
    pub(super) fn preflight(&self) -> Result<Option<Step>> {
        if !self.run.worktree.exists() || !corral_git::is_clean(&self.run.worktree)? {
            return Ok(None);
        }
        let mut key = match RepairKey::new(self.task, self.run, "preflight_clean") {
            Ok(key) => key,
            Err(error) => {
                return self
                    .escalate(&format!(
                        "integration preflight infrastructure error: {error:#}"
                    ))
                    .map(Some);
            }
        };
        match corral_git::merge_preflight(&self.run.worktree, &key.base, &key.head) {
            Ok(MergePreflight::Clean { .. }) => {
                let packet = IntegrationPacket::build(
                    self.ctx,
                    self.task,
                    self.run,
                    &self.policy,
                    key.clone(),
                    Vec::new(),
                    None,
                )?;
                key.check(&self.ctx.store.get(self.id())?)?;
                packet.attach(self.ctx)?;
                Ok(None)
            }
            Ok(MergePreflight::Conflict {
                paths, messages, ..
            }) => {
                key.reason = "merge_conflict".into();
                self.integration_changes(key, paths, Some(format!("{messages:#?}")))
                    .map(Some)
            }
            Err(error) => self
                .escalate(&format!(
                    "integration preflight infrastructure error: {error:#}"
                ))
                .map(Some),
        }
    }

    pub(super) fn integration_changes(
        &self,
        key: RepairKey,
        paths: Vec<String>,
        details: Option<String>,
    ) -> Result<Step> {
        let packet = IntegrationPacket::build(
            self.ctx,
            self.task,
            self.run,
            &self.policy,
            key.clone(),
            paths,
            details,
        )?;
        key.check(&self.ctx.store.get(self.id())?)?;
        let artifact = packet.attach(self.ctx)?;
        let summary = if key.reason == "base_moved" {
            format!(
                "rebase onto `{}`: merge = ff requires the current base",
                self.run.base
            )
        } else {
            format!("rebase onto `{}` and resolve conflicts", self.run.base)
        };
        let details = format!(
            "{}: {}.\n\nIntegration packet: {}",
            key.reason,
            packet.compact(),
            self.ctx.store.dir().join(&artifact.path).display()
        );
        self.send_request(&summary, &details, &key, Some(&artifact.path))
    }

    pub(super) fn reviewer_changes(&self, review: &ReviewRecord) -> Result<Step> {
        let notes = review.notes.as_deref().unwrap_or("").trim();
        let summary = format!(
            "the reviewer ({}) requested changes: {}",
            review.reviewer,
            first_line(notes)
        );
        let details = format!("Reviewer notes:\n\n{notes}");
        self.request_changes(&summary, &details)
    }

    pub(super) fn request_changes(&self, summary: &str, details: &str) -> Result<Step> {
        let key = RepairKey::new(self.task, self.run, summary)?;
        self.send_request(summary, details, &key, None)
    }

    pub(super) fn rebase(&self) -> Result<Step> {
        let mut key = RepairKey::new(self.task, self.run, "rebase_conflict")?;
        if self.policy.merge == MergeMode::Ff {
            key.reason = "base_moved".into();
            return self.integration_changes(key, Vec::new(), None);
        }
        // The real replay is authoritative even when merge-tree was clean.
        let rebased = self
            .ctx
            .store
            .dispatch_update(self.id(), |store, current| {
                crate::operating::require(store, current, OperatingActionKind::Repair)?;
                key.check(current)?;
                crate::verdict_carry::rebase(self.ctx, current, self.run, &key.base)
            })
            .map(|(_, outcome)| outcome);
        match rebased {
            Ok(RebaseOutcome::Conflict { message, paths }) => {
                self.integration_changes(key, paths, Some(message))
            }
            Ok(RebaseOutcome::Rebased) => {
                let head = corral_git::rev_sha(&self.run.worktree, "HEAD")?;
                let note = format!(
                    "rebased onto {} at {}; the gate result is stale",
                    self.run.base,
                    short(&head)
                );
                self.moved(AutoStage::Gating, Some(head), None, &note)
            }
            Err(error) => self.escalate(&format!("rebase infrastructure error: {error:#}")),
        }
    }
}
