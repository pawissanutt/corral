//! Task-independent build session commands. Mutating verbs never use assistant shell authority.
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;
use corral_ops::build_session::{gate, land, record};

#[derive(Subcommand)]
pub enum BuildCmd {
    /// Run a project's configured gate on a clean session candidate, without a task slot
    Gate {
        #[arg(long)]
        session: String,
        #[arg(long)]
        worktree: PathBuf,
        #[arg(long)]
        project: String,
        #[arg(long, value_parser = ["quick", "full"])]
        tier: String,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Owner only: fast-forward the project base using an exact passing full gate
    Land {
        #[arg(long)]
        session: String,
        #[arg(long)]
        worktree: PathBuf,
        #[arg(long)]
        project: String,
        #[arg(long)]
        expected_head: String,
        #[arg(long)]
        gate_attempt: String,
        #[arg(long)]
        json: bool,
    },
    /// Owner only: publish the session's versioned build record
    Record {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// List published build session records
    List {
        #[arg(long)]
        json: bool,
    },
    /// Read a published build session record
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(inv: &crate::invocation::Invocation<'_>, cmd: BuildCmd) -> Result<()> {
    execute(inv, cmd).map_err(|error| {
        // This family promises bare `code: detail` refusals, not the generic `error:` prefix.
        let detail = format!("{error:#}").replace(['\r', '\n'], " ");
        eprintln!("{detail}");
        crate::invocation::Rendered {
            exit: 1,
            failure: Some(detail),
        }
        .into()
    })
}

fn execute(inv: &crate::invocation::Invocation<'_>, cmd: BuildCmd) -> Result<()> {
    let ctx = inv.ctx()?;
    match cmd {
        BuildCmd::Gate {
            session,
            worktree,
            project,
            tier,
            base,
            json,
        } => {
            let receipt = gate::run(
                ctx,
                &gate::GateRequest {
                    session: &session,
                    worktree: &worktree,
                    project: &project,
                    tier: &tier,
                    base: base.as_deref(),
                },
            )?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&receipt).context("encoding build gate receipt")?
                );
            } else {
                println!(
                    "{} {}: {} at {}",
                    receipt.attempt, receipt.tier, receipt.result, receipt.head
                );
            }
        }
        BuildCmd::Land {
            session,
            worktree,
            project,
            expected_head,
            gate_attempt,
            json,
        } => {
            let result = land::run(
                ctx,
                &session,
                &worktree,
                &project,
                &expected_head,
                &gate_attempt,
            )?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&result).context("encoding build landing")?
                );
            } else {
                println!(
                    "landed {} on {} (previous {})",
                    result.landed, result.base_branch, result.previous
                );
            }
        }
        BuildCmd::Record { file, json } => {
            corral_ops::build_session::require_owner(ctx)?;
            let input = std::fs::read_to_string(&file)
                .with_context(|| format!("reading build record {}", file.display()))?;
            let published = record::publish(ctx, &input)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&published).context("encoding published build record")?
                );
            } else {
                println!("{} revision {}", published.id, published.revision);
            }
        }
        BuildCmd::List { json } => {
            let rows = record::list(ctx)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&rows).context("encoding build record list")?
                );
            } else {
                print!("{}", record::render_list(&rows));
            }
        }
        BuildCmd::Show { id, json } => {
            let value = record::show(ctx, &id)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&value).context("encoding build record")?
                );
            } else {
                print!("{}", record::render_show(&value));
            }
        }
    }
    Ok(())
}
