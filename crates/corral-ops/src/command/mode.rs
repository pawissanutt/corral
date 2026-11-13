//! `mode` command executors and shared local/routed policy printers.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use corral_core::{
    Limits, OperatingAuthority, OperatingMode, OperatingPreview, OperatingSelection,
};
use corral_protocol::cmd::mode::ModeCommand;

use super::{Exec, Output, Verb};
use crate::{Ctx, SlotUsage};

impl Verb for ModeCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        match self {
            Self::Assist {} => {
                cx.ctx.store.config = cx.ctx.store.assist_operating_policy()?;
            }
            Self::Pause {} => {
                cx.ctx.store.config = cx.ctx.store.pause_operating_policy(true)?;
            }
            Self::Resume {} => {
                crate::require_automation_owner(&cx.ctx.store, cx.invoker)?;
                cx.ctx.store.config = cx.ctx.store.pause_operating_policy(false)?;
            }
            Self::Turbo { off } => {
                if !off {
                    crate::require_automation_owner(&cx.ctx.store, cx.invoker)?;
                }
                cx.ctx.store.config = cx.ctx.store.turbo_operating_policy(!off)?;
            }
            Self::Autopilot {
                projects,
                include_new,
                expires_after_days,
                max_new_tasks,
                tasks,
                exclusions,
                plan_scopes,
                dispatch,
                accept,
                plan_accept,
                aggregate_accept,
                confirm,
                json,
            } => {
                let mut scopes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                for scope in plan_scopes {
                    let (task, label) = scope
                        .split_once(':')
                        .context("--plan-scope needs TASK:LABEL")?;
                    ensure!(!label.is_empty(), "plan scope label must not be empty");
                    scopes
                        .entry(corral_core::Store::normalize_id(task)?)
                        .or_default()
                        .insert(label.to_owned());
                }
                let selection = OperatingSelection {
                    projects: projects.into_iter().collect(),
                    tasks: tasks.into_iter().collect(),
                    exclusions: exclusions.into_iter().collect(),
                    plan_scopes: scopes,
                    authority: OperatingAuthority {
                        dispatch,
                        accept,
                        plan_accept,
                        aggregate_accept,
                    },
                    include_new,
                    expires_after_days,
                    max_new_tasks,
                };
                let preview = crate::automation_preview(&cx.ctx.store, &selection)?;
                ensure!(
                    confirm == preview.id,
                    "preview changed; review a fresh mode autopilot preview"
                );
                cx.ctx.store.config =
                    crate::automation_confirm(&cx.ctx.store, &preview, &confirm, cx.invoker)?;
                if json {
                    return status(&cx.ctx, true, &mut cx.out);
                }
            }
            Self::Off { json } => {
                cx.ctx.store.config = cx.ctx.store.pause_operating_policy(true)?;
                if !json {
                    cx.out.line(format_args!(
                        "Workspace Autopilot paused across dispatch, review, repair and acceptance."
                    ));
                }
                return status(&cx.ctx, json, &mut cx.out);
            }
            Self::Other => return Err(super::unknown_command("other")),
        }
        status(&cx.ctx, false, &mut cx.out)
    }
}

