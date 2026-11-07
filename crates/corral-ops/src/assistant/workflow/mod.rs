//! Local design persistence only. Public entry/execution/review/handoff are deliberately
//! unavailable until their admission/lifecycle owners land. No board task is allocated here.
//! Lock order: authenticated admission, then the assistant's dispatch/store lock. Artifact
//! preparation is content-addressed; state, history rows and receipt commit in one record unit.
use anyhow::{Context, Result};
use corral_core::Store;
use corral_core::assistant::{AssistantContainer, OwnerInputSource};
use corral_core::assistant_workflow::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::{RejectCode, Rejection};

pub mod boundary;
mod publication;
mod read;
pub use publication::{Publish, publish, stage};
pub use read::{read_revision, read_state};

pub(crate) const STAGING: &str = "assistant-design-staging";
pub(crate) const REVISIONS: &str = "assistant-design-revisions";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    pub workflow: String,
    pub state_revision: u64,
    pub authority_epoch: u64,
    pub plan_revision: Option<u32>,
}

pub(crate) fn bad(why: &str) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, why).into()
}

pub(crate) fn current(container: &AssistantContainer) -> Result<&Workflow> {
    container.validate_protocol()?;
    container
        .workflow
        .as_ref()
        .map(|index| &index.current)
        .ok_or_else(|| bad("no assistant workflow"))
}

pub(crate) fn compare(container: &AssistantContainer, expected: &Expected) -> Result<()> {
    let w = current(container)?;
    if w.id != expected.workflow
        || w.state_revision != expected.state_revision
        || w.authority_epoch != expected.authority_epoch
        || w.current_plan.as_ref().map(|p| p.revision) != expected.plan_revision
    {
        return Err(Rejection::new(
            RejectCode::StaleRevision,
            "assistant workflow changed; read the current state",
        )
        .with_detail(summary(container)?)
        .into());
    }
    Ok(())
}

pub(crate) fn key(session: &str, workflow: &str) -> String {
    format!("{session}/{workflow}")
}

pub(crate) fn revision_key(session: &str, workflow: &str, revision: u32) -> String {
    format!("{session}/{workflow}/r{revision:04}")
}

pub(crate) fn summary(container: &AssistantContainer) -> Result<Value> {
    container.validate_protocol()?;
    let Some(index) = &container.workflow else {
        return Ok(json!({"workflow": null, "phase": "ordinary"}));
    };
    let w = &index.current;
    Ok(json!({
        "version": index.version, "workflow": w.id, "state_revision": w.state_revision,
        "authority_epoch": w.authority_epoch, "phase": w.phase, "owner": w.owner,
        "original_request": w.original_request,
        "current_plan": w.current_plan.as_ref().map(|p| json!({
            "revision": p.revision, "artifact": p.artifact, "stage": p.stage,
            "assumptions": p.assumptions, "execution_scope": p.execution_scope,
        })),
        "decisions": w.decision_refs.len(), "reviews": w.reviews.len(),
        "execution": w.execution, "handoff": w.handoff, "history_count": index.history_refs.len(),
    }))
}

/// Internal seam for the execution-boundary owner. Call ONLY inside a host-authenticated
/// assistant commit after closing admission. This does not acknowledge an activity drain.
/// It is intentionally not wired to an agent action, CLI mutation or /plan command yet.
pub fn prepare_entry(
    store: &Store,
    session: &str,
    container: &mut AssistantContainer,
    input: &str,
) -> Result<()> {
    container.validate_protocol()?;
    if container.workflow.is_some() {
        return Err(bad("an existing workflow must be reconciled before entry"));
    }
    let evidence = container
        .owner_input(input)
        .context("not a journaled owner request")?
        .clone();
    if !matches!(
        evidence.source,
        OwnerInputSource::Pane | OwnerInputSource::Dialog
    ) || evidence.generation != container.state.binding.generation
        || evidence.truncated
        || evidence.text.trim().is_empty()
        || corral_core::blob_ref::BlobHash::of(evidence.text.as_bytes()).as_str() != evidence.sha256
    {
        return Err(bad(
            "entry requires a complete genuine current-generation owner request",
        ));
    }
    let artifact = publication::artifact(store, session, "W1", evidence.text.as_bytes())?;
    container.version = CONTAINER_VERSION;
    container.workflow = Some(WorkflowIndex {
        version: WORKFLOW_VERSION,
        issued: 1,
        authority_epoch: 1,
        current: Workflow {
            id: "W1".into(),
            state_revision: 1,
            authority_epoch: 1,
            phase: WorkflowPhase::Entering,
            owner: ScopeOwner::Local,
            original_request: artifact,
            owner_evidence: evidence,
            current_plan: None,
            decision_refs: Vec::new(),
            execution: None,
            reviews: Vec::new(),
            handoff: None,
        },
        history_refs: Vec::new(),
    });
    Ok(())
}

#[cfg(test)]
mod tests;
