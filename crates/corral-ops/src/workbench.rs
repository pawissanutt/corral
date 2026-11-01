//! Bounded, read-only projection for Pi inspection. No full task/history crosses the UI seam.
//! Freshness is delegated to completion/verification, never inferred from a displayed SHA.
use anyhow::{Context, Result, bail};
use corral_core::{GateResult, ObservationStatus, RunOutcome, Task, Verdict};
use serde::Serialize;

use crate::{Ctx, completion, verification};

pub const MAX_CRITERIA: usize = 100;
pub const MAX_EVIDENCE: usize = 2000;
pub const MAX_ARTIFACTS: usize = 500;
pub const MAX_RESPONSE_BYTES: usize = 128 * 1024;

#[derive(Debug, Serialize)]
pub struct Inspection {
    pub version: u32,
    pub task: String,
    pub run: String,
    pub root: std::path::PathBuf,
    pub base: String,
    pub head: String,
    pub dirty: bool,
    pub manifest_digest: Option<String>,
    pub audited: bool,
    pub manifest_failures: Vec<String>,
    pub criteria: Vec<CriterionRow>,
    pub gate: Option<GateRow>,
    pub review: Option<ReviewRow>,
    pub findings: Vec<FindingRow>,
    pub findings_omitted: usize,
    pub artifacts: Vec<ArtifactRow>,
    pub artifacts_omitted: usize,
    pub worker_summary: Option<String>,
    /// A task lifecycle label, deliberately not a criterion acceptance boolean.
    pub task_status: corral_core::Status,
}

