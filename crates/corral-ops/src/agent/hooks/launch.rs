//! Launch and bootstrap seam: the pane command of a pi-native worker run.
//! Owner: T-252 child host-bridge (writes the bootstrap and renders `{corral} agent launch
//! {delivery_args}`).
use anyhow::{Context, Result};
use corral_core::{Run, Task};

use super::consulted;
use crate::Ctx;
use crate::agent::BindTarget;
use crate::agent::launch::{HostSpec, prepare};

pub fn worker_command(ctx: &Ctx, task: &Task, run: &Run) -> Result<String> {
    consulted("launch");
    let pane = &run.herdr.as_ref().context("launch pane missing")?.pane_id;
    let spec = HostSpec {
        cwd: run.worktree.clone(),
        model: run.model.clone().context("a pi-native run needs a model")?,
        effort: run.effort.clone(),
        prompt_file: Some(
            ctx.store
                .prompts_dir()
                .join(format!("{}-{}.md", task.id, run.id)),
        ),
    };
    let target = BindTarget::Worker {
        task: task.id.clone(),
        run: run.id.clone(),
    };
    prepare(ctx, &target, pane, &spec)
}
