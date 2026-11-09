//! Launch and bootstrap seam: the pane command of a corral-pi worker run.
//! Owner: T-252 child host-bridge (writes the bootstrap and renders `{corral} agent launch
//! {delivery_args}`).
use anyhow::{Context, Result};
use corral_core::{ResolvedProfile, Run, Task};
use std::path::Path;

use super::consulted;
use crate::Ctx;
use crate::agent::BindTarget;
use crate::agent::launch::{HostSpec, prepare};

/// The Pi provider a host bound to `vendor` (else the model's inferred vendor) runs on.
pub fn host_provider(vendor: Option<&str>, model: &str) -> Option<String> {
    if vendor == Some("llama.cpp") {
        return Some("llama.cpp".to_owned());
    }
    let vendor = vendor
        .map(str::to_owned)
        .or_else(|| corral_core::infer_vendor(model))?;
    crate::agent::credentials::provider_for_vendor(&vendor).map(|p| p.id.to_owned())
}

pub fn worker_command(ctx: &Ctx, task: &Task, run: &Run) -> Result<String> {
    consulted("launch");
    let pane = &run.herdr.as_ref().context("launch pane missing")?.pane_id;
    let resolved = run.execution.as_ref().map(|execution| &execution.resolved);
    let spec = HostSpec {
        binding: crate::agent::connections::snapshot(run.execution.as_ref())?,
        cwd: run.worktree.clone(),
        model: run.model.clone().context("a corral-pi run needs a model")?,
        provider: match resolved {
            Some(resolved) => crate::agent::connections::provider(resolved)?,
            None => run
                .model
                .as_deref()
                .and_then(|model| host_provider(run.vendor.as_deref(), model)),
        },
        effort: run.effort.clone(),
        prompt_file: Some(crate::prompt::run_prompt_file(ctx, &task.id, &run.id)?),
        adopt: false,
        fresh_session: false,
    };
    let target = BindTarget::Worker {
        task: task.id.clone(),
        run: run.id.clone(),
    };
    prepare(ctx, &target, pane, &spec)
}

/// Planning containers use their own binding; never resolve them through coding runs.
pub fn plan_command(
    ctx: &Ctx,
    target: &BindTarget,
    pane: &str,
    resolved: &ResolvedProfile,
    cwd: &Path,
    prompt_file: &Path,
) -> Result<String> {
    consulted("launch");
    crate::plan_review::context::require_profile(&ctx.store, target, resolved)?;
    let model = resolved
        .model
        .clone()
        .context("a corral-pi planner needs a model")?;
    let spec = HostSpec {
        binding: crate::agent::connections::explicit(resolved)?,
        cwd: cwd.to_path_buf(),
        provider: crate::agent::connections::provider(resolved)?,
        model,
        effort: resolved.effort.clone(),
        prompt_file: Some(prompt_file.to_path_buf()),
        adopt: false,
        fresh_session: false,
    };
    prepare(ctx, target, pane, &spec)
}
