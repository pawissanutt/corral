//! Scope amendments use the ordinary task file and board lock, with explicit revision history.
use anyhow::{Context, Result, ensure};

use super::Store;
use crate::{ApprovedScope, ApprovedScopeRevision, Task};

pub(super) enum ScopeWrite {
    Preserve,
    Amend,
    /// Only `member_gates` may change.
    MemberGate,
}

impl ApprovedScope {
    /// Validate aggregate criteria and any exact, current approved design link.
    ///
    /// # Errors
    /// Rejects blank or duplicate criteria, stale or unapproved design, and mismatched scope.
    pub fn validate(&self, store: &Store, project: Option<&str>) -> Result<()> {
        ensure!(!self.goal.trim().is_empty(), "Plan goal must not be blank");
        ensure!(
            !self.acceptance.is_empty(),
            "Plan acceptance must not be empty"
        );
        let mut criteria = std::collections::BTreeSet::new();
        for acceptance in &self.acceptance {
            ensure!(
                !acceptance.scenario.trim().is_empty() && !acceptance.expected.trim().is_empty(),
                "blank Plan acceptance"
            );
            if let Some(id) = &acceptance.criterion {
                ensure!(
                    !id.trim().is_empty() && criteria.insert(id),
                    "blank or duplicate Plan criterion"
                );
            }
        }
        if let Some(link) = &self.approved_design {
            let (id, revision) = link
                .split_once('@')
                .context("approved design must be <task>@<revision>")?;
            let task = store.get(id)?;
            let plan = task
                .plans
                .last()
                .filter(|plan| plan.id == revision && plan.approval.is_some())
                .context("stale approved design revision; link the current accepted plan")?;
            crate::plan_policy::validate_approval(store, &store.config, &task, plan)?;
            ensure!(
                task.project.as_deref() == project
                    && self.goal == plan.document.goal
                    && self.acceptance == plan.document.acceptance
                    && self.delivery_target
                        == plan.document.delivery_target.clone().unwrap_or_default(),
                "approved Plan scope differs from the linked design revision"
            );
        }
        Ok(())
    }
}

impl Task {
    /// Generic task editing cannot replace approved scope, its revision history or the owner's
    /// member gates.
    ///
    /// # Errors
    /// Rejects any scope-field or member-gate change; callers must use the amendment operation
    /// instead.
    pub fn check_scope_unchanged(&self, proposed: &Task) -> Result<()> {
        self.check_approved_scope_unchanged(proposed)?;
        ensure!(
            self.member_gates == proposed.member_gates,
            "{}: member gates require a revision-guarded amendment; use corral plan member-gate <member> --file <gate.yaml> --expected-revision <n> --reason <text>",
            self.id
        );
        ensure!(
            self.membership_revision == proposed.membership_revision,
            "{}: the membership revision changes only through corral plan move-member",
            self.id
        );
        Ok(())
    }

    pub(super) fn check_approved_scope_unchanged(&self, proposed: &Task) -> Result<()> {
        ensure!(
            self.approved_scope == proposed.approved_scope
                && self.approved_scope_revision == proposed.approved_scope_revision
                && self.approved_scope_history == proposed.approved_scope_history,
            "{}: approved Plan scope requires a revision-guarded amendment; use corral import <file> --amend {} --expected-revision {}",
            self.id,
            self.id,
            self.approved_scope_revision
        );
        Ok(())
    }
}

impl Store {
    /// Amend only a tracker's scope, retaining its identity, members and prior approvals.
    /// `validate` runs against fresh configuration under the board/task locks and must not write.
    ///
    /// # Errors
    /// Rejects non-trackers, stale or exhausted revisions, invalid scope, or persistence failures.
    pub fn amend_plan_scope(
        &self,
        id: &str,
        expected_revision: u64,
        scope: ApprovedScope,
        validate: impl FnOnce(&Store, &Task, &ApprovedScope) -> Result<()>,
    ) -> Result<Task> {
        let _board = self.lock("dispatch")?;
        let id = Self::normalize_id(id)?;
        let _task = self.lock(&id)?;
        let fresh = self.reopen_unobserved()?;
        let mut task = fresh.get(&id)?;
        ensure!(task.tracker, "{} is not a tracker Plan", task.id);
        ensure!(
            task.approved_scope_revision == expected_revision,
            "stale Plan scope amendment: expected revision {expected_revision}, current revision {}",
            task.approved_scope_revision
        );
        let revision = expected_revision
            .checked_add(1)
            .context("Plan scope revision exhausted")?;
        for child in &task.children {
            ensure!(
                fresh.get(child)?.project == task.project,
                "an approved Plan must belong to one project"
            );
        }
        scope.validate(&fresh, task.project.as_deref())?;
        validate(&fresh, &task, &scope)?;
        task.approved_scope_history.push(ApprovedScopeRevision {
            revision: expected_revision,
            approved_scope: task.approved_scope.clone(),
            superseded_at: jiff::Timestamp::now(),
        });
        task.approved_scope = Some(scope);
        task.approved_scope_revision = revision;
        let saved = fresh.save_task(&task, ScopeWrite::Amend)?;
        fresh.log_event(
            Some(&saved.id),
            "plan_scope_amended",
            serde_json::json!({
                "from_revision": expected_revision, "revision": revision,
                "approved_scope": saved.approved_scope,
            }),
        )?;
        Ok(saved)
    }
}
