use anyhow::{Result, bail};
use clap::Subcommand;
use corral_ops::PmAction;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum PmCmd {
    /// Manage durable PM context as the owner
    Notes {
        #[command(subcommand)]
        cmd: crate::pm_notes_cmd::PmNotesCmd,
    },
    /// Resume a PM paused by the cache usage guard
    GuardResume {
        #[arg(long)]
        json: bool,
    },
    /// Recover the recorded corral-pi PM in the same conversation, without taking focus
    Relaunch {
        #[arg(long)]
        replace: bool,
        #[arg(long, default_value = "owner_request")]
        reason: String,
        #[arg(long)]
        json: bool,
    },
    /// Start a fresh PM conversation in a new generation once nothing in the old one would be
    /// lost; prints the blocking reasons otherwise
    Rotate {
        #[arg(long, default_value = "owner_request")]
        reason: String,
        /// Rotate despite blockers, except an open turn, an open delivery envelope or a
        /// starting successor
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    /// Shadow dispatcher observations (no effect)
    Shadow {
        #[command(subcommand)]
        cmd: crate::pm_shadow_cmd::PmShadowCmd,
    },
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
    /// Read authorized actions, pending decisions and waits without changing state
    Next {
        #[arg(long)]
        json: bool,
    },
    /// Compact board state and the next decision, without syncing
    Brief {
        #[arg(long)]
        json: bool,
    },
    /// Revalidate a revision-bound action and run the existing authorized executors
    Act {
        action: String,
        #[arg(long)]
        json: bool,
    },
    /// Acknowledge presentation, not resolution, of an exact owner decision
    Present {
        decision: String,
        #[arg(long)]
        delivery: String,
        /// Omit when delivery is uncertain; uncertain presentations are not blindly repeated
        #[arg(long)]
        confirmed: bool,
    },
    /// Record an actual owner answer to a workspace coordination decision
    Resolve {
        decision: String,
        #[arg(long)]
        answer: String,
    },
    /// The PM inbox: requests from assistants and the owner, each with a recorded outcome
    Inbox {
        #[command(subcommand)]
        cmd: crate::pm_inbox_cmd::PmInboxCmd,
    },
}

impl PmCmd {
    /// Notes, inbox posts and withdrawals act as their caller, not as the PM.
    pub fn acts_as_caller(&self) -> bool {
        matches!(self, Self::Notes { .. })
            || matches!(self, Self::Inbox { cmd } if cmd.acts_as_caller())
    }
}

/// Evaluates `pass` on one `read_only_snapshot`: one read of the board and of the session
/// binding, and one ownership verdict for its task list. `CORRAL_WORK_REPORT=1` prints this
/// thread's work counters to stderr afterwards.
pub(crate) fn evaluate<T>(
    ctx: &corral_ops::Ctx,
    pass: impl FnOnce(&corral_ops::Ctx) -> Result<T>,
) -> Result<T> {
    let snapshot = corral_ops::Ctx {
        store: ctx.store.read_only_snapshot()?,
        ..ctx.clone()
    };
    corral_ops::lesson_rows::prepare_coordination(&snapshot)?;
    let _hot = corral_core::work::HotScope::enter();
    let result = pass(&snapshot);
    if std::env::var_os("CORRAL_WORK_REPORT").is_some_and(|value| !value.is_empty()) {
        use corral_core::work::{Work, count};
        let counts: serde_json::Map<_, _> = Work::ALL
            .iter()
            .map(|work| (work.name().to_owned(), count(*work).into()))
            .collect();
        eprintln!("work: {}", serde_json::Value::Object(counts));
    }
    result
}

fn readback(ctx: &corral_ops::Ctx) -> Result<serde_json::Value> {
    let state = corral_ops::pm_workset(ctx)?;
    let mut action = serde_json::to_value(&state.next)?;
    action["workset"] = serde_json::to_value(&state)?;
    action["automation"] = corral_ops::automation_summary(ctx)?.into();
    Ok(action)
}

fn print_workset(ctx: &corral_ops::Ctx) -> Result<()> {
    let state = corral_ops::pm_workset(ctx)?;
    println!(
        "{}: {}/{} accepted; {} scoped running; independent work continues: {}",
        state.status.as_str(),
        state.progress.accepted,
        state.progress.total,
        state.progress.running,
        state.independent_work_continues,
    );
    for decision in state.decisions {
        let newly_required = decision.presentation.is_none();
        println!(
            "Decision {} ({}): {} [{}]",
            decision.id,
            decision.task,
            decision.action.why,
            match decision.presentation {
                Some(receipt) if receipt.confirmed => "presented; answer pending",
                Some(_) => "delivery uncertain; answer pending",
                None => "not yet presented",
            },
        );
        if newly_required {
            for evidence in decision.action.evidence {
                println!("  {evidence}");
            }
        }
    }
    for blocker in state.blockers {
        println!("Blocker: {blocker}");
    }
    for item in state.inbox {
        println!("Inbox: {} [{}]", item.why, item.evidence.join("; "));
    }
    Ok(())
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
    if let Some(digest) = &action.question_digest {
        for line in corral_ops::question_digest_lines(digest) {
            println!("{line}");
        }
        println!("Verify: {}", action.verify);
        return;
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

/// Prints the new generation's pane or every blocking reason; a refusal exits non-zero.
fn rotate(ctx: &corral_ops::Ctx, reason: &str, force: bool, json: bool) -> Result<()> {
    let outcome = corral_ops::rotate_pm(ctx, reason, force)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    }
    match outcome {
        corral_ops::RotateOutcome::Rotated {
            pane,
            generation,
            fenced,
            overridden,
            ..
        } => {
            if !json {
                println!(
                    "PM rotated: generation {generation} in pane {pane} starts a fresh \
                     conversation (generation {fenced} fenced)"
                );
                for reason in overridden {
                    println!("Overridden: {}", reason.sentence);
                }
            }
            Ok(())
        }
        corral_ops::RotateOutcome::Blocked { reasons, .. } => {
            if !json {
                println!("PM rotation blocked; nothing changed:");
                for reason in &reasons {
                    println!("- {}", reason.sentence);
                }
            }
            bail!("PM rotation blocked by {} reason(s)", reasons.len())
        }
    }
}

pub fn run(start: &Path, cmd: Option<PmCmd>, entry: &crate::entry::Entry) -> Result<()> {
    match cmd {
        Some(PmCmd::Notes { cmd }) => crate::pm_notes_cmd::run(start, cmd, entry)?,
        Some(PmCmd::GuardResume { json }) => {
            let resumed = corral_ops::pm_guard_resume(&entry.open(start)?)?;
            if json {
                println!("{}", serde_json::to_string(&resumed)?);
            } else if resumed {
                println!("PM usage guard resumed");
            } else {
                println!("PM usage guard was not paused");
            }
        }
        None => println!("PM pane {}", corral_ops::open_pm(&entry.open(start)?)?),
        Some(PmCmd::Relaunch {
            replace,
            reason,
            json,
        }) => {
            let pane = corral_ops::relaunch_pm(&entry.open(start)?, &reason, replace)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({"pane": pane}))?
                );
            } else {
                println!("{pane}");
            }
        }
        Some(PmCmd::Rotate {
            reason,
            force,
            json,
        }) => rotate(&entry.open(start)?, &reason, force, json)?,
        Some(PmCmd::Shadow { cmd }) => crate::pm_shadow_cmd::run(start, cmd, entry)?,
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
        Some(PmCmd::Next { json }) => evaluate(&entry.open(start)?, |ctx| {
            if json {
                println!("{}", serde_json::to_string_pretty(&readback(ctx)?)?);
            } else {
                println!("{}", corral_ops::automation_summary(ctx)?);
                print_workset(ctx)?;
                print_action(&corral_ops::pm_next(ctx)?);
            }
            Ok(())
        })?,
        Some(PmCmd::Act { action, json }) => {
            let result = corral_ops::pm_act(&entry.open(start)?, &action)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("{:?}: {}", result.outcome, result.workset.status.as_str());
                print_action(&result.workset.next);
            }
        }
        Some(PmCmd::Present {
            decision,
            delivery,
            confirmed,
        }) => {
            corral_ops::pm_present(
                &entry.open(start)?,
                &decision,
                &delivery,
                confirmed,
                jiff::Timestamp::now(),
            )?;
            println!("Presentation recorded; owner decision remains pending.");
        }
        Some(PmCmd::Resolve { decision, answer }) => {
            corral_ops::pm_resolve(&entry.open(start)?, &decision, &answer)?;
            println!("Owner answer recorded; task restrictions remain enforced.");
        }
        Some(PmCmd::Inbox { cmd }) => crate::pm_inbox_cmd::run(start, cmd, entry)?,
        Some(PmCmd::Brief { json }) => {
            let ctx = entry.open(start)?;
            let health = corral_ops::pm_health(&ctx)?;
            let setup = corral_ops::setup_status(&ctx.store)?;
            let tasks = ctx.store.list()?;
            let (used, limit) = corral_ops::slots(&ctx)?;
            let action = corral_ops::pm_next(&ctx)?;
            let next = readback(&ctx)?;
            let now = jiff::Timestamp::now();
            let due = corral_ops::lessons_due(&ctx, now)?;
            let score = |lesson: &corral_core::Lesson| {
                let age = now.duration_since(lesson.last_seen).as_secs_f64().max(0.0);
                lesson.confidence
                    * if lesson.pinned {
                        1.0
                    } else {
                        2.0_f64.powf(-age / (90.0 * 86400.0))
                    }
            };
            let mut lessons = std::collections::BTreeMap::new();
            for project in ctx.store.config.projects.keys() {
                let mut selected = ctx.store.lessons(project)?;
                selected.retain(|l| !l.contradicted);
                selected.sort_by(|a, b| {
                    b.pinned
                        .cmp(&a.pinned)
                        .then_with(|| score(b).total_cmp(&score(a)))
                        .then_with(|| a.id.cmp(&b.id))
                });
                selected.truncate(ctx.store.config.lessons.top);
                if !selected.is_empty() {
                    lessons.insert(project.clone(), selected);
                }
            }
            let mut counts = std::collections::BTreeMap::new();
            for task in &tasks {
                *counts.entry(task.status.as_str()).or_insert(0usize) += 1;
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "slots": {"used":used,"limit":limit}, "counts":counts, "next":next,
                        "operating": next["workset"],
                        "lessons":lessons, "lessons_due":due,
                        "health":health,
                        "setup":setup,
                        "automation":next["automation"],
                    }))?
                );
            } else {
                println!("{}", corral_ops::automation_summary(&ctx)?);
                print_workset(&ctx)?;
                println!("Health: {}", serde_json::to_string(&health)?);
                println!("Setup: {}", serde_json::to_string(&setup)?);
                println!(
                    "Slots: {used}/{limit}; states: {}",
                    serde_json::to_string(&counts)?
                );
                print_action(&action);
                for (project, rules) in lessons {
                    for rule in rules {
                        println!("Lesson {project}/{}: {}", rule.id, rule.rule);
                    }
                }
                for schedule in due {
                    println!(
                        "Lessons due {}: {}",
                        schedule.project,
                        schedule.reasons.join(", ")
                    );
                }
            }
        }
    }
    Ok(())
}
