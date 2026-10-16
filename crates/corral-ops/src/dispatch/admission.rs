use anyhow::{Result, bail};
use corral_core::{DispatchPlan, Store, Task};
use jiff::Timestamp;
use serde_json::{Value, json};

use super::DispatchOpts;
use crate::Ctx;

pub(crate) enum Admission {
    Manual,
    Queued { rank: usize, at: Timestamp },
    Automatic { inputs: Value, at: Timestamp },
}

impl Admission {
    pub(crate) fn plan_requested(&self, task: &Task, opts: &DispatchOpts) -> bool {
        opts.plan
            || matches!(self, Self::Queued { .. }) && crate::queue::queued_plan_requested(task)
    }

    /// Recheck admission against the store inside the atomic dispatch claim.
    pub(crate) fn reason(
        &self,
        ctx: &Ctx,
        store: &Store,
        task: &Task,
        opts: &DispatchOpts,
    ) -> Result<Option<String>> {
        if !self.plan_requested(task, opts)
            && let Some(reason) = crate::queue::pending_plan_reason(task)?
        {
            return Ok(Some(reason));
        }
        let mut reason = if matches!(self, Self::Queued { .. }) {
            crate::queue::ready_reason(store, task)
        } else {
            None
        };
        if reason.is_none() {
            reason = crate::queue::capacity_reason(ctx, store, task, opts.force_resources)?;
        }
        if reason.is_none() && !matches!(self, Self::Manual) {
            if task.dispatch_hold {
                bail!("{} was dequeued; automatic dispatch is held", task.id);
            }
            if let Self::Automatic { inputs, .. } = self {
                let authorized = match inputs["source"].as_str() {
                    Some("backlog" | "idea") => crate::queue::authorized_ready(store, task),
                    _ => false,
                };
                if !authorized {
                    bail!(
                        "{} is no longer authorized by automatic dispatch policy",
                        task.id
                    );
                }
            }
            reason = crate::dispatch_policy::automatic_capacity_reason(store, task)?;
        }
        Ok(reason)
    }

    /// Record automatic policy inputs as part of the successful atomic claim.
    pub(crate) fn audit(
        &self,
        ctx: &Ctx,
        store: &Store,
        task: &Task,
        plan: &DispatchPlan,
        run_id: &str,
    ) -> Result<()> {
        let mut detail = match self {
            Self::Manual => return Ok(()),
            Self::Queued { rank, at } => json!({
                "source":"ready", "reason":"authorized Ready queue admission",
                "rank":rank, "at":at, "priority":task.priority, "created":task.created,
                "eligibility":{"status":"ready","depends_on_done":task.depends_on},
            }),
            Self::Automatic { inputs, at } => {
                let mut detail = inputs.clone();
                detail["at"] = json!(at);
                detail
            }
        };
        detail["action"] = json!("dispatch");
        detail["run"] = json!(run_id);
        detail["slots"] = json!(crate::dispatch_policy::free_slots(&Ctx {
            store: store.clone(),
            ..ctx.clone()
        })?);
        detail["review_headroom"] = json!(store.config.limits.review);
        detail["route"] = json!(plan.route);
        store.log_event(Some(&task.id), "dispatch_policy", detail)
    }
}
