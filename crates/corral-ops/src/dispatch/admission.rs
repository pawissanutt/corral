use anyhow::{Result, bail};
use corral_core::{DispatchPlan, OperatingActionKind, Store, TaskRef};
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
    pub(crate) fn origin<T: TaskRef + ?Sized>(
        &self,
        store: &Store,
        task: &T,
    ) -> Result<crate::operating::Origin> {
        match self {
            Self::Manual => Ok(crate::operating::Origin::Owner),
            Self::Queued { .. } => crate::operating::queued_origin(store, task),
            Self::Automatic { .. } => Ok(crate::operating::Origin::Automatic),
        }
    }

    pub(crate) fn plan_requested<T: TaskRef + ?Sized>(
        &self,
        task: &T,
        opts: &DispatchOpts,
    ) -> bool {
        opts.plan
            || matches!(self, Self::Queued { .. }) && crate::queue::queued_plan_requested(task)
    }

    /// Recheck admission against the store inside the atomic dispatch claim.
    pub(crate) fn reason<T: TaskRef + ?Sized>(
        &self,
        ctx: &Ctx,
        store: &Store,
        task: &T,
        opts: &DispatchOpts,
    ) -> Result<Option<String>> {
        corral_core::Park::require_unparked_ref(task)?;
        let origin = self.origin(store, task)?;
        crate::operating::require_origin(
            store,
            task,
            if crate::plan_dispatch::needed(ctx, task, opts) {
                OperatingActionKind::Plan
            } else {
                OperatingActionKind::Dispatch
            },
            origin,
        )?;
        // The plan, capacity and slot checks read head fields and current containers only.
        let shell = task.shell_task()?;
        if !self.plan_requested(task, opts)
            && let Some(reason) = crate::queue::pending_plan_reason(store, &shell)?
        {
            return Ok(Some(reason));
        }
        let mut reason = if matches!(self, Self::Queued { .. }) {
            crate::queue::ready_reason(store, task)
        } else {
            None
        };
        if reason.is_none()
            && (store.config.limits.capacity.is_some()
                || !crate::plan_dispatch::needed(ctx, task, opts))
        {
            reason = crate::wip::reason(store, &shell)?;
        }
        if reason.is_none() {
            reason = crate::queue::capacity_reason(ctx, store, &shell, opts.force_resources)?;
        }
        if reason.is_none() && !matches!(self, Self::Manual) {
            if *task.dispatch_hold() {
                bail!("{} was dequeued; automatic dispatch is held", task.id());
            }
            if let Self::Automatic { inputs, .. } = self {
                let authorized = match inputs["source"].as_str() {
                    Some("backlog" | "idea" | "repair" | "availability") => {
                        crate::queue::authorized_ready(store, task)
                    }
                    _ => false,
                };
                if !authorized {
                    bail!(
                        "{} is no longer authorized by automatic dispatch policy",
                        task.id()
                    );
                }
            }
            if origin == crate::operating::Origin::Automatic {
                reason = crate::dispatch_policy::automatic_capacity_reason(store, &shell)?;
            } else {
                reason = crate::dispatch_policy::queued_capacity_reason(store, &shell)?;
            }
        }
        Ok(reason)
    }

    /// Record automatic policy inputs as part of the successful atomic claim.
    pub(crate) fn audit<T: TaskRef + ?Sized>(
        &self,
        ctx: &Ctx,
        store: &Store,
        task: &T,
        plan: &DispatchPlan,
        run_id: &str,
    ) -> Result<()> {
        let mut detail = match self {
            Self::Manual => return Ok(()),
            Self::Queued { rank, at } => json!({
                "source":"ready", "reason":"authorized Ready queue admission",
                "rank":rank, "at":at, "priority":task.priority(), "created":task.created(),
                "eligibility":{"status":"ready","depends_on_done":task.depends_on()},
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
        if let Some(capacity) = &store.config.limits.capacity {
            detail["review_headroom"] = json!(0);
            detail["capacity"] = json!(capacity);
            detail["lane"] = json!(task.capacity_lane());
        } else {
            detail["review_headroom"] = json!(store.config.limits.review);
        }
        detail["route"] = json!(plan.route);
        store.log_event(Some(task.id()), "dispatch_policy", detail)
    }
}
