//! One workspace mode control; previews never confer authority.
use std::io::IsTerminal;

use anyhow::Result;
use anyhow::ensure;
use clap::{Args, Subcommand};
use corral_core::{OperatingMode, OperatingPreview};
use corral_ops::Ctx;

use crate::automation_cmd::SelectionArgs;

#[derive(Args)]
pub struct ModeArgs {
    #[command(subcommand)]
    cmd: ModeCmd,
}

#[derive(Subcommand)]
enum ModeCmd {
    /// Read effective mode, finite scope, authority, blockers and shared progress
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Withdraw automatic authority without stopping running work
    Assist,
    /// Preview a finite scope; grant only after exact owner confirmation
    Autopilot {
        #[command(flatten)]
        selection: SelectionArgs,
        /// Confirm the exact previously displayed preview identity (also noninteractively)
        #[arg(long)]
        confirm: Option<String>,
        /// Print the preview as JSON; without --confirm this never grants authority
        #[arg(long)]
        json: bool,
    },
    /// Stop new automatic actions without interrupting agents or receipt collection
    Pause,
    /// Resume the existing matching grant; never expands or refreshes its scope
    Resume,
}

pub(crate) fn status(ctx: &Ctx, json: bool) -> Result<()> {
    let workset = corral_ops::pm_workset(ctx)?;
    let effective = ctx.store.effective_operating_policy()?;
    let config = ctx.store.config_layers()?.config()?;
    if json {
        let mut value = serde_json::to_value(&workset)?;
        value["effective"] = serde_json::to_value(&effective)?;
        value["policy"] = serde_json::to_value(&config.automation)?;
        value["approval"] = serde_json::to_value(ctx.store.operating_state()?.approval)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!(
        "Mode: {}{}",
        match workset.mode {
            OperatingMode::Assist => "Assist",
            OperatingMode::Autopilot => "Autopilot",
        },
        if workset.paused { " (paused)" } else { "" }
    );
    println!(
        "{}: {}/{} accepted; {} scoped running; independent work continues: {}",
        workset.status.as_str(),
        workset.progress.accepted,
        workset.progress.total,
        workset.progress.running,
        workset.independent_work_continues,
    );
    println!("Policy revision: {}", workset.policy_revision);
    println!(
        "Grant: {}",
        effective.authorization.as_deref().unwrap_or("none")
    );
    println!(
        "Configured authority (subject to effective refusals): dispatch={}, accept={}, plan_accept={}",
        config.automation.authority.dispatch,
        config.automation.authority.accept,
        config.automation.authority.plan_accept,
    );
    for (id, member) in &config.automation.scope.tasks {
        println!(
            "Scope {id}: {} ({})",
            member.project,
            member.repository.display()
        );
        if let Some(task) = effective.tasks.get(id) {
            for (action, reasons) in &task.refusals {
                println!("  {action:?}: {}", reasons.join("; "));
            }
        }
    }
    for excluded in &config.automation.scope.exclusions {
        println!("Excluded: {excluded}");
    }
    for decision in &workset.decisions {
        println!(
            "Decision {} ({}): {}",
            decision.id, decision.task, decision.action.why
        );
    }
    for wait in &workset.waits {
        println!(
            "Waiting {}: {}",
            wait.task.as_deref().unwrap_or("workspace"),
            wait.why
        );
    }
    for blocker in &workset.blockers {
        println!("Blocker: {blocker}");
    }
    if workset.paused {
        println!("No new automatic actions; running work and receipt collection continue.");
    }
    crate::cmds::dispatch_status::print(ctx)?;
    Ok(())
}

pub(crate) fn print_preview(preview: &OperatingPreview) -> Result<()> {
    println!("Autopilot preview: finite current tasks, never future backlog.");
    for (id, member) in &preview.policy.scope.tasks {
        println!("{id} @ {}: {}", member.project, member.repository.display());
        let constraints = &preview.constraints[id];
        println!(
            "  Owner-only acceptance: {}; owner-only plan acceptance: {}",
            constraints.owner_acceptance, constraints.owner_plan_acceptance,
        );
        println!(
            "  State: {}; dispatch hold: {}; cancelled plan: {}; decisions: {}",
            constraints.lifecycle.as_str(),
            constraints.dispatch_hold,
            constraints.cancelled_plan,
            constraints.pending_decisions.len(),
        );
        let review = &constraints.review;
        println!("  Gates: {}", serde_json::to_string(&review.gate)?);
        println!(
            "  Reviewer: {:?}; cross-family required: {}",
            review.reviewer,
            review.cross_family_required(),
        );
        println!(
            "  Limits: {} repair rounds; {} diff lines; source: {:?}",
            review.max_rounds, review.max_diff_lines, review.size_limits,
        );
        println!(
            "  Owner paths: {}",
            serde_json::to_string(&review.owner_paths)?
        );
    }
    for plan in &preview.policy.scope.plans {
        println!(
            "Reviewed plan {}@{} scopes: {}",
            plan.task,
            plan.revision,
            plan.scopes.keys().cloned().collect::<Vec<_>>().join(", "),
        );
    }
    println!(
        "Exclusions: {}",
        serde_json::to_string(&preview.policy.scope.exclusions)?
    );
    println!(
        "Delegated authority: dispatch={}, accept={}, plan_accept={}",
        preview.policy.authority.dispatch,
        preview.policy.authority.accept,
        preview.policy.authority.plan_accept,
    );
    println!("Existing verification, owner restrictions and resource limits still apply.");
    println!("Exact confirmation digest:\n{}", preview.id);
    println!("Stored grant digest:\n{}", preview.grant_digest);
    Ok(())
}

pub fn run(mut ctx: Ctx, args: ModeArgs) -> Result<()> {
    match args.cmd {
        ModeCmd::Status { json } => return status(&ctx, json),
        ModeCmd::Assist => ctx.store.config = ctx.store.assist_operating_policy()?,
        ModeCmd::Pause => ctx.store.config = ctx.store.pause_operating_policy(true)?,
        ModeCmd::Resume => {
            corral_ops::require_automation_owner(&ctx.store)?;
            ctx.store.config = ctx.store.pause_operating_policy(false)?;
        }
        ModeCmd::Autopilot {
            selection,
            confirm,
            json,
        } => {
            let preview = corral_ops::automation_preview(&ctx.store, &selection.resolve()?)?;
            if let Some(digest) = confirm {
                ensure!(
                    digest == preview.id,
                    "preview changed; review a fresh mode autopilot preview"
                );
                ctx.store.config = corral_ops::automation_confirm(&ctx.store, &preview, &digest)?;
                if json {
                    return status(&ctx, true);
                }
            } else {
                if json {
                    println!("{}", serde_json::to_string_pretty(&preview)?);
                } else {
                    print_preview(&preview)?;
                }
                if json || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                    if !json {
                        println!(
                            "No grant written. Review this preview, then repeat with --confirm {}",
                            preview.id
                        );
                    }
                    return Ok(());
                }
                let Some(config) = crate::automation_cmd::confirm_terminal(&ctx, &preview)? else {
                    return Ok(());
                };
                ctx.store.config = config;
            }
        }
    }
    status(&ctx, false)
}

pub(crate) fn legacy(mut ctx: Ctx, action: &str, json: bool) -> Result<()> {
    match action {
        "off" => {
            ctx.store.config = ctx.store.pause_operating_policy(true)?;
            if !json {
                println!(
                    "Workspace Autopilot paused across dispatch, review, repair and acceptance."
                );
            }
        }
        "on" if !json => {
            println!(
                "No grant written. Use `corral mode autopilot --project <project> --dispatch` \
                 to preview an exact finite scope and confirm it; review-only automation is no longer a control."
            );
        }
        _ => {}
    }
    status(&ctx, json)
}
