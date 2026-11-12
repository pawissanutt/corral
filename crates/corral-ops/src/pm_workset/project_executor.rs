//! `pm act` requests a daemon pass, or runs the existing project-scoped sync when no
//! daemon owns the loop. Existing action identity and authorization apply in either case.
#[cfg(test)]
#[path = "project_executor_tests.rs"]
mod tests;

use anyhow::{Context, Result};

use super::{PmActionOutcome, PmActionResult, PmExecutable, pm_workset, task_identity};
use crate::Ctx;
use crate::pm_next::PmActionKind;
use crate::sync::SyncScope;

/// What the daemon actually acknowledged, or why no acknowledgement was obtained.
/// An error does not claim the daemon failed to receive or execute the nudge.
#[derive(Debug, serde::Serialize)]
pub struct PmDaemonPass {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack: Option<corral_protocol::NudgeAck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `pm act`: recompute the revision-bound action, run the caller's `check` on it before any
/// effect (the native PM action checks its scope there), then execute it. An ID absent from
/// the fresh workset, including any owner-required option, is stale and runs nothing.
pub(crate) fn act(
    ctx: &Ctx,
    id: &str,
    check: impl FnOnce(&PmExecutable) -> Result<()>,
) -> Result<PmActionResult> {
    let current = pm_workset(ctx)?;
    let Some(selected) = current.actions.iter().find(|action| action.id == id) else {
        return Ok(PmActionResult {
            outcome: PmActionOutcome::Stale,
            workset: current,
            diagnosis: None,
            daemon_pass: None,
        });
    };
    check(selected)?;
    let task = selected
        .action
        .task
        .as_deref()
        .context("automatic action has no task")?;
    if selected.action.kind == PmActionKind::Diagnosis {
        let diagnosis = crate::repair_runtime::repair_diagnose(ctx, task)?;
        return Ok(PmActionResult {
            outcome: PmActionOutcome::Progressed,
            workset: pm_workset(ctx)?,
            diagnosis: Some(diagnosis),
            daemon_pass: None,
        });
    }
    execute(ctx, task)
}

/// With a daemon, request a fresh pass and wait at most 20 seconds for its completion
/// acknowledgement. Otherwise retain the existing project-scoped executor. Progress
/// remains the selected task's identity changing across the operation.
fn execute(ctx: &Ctx, task: &str) -> Result<PmActionResult> {
    let scope = SyncScope::Project(ctx.store.head(task)?.project.clone());
    let before = task_identity(&ctx.store.get(task)?)?;
    let daemon_pass = if ctx.store.loop_owned_elsewhere()? {
        Some(request_daemon_pass(ctx))
    } else {
        crate::sync::sync_in(ctx, &scope)?;
        None
    };
    let after = task_identity(&ctx.store.get(task)?)?;
    Ok(PmActionResult {
        outcome: if before == after {
            PmActionOutcome::Waiting
        } else {
            PmActionOutcome::Progressed
        },
        workset: pm_workset(ctx)?,
        diagnosis: None,
        daemon_pass,
    })
}

fn request_daemon_pass(ctx: &Ctx) -> PmDaemonPass {
    let result = (|| {
        let mut client = crate::daemon_client::Client::connect(&ctx.store)?
            .context("loop owner is not reachable over its daemon socket")?;
        client.nudge("pm_act", Some(std::time::Duration::from_secs(20)))
    })();
    match result {
        Ok(ack) => PmDaemonPass {
            ack: Some(ack),
            error: None,
        },
        Err(error) => PmDaemonPass {
            ack: None,
            error: Some(format!("{error:#}")),
        },
    }
}
