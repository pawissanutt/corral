//! Bounded inputs for the existing Task-typed contract and plan-policy checks.
use anyhow::{Context, Result, ensure};

use super::{ScopedTask, digest};
use crate::backend::{PageReq, RowQuery, TaskPart};
use crate::work::{self, Counter, Subject};
use crate::{Config, Status, Store, Task, TaskHead};

impl Store {
    /// Fingerprint a head and its direct execution Plan without hydrating either task.
    ///
    /// # Errors
    /// Returns project-identity and Plan-membership errors.
    pub fn operating_member_head(&self, config: &Config, task: &TaskHead) -> Result<ScopedTask> {
        let project = task.project.as_ref().context("task has no project")?;
        let (repository, repository_identity) = self.project_identity(config, project)?;
        let contract = task.operating_contract_revision();
        let plan = if task.tracker {
            None
        } else {
            self.execution_plan_head(task)?
        };
        let contract_revision = match plan {
            Some(plan) => digest(&(contract, plan.operating_contract_revision()))?,
            None => contract.to_owned(),
        };
        Ok(ScopedTask {
            project: project.clone(),
            repository_identity,
            repository,
            contract_revision,
        })
    }

    pub(super) fn operating_task(&self, head: &TaskHead) -> Result<Task> {
        let mut task = head.to_shell_task()?;
        if let Some(run) = &head.current_run
            && run.facts.pending_decision.is_some()
            && let Some(current) = task.runs.last_mut()
        {
            current.obligations.decisions = head.open_obligations(&run.id)?.decisions;
        }
        Ok(task)
    }

    /// Approval checks inspect the current review and, for owner acceptance, its immediate
    /// predecessor, plus the exact planner named by the current plan. No review history walk.
    pub(super) fn operating_approval_task(&self, head: &TaskHead) -> Result<Task> {
        let mut task = self.operating_task(head)?;
        if let Some(plan) = &head.last_plan {
            if plan
                .approval
                .as_ref()
                .is_some_and(|approval| approval.by == "owner")
                && plan.facts.children >= 2
                && let Some(previous) =
                    head.plan_review_at(&plan.id, usize::try_from(plan.facts.children - 1)?)?
                && let Some(record) = task.plans.last_mut()
            {
                record
                    .reviews
                    .insert(0, serde_json::from_value(serde_json::to_value(previous)?)?);
            }
            if let Some(id) = &plan.run
                && !task.plan_runs.iter().any(|run| &run.id == id)
            {
                let rows = self.view()?.rows(
                    head.num,
                    &RowQuery::part(TaskPart::PlanRun).with_index(format!("id:{id}")),
                    PageReq::first(1),
                )?;
                if let Some(row) = rows.items.into_iter().next() {
                    work::decoded(
                        Counter::HistoryRowDecodes,
                        u64::try_from(serde_json::to_vec(&row.doc)?.len())?,
                        Subject::Plain,
                    );
                    // The row is already a projection; inbox history is stored separately.
                    let mut doc = row.doc;
                    if let Some(object) = doc.as_object_mut() {
                        object.remove(crate::store::codec::FACTS);
                    }
                    task.plan_runs.insert(0, serde_json::from_value(doc)?);
                }
            }
        }
        Ok(task)
    }

    pub(super) fn operating_owner_resolution(
        &self,
        config: &Config,
        head: &TaskHead,
    ) -> Result<()> {
        let task = self.operating_approval_task(head)?;
        if let Some(plan) = task.plans.last() {
            return crate::plan_policy::validate_approval(self, config, &task, plan);
        }
        let (parent_id, revision) = task
            .plan
            .as_deref()
            .and_then(|link| link.split_once('@'))
            .context("owner decisions have no accepted plan")?;
        let parent_head = self.head(parent_id)?;
        let parent = self.operating_approval_task(&parent_head)?;
        ensure!(
            parent.status == Status::Done && parent.children.contains(&task.id),
            "owner resolution is not attached to a published child"
        );
        let plan = parent
            .plans
            .last()
            .filter(|plan| plan.id == revision)
            .context("owner resolution references a missing or replaced parent plan")?;
        crate::plan_policy::validate_approval(self, config, &parent, plan)?;
        ensure!(
            plan.approval
                .as_ref()
                .is_some_and(|approval| approval.by == "owner"),
            "inherited owner decisions require owner acceptance"
        );
        ensure!(
            parent_head
                .current_run
                .as_ref()
                .is_none_or(|run| run.facts.pending_decision.is_none()),
            "parent has a pending owner decision"
        );
        let notes = plan
            .reviews
            .last()
            .and_then(|review| review.notes.as_deref())
            .unwrap_or("");
        ensure!(
            crate::plan_policy::decisions_resolved(&task, plan, notes),
            "inherited owner decisions require explicit resolutions"
        );
        Ok(())
    }
}
