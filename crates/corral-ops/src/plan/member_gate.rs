//! `corral plan member-gate`: the owner's revision-guarded change of a published member's gate.
//! The member's current tiered run gets the new review-phase commands as its completion command
//! snapshot before the tracker saves the record, so a crash in between leaves `done` refusing
//! with `completion_policy_changed` until the owner's retry lands the record.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use corral_core::{
    FULL_TIER, MemberGateAmendment, MemberGateRecord, PlanDeliveryTarget, ScopeGate, Status, Store,
    Task,
};
use serde::Serialize;

use super::scope_gate::{self, ScopeGateError};
use crate::Ctx;

#[cfg(test)]
#[path = "member_gate_tests.rs"]
mod tests;

pub struct MemberGateRequest<'a> {
    pub member: &'a str,
    /// When given, the member must belong to this tracker.
    pub tracker: Option<&'a str>,
    pub file: &'a Path,
    pub expected_revision: u64,
    pub reason: &'a str,
}

/// Where the amendment was entered from; only an owner entry may amend.
pub struct OwnerEntry {
    pub agent: bool,
    pub pm: bool,
    pub cwd: PathBuf,
}

impl OwnerEntry {
    /// # Errors
    /// The current directory cannot be read.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            agent: std::env::var_os("AI_AGENT").is_some()
                || std::env::var_os("PI_CODING_AGENT").is_some(),
            pm: std::env::var("CORRAL_PM").as_deref() == Ok("1"),
            cwd: std::env::current_dir().context("reading member-gate amendment directory")?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MemberGateOutcome {
    pub tracker: String,
    pub member: String,
    pub source: String,
    pub record: MemberGateRecord,
    pub reconciled_run: Option<String>,
}

/// Amends a published member's gate as the owner of this OS session.
///
/// # Errors
/// Refuses PM, agent and run-worktree entries, and every amendment [`plan_member_gate_from`]
/// refuses.
pub fn plan_member_gate(ctx: &Ctx, request: &MemberGateRequest) -> Result<MemberGateOutcome> {
    plan_member_gate_from(ctx, request, &OwnerEntry::from_env()?)
}

/// [`plan_member_gate`] with an explicit entry, for tests that run under an agent's environment.
///
/// # Errors
/// Refuses a non-owner entry, an unreadable or invalid gate file, a blank reason, a stale
/// revision, a member that is not in exactly one integration tracker (or not in `tracker`), an
/// unpublished tracker, an undefined tier, and a gate equal to the member's current one.
#[doc(hidden)]
pub fn plan_member_gate_from(
    ctx: &Ctx,
    request: &MemberGateRequest,
    entry: &OwnerEntry,
) -> Result<MemberGateOutcome> {
    crate::automation_controls::require_owner_entry(&ctx.store, &entry.cwd, entry.agent, entry.pm)
        .context("corral plan member-gate is owner-only")?;
    let gate = read_gate(ctx, request.file)?;
    let member = ctx.store.get(request.member)?;
    let tracker = owning_tracker(&ctx.store, &member)?;
    if let Some(named) = request.tracker {
        let named = ctx.store.get(named)?;
        ensure!(
            named.id == tracker.id,
            "{} is a member of {}, not {}",
            member.id,
            tracker.id,
            named.id
        );
    }
    let amendment = MemberGateAmendment {
        member: member.id.clone(),
        expected_revision: request.expected_revision,
        gate,
        reason: request.reason.to_owned(),
        by: "owner".into(),
    };
    let amended = ctx.store.amend_member_gate(
        &tracker.id,
        &amendment,
        |store, tracker| validate(store, tracker, &amendment.member, &amendment.gate),
        |store, task| reconcile_snapshot(store, task, &amendment.gate),
    )?;
    Ok(MemberGateOutcome {
        source: format!(
            "member-gate:{}@{}",
            amended.tracker.id, amended.record.revision
        ),
        tracker: amended.tracker.id,
        member: amendment.member,
        record: amended.record,
        reconciled_run: amended.reconciled_run,
    })
}

fn read_gate(ctx: &Ctx, path: &Path) -> Result<ScopeGate> {
    let bytes = super::read_plan(ctx, path)?;
    let text = std::str::from_utf8(&bytes).context("member gate file must be UTF-8")?;
    let gate: ScopeGate = serde_yaml_ng::from_str(text)
        .with_context(|| format!("parsing member gate {}", path.display()))?;
    gate.validate()?;
    Ok(gate)
}

fn owning_tracker(store: &Store, member: &Task) -> Result<Task> {
    let tracker = store
        .execution_plan(member)?
        .with_context(|| format!("{} is not a member of an approved Plan", member.id))?;
    ensure!(
        tracker.id != member.id,
        "{} is a tracker, not a member",
        member.id
    );
    Ok(tracker)
}

fn validate(store: &Store, tracker: &Task, member: &str, gate: &ScopeGate) -> Result<()> {
    let task = store.get(member)?;
    ensure!(
        owning_tracker(store, &task)?.id == tracker.id,
        "{member} is no longer a member of {}",
        tracker.id
    );
    let scope = tracker
        .approved_scope
        .as_ref()
        .context("Plan scope missing")?;
    ensure!(
        matches!(
            scope.delivery_target,
            PlanDeliveryTarget::Integration { .. }
        ),
        "{} delivers to main; member gates apply only to integration Plans",
        tracker.id
    );
    if let Some(parent) = &tracker.parent {
        let design = store.get(parent)?;
        ensure!(
            design.status == Status::Done && design.children == tracker.children,
            "cannot amend a member gate before decomposition publication completes"
        );
    }
    let tiers = store
        .config
        .review_policy(tracker.project.as_deref(), None)
        .gate_tiers;
    ensure!(
        gate.tier == FULL_TIER || tiers.contains_key(&gate.tier),
        "gate tier {:?} is not defined for project {}",
        gate.tier,
        tracker.project.as_deref().unwrap_or("(none)")
    );
    // A member whose published binding cannot be resolved has no current gate to compare.
    let current = match scope_gate::resolve(store, &task) {
        Ok(current) => current,
        Err(error) if error.downcast_ref::<ScopeGateError>().is_some() => None,
        Err(error) => return Err(error),
    };
    if let Some(current) = current {
        ensure!(
            current.gate != *gate,
            "no-op member-gate amendment: {member} already has this gate (source {})",
            current.source
        );
    }
    Ok(())
}

/// Points the member's current tiered run at the new review-phase commands and clears a stale
/// audit. Idempotent: an equal snapshot is left alone. Returns the run when it is tiered.
pub(crate) fn reconcile_snapshot(
    store: &Store,
    task: &mut Task,
    gate: &ScopeGate,
) -> Result<Option<String>> {
    let policy = store.config.task_review_policy(task);
    let Some(run) = task.current_run_mut() else {
        return Ok(None);
    };
    if run.gate_rules.is_legacy() || run.plan_target.is_none() || policy.gate_tiers.is_empty() {
        return Ok(None);
    }
    let commands = crate::gate_tier::tier_commands(&policy, &gate.tier)
        .with_context(|| format!("gate tier {:?} is not defined", gate.tier))?;
    if run.completion.required_checks.as_ref() != Some(&commands) {
        run.completion.required_checks = Some(commands);
        run.completion.audit = None;
    }
    Ok(Some(run.id.clone()))
}
