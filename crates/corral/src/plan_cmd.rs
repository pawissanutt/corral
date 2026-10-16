use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use corral_core::Verdict;
use corral_ops::{Ctx, PlanVerdict};

#[derive(Subcommand)]
pub enum PlanCmd {
    /// Snapshot a plan (YAML metadata + markdown) against the current task spec
    Submit {
        id: String,
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Inspect current and historical plan snapshots and verdicts
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Review a plan without starting implementation or taking a coding slot
    Review {
        id: String,
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Record a plan-only verdict; callbacks must include their exact attempt
    Verdict {
        id: String,
        #[arg(value_parser = ["approve", "changes", "escalate"])]
        verdict: String,
        #[arg(long)]
        plan: String,
        /// Omit only for an explicit manual owner verdict
        #[arg(long)]
        attempt: Option<String>,
        /// Explicitly accept or reject as the owner, after independent review
        #[arg(long, conflicts_with = "attempt")]
        owner: bool,
        #[arg(long)]
        notes_file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Set the task's plan requirement, or inherit workspace/project review policy
    Policy {
        id: String,
        #[arg(value_parser = ["required", "off", "inherit"])]
        policy: String,
    },
}

pub fn run(ctx: &Ctx, cmd: PlanCmd) -> Result<()> {
    match cmd {
        PlanCmd::Submit { id, path, json } => {
            let record = corral_ops::plan_submit(ctx, &id, &path)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&record)?);
            } else {
                println!(
                    "{} plan {} submitted: {} spec {}",
                    id, record.id, record.artifact.sha256, record.spec_revision
                );
            }
        }
        PlanCmd::Show { id, json } => {
            let task = ctx.store.get(&id)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "task": task.id,
                        "required": task.design || ctx.store.config.plan_required(task.project.as_deref(), task.plan_required),
                        "spec_revision": corral_ops::plan_spec_revision(&task)?,
                        "plans": task.plans,
                        "plan_runs": task.plan_runs,
                        "plan": task.plan,
                        "children": task.children,
                    }))?
                );
            } else {
                for run in &task.plan_runs {
                    println!(
                        "{} [{}] {:?}, model {}, round {}",
                        run.id,
                        run.profile,
                        run.state,
                        run.model.as_deref().unwrap_or("unknown"),
                        run.rounds + 1
                    );
                }
                for plan in &task.plans {
                    println!(
                        "{} {} spec {} snapshot {}",
                        plan.id,
                        plan.artifact.sha256,
                        plan.spec_revision,
                        ctx.store.dir().join(&plan.artifact.path).display()
                    );
                    for review in &plan.reviews {
                        println!(
                            "  {} {} {}: {}",
                            review.id,
                            review.reviewer,
                            review.verdict.map_or("pending", Verdict::as_str),
                            review.notes.as_deref().unwrap_or("")
                        );
                    }
                }
            }
        }
        PlanCmd::Review { id, reviewer, json } => {
            let record = corral_ops::plan_review(ctx, &id, reviewer.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&record)?);
            } else {
                println!(
                    "{} plan review {} by {} started",
                    id, record.id, record.reviewer
                );
            }
        }
        PlanCmd::Verdict {
            id,
            verdict,
            plan,
            attempt,
            owner,
            notes_file,
            json,
        } => {
            if !owner && attempt.is_none() {
                bail!("plan verdict requires --owner or --attempt");
            }
            let notes = std::fs::read_to_string(&notes_file)
                .with_context(|| format!("reading {}", notes_file.display()))?;
            let Some(verdict) = Verdict::parse(&verdict) else {
                bail!("invalid plan verdict");
            };
            let record = corral_ops::plan_verdict(
                ctx,
                &id,
                &PlanVerdict {
                    plan: &plan,
                    attempt: attempt.as_deref(),
                    verdict,
                    notes: &notes,
                },
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&record)?);
            } else {
                println!(
                    "{} plan {}: {} by {}",
                    id,
                    plan,
                    record.verdict.map_or("pending", Verdict::as_str),
                    record.reviewer
                );
            }
        }
        PlanCmd::Policy { id, policy } => {
            let required = match policy.as_str() {
                "required" => Some(true),
                "off" => Some(false),
                "inherit" => None,
                _ => bail!("invalid plan policy"),
            };
            let task = corral_ops::plan_policy_set(ctx, &id, required)?;
            println!("{}", serde_json::to_string_pretty(&task)?);
        }
    }
    Ok(())
}
