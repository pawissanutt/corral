//! Reports and diagnosis cannot enlarge coding authority; renewal is owner-only.
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use corral_core::{RepairPolicy, RepairRenewal, Store};
use serde::{Deserialize, de::DeserializeOwned};
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiagnosisResult {
    diagnosis: String,
    proposal: String,
    evidence: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Renewal {
    renewal: RepairRenewal,
    policy: RepairPolicy,
}

fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_reader(
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?,
    )
    .with_context(|| format!("decoding {}", path.display()))
}

pub fn run(root: &Path, args: RepairArgs, entry: &crate::entry::Entry) -> Result<()> {
    let store = Store::open_unobserved(&Store::find_root(root)?)?;
    entry.apply(&store)?;
    let ctx = corral_ops::Ctx::from_store(store);
    let value = match args.cmd {
        RepairCmd::Status { id } => match corral_ops::repair_status(&ctx, &id)? {
            Some(ledger) => {
                serde_json::json!({"digest":ledger.digest()?, "transition":ledger.transition()?, "ledger":ledger})
            }
            None => serde_json::Value::Null,
        },
        RepairCmd::TriageStatus { id } => {
            serde_json::to_value(corral_ops::gate_triage::status(&ctx, &id)?)?
        }
        RepairCmd::Triage { id, file, owner } => serde_json::to_value(
            corral_ops::gate_triage::diagnose(&ctx, &id, read(&file)?, owner)?,
        )?,
        RepairCmd::Finding { id, file } => {
            serde_json::to_value(corral_ops::repair_finding(&ctx, &id, read(&file)?)?)?
        }
        RepairCmd::Diagnose { id, owner } => serde_json::to_value(if owner {
            corral_ops::repair_diagnose_owner(&ctx, &id)?
        } else {
            corral_ops::repair_diagnose(&ctx, &id)?
        })?,
        RepairCmd::Diagnosed { id, file } => {
            let result: DiagnosisResult = read(&file)?;
            serde_json::to_value(corral_ops::repair_diagnosed(
                &ctx,
                &id,
                &result.diagnosis,
                &result.proposal,
                result.evidence,
            )?)?
        }
        RepairCmd::Renew { id, file } => {
            corral_ops::require_automation_owner(&ctx.store)?;
            let renewal: Renewal = read(&file)?;
            serde_json::to_value(corral_ops::repair_renew(
                &ctx,
                &id,
                renewal.renewal,
                renewal.policy,
            )?)?
        }
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
