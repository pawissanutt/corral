//! Evidence/routing metadata, never a replacement for a gate receipt. Shared by board and co-work.
use anyhow::{Context, Result, ensure};
use corral_core::{GateRecord, GateResult, Store, TaskRef as _};
use serde::{Deserialize, Serialize};

use crate::{Ctx, verification};

const AREA: &str = "gate-triage";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Candidate,
    RedBase,
    Environment,
    Flaky,
    Unknown,
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    pub task: String,
    pub run: String,
    pub attempt: String,
}

/// Attribution must cite retained evidence. Red-base attribution additionally proves every
/// failing command against a canonical receipt from a clean baseline gate, not a log label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateDiagnosis {
    pub attempt: String,
    pub class: FailureClass,
    pub explanation: String,
    pub evidence: Vec<String>,
    #[serde(default)]
    pub baseline: Option<Baseline>,
    #[serde(default)]
    pub retry: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureEpisode {
    pub gate: GateRecord,
    pub diagnoses: Vec<GateDiagnosis>,
    /// A collected, acceptable PASS closes this episode, not its retained failure facts or
    /// the task's consumed diagnostic allowance. Old records remain active until observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<GateRecord>,
}

impl FailureEpisode {
    fn class(&self) -> FailureClass {
        self.diagnoses
            .last()
            .map_or(FailureClass::Unknown, |d| d.class)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GateTriage {
    pub failures: Vec<FailureEpisode>,
    /// At most one diagnostic launch before a visible block. Neither a new run/candidate nor
    /// a driver transfer clears it. Admission and its attempt commit together with GateStart.
    pub diagnostic_attempt: Option<String>,
    #[serde(default)]
    pub diagnostic_history: Vec<String>,
    #[serde(default)]
    pub owner_investigations: Vec<String>,
    pub retry_requested: bool,
}

impl GateTriage {
    fn active_failures(&self) -> impl Iterator<Item = &FailureEpisode> {
        self.failures
            .iter()
            .filter(|entry| entry.resolved_by.is_none())
    }
}

pub fn status(ctx: &Ctx, id: &str) -> Result<GateTriage> {
    Ok(ctx
        .store
        .record(AREA, &Store::normalize_id(id)?)?
        .unwrap_or_default())
}

pub(crate) fn observe(store: &Store, id: &str, gate: &GateRecord) -> Result<GateTriage> {
    let mut state: GateTriage = store.record(AREA, id)?.unwrap_or_default();
    let mut changed = false;
    // A stale receipt has no verdict. Release only the matching launch, including when
    // observing a receipt retained by an older version; replay cannot release its successor.
    if gate.result == GateResult::Stale
        && gate.identity.as_ref().is_some_and(|identity| {
            state.diagnostic_attempt.as_deref() == Some(identity.attempt.as_str())
        })
    {
        state.diagnostic_attempt = None;
        state.retry_requested = true;
        changed = true;
    }
    if gate.result == GateResult::Fail && !state.failures.iter().any(|e| e.gate == *gate) {
        state.failures.push(FailureEpisode {
            gate: gate.clone(),
            diagnoses: Vec::new(),
            resolved_by: None,
        });
        changed = true;
    } else if gate.result == GateResult::Pass && require_state_pass(&state, Some(gate)).is_ok() {
        for entry in state
            .failures
            .iter_mut()
            .filter(|entry| entry.resolved_by.is_none())
        {
            entry.resolved_by = Some(gate.clone());
            changed = true;
        }
    }
    if changed {
        store.save_record(AREA, id, &state)?;
    }
    Ok(state)
}

fn unresolved(state: &GateTriage) -> bool {
    state.active_failures().any(|entry| {
        matches!(
            entry.class(),
            FailureClass::Unknown | FailureClass::Mixed | FailureClass::Flaky
        )
    })
}

/// A diagnostic green does not erase an unexplained red. A candidate defect needs a changed
/// candidate as well as a fresh successful receipt. Base/environment metadata never grants PASS.
pub(crate) fn require_pass(store: &Store, id: &str, gate: Option<&GateRecord>) -> Result<()> {
    let state: GateTriage = store.record(AREA, id)?.unwrap_or_default();
    require_state_pass(&state, gate)
}

fn require_state_pass(state: &GateTriage, gate: Option<&GateRecord>) -> Result<()> {
    ensure!(
        !unresolved(state),
        "gate diagnosis unresolved (unknown/mixed/flaky); a green rerun is not acceptance evidence"
    );
    if !state.failures.is_empty() {
        let gate = gate.context("a fresh successful candidate gate is required after diagnosis")?;
        ensure!(
            gate.result == GateResult::Pass,
            "a fresh successful candidate gate is required after diagnosis"
        );
        ensure!(
            state
                .failures
                .iter()
                .all(|entry| entry.gate.identity.as_ref().map(|i| &i.attempt)
                    != gate.identity.as_ref().map(|i| &i.attempt)),
            "diagnosis cannot turn a failed receipt into PASS"
        );
        ensure!(state.active_failures().all(|entry| entry.class() != FailureClass::Candidate || entry.gate.sha != gate.sha),
            "candidate findings require a repaired candidate, not rerun-until-green");
    }
    Ok(())
}

pub(crate) fn check_retry(
    store: &Store,
    id: &str,
    previous: Option<&GateRecord>,
    head: &str,
    attempt: Option<&str>,
) -> Result<bool> {
    let state: GateTriage = store.record(AREA, id)?.unwrap_or_default();
    let active = state.active_failures().next().is_some();
    let repaired = active
        && state
            .active_failures()
            .all(|entry| entry.class() == FailureClass::Candidate && entry.gate.sha != head);
    let passed = previous.is_some_and(|gate| gate.result == GateResult::Pass)
        && require_state_pass(&state, previous).is_ok();
    let diagnostic = active && !repaired && !passed;
    if diagnostic {
        ensure!(
            state
                .diagnostic_attempt
                .as_deref()
                .is_none_or(|old| Some(old) == attempt),
            "gate diagnosis retry exhausted; inspect retained failures and resolve attribution before another gate"
        );
    }
    Ok(diagnostic)
}

/// Called inside GateStart's transaction, before replacing the previous receipt.
pub(crate) fn admit(
    store: &Store,
    id: &str,
    previous: Option<&GateRecord>,
    head: &str,
    attempt: &str,
) -> Result<()> {
    let mut state = match previous {
        Some(gate) => observe(store, id, gate)?,
        None => store.record(AREA, id)?.unwrap_or_default(),
    };
    if check_retry(store, id, previous, head, Some(attempt))? {
        state.diagnostic_attempt = Some(attempt.into());
        if !state.diagnostic_history.iter().any(|old| old == attempt) {
            state.diagnostic_history.push(attempt.into());
        }
        state.retry_requested = false;
        store.save_record(AREA, id, &state)?;
    }
    Ok(())
}

pub(crate) enum Route {
    Repair,
    Retry,
    Block(String, bool),
}

pub(crate) fn route(store: &Store, id: &str, gate: &GateRecord) -> Result<Route> {
    let state = observe(store, id, gate)?;
    if state.retry_requested && state.diagnostic_attempt.is_none() {
        return Ok(Route::Retry);
    }
    if unresolved(&state) {
        return Ok(Route::Block("gate failure needs bounded diagnosis (unknown/mixed/flaky); no code-repair round spent. Inspect `corral repair triage-status` and record evidence with `corral repair triage`. One diagnostic retry is available; green alone cannot clear the failure".into(), true));
    }
    let class = state
        .failures
        .iter()
        .find(|e| e.gate == *gate)
        .map(FailureEpisode::class);
    Ok(match class {
        Some(FailureClass::Candidate) => Route::Repair,
        _ => Route::Block("proven base/environment failure held without a code-repair charge; retain evidence, correct the cause, then request a fresh candidate gate through `corral repair triage`".into(), false),
    })
}

fn require_driver(task: &impl corral_core::TaskRef, owner: bool) -> Result<()> {
    if let crate::assistant::owner_action::OwnerAuthority::ViaAssistant(via) =
        crate::assistant::owner_action::authority()
    {
        ensure!(
            !owner
                && task
                    .cowork_run()
                    .and_then(|cowork| cowork.assistant_driver())
                    .is_some_and(|(session, _)| session == via.session),
            "diagnosis belongs to the current assistant driver; owner investigation cannot be inferred from Ship"
        );
    }
    Ok(())
}

/// Record diagnosis and queue the next existing pipeline stage. No worker, reviewer or gate
/// is launched here. The caller's normal diagnosis authority and every live stop still apply.
pub fn diagnose(ctx: &Ctx, id: &str, report: GateDiagnosis, owner: bool) -> Result<GateTriage> {
    let id = Store::normalize_id(id)?;
    if owner {
        crate::require_automation_owner(&ctx.store)?;
    }
    let _pilot = ctx
        .store
        .try_lock(&format!("autopilot-{id}"))?
        .context("autopilot is advancing this task")?;
    let head = ctx.store.head(&id)?;
    require_driver(&head, owner)?;
    crate::operating::require_diagnosis(&ctx.store, &head, owner)?;
    ensure!(
        !report.explanation.trim().is_empty() && !report.evidence.is_empty(),
        "diagnosis requires explanation and retained evidence"
    );
    let task = ctx.store.get(&id)?;
    for reference in &report.evidence {
        let artifact = task
            .artifacts
            .iter()
            .find(|a| a.path == *reference)
            .context("diagnosis evidence is not attached to this task")?;
        crate::artifact_read(ctx, artifact)?;
    }
    let run = head
        .current_run_head()?
        .context("diagnosis needs a current run")?
        .into_owned();
    ensure!(
        matches!(
            head.status,
            corral_core::Status::Review | corral_core::Status::NeedsMe
        ) && run.outcome == Some(corral_core::RunOutcome::Done)
            && run.gate.as_ref().is_none_or(|gate| !gate.holds_slot())
            && run
                .review
                .as_ref()
                .is_none_or(|review| review.verdict.is_some()),
        "diagnosis requires a settled candidate, not an active writer/check"
    );
    let mut state = match run.gate.as_ref() {
        Some(gate) => observe(&ctx.store, &id, gate)?,
        None => status(ctx, &id)?,
    };
    let entry = state
        .failures
        .iter_mut()
        .find(|entry| {
            entry
                .gate
                .identity
                .as_ref()
                .is_some_and(|identity| identity.attempt == report.attempt)
        })
        .context("diagnosis attempt is not a retained failed receipt")?;
    if entry.diagnoses.contains(&report) {
        return Ok(state);
    }
    ensure!(
        entry.diagnoses.is_empty() || owner,
        "diagnosis already recorded; a changed attribution requires explicit owner investigation"
    );
    if report.class == FailureClass::RedBase {
        prove_baseline(
            ctx,
            &entry.gate,
            report
                .baseline
                .as_ref()
                .context("red-base diagnosis requires baseline receipt")?,
        )?;
    }
    if report.retry {
        ensure!(
            run.gate
                .as_ref()
                .is_some_and(|gate| gate.result == GateResult::Fail
                    && gate
                        .identity
                        .as_ref()
                        .is_some_and(|identity| identity.attempt == report.attempt)),
            "retry diagnosis must name the current failed attempt, not a newer passing/running candidate"
        );
        ensure!(
            report.class != FailureClass::Candidate,
            "candidate defects require repair, not diagnostic reruns"
        );
        ensure!(
            state.diagnostic_attempt.is_none() || owner,
            "single diagnostic retry already consumed; another bounded investigation requires explicit owner evidence"
        );
        if owner {
            state.diagnostic_attempt = None;
        }
        if report.class == FailureClass::RedBase {
            let base = corral_git::rev_sha(&run.worktree, &run.base)?;
            ensure!(
                entry
                    .gate
                    .identity
                    .as_ref()
                    .is_some_and(|i| i.integration_base != base),
                "red base has not moved; wait for base repair before refreshing the candidate"
            );
        }
        state.retry_requested = true;
    }
    entry.diagnoses.push(report.clone());
    // A changed owner attribution must be validated again; an old PASS cannot explain a
    // newly unknown/mixed/flaky episode or an unchanged candidate defect.
    entry.resolved_by = None;
    state.retry_requested = report.retry;
    if owner {
        state
            .owner_investigations
            .push(verification::digest(&serde_json::to_vec(&report)?));
    }
    ctx.store.dispatch_update_head(&id, |store, current| {
        require_driver(&*current, owner)?;
        crate::operating::require_diagnosis(store, &*current, owner)?;
        ensure!(
            current.current_run_head()?.as_deref() == Some(&run),
            "diagnosis candidate changed"
        );
        store.save_record(AREA, &id, &state)?;
        current.status = corral_core::Status::Review;
        current
            .current_run_mut()
            .context("run disappeared")?
            .autopilot = Some(corral_core::AutopilotState {
            stage: corral_core::AutoStage::Gating,
            reason: Some(report.explanation.clone()),
            sha: Some(
                run.gate
                    .as_ref()
                    .map_or_else(String::new, |g| g.sha.clone()),
            ),
            at: store.now(),
        });
        store.log_event(Some(&id), "gate_diagnosis", serde_json::to_value(&report)?)?;
        Ok(())
    })?;
    Ok(state)
}

fn prove_baseline(ctx: &Ctx, failed: &GateRecord, baseline: &Baseline) -> Result<()> {
    let candidate = failed
        .identity
        .as_ref()
        .context("failed receipt has no identity")?;
    let task = ctx.store.get(&baseline.task)?;
    let run = task
        .runs
        .iter()
        .find(|r| r.id == baseline.run)
        .context("baseline run missing")?;
    let gate = run.gate.as_ref().context("baseline gate missing")?;
    let identity = gate
        .identity
        .as_ref()
        .context("baseline identity missing")?;
    ensure!(
        identity.attempt == baseline.attempt
            && gate.result == GateResult::Fail
            && identity.head == candidate.integration_base
            && identity.environment == candidate.environment
            && identity.gate_tier == candidate.gate_tier,
        "baseline does not match exact base/environment/command contract"
    );
    let candidate_task = ctx.store.head(&candidate.task)?;
    ensure!(
        task.project == candidate_task.project,
        "baseline belongs to another project"
    );
    let repo = ctx.store.project_path(
        task.project
            .as_deref()
            .context("baseline project missing")?,
    )?;
    ensure!(
        run.worktree != repo
            && candidate_task
                .current_run
                .as_ref()
                .is_none_or(|candidate| candidate.worktree != run.worktree),
        "baseline diagnosis requires a separate isolated checkout, never main or the candidate checkout"
    );
    let expected = signatures(ctx, failed)?;
    let observed = signatures(ctx, gate)?;
    ensure!(
        !expected.is_empty() && expected.iter().all(|failure| observed.contains(failure)),
        "baseline does not explain every candidate failure; preserve mixed/unknown attribution"
    );
    Ok(())
}

fn signatures(ctx: &Ctx, gate: &GateRecord) -> Result<Vec<(String, i32, String)>> {
    let identity = gate.identity.as_ref().context("gate identity missing")?;
    let bytes =
        crate::gate::evidence::attempt_log(ctx, &identity.task, &identity.run, &identity.attempt)?
            .context("retained gate log missing or corrupt")?;
    ensure!(
        gate.log_sha256.as_deref() == Some(&verification::digest(&bytes)),
        "retained gate log digest changed"
    );
    gate.commands
        .iter()
        .filter(|command| command.exit_code != 0)
        .map(|command| {
            let start = usize::try_from(command.log_start)?;
            let end = usize::try_from(command.log_end)?;
            let output = bytes
                .get(start..end)
                .context("failed command log bounds invalid")?;
            Ok((
                command.command.clone(),
                command.exit_code,
                verification::digest(output),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests;