pub fn status(ctx: &Ctx, json: bool, out: &mut Output) -> Result<()> {
    let workset = crate::pm_workset(ctx)?;
    let effective = ctx.store.effective_operating_policy()?;
    let config = ctx.store.config_layers()?.config()?;
    if json {
        let mut value = serde_json::to_value(&workset)?;
        value["effective"] = serde_json::to_value(&effective)?;
        value["policy"] = serde_json::to_value(&config.automation)?;
        value["approval"] = serde_json::to_value(ctx.store.operating_state()?.approval)?;
        out.line(format_args!("{}", serde_json::to_string_pretty(&value)?));
        return Ok(());
    }
    out.line(format_args!(
        "Mode: {}{}",
        match workset.mode {
            OperatingMode::Assist => "Assist",
            OperatingMode::Autopilot => "Autopilot",
        },
        if workset.paused { " (paused)" } else { "" }
    ));
    if config.automation.turbo {
        out.line(format_args!(
            "Turbo: on (owner-only acceptance and round limits ignored; `corral mode turbo --off` stops it)"
        ));
    }
    out.line(format_args!(
        "{}: {}/{} accepted; {} scoped running; independent work continues: {}",
        workset.status.as_str(),
        workset.progress.accepted,
        workset.progress.total,
        workset.progress.running,
        workset.independent_work_continues,
    ));
    out.line(format_args!("Policy revision: {}", workset.policy_revision));
    out.line(format_args!(
        "Grant: {}",
        effective.authorization.as_deref().unwrap_or("none")
    ));
    out.line(format_args!(
        "Configured authority (subject to effective refusals): dispatch={}, accept={}, plan_accept={}, aggregate_accept={}",
        config.automation.authority.dispatch,
        config.automation.authority.accept,
        config.automation.authority.plan_accept,
        config.automation.authority.aggregate_accept,
    ));
    for (id, member) in &config.automation.scope.tasks {
        out.line(format_args!(
            "Scope {id}: {} ({})",
            member.project,
            member.repository.display()
        ));
        if let Some(task) = effective.tasks.get(id) {
            for (action, reasons) in &task.refusals {
                out.line(format_args!("  {action:?}: {}", reasons.join("; ")));
            }
        }
    }
    for project in config.automation.scope.projects.keys() {
        out.line(format_args!(
            "Project {project}: unfinished and new tasks{}",
            project_limits(&config.automation.scope)
        ));
    }
    for excluded in &config.automation.scope.exclusions {
        out.line(format_args!("Excluded: {excluded}"));
    }
    for decision in &workset.decisions {
        out.line(format_args!(
            "Decision {} ({}): {}",
            decision.id, decision.task, decision.action.why
        ));
    }
    for wait in &workset.waits {
        out.line(format_args!(
            "Waiting {}: {}",
            wait.task.as_deref().unwrap_or("workspace"),
            wait.why
        ));
    }
    for blocker in &workset.blockers {
        out.line(format_args!("Blocker: {blocker}"));
    }
    if workset.paused {
        out.line(format_args!(
            "No new automatic actions; running work and receipt collection continue."
        ));
    }
    dispatch_status(ctx, out)?;
    Ok(())
}

fn project_limits(scope: &corral_core::OperatingScope) -> String {
    let mut limits = String::new();
    if let Some(days) = scope.expires_after_days {
        limits.push_str(&format!("; grant ends {days} days after confirmation"));
    }
    if let Some(max) = scope.max_new_tasks {
        limits.push_str(&format!("; at most {max} new tasks"));
    }
    limits
}

pub fn print_preview(preview: &OperatingPreview, out: &mut Output) -> Result<()> {
    if preview.policy.scope.projects.is_empty() {
        out.line(format_args!(
            "Autopilot preview: finite current tasks, never future backlog."
        ));
    }
    for project in preview.policy.scope.projects.keys() {
        out.line(format_args!(
            "Project {project}: every unfinished task, including tasks filed later{}",
            project_limits(&preview.policy.scope)
        ));
    }
    for (id, constraints) in &preview.constraints {
        if !preview.policy.scope.tasks.contains_key(id) {
            out.line(format_args!(
                "{id}: covered through its project; {}",
                constraints.lifecycle.as_str()
            ));
        }
    }
    for (id, member) in &preview.policy.scope.tasks {
        out.line(format_args!(
            "{id} @ {}: {}",
            member.project,
            member.repository.display()
        ));
        let constraints = &preview.constraints[id];
        out.line(format_args!(
            "  Owner-only acceptance: {}; owner-only plan acceptance: {}",
            constraints.owner_acceptance, constraints.owner_plan_acceptance,
        ));
        out.line(format_args!(
            "  State: {}; dispatch hold: {}; cancelled plan: {}; decisions: {}",
            constraints.lifecycle.as_str(),
            constraints.dispatch_hold,
            constraints.cancelled_plan,
            constraints.pending_decisions.len(),
        ));
        let review = &constraints.review;
        out.line(format_args!(
            "  Gates: {}",
            serde_json::to_string(&review.gate)?
        ));
        out.line(format_args!(
            "  Reviewer: {:?}; cross-family required: {}",
            review.reviewer,
            review.cross_family_required(),
        ));
        out.line(format_args!(
            "  Limits: {} repair rounds; source: {:?}",
            review.max_rounds, review.size_limits,
        ));
        out.line(format_args!(
            "  Owner paths: {}",
            serde_json::to_string(&review.owner_paths)?
        ));
    }
    for plan in &preview.policy.scope.plans {
        out.line(format_args!(
            "Reviewed plan {}@{} scopes: {}",
            plan.task,
            plan.revision,
            plan.scopes.keys().cloned().collect::<Vec<_>>().join(", "),
        ));
    }
    out.line(format_args!(
        "Exclusions: {}",
        serde_json::to_string(&preview.policy.scope.exclusions)?
    ));
    out.line(format_args!(
        "Delegated authority: dispatch={}, accept={}, plan_accept={}, aggregate_accept={}",
        preview.policy.authority.dispatch,
        preview.policy.authority.accept,
        preview.policy.authority.plan_accept,
        preview.policy.authority.aggregate_accept,
    ));
    out.line(format_args!(
        "Existing verification, owner restrictions and resource limits still apply."
    ));
    out.line(format_args!("Exact confirmation digest:\n{}", preview.id));
    out.line(format_args!(
        "Stored grant digest:\n{}",
        preview.grant_digest
    ));
    Ok(())
}

