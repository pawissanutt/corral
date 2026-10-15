use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;
use corral_core::{DecisionRequest, ResumeReceipt, WaitRequest};
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
}

fn payload<T: DeserializeOwned>(path: &PathBuf) -> Result<T> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn run(ctx: &Ctx, cmd: RunCmd) -> Result<()> {
    let now = Timestamp::now();
    let task = match cmd {
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
                _ => anyhow::bail!("select exactly one of --wait or --decision"),
            };
            corral_ops::resume_run(ctx, &id, target, payload::<ResumeReceipt>(&file)?, now)?
        }
    };
    println!("{}", serde_json::to_string_pretty(&task)?);
    Ok(())
}
