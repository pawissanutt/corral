//! Repair mutations route with client-read files; status remains an unobserved local read.
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use corral_core::Store;
use corral_protocol::cmd::{Command, FileInput, repair::RepairCommand};
use std::io::Read as _;
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct RepairArgs {
    #[command(subcommand)]
    pub cmd: RepairCmd,
}

#[derive(Subcommand)]
pub enum RepairCmd {
    /// Read all allowances, exact counters, findings and diagnosis without mutation
    Status { id: String },
    /// Read retained failed receipts, attributions and the diagnostic retry state
    TriageStatus { id: String },
    /// Attribute a failed gate with retained evidence; never grants PASS or coding allowance
    Triage {
        id: String,
        #[arg(long)]
        file: PathBuf,
        /// Explicit owner investigation, required to revise an earlier attribution
        #[arg(long)]
        owner: bool,
    },
    /// Append a structured observation; matching is host-derived from exact identity
    Finding {
        id: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// Consume the single PM diagnosis allowance and return the full history
    Diagnose {
        id: String,
        /// Explicit owner request in Assist; unavailable to PM/agent entry points
        #[arg(long)]
        owner: bool,
    },
    /// Finish diagnosis with a concrete bounded proposal and evidence, never a coding retry
    Diagnosed {
        id: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// Explicit owner-approved allowance renewal, bound to the prior ledger digest
    Renew {
        id: String,
        #[arg(long)]
        file: PathBuf,
    },
}

impl RepairCmd {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Status { .. } => "status",
            Self::TriageStatus { .. } => "triage-status",
            Self::Triage { .. } => "triage",
            Self::Finding { .. } => "finding",
            Self::Diagnose { .. } => "diagnose",
            Self::Diagnosed { .. } => "diagnosed",
            Self::Renew { .. } => "renew",
        }
    }
}

fn read(path: &Path) -> Result<FileInput> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .with_context(|| format!("decoding {}", path.display()))?;
    Ok(FileInput {
        path: path.display().to_string(),
        text,
    })
}

pub fn run(inv: &crate::invocation::Invocation, args: RepairArgs) -> Result<()> {
    let (id, triage) = match args.cmd {
        RepairCmd::Status { id } => (id, false),
        RepairCmd::TriageStatus { id } => (id, true),
        RepairCmd::Triage { id, file, owner } => {
            return inv.submit(Command::Repair(RepairCommand::Triage {
                id,
                file: read(&file)?,
                owner,
            }));
        }
        RepairCmd::Finding { id, file } => {
            return inv.submit(Command::Repair(RepairCommand::Finding {
                id,
                file: read(&file)?,
            }));
        }
        RepairCmd::Diagnose { id, owner } => {
            return inv.submit(Command::Repair(RepairCommand::Diagnose { id, owner }));
        }
        RepairCmd::Diagnosed { id, file } => {
            return inv.submit(Command::Repair(RepairCommand::Diagnosed {
                id,
                file: read(&file)?,
            }));
        }
        RepairCmd::Renew { id, file } => {
            return inv.submit(Command::Repair(RepairCommand::Renew {
                id,
                file: read(&file)?,
            }));
        }
    };
    // Read leaves retain the unobserved, attributed open without config observation.
    let invoker = inv.entry().invoker();
    let store = Store::open_unobserved(&Store::find_root(&inv.start()?)?)?;
    invoker.apply(&store)?;
    let ctx = corral_ops::Ctx::from_store(store);
    let value = if triage {
        serde_json::to_value(corral_ops::gate_triage::status(&ctx, &id)?)?
    } else {
        match corral_ops::repair_status(&ctx, &id)? {
            Some(ledger) => {
                serde_json::json!({"digest":ledger.digest()?, "transition":ledger.transition()?,
                    "local_counters":ledger.active()?.history.counters()?, "task_counters":ledger.task_counters()?,
                    "unresolved":ledger.unresolved()?, "ledger":ledger})
            }
            None => serde_json::Value::Null,
        }
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
