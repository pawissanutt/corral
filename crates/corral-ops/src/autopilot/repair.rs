use super::*;
use anyhow::Context;
use corral_git::MergePreflight;

pub(super) enum RepairBudget {
    Legacy,
    Coding {
        switch: Option<corral_core::ProfileSwitch>,
        max: u32,
    },
    Stop(Step),
}

impl Pilot<'_> {
    pub(super) fn resume_repair_cycle(&self) -> Result<Option<Step>> {
        let Some(cycle) = crate::repair_cycle::pending(&self.ctx.store, self.head)? else {
            return Ok(None);
        };
        let key = RepairKey::new(self.head, self.run, &cycle.key.reason)?;
        if key.head != cycle.key.head {
            return self.escalate("transferred repair candidate changed; reconcile preserved work before continuation").map(Some);
        }
        if self.require(OperatingActionKind::Repair).is_err() {
            return Ok(Some(Step::Wait));
        }
        self.send_request(&cycle.summary, &cycle.details, &key, None)
            .map(Some)
    }

    pub(super) fn repair_budget(&self, digest: &str, summary: &str) -> Result<RepairBudget> {
        let Some(ledger) =
            crate::repair_runtime::observations(self.ctx, self.head, Some((digest, summary)))?
        else {
            return Ok(RepairBudget::Legacy);
        };
        match crate::repair_runtime::transition(self.ctx, self.head, &ledger, false)? {
            corral_core::RepairTransition::Current { profile }
            | corral_core::RepairTransition::Stronger { profile } => {
                let switch = (profile != self.run.agent).then(|| corral_core::ProfileSwitch {
                    from: self.run.agent.clone(), to: profile,
                    reason: "host-enforced staged repair allowance".into(),
                });
                let corral_core::RepairPolicy::V1 { overall_limit, .. } = ledger.active()?.history.policy();
                Ok(RepairBudget::Coding { switch, max: *overall_limit })
            }
            corral_core::RepairTransition::Diagnose => self.moved(AutoStage::Escalated,
                run_head_sha(self.run), Some("Bounded PM diagnosis ready; no coding allowance remains"),
                "PM must inspect repair history and propose a bounded recovery; no extra coding retries")
                .map(RepairBudget::Stop),
            corral_core::RepairTransition::Owner { reason } => self.escalate(&reason).map(RepairBudget::Stop),
        }
    }

    /// Re-evaluate exact SHAs before spending on gates/review, and again if main moves.
    pub(super) fn preflight(&self) -> Result<Option<Step>> {
        crate::plan_target::validate_run(&self.ctx.store, self.head, self.run)?;
        if !self.run.worktree.exists() || !corral_git::is_clean(&self.run.worktree)? {
            return Ok(None);
        }
        let mut key = match RepairKey::new(self.head, self.run, "preflight_clean") {
            Ok(key) => key,
            Err(error) => {
                return self
                    .escalate_error("integration preflight infrastructure error", error)
                    .map(Some);
            }
        };
        match corral_git::merge_preflight(&self.run.worktree, &key.base, &key.head) {
            Ok(MergePreflight::Clean { .. }) => {
                if !corral_git::is_ancestor(&self.run.worktree, &key.base, &key.head)?
                    && !matches!(self.stage(), Some(AutoStage::Landing | AutoStage::Blocked))
                    && self
                        .run
                        .review
                        .as_ref()
                        .is_none_or(|review| review.verdict.is_some())
                {
                    // Do not rewrite a candidate while its gate still runs. Reload
                    // after collection so the rebase precedes any replacement claim.
                    if self.run.gate.as_ref().is_some_and(GateRecord::holds_slot) {
                        return Ok(Some(if gate_collect(self.ctx, self.id())?.is_some() {
                            Step::Acted
                        } else {
                            Step::Wait
                        }));
                    }
                    return self.rebase().map(Some);
                }
                let packet = IntegrationPacket::build(
                    self.ctx,
                    self.head,
                    self.run,
                    &self.policy,
                    key.clone(),
                    Vec::new(),
                    None,
                )?;
                key.check(&self.ctx.store.head(self.id())?)?;
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
                .escalate_error("integration preflight infrastructure error", error)
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
            self.head,
            self.run,
            &self.policy,
            key.clone(),
            paths,
            details,
        )?;
        key.check(&self.ctx.store.head(self.id())?)?;
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
            crate::artifact_path(self.ctx, &artifact)?.display()
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
        let key = RepairKey::new(self.head, self.run, summary)?;
        self.send_request(summary, details, &key, None)
    }

    pub(super) fn rebase(&self) -> Result<Step> {
        let mut key = RepairKey::new(self.head, self.run, "rebase_conflict")?;
        // The real replay is authoritative even when merge-tree was clean.
        let rebased = self
            .ctx
            .store
            .dispatch_update_head(self.id(), |store, current| {
                key.check(&*current)?;
                // A clean integration refresh is verification preparation, never a code
                // repair charge. Actual conflicts below still route to the current driver.
                let run = current.current_run_head()?.context("missing current run")?;
                let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
                let approved = match run.review.as_ref() {
                    Some(review) if review.verdict == Some(Verdict::Approve) => {
                        review.sha == head
                            || crate::verdict_carry::clean_replay(
                                self.ctx,
                                &*current,
                                &run,
                                &review.sha,
                                &head,
                            )?
                    }
                    _ => false,
                };
                // Unapproved ff integration still belongs to the worker. A verified clean
                // replay of an approved candidate may prepare its new gate automatically.
                if self.policy.merge == corral_core::MergeMode::Ff && !approved {
                    return Ok(None);
                }
                let action = OperatingActionKind::Gate;
                let _action = corral_core::work::action(self.id(), action);
                crate::operating::require(store, &*current, action)?;
                crate::verdict_carry::rebase(self.ctx, &*current, &run, &key.base).map(Some)
            })
            .map(|(_, outcome)| outcome);
        match rebased {
            Ok(None) => {
                key.reason = "base_moved".into();
                self.integration_changes(key, Vec::new(), None)
            }
            Ok(Some(RebaseOutcome::Conflict { message, paths })) => {
                self.integration_changes(key, paths, Some(message))
            }
            Ok(Some(RebaseOutcome::Rebased)) => {
                let head = corral_git::rev_sha(&self.run.worktree, "HEAD")?;
                let note = format!(
                    "rebased onto {} at {}; gate required on the integrated candidate",
                    self.run.base,
                    short(&head)
                );
                self.moved(AutoStage::Gating, Some(head), None, &note)
            }
            Err(error) => self.escalate_error("rebase infrastructure error", error),
        }
    }
}
