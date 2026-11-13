//! `run` command executors.
use anyhow::{Context, Result, bail};
use corral_core::{
    CheckpointUpdate, CriteriaManifest, DecisionRequest, ProgressUpdate, WaitRequest,
};
use corral_protocol::cmd::{FileInput, run::RunCommand};
use serde::de::DeserializeOwned;

use super::render::ErrorStyle;
use super::{Exec, Verb};

impl Verb for RunCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        match self {
            Self::Wait { id, file } => {
                let input: WaitRequest = payload(&file)?;
                let task = crate::register_wait(&cx.ctx, &id, input, corral_core::clock::now())?;
                cx.out.json_pretty(&task)
            }
            Self::Decision { id, file } => {
                let input: DecisionRequest = payload(&file)?;
                let task =
                    crate::register_decision(&cx.ctx, &id, input, corral_core::clock::now())?;
                cx.out.json_pretty(&task)
            }
            Self::Progress { id, file, .. } => progress(cx, &id, &file, false),
            Self::Checkpoint { id, file, .. } => progress(cx, &id, &file, true),
            Self::CriteriaRegister { id, file } => {
                let value: serde_json::Value = payload(&file)?;
                let manifest: CriteriaManifest =
                    serde_json::from_value(value).context("parsing criteria manifest")?;
                let task = crate::register_criteria(&cx.ctx, &id, &manifest)?;
                cx.out.json_pretty(&task.completion)
            }
            Self::Other => Err(super::unknown_command("other")),
        }
    }

    fn touches_runtime(&self) -> bool {
        matches!(self, Self::Wait { .. } | Self::Decision { .. })
    }

    fn error_style(&self) -> ErrorStyle {
        match self {
            Self::Progress { id, json: true, .. } | Self::Checkpoint { id, json: true, .. } => {
                ErrorStyle::Run {
                    command: format!("run {}", self.verb()),
                    task: id.clone(),
                }
            }
            _ => ErrorStyle::Plain,
        }
    }
}

fn payload<T: DeserializeOwned>(file: &FileInput) -> Result<T> {
    serde_json::from_str(&file.text).with_context(|| format!("parsing {}", file.path))
}

fn read_input<T: DeserializeOwned>(file: &FileInput, max: u64) -> Result<T> {
    if file.text.len() as u64 > max {
        bail!("{} exceeds artifacts.max_bytes ({max} bytes)", file.path);
    }
    serde_json::from_slice(file.text.as_bytes())
        .with_context(|| format!("parsing typed JSON input {}", file.path))
}

fn progress(cx: &mut Exec<'_>, id: &str, file: &FileInput, checkpoint: bool) -> Result<()> {
    let max = cx.ctx.store.config.artifacts.max_bytes;
    if checkpoint {
        let input: CheckpointUpdate = read_input(file, max)?;
        crate::save_checkpoint(&cx.ctx, id, &input)
            .with_context(|| format!("checkpointing {id} run {}", input.run))?;
    } else {
        let input: ProgressUpdate = read_input(file, max)?;
        crate::update_progress(&cx.ctx, id, &input)
            .with_context(|| format!("updating {id} run {}", input.run))?;
    }
    Ok(())
}
