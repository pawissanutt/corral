use anyhow::{Result, bail};
use clap::Subcommand;
use corral_ops::pm_read::{self, PmReadout};
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
        /// Compute locally instead of using a daemon readout
        #[arg(long)]
        fresh: bool,
    },
    /// Compact board state and the next decision, without syncing
    Brief {
        #[arg(long)]
        json: bool,
        /// Compute locally instead of using a daemon readout
        #[arg(long)]
        fresh: bool,
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
    pm_read::evaluate(ctx, pass)
}

/// Prints the new generation's pane, why the rotation degraded to a relaunch, or every
/// blocking reason; anything but a rotation exits non-zero.
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
        corral_ops::RotateOutcome::RelaunchedInstead {
            pane,
            generation,
            reasons,
            ..
        } => {
            let because: Vec<&str> = reasons.iter().map(|r| r.sentence.as_str()).collect();
            let line = format!(
                "rotation deferred: conversation continued in generation {generation} (pane \
                 {pane}) because {}",
                because.join("; ")
            );
            if !json {
                println!("{line}");
            }
            bail!(line)
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
        Some(PmCmd::Next { json, fresh }) => {
            let ctx = entry.open(start)?;
            let readout = served(&ctx, corral_protocol::read::ReadKind::PmNext, fresh)
                .map(Ok)
                .unwrap_or_else(|| pm_read::next(&ctx))?;
            print_readout(readout, json)?;
        }
        Some(PmCmd::Act { action, json }) => {
            let result = corral_ops::pm_act(&entry.open(start)?, &action)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("{:?}: {}", result.outcome, result.workset.status.as_str());
                print!("{}", pm_read::format_action(&result.workset.next));
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
        Some(PmCmd::Brief { json, fresh }) => {
            let ctx = entry.open(start)?;
            let readout = served(&ctx, corral_protocol::read::ReadKind::PmBrief, fresh)
                .map(Ok)
                .unwrap_or_else(|| pm_read::brief(&ctx))?;
            print_readout(readout, json)?;
        }
    }
    Ok(())
}

fn served(
    ctx: &corral_ops::Ctx,
    kind: corral_protocol::read::ReadKind,
    fresh: bool,
) -> Option<PmReadout> {
    if fresh || !ctx.store.loop_owned_elsewhere().unwrap_or(true) {
        return None;
    }
    corral_ops::daemon_client::served(&ctx.store, kind)
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn print_readout(readout: PmReadout, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&readout.json)?);
    } else {
        print!("{}", readout.text);
    }
    Ok(())
}