/// Legacy automation preview retains its JSON-and-digests presentation.
pub fn print_automation_preview(preview: &OperatingPreview, out: &mut Output) -> Result<()> {
    out.json_pretty(preview)?;
    out.line(format_args!("Exact confirmation digest: {}", preview.id));
    out.line(format_args!(
        "Stored grant digest: {}",
        preview.grant_digest
    ));
    Ok(())
}

fn dispatch_status(ctx: &Ctx, out: &mut Output) -> Result<()> {
    let config = &ctx.store.config;
    let usage = crate::slot_usage(ctx)?;
    let slots = slot_summary(&usage, &config.limits);
    out.line(format_args!("slots: {slots}"));
    out.line(format_args!(
        "dispatch: paused={}, auto_backlog={}, promote_ideas={:?}, daily cap={}",
        config.dispatch.paused,
        config.dispatch.auto_backlog,
        config.dispatch.promote_ideas,
        config.dispatch.max_auto_promotions_per_day
    ));
    for project in config.projects.keys() {
        let policy = config.dispatch_policy(Some(project));
        out.line(format_args!(
            "  {project}: auto_backlog={}, promote_ideas={:?}, daily cap={}",
            policy.auto_backlog, policy.promote_ideas, policy.max_auto_promotions_per_day
        ));
    }
    let automatic = crate::automatic_dispatch_status(ctx)?;
    out.line(format_args!(
        "automatic coding slots: {}; promotions today (UTC): {}",
        automatic.free_slots, automatic.promotions_today
    ));
    for event in automatic.recent {
        out.line(format_args!(
            "  policy {} {}: {}",
            event.ts,
            event.task.as_deref().unwrap_or("-"),
            event.detail
        ));
    }
    Ok(())
}

/// Shared full slot summary used by policy status and the board.
pub fn slot_summary(u: &SlotUsage, limits: &Limits) -> String {
    if let (Some(used), Some(cap)) = (&u.independent, &limits.capacity) {
        return format!(
            "agents {}/{} (normal {}/{}) · gates {}/{} · unfinished {}/{} (normal {}/{}) · ad-hoc reserve {}",
            used.agents.total(),
            cap.agents,
            used.agents.normal,
            cap.normal_agents(),
            used.gates,
            cap.gates,
            used.unfinished.total(),
            cap.unfinished,
            used.unfinished.normal,
            cap.normal_unfinished(),
            cap.adhoc_reserve
        );
    }
    let parts = slot_parts(u, limits, false);
    let used = format!("{}/{} used{}", u.total(), limits.running, soft(limits));
    if parts.is_empty() {
        used
    } else {
        format!("{used}: {}", parts.join(" · "))
    }
}

/// `" (soft target 2)"` while `limits.running_target` holds headroom below the hard limit.
fn soft(limits: &Limits) -> String {
    let target = crate::soft_target(limits);
    if target < limits.running {
        format!(" (soft target {target})")
    } else {
        String::new()
    }
}

/// The breakdown pieces, long (`"1 review"`, `"1 gate uncounted"`) or short (`"r1"`, `"(g1)"`).
fn slot_parts(u: &SlotUsage, limits: &Limits, short: bool) -> Vec<String> {
    let parts = [
        (u.tasks, "task", "tasks", 't', true),
        (u.reviews, "review", "reviews", 'r', limits.count_reviews),
        (u.gates, "gate", "gates", 'g', limits.count_gates),
        (u.reservations, "reserved", "reserved", 'q', true),
        (u.parked, "parked", "parked", 'p', false),
    ];
    parts
        .into_iter()
        .filter(|p| p.0 > 0)
        .map(|(n, one, many, letter, counted)| match (short, counted) {
            (true, true) => format!("{letter}{n}"),
            (true, false) => format!("({letter}{n})"),
            (false, c) => {
                let word = if n == 1 { one } else { many };
                format!("{n} {word}{}", if c { "" } else { " uncounted" })
            }
        })
        .collect()
}
