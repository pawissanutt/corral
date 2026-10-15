use anyhow::Result;
use clap::Subcommand;
use corral_ops::{Ctx, PmAction};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum PmCmd {
    /// Replay frozen events and score code policy or imported model decisions, offline
    Bench {
        #[arg(long)]
        events: PathBuf,
        /// Independently labelled snapshots supplementing events with log/transcript evidence
        #[arg(long)]
        cases: Option<PathBuf>,
        /// Offline JSONL case_id/model/option/reported_cost_usd decisions
        #[arg(long)]
        responses: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Return exactly one read-only, evidence-backed PM action
    Next {
        #[arg(long)]
        json: bool,
    },
    /// Compact board state and the next decision, without syncing
    Brief {
        #[arg(long)]
        json: bool,
    },
}

fn print_action(action: &PmAction) {
    println!(
        "{}: {}",
        action.task.as_deref().unwrap_or("workspace"),
        action.why
    );
    for evidence in &action.evidence {
        println!("Evidence: {evidence}");
    }
    println!(
        "Recommended: {}{}",
        action.recommended,
        if action.requires_owner {
            " (owner required)"
        } else {
            ""
        }
    );
    println!("Command: {}", action.command);
    println!("Verify: {}", action.verify);
    for option in &action.options {
        println!(
            "Option {}: {} -> {}",
            option.id, option.label, option.command
        );
    }
}

pub fn run(start: &Path, cmd: Option<PmCmd>) -> Result<()> {
    match cmd {
        None => println!("PM pane {}", corral_ops::open_pm(&Ctx::open(start)?)?),
        Some(PmCmd::Bench {
            events,
            cases,
            responses,
            json,
        }) => {
            let report =
                corral_ops::pm_bench::pm_bench(&events, cases.as_deref(), responses.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "Replayed {} events; {} labelled decisions; {} of 7 criteria covered.",
                    report.trace.len(),
                    report.cases.len(),
                    report.covered_criteria.len()
                );
                for model in &report.models {
                    println!(
                        "{}: {}/{} passed; {} policy violations; cost/decision USD {:?} ({})",
                        model.model,
                        model.passed,
                        model.decisions.len(),
                        model.policy_violations,
                        model.cost_per_decision_usd,
                        model.cost_source
                    );
                }
                for limitation in &report.limitations {
                    println!("Limit: {limitation}");
                }
            }
        }
        Some(PmCmd::Next { json }) => {
            let action = corral_ops::pm_next(&Ctx::open(start)?)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&action)?);
            } else {
                print_action(&action);
            }
        }
        Some(PmCmd::Brief { json }) => {
            let ctx = Ctx::open(start)?;
            let tasks = ctx.store.list()?;
            let (used, limit) = corral_ops::slots(&ctx)?;
            let action = corral_ops::pm_next(&ctx)?;
            let mut counts = std::collections::BTreeMap::new();
            for task in &tasks {
                *counts.entry(task.status.as_str()).or_insert(0usize) += 1;
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "slots": {"used":used,"limit":limit}, "counts":counts, "next":action,
                    }))?
                );
            } else {
                println!(
                    "Slots: {used}/{limit}; states: {}",
                    serde_json::to_string(&counts)?
                );
                print_action(&action);
            }
        }
    }
    Ok(())
}