#[derive(Debug, Serialize)]
pub struct CriterionRow {
    pub id: String,
    pub required: bool,
    pub live: bool,
    pub scenario: String,
    pub expected: String,
    pub implemented: bool,
    pub observation: Option<ObservationRow>,
    pub failures: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ObservationRow {
    pub key: String,
    pub authority: &'static str,
    pub status: ObservationStatus,
    pub observed: String,
    pub action: String,
    pub environment: String,
    pub captured: String,
    pub head: String,
    pub manifest_digest: String,
    pub artifacts: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ArtifactRow {
    pub path: String,
    pub title: String,
    pub sha256: String,
}

#[derive(Debug, Serialize)]
pub struct GateRow {
    pub result: GateResult,
    pub tier: Option<String>,
    pub fresh: bool,
    pub log_passed: bool,
    pub head: String,
    pub attempt: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReviewRow {
    pub verdict: Option<Verdict>,
    pub fresh: bool,
    pub head: String,
    pub reviewer: String,
    pub notes: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FindingRow {
    pub run: String,
    pub attempt: String,
    pub head: String,
    pub verdict: Option<Verdict>,
    pub superseded: bool,
    pub notes: String,
}

fn excerpt(text: &str) -> String {
    let mut chars = text.chars();
    let mut value: String = chars.by_ref().take(1500).collect();
    if chars.next().is_some() {
        value.push_str("… [excerpt]");
    }
    value
}

/// Explicit run selection prevents a resumed host from silently inspecting a newer run.
/// One store read per refresh; oversize inputs fail closed rather than fabricate empty results.
pub fn inspect(ctx: &Ctx, id: &str, run_id: &str) -> Result<Inspection> {
    let task = ctx.store.get(id)?;
    let run = task
        .runs
        .iter()
        .find(|r| r.id == run_id)
        .context("inspection_run_missing")?;
    let criteria = task.completion.as_ref().map_or(0, |m| m.criteria.len());
    let artifacts = task.artifacts.iter().filter(|a| a.run == run.id).count();
    if criteria > MAX_CRITERIA
        || run.completion.evidence.len() > MAX_EVIDENCE
        || artifacts > MAX_ARTIFACTS
    {
        bail!(
            "inspection_limit: at most {MAX_CRITERIA} criteria, {MAX_EVIDENCE} observations and {MAX_ARTIFACTS} run artifacts"
        );
    }
    let identity = completion::current_identity(ctx, &task, run)?;
    let current = completion::Admission {
        identity: &identity,
        start: completion::attempt_start(run),
        end: None,
    };
    let carry = completion::evidence_carry::live(&task, run, &identity)?;
    let carried = carry.map(|c| completion::Admission {
        identity: &c.from,
        start: c.window_start,
        end: Some(c.at),
    });
    let failures =
        completion::inspection_failures(ctx, &task, run, &current, carried.as_ref(), true)?;
    let manifest = task.completion.as_ref();
    let manifest_failures = completion::manifest_failures(&task, run)?;
    let rows = manifest
        .into_iter()
        .flat_map(|m| &m.criteria)
        .map(|criterion| {
            let record = run
                .completion
                .evidence
                .iter()
                .rev()
                .find(|e| e.observation.criterion == criterion.id);
            CriterionRow {
                id: criterion.id.clone(),
                required: criterion.required,
                live: criterion.live,
                scenario: excerpt(&criterion.scenario),
                expected: excerpt(&criterion.expected),
                implemented: run
                    .progress
                    .as_ref()
                    .is_some_and(|p| p.criterion_complete(&criterion.id)),
                observation: record.map(|r| ObservationRow {
                    key: r.observation.key.clone(),
                    authority: "worker_assertion",
                    status: r.observation.status,
                    observed: excerpt(&r.observation.observed),
                    action: excerpt(&r.observation.action),
                    environment: excerpt(&r.observation.environment),
                    captured: r.observation.captured.to_string(),
                    head: r.identity.head.clone(),
                    manifest_digest: r.manifest_digest.clone(),
                    artifacts: r.observation.artifacts.clone(),
                }),
                failures: failures
                    .iter()
                    .filter(|f| f.split_once(':').is_some_and(|(_, id)| id == criterion.id))
                    .cloned()
                    .collect(),
            }
        })
        .collect();
    let gate = run
        .gate
        .as_ref()
        .map(|g| -> Result<GateRow> {
            Ok(GateRow {
                result: g.result,
                tier: g.tier.as_ref().map(|t| t.tier.clone()),
                fresh: verification::gate_fresh_for_review(ctx, &task, run, g)?,
                log_passed: crate::gate::gate_log_passed(ctx, &task, run)?,
                head: g.sha.clone(),
                attempt: g.identity.as_ref().map(|i| i.attempt.clone()),
            })
        })
        .transpose()?;
    let review = run
        .review
        .as_ref()
        .map(|r| -> Result<ReviewRow> {
            Ok(ReviewRow {
                verdict: r.verdict,
                fresh: verification::review_fresh(ctx, &task, run)?,
                head: r.sha.clone(),
                reviewer: excerpt(&r.reviewer),
                notes: r.notes.as_deref().map(excerpt),
            })
        })
        .transpose()?;
    let (findings, findings_omitted) = findings(&task);
    let inspection = Inspection {
        version: 1,
        task: task.id.clone(),
        run: run.id.clone(),
        root: run.worktree.clone(),
        base: corral_git::merge_base(&run.worktree, &identity.integration_base, &identity.head)?,
        head: identity.head,
        dirty: !corral_git::is_clean(&run.worktree)?,
        manifest_digest: manifest.map(completion::manifest_digest).transpose()?,
        audited: manifest.is_some_and(|m| m.audited),
        manifest_failures,
        criteria: rows,
        gate,
        review,
        findings,
        findings_omitted,
        artifacts: task
            .artifacts
            .iter()
            .filter(|a| a.run == run.id)
            .take(100)
            .map(|a| ArtifactRow {
                path: a.path.clone(),
                title: excerpt(&a.title),
                sha256: a.sha256.clone(),
            })
            .collect(),
        artifacts_omitted: artifacts.saturating_sub(100),
        worker_summary: run.summary.as_deref().map(excerpt),
        task_status: task.status,
    };
    if serde_json::to_vec(&inspection)?.len() > MAX_RESPONSE_BYTES {
        bail!("inspection_limit: projection exceeds {MAX_RESPONSE_BYTES} bytes");
    }
    Ok(inspection)
}

fn findings(task: &Task) -> (Vec<FindingRow>, usize) {
    let mut rows = Vec::new();
    let mut count: usize = 0;
    for run in task.runs.iter().rev() {
        for child in run.review_runs.iter().rev() {
            let Some(review) = &child.review else {
                continue;
            };
            let Some(notes) = &review.notes else { continue };
            count += 1;
            if rows.len() < 20 {
                rows.push(FindingRow {
                    run: run.id.clone(),
                    attempt: child.id.clone(),
                    head: review.sha.clone(),
                    verdict: review.verdict,
                    superseded: child.outcome == Some(RunOutcome::Superseded)
                        || run.outcome == Some(RunOutcome::Superseded),
                    notes: excerpt(notes),
                });
            }
        }
    }
    (rows, count.saturating_sub(20))
}

#[cfg(test)]
#[path = "workbench_tests.rs"]
mod tests;
