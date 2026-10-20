//! Candidate-bound review packets (PRF-08): the complete object under review, criteria,
//! decisions, raw receipt/artifact references and previous findings, with a digest that a
//! verdict must still match. Worker prose is carried as an assertion, never as a PASS.
use std::path::PathBuf;

use anyhow::{Context, Result};
use corral_core::{
    ArtifactKind, CandidateIdentity, Criterion, GateResult, IdentityChange, ObservationStatus,
    ReviewCheck, ReviewContractKind, ReviewRecord, ReviewRoute, Run, RunOutcome, Task, Verdict,
};
use serde::Serialize;

use crate::Ctx;
use crate::verification;

pub const REVIEW_PACKET_VERSION: u32 = 1;
const WORKER_ASSERTION: &str = "worker_assertion";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapCode {
    GateMissing,
    GateStale,
    GateFailed,
    GateLogContradicts,
    CriterionUnobserved,
    CriterionFailed,
    CriterionStale,
    VisualEvidenceMissing,
    OwnerDecisionPending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceGap {
    pub code: GapCode,
    pub detail: String,
}

impl EvidenceGap {
    /// Correctable proof goes back to the worker; only a genuine owner decision escalates.
    pub fn verdict(&self) -> Verdict {
        match self.code {
            GapCode::OwnerDecisionPending => Verdict::Escalate,
            _ => Verdict::Changes,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewPacket {
    pub digest: String,
    pub bound: BoundPacket,
}

/// Everything here participates in the digest; nothing depends on wall-clock time.
#[derive(Debug, Clone, Serialize)]
pub struct BoundPacket {
    pub version: u32,
    pub task: String,
    pub title: String,
    pub run: String,
    pub attempt: String,
    pub candidate: Option<CandidateIdentity>,
    pub goal: String,
    pub criteria: Vec<Criterion>,
    pub owner_decisions: Vec<String>,
    pub pending_owner_decision: bool,
    pub approved_plan: Option<String>,
    pub requirement: corral_core::ReviewRequirement,
    pub contract_digest: String,
    pub scope: corral_core::ReviewScope,
    pub round: u32,
    pub reviewer: ReviewerRef,
    pub object: ObjectRef,
    pub evidence: Evidence,
    pub gaps: Vec<EvidenceGap>,
    pub previous_findings: Vec<Finding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewerRef {
    pub profile: String,
    pub profile_revision: String,
    /// Owner-approved custom guidance; subordinate to the review contract.
    pub instructions: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObjectRef {
    pub base: String,
    pub head: String,
    pub diff_file: PathBuf,
    pub diff_sha256: String,
    pub changed_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub gate: Option<GateEvidence>,
    pub artifacts: Vec<ArtifactRef>,
    pub worker_summary: Option<Assertion>,
    pub criterion_observations: Vec<ObservationRef>,
    /// Fresh corral receipts the reviewer may reuse instead of re-running.
    pub reusable: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateEvidence {
    pub authority: &'static str,
    pub result: GateResult,
    pub sha: String,
    pub fresh: bool,
    pub log_passed: bool,
    pub log: PathBuf,
    pub failed_cmd: Option<String>,
    pub attempt: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactRef {
    pub kind: ArtifactKind,
    pub title: String,
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Assertion {
    pub authority: &'static str,
    pub machine_pass: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObservationRef {
    pub authority: &'static str,
    pub machine_pass: bool,
    pub criterion: String,
    pub status: ObservationStatus,
    pub observed: String,
    pub artifacts: Vec<String>,
    pub head: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub run: String,
    pub attempt: String,
    pub reviewer: String,
    pub verdict: Option<Verdict>,
    pub superseded: bool,
    pub sha: String,
    pub notes: String,
}

fn paths(ctx: &Ctx, task: &Task, run: &Run, attempt: &str) -> (PathBuf, PathBuf) {
    let dir = ctx.store.prompts_dir();
    let stem = format!("{}-{}-{attempt}", task.id, run.id);
    (
        dir.join(format!("{stem}-review-packet.json")),
        dir.join(format!("{stem}-review.diff")),
    )
}

pub(crate) fn evidence_gaps(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    route: &ReviewRoute,
) -> Result<Vec<EvidenceGap>> {
    let mut gaps = Vec::new();
    let mut gap = |code, detail: String| gaps.push(EvidenceGap { code, detail });
    let policy = ctx.store.config.task_review_policy(task);
    let prose = route.requirement.contract.kind == ReviewContractKind::Prose;
    match &run.gate {
        None if !policy.gate.is_empty() && !prose => {
            gap(
                GapCode::GateMissing,
                "no gate receipt for this candidate".into(),
            );
        }
        None => {}
        Some(g) => {
            if g.result != GateResult::Running && !verification::gate_fresh(ctx, task, run, g)? {
                gap(
                    GapCode::GateStale,
                    format!("gate receipt at {} is stale", g.sha),
                );
            }
            match g.result {
                GateResult::Fail => gap(
                    GapCode::GateFailed,
                    g.failed_cmd.clone().unwrap_or_default(),
                ),
                GateResult::Pass if !crate::gate::gate_log_passed(ctx, task, run)? => gap(
                    GapCode::GateLogContradicts,
                    "receipt says pass but the gate log does not".into(),
                ),
                GateResult::Running if !prose => {
                    gap(GapCode::GateMissing, "gate still running".into());
                }
                _ => {}
            }
        }
    }
    let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
    for criterion in task
        .completion
        .iter()
        .flat_map(|m| &m.criteria)
        .filter(|c| c.required)
    {
        let observed: Vec<_> = run
            .completion
            .evidence
            .iter()
            .filter(|e| e.observation.criterion == criterion.id)
            .collect();
        if observed.is_empty() {
            gap(GapCode::CriterionUnobserved, criterion.id.clone());
        } else if !observed.iter().any(|e| e.identity.head == head) {
            gap(GapCode::CriterionStale, criterion.id.clone());
        } else if observed
            .iter()
            .filter(|e| e.identity.head == head)
            .any(|e| e.observation.status != ObservationStatus::Pass)
        {
            gap(GapCode::CriterionFailed, criterion.id.clone());
        }
    }
    if route
        .requirement
        .checks
        .contains(&ReviewCheck::VisualEvidence)
        && !task
            .artifacts
            .iter()
            .any(|a| a.run == run.id && a.kind == ArtifactKind::Screenshot)
    {
        gap(
            GapCode::VisualEvidenceMissing,
            "no screenshot artifact for this run".into(),
        );
    }
    if run.obligations.pending_decision().is_some() {
        gap(
            GapCode::OwnerDecisionPending,
            "an owner decision is unresolved".into(),
        );
    }
    Ok(gaps)
}

/// Reviewer screens, transcripts and notes describe reviews, not the candidate under review.
fn reviewer_side(ctx: &Ctx, artifact: &corral_core::Artifact) -> bool {
    std::path::Path::new(&artifact.source).starts_with(ctx.store.state_dir().join("review-watch"))
        || (artifact.kind == ArtifactKind::Report && artifact.title.starts_with("Review ("))
}

fn findings(task: &Task, exclude: &str) -> Vec<Finding> {
    task.runs
        .iter()
        .flat_map(|run| run.review_runs.iter().map(move |child| (run, child)))
        .filter(|(_, child)| child.id != exclude)
        .filter_map(|(run, child)| {
            let review = child.review.as_ref()?;
            let notes = review.notes.clone()?;
            (review.verdict != Some(Verdict::Approve)).then(|| Finding {
                run: run.id.clone(),
                attempt: child.id.clone(),
                reviewer: review.reviewer.clone(),
                verdict: review.verdict,
                superseded: child.outcome == Some(RunOutcome::Superseded),
                sha: review.sha.clone(),
                notes,
            })
        })
        .collect()
}

pub(crate) fn build(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    attempt: &str,
    record: &ReviewRecord,
    route: &ReviewRoute,
) -> Result<(ReviewPacket, String)> {
    let base = crate::review_contracts::patch_base(run)?;
    let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let diff = corral_git::diff_text(&run.worktree, &base)?;
    let changed_paths = crate::review_contracts::changed_paths(run)?;
    let (_, diff_file) = paths(ctx, task, run, attempt);
    let gate = match &run.gate {
        Some(g) => Some(GateEvidence {
            authority: "corral_gate_receipt",
            result: g.result,
            sha: g.sha.clone(),
            fresh: verification::gate_fresh(ctx, task, run, g)?,
            log_passed: crate::gate::gate_log_passed(ctx, task, run)?,
            log: crate::gate::gate_paths(ctx, &task.id, &run.id).log,
            failed_cmd: g.failed_cmd.clone(),
            attempt: g.identity.as_ref().map(|i| i.attempt.clone()),
        }),
        None => None,
    };
    let reusable = gate
        .as_ref()
        .filter(|g| g.result == GateResult::Pass && g.fresh && g.log_passed)
        .map(|_| vec!["gate".to_string()])
        .unwrap_or_default();
    let config = &ctx.store.config;
    let bound = BoundPacket {
        version: REVIEW_PACKET_VERSION,
        task: task.id.clone(),
        title: task.title.clone(),
        run: run.id.clone(),
        attempt: attempt.into(),
        candidate: record.identity.clone(),
        goal: task.body.clone(),
        criteria: task
            .completion
            .as_ref()
            .map(|m| m.criteria.clone())
            .unwrap_or_default(),
        owner_decisions: task
            .completion
            .as_ref()
            .map(|m| m.owner_decisions.clone())
            .unwrap_or_default(),
        pending_owner_decision: run.obligations.pending_decision().is_some(),
        approved_plan: task.plan.clone(),
        requirement: route.requirement.clone(),
        contract_digest: route.contract_digest.clone(),
        scope: route.scope,
        round: route.round,
        reviewer: ReviewerRef {
            profile: route.profile.clone(),
            profile_revision: crate::review_contracts::profile_revision(config, &route.profile),
            instructions: config
                .profiles
                .get(&route.profile)
                .and_then(|p| p.metadata.as_ref())
                .map(|m| m.instructions.clone())
                .unwrap_or_default(),
        },
        object: ObjectRef {
            base,
            head,
            diff_file,
            diff_sha256: verification::digest(diff.as_bytes()),
            changed_paths,
        },
        evidence: Evidence {
            gate,
            artifacts: task
                .artifacts
                .iter()
                .filter(|a| a.run == run.id && !reviewer_side(ctx, a))
                .map(|a| ArtifactRef {
                    kind: a.kind,
                    title: a.title.clone(),
                    path: ctx.store.dir().join(&a.path),
                    sha256: a.sha256.clone(),
                })
                .collect(),
            worker_summary: run.summary.clone().map(|text| Assertion {
                authority: WORKER_ASSERTION,
                machine_pass: false,
                text,
            }),
            criterion_observations: run
                .completion
                .evidence
                .iter()
                .map(|e| ObservationRef {
                    authority: WORKER_ASSERTION,
                    machine_pass: false,
                    criterion: e.observation.criterion.clone(),
                    status: e.observation.status,
                    observed: e.observation.observed.clone(),
                    artifacts: e.observation.artifacts.clone(),
                    head: e.identity.head.clone(),
                })
                .collect(),
            reusable,
        },
        gaps: evidence_gaps(ctx, task, run, route)?,
        previous_findings: findings(task, attempt),
    };
    let digest = verification::digest(&serde_json::to_vec(&bound)?);
    Ok((ReviewPacket { digest, bound }, diff))
}

pub(crate) fn write(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    attempt: &str,
    record: &ReviewRecord,
    route: &ReviewRoute,
) -> Result<(String, String)> {
    let (packet, diff) = build(ctx, task, run, attempt, record, route)?;
    let (packet_file, diff_file) = paths(ctx, task, run, attempt);
    std::fs::create_dir_all(ctx.store.prompts_dir()).context("creating prompt directory")?;
    std::fs::write(&diff_file, diff).with_context(|| format!("writing {}", diff_file.display()))?;
    let json = serde_json::to_string_pretty(&packet)?;
    std::fs::write(&packet_file, &json)
        .with_context(|| format!("writing {}", packet_file.display()))?;
    let section = format!(
        "\n## Review packet\n\nBound to this candidate: {} (digest {}). Full diff: {}.\n\
         Evidence marked `worker_assertion` is an index to investigate, not a PASS.\n\n```json\n{}\n```\n",
        packet_file.display(),
        packet.digest,
        diff_file.display(),
        serde_json::to_string_pretty(&packet.bound)?
    );
    Ok((packet.digest, section))
}

/// Differences between the route/packet a review started with and the current candidate.
pub(crate) fn route_changes(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    record: &ReviewRecord,
    route: &ReviewRoute,
) -> Result<Vec<IdentityChange>> {
    let attempt = run
        .review_runs
        .last()
        .map(|c| c.id.clone())
        .context("review attempt missing")?;
    let mut current = route.clone();
    let config = &ctx.store.config;
    if route.signals.subject == corral_core::ReviewSubjectKind::Implementation {
        let (signals, escalated_from) =
            crate::review_contracts::implementation_signals(config, task, run, Some(&attempt))?;
        current.requirement = corral_core::review_requirement(&signals);
        current.escalated_from = escalated_from;
    }
    current.contract_digest =
        crate::review_contracts::contract_digest(current.requirement.contract.kind);
    current.profile_revision = crate::review_contracts::profile_revision(config, &route.profile);
    current.packet_digest = Some(build(ctx, task, run, &attempt, record, &current)?.0.digest);
    if route.same_assessment(&current) {
        return Ok(Vec::new());
    }
    let field = |name: &str, a: String, b: String| IdentityChange {
        field: name.into(),
        expected: a,
        actual: b,
    };
    Ok(vec![
        field(
            "review_requirement",
            serde_json::to_string(&route.requirement)?,
            serde_json::to_string(&current.requirement)?,
        ),
        field(
            "profile_revision",
            route.profile_revision.clone(),
            current.profile_revision.clone(),
        ),
        field(
            "packet_digest",
            route.packet_digest.clone().unwrap_or_default(),
            current.packet_digest.clone().unwrap_or_default(),
        ),
    ]
    .into_iter()
    .filter(|c| c.expected != c.actual)
    .collect())
}

/// An approval is recorded only when the contract's proof holds and the reviewer can meet it.
pub(crate) fn enforce(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    route: &ReviewRoute,
    verdict: Verdict,
    notes: &str,
) -> Result<(Verdict, String)> {
    if verdict != Verdict::Approve {
        return Ok((verdict, notes.to_string()));
    }
    if route.capability_mismatch {
        return Ok((
            Verdict::Escalate,
            format!(
                "{notes}\n\nEscalated by corral: reviewer '{}' lacks the capability required by the {} contract.",
                route.profile,
                route.requirement.contract.kind.as_str()
            ),
        ));
    }
    let gaps = evidence_gaps(ctx, task, run, route)?;
    if gaps.is_empty() {
        return Ok((verdict, notes.to_string()));
    }
    let converted = if gaps.iter().any(|g| g.verdict() == Verdict::Escalate) {
        Verdict::Escalate
    } else {
        Verdict::Changes
    };
    Ok((
        converted,
        format!(
            "{notes}\n\nCorral evidence check: approve recorded as {} because proof is missing, stale or contradictory:\n```json\n{}\n```",
            converted.as_str(),
            serde_json::to_string_pretty(&gaps)?
        ),
    ))
}
