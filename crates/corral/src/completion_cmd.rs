use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use corral_core::{CriteriaManifest, EvidenceObservation};
use corral_ops::Ctx;

#[derive(Subcommand)]
pub enum CriteriaCmd {
    /// Register an explicitly owner-reviewed, version 1 JSON manifest for the current spec
    Register {
        id: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// Show the registered manifest and current spec digest
    List { id: String },
}

#[derive(Subcommand)]
pub enum EvidenceCmd {
    /// Append a worker observation referencing existing artifact snapshot paths
    Record {
        id: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// Show canonical observations for the current run (never machine PASS receipts)
    List {
        id: String,
        #[arg(long)]
        json: bool,
    },
}

fn read_json(file: &PathBuf) -> Result<serde_json::Value> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", file.display()))
}

pub fn criteria(ctx: &Ctx, cmd: CriteriaCmd) -> Result<()> {
    match cmd {
        CriteriaCmd::Register { id, file } => {
            let manifest: CriteriaManifest =
                serde_json::from_value(read_json(&file)?).context("parsing criteria manifest")?;
            let task = corral_ops::register_criteria(ctx, &id, &manifest)?;
            println!("{}", serde_json::to_string_pretty(&task.completion)?);
        }
        CriteriaCmd::List { id } => {
            let task = ctx.store.get(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "manifest": task.completion,
                    "spec_digest": corral_ops::completion_spec_digest(&task)?,
                }))?
            );
        }
    }
    Ok(())
}

pub fn evidence(ctx: &Ctx, cmd: EvidenceCmd) -> Result<()> {
    match cmd {
        EvidenceCmd::Record { id, file } => {
            let observation: EvidenceObservation = serde_json::from_value(read_json(&file)?)
                .context("parsing evidence observation")?;
            let record = corral_ops::record_evidence(ctx, &id, &observation)?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        EvidenceCmd::List { id, json } => {
            let records = corral_ops::evidence_list(ctx, &id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else {
                for record in records {
                    println!(
                        "{} {} {:?} worker_assertion {}",
                        record.observation.key,
                        record.observation.criterion,
                        record.observation.status,
                        record.identity.attempt
                    );
                }
            }
        }
    }
    Ok(())
}

pub fn check(ctx: &Ctx, id: &str, summary: Option<&str>, json: bool) -> Result<()> {
    let task = ctx.store.get(id)?;
    let summary = summary
        .or_else(|| task.current_run().and_then(|run| run.summary.as_deref()))
        .unwrap_or("");
    let audit = corral_ops::completion_check_task(ctx, &task, summary)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&audit)?);
    } else {
        println!(
            "{} completion: {} ({})",
            id,
            if audit.passed { "pass" } else { "fail" },
            if audit.audited { "audited" } else { "legacy" }
        );
        for failure in &audit.failures {
            println!("{failure}");
        }
    }
    if !audit.passed {
        bail!("completion audit failed");
    }
    Ok(())
}
