use anyhow::Result;
use corral_core::{AutoStage, AutopilotState, Run, RunOutcome, Task};
use serde_json::json;

use crate::Ctx;

/// Installs the corral skill for the run's harness (its `flags` map name, else its key) when that harness has
/// none yet under $HOME; logs "skill_installed". A failed install is reported and does not stop the run.
pub(super) fn install_skill(ctx: &Ctx, id: &str, harness: &str) -> Result<()> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(());
    };
    let kind = ctx
        .store
        .config
        .harnesses
        .get(harness)
        .and_then(|h| h.flags.as_deref())
        .unwrap_or(harness);
    match crate::skill::ensure_skill(std::path::Path::new(&home), kind) {
        Ok(Some(path)) => ctx.store.log_event(
            Some(id),
            "skill_installed",
            json!({ "harness": harness, "path": path }),
        ),
        Ok(None) => Ok(()),
        Err(err) => {
            crate::messages::report(
                Some(id),
                "skill",
                format!("installing the corral skill for harness {harness}: {err:#}"),
            );
            Ok(())
        }
    }
}

/// The previous run and its autopilot record when the autopilot requested changes and the run was then
/// superseded (its agent pane was gone): the next run continues that change request.
pub(super) fn carried_changes(task: &Task) -> Option<(Run, AutopilotState)> {
    let prev = task
        .runs
        .last()
        .filter(|r| r.outcome == Some(RunOutcome::Superseded))?;
    let state = prev
        .autopilot
        .clone()
        .filter(|a| a.stage == AutoStage::Changes)?;
    Some((prev.clone(), state))
}
