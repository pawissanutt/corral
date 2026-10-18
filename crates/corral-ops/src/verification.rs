//! One identity contract for gate, semantic review and the landing boundary.
use anyhow::{Context, Result, bail};
use corral_core::{
    CandidateIdentity, GateRecord, GateResult, IdentityChange, ReviewPolicy, ReviewRecord, Run,
    Task, VERIFICATION_ENV,
};
use sha2::{Digest, Sha256};

use crate::Ctx;

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn evidence_digest(run: &Run) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&run.completion.evidence)?))
}

pub(crate) fn spec_digest(task: &Task) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&(
        &task.id,
        &task.title,
        &task.project,
        &task.body,
    ))?))
}

/// Reload policy at each trust boundary, including long-running CLI invocations.
pub(crate) fn policy(ctx: &Ctx, task: &Task) -> Result<ReviewPolicy> {
    let store = ctx.store.reopen()?;
    store.config.validate_review()?;
    Ok(store.config.task_review_policy(task))
}

pub(crate) fn require_gate(policy: &ReviewPolicy, task: &Task) -> Result<()> {
    if policy.gate.is_empty() {
        bail!(
            "project {} has no gate configured",
            task.project.as_deref().unwrap_or("(none)")
        );
    }
    Ok(())
}

pub(crate) fn capture_identity(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    attempt: &str,
) -> Result<CandidateIdentity> {
    let fresh = Ctx {
        store: ctx.store.reopen()?,
        ..ctx.clone()
    };
    fresh.store.config.validate_review()?;
    let policy = fresh.store.config.task_review_policy(task);
    let mut environment: std::collections::BTreeMap<_, _> = VERIFICATION_ENV
        .iter()
        .map(|name| {
            (
                name.to_string(),
                std::env::var_os(name).map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    for (name, value) in &policy.env {
        environment.insert(name.clone(), Some(value.clone()));
    }
    let size_limits =
        crate::integration::effective_size_limits(&fresh, task, &run.worktree, &policy)?;
    environment.insert(
        "CORRAL_SIZE_LIMITS".into(),
        Some(serde_json::to_string(&size_limits)?),
    );
    identity_inputs(ctx, task, run, attempt, environment.into_iter().collect())
}

/// A semantic review shares the gate's captured execution inputs when one exists.
pub(crate) fn review_identity(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    attempt: &str,
) -> Result<CandidateIdentity> {
    match run.gate.as_ref().and_then(|g| g.identity.as_ref()) {
        Some(expected) if expected.attempt == attempt => identity(ctx, task, run, expected),
        _ => capture_identity(ctx, task, run, attempt),
    }
}

/// Recompute mutable inputs without consulting the verifier's process environment.
pub(crate) fn identity(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    expected: &CandidateIdentity,
) -> Result<CandidateIdentity> {
    identity_inputs(
        ctx,
        task,
        run,
        &expected.attempt,
        expected.environment.clone(),
    )
}

fn identity_inputs(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    attempt: &str,
    environment: Vec<(String, Option<String>)>,
) -> Result<CandidateIdentity> {
    let store = ctx.store.reopen()?;
    let policy = store.config.task_review_policy(task);
    // Allowlist verification inputs. The effective project policy is hashed below;
    // scheduling, UI, notifications and other operational settings are not evidence inputs.
    // Keep profile/harness definitions (including legacy agents) and reviewer selection:
    // a reviewer may also be selected explicitly at review-run time.
    let config = serde_json::to_vec(&(
        &store.config.harnesses,
        &store.config.profiles,
        &store.config.agents,
        &store.config.routing.reviewer_by_vendor,
        &store.config.pm.agent,
    ))?;
    let mut file_inputs = Vec::new();
    for path in [
        run.worktree.join("AGENTS.md"),
        run.worktree.join(".cargo/config.toml"),
        run.worktree.join("rust-toolchain.toml"),
        run.worktree.join("scripts/check-candidate.py"),
        ctx.store.prompts_dir().join("review.md"),
        ctx.store.prompts_dir().join("run.md"),
    ] {
        let hash = match std::fs::read(&path) {
            Ok(bytes) => Some(digest(&bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("reading verification input {}", path.display()));
            }
        };
        file_inputs.push((path, hash));
    }
    Ok(CandidateIdentity {
        version: 2,
        task: task.id.clone(),
        run: run.id.clone(),
        attempt: attempt.to_string(),
        head: corral_git::rev_sha(&run.worktree, "HEAD")?,
        tree: corral_git::rev_sha(&run.worktree, "HEAD^{tree}")?,
        integration_base: corral_git::rev_sha(&run.worktree, &run.base)?,
        dispatch_base: run.base_sha.clone(),
        spec_digest: spec_digest(task)?,
        policy_digest: digest(&serde_json::to_vec(&(policy, &task.completion))?),
        inputs_digest: digest(&serde_json::to_vec(&(
            config,
            &environment,
            file_inputs,
            include_str!("gate_script.rs"),
            include_str!("verification.rs"),
            env!("CARGO_PKG_VERSION"),
        ))?),
        environment,
    })
}

pub(crate) fn gate_fresh(ctx: &Ctx, task: &Task, run: &Run, gate: &GateRecord) -> Result<bool> {
    let Some(expected) = &gate.identity else {
        return Ok(false);
    };
    Ok(gate.sha == expected.head
        && corral_git::is_clean(&run.worktree)?
        && identity(ctx, task, run, expected)? == *expected)
}

pub(crate) fn gate_passed(ctx: &Ctx, task: &Task, run: &Run) -> Result<bool> {
    match &run.gate {
        Some(g) if g.result == GateResult::Pass => {
            Ok(crate::gate::gate_log_passed(ctx, task, run)? && gate_fresh(ctx, task, run, g)?)
        }
        _ => Ok(false),
    }
}

pub(crate) fn review_fresh(ctx: &Ctx, task: &Task, run: &Run) -> Result<bool> {
    let Some(review) = &run.review else {
        return Ok(false);
    };
    Ok(corral_git::is_clean(&run.worktree)? && review_changes(ctx, task, run, review)?.is_empty())
}

pub(crate) fn review_changes(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    review: &ReviewRecord,
) -> Result<Vec<IdentityChange>> {
    let Some(expected) = &review.identity else {
        return Ok(vec![IdentityChange {
            field: "identity".into(),
            expected: "candidate-bound receipt".into(),
            actual: "missing legacy identity".into(),
        }]);
    };
    let gate = run.gate.as_ref().and_then(|g| g.identity.as_ref());
    let actual = identity(ctx, task, run, gate.unwrap_or(expected))?;
    let mut changes = expected.differences(&actual);
    let actual_evidence = evidence_digest(run)?;
    if review.evidence_digest.as_deref() != Some(&actual_evidence) {
        changes.push(IdentityChange {
            field: "evidence_digest".into(),
            expected: review
                .evidence_digest
                .clone()
                .unwrap_or_else(|| "missing".into()),
            actual: actual_evidence,
        });
    }
    Ok(changes)
}
