//! The combined candidate and its identity. Unmet required work is named, never skipped.
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use corral_core::{
    AGGREGATE_RUN, AggregateMember, CandidateIdentity, Event, PlanDeliveryTarget, RunOutcome,
    Status, Store, Task, VERIFICATION_ENV, operating_contract_revision,
};

use crate::Ctx;
use crate::verification::digest;

/// Every reason a Plan cannot be verified or accepted yet.
#[derive(Debug)]
pub(crate) struct Unmet(pub(crate) Vec<String>);

impl std::fmt::Display for Unmet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Plan has unmet required work: {}", self.0.join("; "))
    }
}

impl std::error::Error for Unmet {}

pub(super) struct Candidate {
    pub(super) identity: CandidateIdentity,
    pub(super) members: Vec<AggregateMember>,
    pub(super) code: bool,
    pub(super) target: PlanDeliveryTarget,
    /// The owned integration checkout when the Plan branch carries the candidate.
    pub(super) checkout: Option<PathBuf>,
}

pub(super) fn repo(store: &Store, tracker: &Task) -> Result<PathBuf> {
    store
        .project_path(tracker.project.as_deref().context("Plan has no project")?)?
        .canonicalize()
        .context("resolving Plan repository")
}

/// Capture the candidate from fresh board, Git and policy state. `environment` replays a
/// recorded attempt; None captures the verifier's own. Caller holds landing_lock.
pub(super) fn capture(
    ctx: &Ctx,
    store: &Store,
    tracker: &Task,
    attempt: &str,
    environment: Option<&[(String, Option<String>)]>,
) -> Result<Candidate> {
    ensure!(
        tracker.plan_acceptance_required(),
        "{} is not a tracker Plan with approved acceptance criteria",
        tracker.id
    );
    let scope = tracker
        .approved_scope
        .as_ref()
        .context("Plan scope missing")?;
    let repo = repo(store, tracker)?;
    let events = store.events_shared()?;
    let mut blockers = Vec::new();
    if tracker.children.is_empty() {
        blockers.push("Plan has no required members".into());
    }
    let mut members = Vec::new();
    for id in &tracker.children {
        match member(ctx, store, tracker, id, &events) {
            Ok(Ok(member)) => members.push(member),
            Ok(Err(reason)) => blockers.push(reason),
            Err(error) => blockers.push(format!("{id}: {error:#}")),
        }
    }
    let main = corral_git::rev_sha(&repo, "refs/heads/main")?;
    let code = members.iter().any(|member| member.code);
    let (label, head, checkout, dispatch_base) = match &scope.delivery_target {
        PlanDeliveryTarget::Main => ("main".to_string(), main.clone(), None, String::new()),
        PlanDeliveryTarget::Integration { branch } => {
            match crate::plan_target::plan_head(store, tracker, branch)? {
                Some((head, checkout, initial)) => (branch.clone(), head, Some(checkout), initial),
                None => (branch.clone(), main.clone(), None, String::new()),
            }
        }
    };
    if code {
        if !corral_git::is_ancestor(&repo, &main, &head)? {
            blockers.push(format!(
                "{label} does not include current main {main}; reconcile main into the Plan branch before verification"
            ));
        }
        for member in members.iter().filter(|member| member.code) {
            if !corral_git::is_ancestor(&repo, &member.receipt_sha, &head)? {
                blockers.push(format!(
                    "{}: accepted work {} is not included in {label}",
                    member.task, member.receipt_sha
                ));
            }
        }
    }
    if !blockers.is_empty() {
        return Err(Unmet(blockers).into());
    }
    let policy = store.config.task_review_policy(tracker);
    let environment = match environment {
        Some(environment) => environment.to_vec(),
        None => verifier_environment(ctx, tracker, &repo, &policy)?,
    };
    let contracts: Vec<_> = members
        .iter()
        .map(|member| (&member.task, &member.contract_revision))
        .collect();
    let (head, tree, integration_base, dispatch_base) = if code {
        let tree = corral_git::rev_sha(&repo, &format!("{head}^{{tree}}"))?;
        (head, tree, main, dispatch_base)
    } else {
        // A no-code Plan binds accepted artifact versions, not an unrelated main head.
        Default::default()
    };
    let identity = CandidateIdentity {
        version: 2,
        task: tracker.id.clone(),
        run: AGGREGATE_RUN.into(),
        attempt: attempt.into(),
        head,
        tree,
        integration_base,
        dispatch_base,
        spec_digest: digest(&serde_json::to_vec(&(
            operating_contract_revision(tracker)?,
            contracts,
        ))?),
        policy_digest: digest(&serde_json::to_vec(&(
            &policy,
            store.effective_operating_policy()?.revision,
        ))?),
        inputs_digest: digest(&serde_json::to_vec(&(
            &members,
            &environment,
            crate::verification::config_inputs(&store.config)?,
            prompt_inputs(store)?,
            include_str!("../gate_script.rs"),
            env!("CARGO_PKG_VERSION"),
        ))?),
        environment,
        gate_tier: None,
    };
    Ok(Candidate {
        identity,
        members,
        code,
        target: scope.delivery_target.clone(),
        checkout,
    })
}

