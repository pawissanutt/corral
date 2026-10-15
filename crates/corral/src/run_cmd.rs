use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use corral_core::{
    CheckpointRecord, CheckpointUpdate, DecisionRequest, ProgressUpdate, ResumeReceipt, Run,
    RunProgress, StepState, WaitRequest,
};
use corral_ops::{Ctx, ResumeTarget};
use jiff::Timestamp;
use serde::de::DeserializeOwned;

#[derive(Subcommand)]
pub enum RunCmd {
    /// Register an explicit event wait (version 1 JSON); never executes monitor commands
    Wait {
        id: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// Ask the owner an exact question with a requested action (version 1 JSON)
    Decision {
        id: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// Record an event or answered decision receipt; never resends or relaunches work
    Resume {
        id: String,
        #[arg(
            long,
            required_unless_present = "decision",
            conflicts_with = "decision"
        )]
        wait: Option<String>,
        #[arg(long)]
        decision: Option<String>,
        #[arg(long)]
        file: PathBuf,
    },
    /// Read identity and state, or replace execution progress from a JSON file
    Progress(RunArgs),
    /// Read identity and state, or save a guarded checkpoint from a JSON file
    Checkpoint(RunArgs),
}

#[derive(Args)]
pub struct RunArgs {
    #[arg(value_name = "TASK-ID")]
    pub id: String,
    /// Ordinary regular JSON file, bounded by artifacts.max_bytes; omitted means read-only
    #[arg(long, value_name = "JSON")]
    file: Option<PathBuf>,
    /// Print identity and state as JSON; errors are JSON on stderr
    #[arg(long)]
    pub json: bool,
}

impl RunCmd {
    pub fn args(&self) -> Option<&RunArgs> {
        match self {
            Self::Progress(args) | Self::Checkpoint(args) => Some(args),
            Self::Wait { .. } | Self::Decision { .. } | Self::Resume { .. } => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Progress(_) => "progress",
            Self::Checkpoint(_) => "checkpoint",
            Self::Wait { .. } => "wait",
            Self::Decision { .. } => "decision",
            Self::Resume { .. } => "resume",
        }
    }
}

fn read_input<T: DeserializeOwned>(path: &Path, max: u64) -> Result<T> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspecting JSON input {}", path.display()))?;
    if !meta.is_file() {
        bail!(
            "{} must be an ordinary regular JSON file (not a symlink or special file)",
            path.display()
        );
    }
    if meta.len() > max {
        bail!(
            "{} exceeds artifacts.max_bytes ({max} bytes)",
            path.display()
        );
    }
    let mut file =
        File::open(path).with_context(|| format!("opening JSON input {}", path.display()))?;
    let meta = file
        .metadata()
        .with_context(|| format!("inspecting opened JSON input {}", path.display()))?;
    if !meta.is_file() {
        bail!("{} is not a regular JSON file", path.display());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading bounded JSON input {}", path.display()))?;
    let mut extra = [0];
    if file
        .read(&mut extra)
        .with_context(|| format!("checking JSON input size {}", path.display()))?
        != 0
    {
        bail!(
            "{} exceeds artifacts.max_bytes ({max} bytes)",
            path.display()
        );
    }
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing typed JSON input {}", path.display()))
}

fn payload<T: DeserializeOwned>(path: &PathBuf) -> Result<T> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn run(ctx: &Ctx, cmd: RunCmd) -> Result<()> {
    let now = Timestamp::now();
    let task = match cmd {
        RunCmd::Progress(args) => return run_progress(ctx, args, false),
        RunCmd::Checkpoint(args) => return run_progress(ctx, args, true),
        RunCmd::Wait { id, file } => {
            corral_ops::register_wait(ctx, &id, payload::<WaitRequest>(&file)?, now)?
        }
        RunCmd::Decision { id, file } => {
            corral_ops::register_decision(ctx, &id, payload::<DecisionRequest>(&file)?, now)?
        }
        RunCmd::Resume {
            id,
            wait,
            decision,
            file,
        } => {
            let target = match (wait.as_deref(), decision.as_deref()) {
                (Some(wait), None) => ResumeTarget::Wait(wait),
                (None, Some(decision)) => ResumeTarget::Decision(decision),
                _ => bail!("select exactly one of --wait or --decision"),
            };
            corral_ops::resume_run(ctx, &id, target, payload::<ResumeReceipt>(&file)?, now)?
        }
    };
    println!("{}", serde_json::to_string_pretty(&task)?);
    Ok(())
}

