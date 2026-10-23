use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use corral_core::Verdict;
use corral_ops::{Ctx, PlanVerdict};

#[derive(Subcommand)]
pub enum PlanCmd {
    /// Read tracker Plan readiness, target-qualified prerequisites and criterion coverage
    Progress {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Start an aggregate attempt and run the gate on the combined Plan candidate
    Verify {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Record an aggregate scenario observation (evidence JSON with run "aggregate")
    Evidence {
        id: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Launch the configured independent reviewer on the current aggregate attempt
    AggregateReview {
        id: String,
        #[arg(long)]
        attempt: String,
        #[arg(long)]
        json: bool,
    },
    /// Record a launched aggregate reviewer's verdict; callbacks name their exact review
    AggregateVerdict {
        id: String,
        #[arg(value_parser = ["approve", "changes", "escalate"])]
        verdict: String,
        #[arg(long)]
        attempt: String,
        #[arg(long)]
        review: String,
        #[arg(long)]
        notes_file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Accept the current aggregate attempt and land it once (owner by default)
    Accept {
        id: String,
        #[arg(long)]
        attempt: String,
        /// Act under the confirmed operating grant instead of as the owner
        #[arg(long)]
        automatic: bool,
        #[arg(long)]
        json: bool,
    },
    /// End a planner session while retaining submitted plans and review history
    Cancel {
        id: String,
        #[arg(long)]
        json: bool,
    },
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
    /// Amend a published integration-Plan member's gate (owner only, revision-guarded)
    MemberGate {
        member: String,
        /// Gate YAML: tier, optional tests and expected_failures [{test, reason}]
        #[arg(long)]
        file: PathBuf,
        /// The member's current member-gate revision (0 before the first amendment)
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        reason: String,
        /// Refuse unless the member belongs to this tracker
        #[arg(long)]
        tracker: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Add an unstarted task to a tracker Plan's required members, e.g. a fix after a failed verify (owner only)
    AddMember {
        tracker: String,
        member: String,
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
        PlanCmd::Progress { id, json } => {
            let progress = corral_ops::plan_progress(ctx, &id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&progress)?);
            } else {
                let counts = &progress.counts;
                println!(
                    "{}: {} accepted, {} running, {} review, {} waiting, {} missing",
                    progress.task,
                    counts.accepted,
                    counts.running,
                    counts.review,
                    counts.waiting,
                    counts.missing,
                );
                for work in &progress.work {
                    println!(
                        "  {} {:?}{}: {}",
                        work.task,
                        work.state,
                        work.run
                            .as_ref()
                            .map(|run| format!(" {run}"))
                            .unwrap_or_default(),
                        work.reason.as_deref().unwrap_or(&work.title),
                    );
                    for dependency in &work.prerequisites {
                        println!(
                            "    {} -> {}: {}",
                            dependency.task,
                            dependency.target,
                            dependency.reason.as_deref().unwrap_or("satisfied"),
                        );
                    }
                }
                for acceptance in &progress.unmapped_criteria {
                    println!(
                        "  unmapped {}: {}",
                        acceptance.criterion.as_deref().unwrap_or("(unnamed)"),
                        acceptance.scenario,
                    );
                }
                if let Some(aggregate) = &progress.aggregate {
                    let state = if aggregate.accepted {
                        "accepted"
                    } else if aggregate.ready {
                        "ready for aggregate verification"
                    } else {
                        "not ready"
                    };
                    println!(
                        "  aggregate: {state}{}",
                        aggregate
                            .attempt
                            .as_ref()
                            .map(|attempt| format!(" (latest {attempt})"))
                            .unwrap_or_default()
                    );
                    for blocker in &aggregate.blockers {
                        println!("    {blocker}");
                    }
                }
            }
        }
        PlanCmd::Verify { id, json } => {
            let attempt = corral_ops::plan_verify(ctx, &id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&attempt)?);
            } else {
                println!(
                    "{id} aggregate attempt {}: gate {}{}",
                    attempt.id,
                    attempt
                        .gate
                        .as_ref()
                        .map_or("not required (no code)", |gate| gate.result.as_str()),
                    attempt
                        .gate
                        .as_ref()
                        .and_then(|gate| gate.failed_cmd.as_deref())
                        .map(|failed| format!(" ({failed})"))
                        .unwrap_or_default()
                );
            }
        }
        PlanCmd::Evidence { id, file, json } => {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let observation =
                serde_json::from_str(&text).context("parsing aggregate evidence JSON")?;
            let record = corral_ops::plan_evidence(ctx, &id, &observation)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&record)?);
            } else {
                println!(
                    "{id} {} {}: {:?}",
                    record.identity.attempt,
                    record.observation.criterion,
                    record.observation.status
                );
            }
        }
        PlanCmd::AggregateReview { id, attempt, json } => {
            let review = corral_ops::plan_aggregate_review(ctx, &id, &attempt)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&review)?);
            } else {
                println!(
                    "{id} aggregate review {} by {} started",
                    review.id, review.reviewer
                );
            }
        }
        PlanCmd::AggregateVerdict {
            id,
            verdict,
            attempt,
            review,
            notes_file,
            json,
        } => {
            let notes = std::fs::read_to_string(&notes_file)
                .with_context(|| format!("reading {}", notes_file.display()))?;
            let Some(verdict) = Verdict::parse(&verdict) else {
                bail!("invalid aggregate verdict");
            };
            let record = corral_ops::plan_aggregate_verdict(
                ctx,
                &id,
                &corral_ops::AggregateVerdict {
                    attempt: &attempt,
                    review: &review,
                    verdict,
                    notes: &notes,
                },
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&record)?);
            } else {
                println!(
                    "{id} {}: {} by {}",
                    record.id,
                    verdict.as_str(),
                    record.reviewer
                );
            }
        }
        PlanCmd::Accept {
            id,
            attempt,
            automatic,
            json,
        } => {
            let acceptance = corral_ops::plan_accept(ctx, &id, &attempt, automatic)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&acceptance)?);
            } else {
                println!(
                    "{id} {attempt} accepted by {} ({:?}){}",
                    acceptance.by,
                    acceptance.strategy,
                    acceptance
                        .after
                        .as_ref()
                        .map(|sha| format!("; main at {sha}"))
                        .unwrap_or_default()
                );
            }
        }
        PlanCmd::Cancel { id, json } => {
            let task = corral_ops::plan_cancel(ctx, &id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&task)?);
            } else {
                println!("{id}: plan run cancelled; plan history retained");
            }
        }
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
        PlanCmd::AddMember {
            tracker,
            member,
            json,
        } => {
            let added = corral_ops::plan_add_member(ctx, &tracker, &member)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&added)?);
            } else {
                println!(
                    "{} added to {}; members: {}",
                    added.member,
                    added.tracker,
                    added.children.join(", ")
                );
            }
        }
        PlanCmd::MemberGate {
            member,
            file,
            expected_revision,
            reason,
            tracker,
            json,
        } => {
            let outcome = corral_ops::plan_member_gate(
                ctx,
                &corral_ops::MemberGateRequest {
                    member: &member,
                    tracker: tracker.as_deref(),
                    file: &file,
                    expected_revision,
                    reason: &reason,
                },
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&outcome)?);
            } else {
                println!(
                    "{} gate on {}: tier {} at revision {} (source {}){}",
                    outcome.member,
                    outcome.tracker,
                    outcome.record.gate.tier,
                    outcome.record.revision,
                    outcome.source,
                    outcome
                        .reconciled_run
                        .as_ref()
                        .map(|run| format!("; {run} completion commands reconciled"))
                        .unwrap_or_default()
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