pub(super) fn verifier_environment(
    ctx: &Ctx,
    tracker: &Task,
    repo: &std::path::Path,
    policy: &corral_core::ReviewPolicy,
) -> Result<Vec<(String, Option<String>)>> {
    let mut environment: std::collections::BTreeMap<_, _> = VERIFICATION_ENV
        .iter()
        .map(|name| {
            (
                name.to_string(),
                std::env::var_os(name).map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    for (name, value) in &policy.env {
        environment.insert(name.clone(), Some(value.clone()));
    }
    let limits = crate::integration::effective_size_limits(ctx, tracker, repo, policy)?;
    environment.insert(
        "CORRAL_SIZE_LIMITS".into(),
        Some(serde_json::to_string(&limits)?),
    );
    Ok(environment.into_iter().collect())
}

/// Reviewer and run prompt bytes, as hashed into ordinary task identities.
pub(super) fn prompt_inputs(store: &Store) -> Result<Vec<(PathBuf, Option<String>)>> {
    ["review.md", "run.md"]
        .into_iter()
        .map(|name| {
            let path = store.prompts_dir().join(name);
            match std::fs::read(&path) {
                Ok(bytes) => Ok((path, Some(digest(&bytes)))),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((path, None)),
                Err(error) => Err(error)
                    .with_context(|| format!("reading verification input {}", path.display())),
            }
        })
        .collect()
}

fn member(
    ctx: &Ctx,
    store: &Store,
    tracker: &Task,
    id: &str,
    events: &[Event],
) -> Result<std::result::Result<AggregateMember, String>> {
    let child = store
        .get(id)
        .with_context(|| format!("missing Plan member {id}"))?;
    ensure!(
        store.execution_plan(&child)?.map(|plan| plan.id) == Some(tracker.id.clone()),
        "execution Plan membership changed"
    );
    crate::queue::dependency_check(store, &child)?;
    if child.status != Status::Done {
        return Ok(Err(format!(
            "{id}: required work is {}",
            child.status.label()
        )));
    }
    let Some(run) = child
        .current_run()
        .filter(|run| run.outcome == Some(RunOutcome::Done))
    else {
        return Ok(Err(format!(
            "{id}: accepted without a completed run receipt"
        )));
    };
    if run.obligations.pending_decision().is_some() {
        return Ok(Err(format!("{id}: unresolved owner decision")));
    }
    if child.project != tracker.project {
        return Ok(Err(format!("{id}: member belongs to another repository")));
    }
    let Some(receipt) = events.iter().rev().find(|event| {
        event.kind == "approved"
            && event.task.as_deref() == Some(id)
            && event.detail["run"].as_str() == Some(run.id.as_str())
    }) else {
        return Ok(Err(format!("{id}: accepted but no acceptance receipt")));
    };
    if !matches!(
        receipt.detail["strategy"].as_str(),
        Some("fast_forward" | "rebase")
    ) {
        return Ok(Err(format!(
            "{id}: accepted branch was not integrated into the Plan target"
        )));
    }
    let sha = receipt.detail["sha"]
        .as_str()
        .context("acceptance receipt has no commit SHA")?;
    if let Some(binding) = &run.plan_target {
        if binding.plan != tracker.id {
            return Ok(Err(format!("{id}: accepted into another Plan")));
        }
        if !crate::plan_target::has_receipt(store, tracker, &child, run, sha)? {
            return Ok(Err(format!(
                "{id}: no published owned-target integration receipt"
            )));
        }
    }
    let code = sha != run.base_sha;
    let accepted = child
        .artifacts
        .iter()
        .filter(|artifact| artifact.run == run.id);
    if !code {
        // No commit carries this member's delivery: its accepted snapshot bytes are the proof.
        for artifact in accepted.clone() {
            if artifact.purged_at.is_some() {
                return Ok(Err(format!(
                    "{id}: accepted artifact {} was purged",
                    artifact.path
                )));
            }
            let intact =
                crate::artifact_storage::artifact_read_confined(ctx, artifact).is_ok_and(|bytes| {
                    u64::try_from(bytes.len()).ok() == Some(artifact.bytes)
                        && digest(&bytes) == artifact.sha256
                });
            if !intact {
                return Ok(Err(format!(
                    "{id}: accepted artifact {} is missing or changed",
                    artifact.path
                )));
            }
        }
    }
    let mut artifacts: Vec<_> = accepted.map(|artifact| artifact.sha256.clone()).collect();
    artifacts.sort();
    Ok(Ok(AggregateMember {
        task: child.id.clone(),
        run: run.id.clone(),
        contract_revision: operating_contract_revision(&child)?,
        receipt_sha: sha.into(),
        code,
        artifacts,
    }))
}
