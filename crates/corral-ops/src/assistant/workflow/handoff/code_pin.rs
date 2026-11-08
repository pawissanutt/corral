//! Service-computed source identity for a continuation choice. Commits and dirty layers
//! changing after preview invalidate the choice; observing a commit never submits it.
use super::{git, lifecycle};
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::TaskRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodePin {
    pub run: Option<String>,
    pub head: String,
    pub state_sha256: String,
}

pub(super) fn current(ctx: &Ctx, task: &impl TaskRef) -> Result<Option<CodePin>> {
    let journal = lifecycle::for_task(&ctx.store, task)?.context("transfer missing")?;
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    let archived = (lifecycle::plan_only(transfer.goal) || !transfer.adoption_proven)
        && transfer.seed_run.is_none();
    let pending = super::adopt::pending_seed(ctx, task)?;
    let (run, mut state) = if !archived
        && !pending
        && let Some(run) = task.current_run_head()?
    {
        (
            Some(run.id.clone()),
            git::scan(&run.worktree, &run.base_sha, &run.branch)?.state,
        )
    } else {
        let (_, package, _) = super::read_package(&ctx.store, &journal.id)?;
        let Some(snapshot) = package.snapshot else {
            return Ok(None);
        };
        let shell = task.shell_task()?;
        let (path, branch) = super::adopt::seed_path(ctx, &shell)?;
        let state = if !archived && path.exists() {
            git::scan(&path, &snapshot.state.base, &branch)?.state
        } else {
            ensure!(
                archived || !transfer.adoption_proven,
                "handoff_missing: adopted code source disappeared; no empty-source fallback"
            );
            snapshot.state
        };
        (None, state)
    };
    // Adoption changes branch aliases, never the chosen HEAD/index/work layers.
    state.branch.clear();
    state.branch_tip = state.head.clone();
    state.detached = false;
    Ok(Some(CodePin {
        run,
        head: state.head.clone(),
        state_sha256: crate::verification::digest(&serde_json::to_vec(&state)?),
    }))
}