fn run_progress(ctx: &Ctx, args: RunArgs, checkpoint: bool) -> Result<()> {
    if let Some(path) = &args.file {
        let max = ctx.store.config.artifacts.max_bytes;
        if checkpoint {
            let input: CheckpointUpdate = read_input(path, max)?;
            corral_ops::save_checkpoint(ctx, &args.id, &input)
                .with_context(|| format!("checkpointing {} run {}", args.id, input.run))?;
        } else {
            let input: ProgressUpdate = read_input(path, max)?;
            corral_ops::update_progress(ctx, &args.id, &input)
                .with_context(|| format!("updating {} run {}", args.id, input.run))?;
        }
    }
    let state = corral_ops::progress_readback(ctx, &args.id)
        .with_context(|| format!("reading identity and state for {}", args.id))?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&state)?);
    } else {
        println!("{} run {}", state.task, state.run);
        println!("spec_revision: {}", state.spec_revision);
        println!(
            "head: {}\nbase: {}\ntree: {}",
            state.head, state.base, state.tree
        );
        println!(
            "checkpoint_status: {}",
            serde_json::to_value(&state.checkpoint_status)?
        );
        for reason in &state.checkpoint_reasons {
            println!("checkpoint_reason: {reason}");
        }
        show_state(state.progress.as_ref(), &state.checkpoints, "");
    }
    Ok(())
}

pub fn show_run(run: &Run, indent: &str) {
    show_state(run.progress.as_ref(), &run.checkpoints, indent);
}

fn show_state(progress: Option<&RunProgress>, checkpoints: &[CheckpointRecord], indent: &str) {
    if let Some(progress) = progress {
        println!(
            "{indent}progress: {} (revision {}, spec {})",
            progress.summary(),
            progress.revision,
            progress.spec_revision
        );
        for phase in &progress.phases {
            println!("{indent}  phase {}: {}", phase.id, phase.label);
            for step in &phase.steps {
                println!(
                    "{indent}    {} [{}]: {}",
                    step.id,
                    step.status.label(),
                    step.label
                );
                if let StepState::Dropped { reason } = &step.status {
                    println!("{indent}      reason: {reason}");
                }
                if !step.criteria.is_empty() {
                    println!("{indent}      criteria: {}", step.criteria.join(", "));
                }
            }
        }
    } else {
        println!("{indent}progress: none (revision 0)");
    }
    for record in checkpoints {
        let checkpoint = &record.checkpoint;
        let input = &checkpoint.input;
        println!(
            "{indent}checkpoint: revision {} at {} -> {}",
            checkpoint.revision, checkpoint.captured, record.artifact
        );
        println!(
            "{indent}  progress revision: {}; spec: {}",
            input.progress_revision, input.spec_revision
        );
        println!(
            "{indent}  head: {}; base: {}; tree: {}",
            input.head, input.base, input.tree
        );
        println!(
            "{indent}  completed: {}; current: {}",
            checkpoint.completed_steps.join(", "),
            checkpoint.current_step.as_deref().unwrap_or("-")
        );
        println!(
            "{indent}  criteria: {}; evidence: {}",
            input.criterion_ids.join(", "),
            input.evidence_refs.join(", ")
        );
        println!("{indent}  next: {}", input.next_action);
        for decision in &input.decisions {
            println!("{indent}  decision: {decision}");
        }
        for wait in &input.waits {
            if let Some(resolution) = &wait.resolution {
                println!("{indent}  resolved wait {}: {resolution}", wait.id);
                continue;
            }
            println!(
                "{indent}  wait {} owner {} handle {}: {}",
                wait.id, wait.owner, wait.handle, wait.condition
            );
        }
        for resource in &input.resources {
            if let Some(resolution) = &resource.resolution {
                println!("{indent}  released resource {}: {resolution}", resource.id);
                continue;
            }
            println!(
                "{indent}  resource {} owner {}: {}; cleanup: {}",
                resource.id, resource.owner, resource.description, resource.cleanup
            );
        }
    }
    if checkpoints.is_empty() {
        println!("{indent}checkpoints: none (revision 0)");
    }
}
