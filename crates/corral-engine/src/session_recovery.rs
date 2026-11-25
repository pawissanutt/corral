use crate::{Engine, Op, Outcome, Role, Run, Status, Store, Task, refuse};
use anyhow::Result;

pub(crate) const LOST_WORKER: &str = "agent session ended without corral done";
pub(crate) const LOST_REVIEWER: &str = "reviewer session ended without a verdict";

impl Store {
    /// Only advertise an ended, latest run matching the block created by reconciliation.
    pub fn recovery_run(&self, task: &Task) -> Result<Option<Run>> {
        let Some(role) = blocked_role(task, false) else {
            return Ok(None);
        };
        Ok(self
            .latest_run(&task.id, role)?
            .filter(|r| r.ended.is_some() && r.outcome == Some(Outcome::Failed)))
    }

    /// Called inside the launch transaction, before any worktree or process effects.
    pub(crate) fn require_relaunch_target(
        &self,
        task: &Task,
        role: Role,
        expected: &str,
    ) -> Result<()> {
        if blocked_role(task, true) == Some(role)
            && self.latest_run(&task.id, role)?.is_some_and(|run| {
                run.id == expected
                    && run.ended.is_some()
                    && matches!(run.outcome, Some(Outcome::Failed | Outcome::Stopped))
            })
        {
            return Ok(());
        }
        Err(refuse(
            "this is no longer the stopped session awaiting relaunch",
            vec![],
        ))
    }
}

fn blocked_role(task: &Task, stopped: bool) -> Option<Role> {
    if task.status != Status::Blocked {
        return None;
    }
    let block = task.blocked.as_ref()?;
    let next = block.next.first()?;
    if next.force {
        return None;
    }
    match (&next.op, block.reason.as_str()) {
        (
            Op::Retry {
                id, fresh: false, ..
            },
            reason,
        ) if id == &task.id
            && (reason == LOST_WORKER || stopped && reason == "stopped by owner") =>
        {
            Some(Role::Worker)
        }
        (Op::Review { id, .. }, reason)
            if id == &task.id
                && (reason == LOST_REVIEWER || stopped && reason == "stopped by owner") =>
        {
            Some(Role::Reviewer)
        }
        _ => None,
    }
}

impl Engine {
    /// A session-scoped button must not supersede the replacement on a repeated click.
    pub fn relaunch_run(&self, run: &Run) -> Result<Run> {
        if run.ended.is_none() {
            self.reconcile(run)?;
        }
        match run.role {
            Role::Worker => {
                self.retry_selected(&run.task, Some(&run.profile), false, None, Some(&run.id))
            }
            Role::Reviewer => {
                self.review_selected(&run.task, Some(&run.profile), self.actor, Some(&run.id))
            }
        }
    }
}
