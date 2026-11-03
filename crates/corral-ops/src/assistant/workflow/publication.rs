use std::io::Read;

use anyhow::{Context, Result};
use corral_core::Store;
use corral_core::assistant::AssistantContainer;
use corral_core::assistant_workflow::*;
use corral_core::blob_ref::BlobHash;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Expected, REVISIONS, STAGING, bad, compare, current, key, revision_key, summary};

const ARTIFACTS: &str = "assistant-design-artifacts";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Staging {
    pub expected: Expected,
    pub text: String,
}

/// The service selects every artifact identity; callers supply bytes, never paths. The
/// content-addressed record is durable BEFORE state can reference it. Interrupted preparation
/// may leave an unreferenced artifact, but can neither change published bytes nor lose a draft.
pub(super) fn artifact(
    store: &Store,
    session: &str,
    workflow: &str,
    bytes: &[u8],
) -> Result<DesignArtifact> {
    if bytes.is_empty() || bytes.len() > DOCUMENT_MAX {
        return Err(bad("design artifact must contain 1 byte through 1 MiB"));
    }
    let reference = DesignArtifact {
        blob: BlobHash::of(bytes).to_ref(),
        bytes: bytes.len() as u32,
    };
    let key = format!("{session}/{workflow}/{}", reference.blob.hash());
    if let Some(existing) = store.record::<DesignArtifact>(ARTIFACTS, &key)? {
        if existing != reference || load_artifact(store, &existing)? != bytes {
            anyhow::bail!("recovery_required: immutable design artifact differs");
        }
    } else {
        store.put_evidence(ARTIFACTS, &key, bytes, |_| json!(reference))?;
    }
    Ok(reference)
}

pub(super) fn load_artifact(store: &Store, reference: &DesignArtifact) -> Result<Vec<u8>> {
    reference.validate()?;
    store.with_blob_lease(|| {
        let mut bytes = Vec::new();
        store
            .blob_reader(reference.blob.hash())?
            .context("recovery_required: design artifact is missing")?
            .take(DOCUMENT_MAX as u64 + 1)
            .read_to_end(&mut bytes)
            .context("reading immutable design artifact")?;
        anyhow::ensure!(
            bytes.len() == reference.bytes as usize
                && BlobHash::of(&bytes) == *reference.blob.hash(),
            "recovery_required: design artifact is corrupt"
        );
        Ok(bytes)
    })
}

fn writable(container: &AssistantContainer, expected: &Expected) -> Result<()> {
    compare(container, expected)?;
    let w = current(container)?;
    if w.phase != WorkflowPhase::Planning || w.owner != ScopeOwner::Local {
        return Err(bad("design publication requires settled local planning"));
    }
    Ok(())
}

/// Chunk retries at an already-written offset must match exact bytes. Staging never increments
/// the plan or state revision; publication consumes the complete upload with its CAS context.
pub fn stage(
    store: &Store,
    session: &str,
    container: &AssistantContainer,
    expected: &Expected,
    offset: usize,
    chunk: &str,
) -> Result<Value> {
    writable(container, expected)?;
    if chunk.is_empty() || chunk.len() > CHUNK_MAX {
        return Err(bad(
            "stage chunk must contain 1 byte through 16 KiB of UTF-8",
        ));
    }
    let key = key(session, &expected.workflow);
    let mut staging: Staging = store.record(STAGING, &key)?.unwrap_or_else(|| Staging {
        expected: expected.clone(),
        text: String::new(),
    });
    if staging.expected != *expected {
        return Err(bad("staged upload belongs to another workflow revision"));
    }
    let end = offset
        .checked_add(chunk.len())
        .ok_or_else(|| bad("invalid stage offset"))?;
    if offset < staging.text.len() {
        if staging.text.as_bytes().get(offset..end) != Some(chunk.as_bytes()) {
            return Err(crate::agent::Rejection::new(
                crate::agent::RejectCode::PayloadMismatch,
                "stage retry differs from stored bytes",
            )
            .into());
        }
    } else if offset == staging.text.len() && end <= DOCUMENT_MAX {
        staging.text.push_str(chunk);
        store.save_record(STAGING, &key, &staging)?;
    } else {
        return Err(bad(
            "stage offset must be the next byte; document limit is 1 MiB",
        ));
    }
    Ok(
        json!({"workflow": expected.workflow, "state_revision": expected.state_revision,
        "authority_epoch": expected.authority_epoch, "staged_bytes": staging.text.len()}),
    )
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Publish {
    pub expected: Expected,
    pub bytes: u32,
    pub digest: BlobHash,
    pub stage: DesignStage,
    #[serde(default)]
    pub execution_scope: Option<ExecutionScope>,
    #[serde(default)]
    pub assumptions: Option<SourceAssumptions>,
    #[serde(default)]
    pub scope_changes: String,
}

/// Called inside the receipted container transaction. Artifact preparation is idempotent and
/// history/current/receipt publication is atomic; a panic after artifact put leaves no revision.
pub fn publish(
    store: &Store,
    session: &str,
    container: &mut AssistantContainer,
    payload: Publish,
) -> Result<Value> {
    writable(container, &payload.expected)?;
    let staging_key = key(session, &payload.expected.workflow);
    let staging: Staging = store
        .record(STAGING, &staging_key)?
        .ok_or_else(|| bad("no staged design"))?;
    if staging.expected != payload.expected
        || staging.text.len() != payload.bytes as usize
        || staging.text.trim().is_empty()
        || BlobHash::of(staging.text.as_bytes()) != payload.digest
    {
        return Err(bad(
            "publication length/digest/context does not match staged UTF-8 bytes",
        ));
    }
    if payload.scope_changes.len() > 2048
        || payload
            .scope_changes
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(bad("scope_changes exceeds 2048 bytes"));
    }
    let revision = payload
        .expected
        .plan_revision
        .unwrap_or(0)
        .checked_add(1)
        .filter(|r| *r <= HISTORY_MAX)
        .ok_or_else(|| bad("design history capacity reached"))?;
    match (&payload.execution_scope, &payload.assumptions) {
        (Some(scope), Some(assumptions)) => {
            scope
                .validate()
                .map_err(|_| bad("invalid execution scope"))?;
            assumptions
                .validate(scope)
                .map_err(|_| bad("invalid source assumptions"))?;
            if !store.config.projects.contains_key(&scope.project) {
                return Err(bad("execution scope must name a registered project"));
            }
        }
        (None, None) if payload.stage == DesignStage::Draft => {}
        _ => {
            return Err(bad(
                "ready revisions require both execution scope and source assumptions",
            ));
        }
    }
    let artifact = artifact(
        store,
        session,
        &payload.expected.workflow,
        staging.text.as_bytes(),
    )?;
    let execution_scope = payload
        .execution_scope
        .map(|scope| artifact_json(store, session, &payload.expected.workflow, &scope))
        .transpose()?;
    let assumptions = payload
        .assumptions
        .map(|assumptions| artifact_json(store, session, &payload.expected.workflow, &assumptions))
        .transpose()?;
    crate::agent::failpoint("workflow_artifact_prepared");
    let published = PlanRevision {
        revision,
        artifact,
        stage: payload.stage,
        assumptions,
        execution_scope,
        published_at: corral_core::clock::now(),
        supersedes: payload.expected.plan_revision,
        scope_changes: payload.scope_changes,
    };
    let row = revision_key(session, &payload.expected.workflow, revision);
    if !store.insert_record(REVISIONS, &row, &published)? {
        anyhow::bail!("recovery_required: design history row already exists without current state");
    }
    let workflow = &mut container
        .workflow
        .as_mut()
        .context("workflow disappeared")?
        .current;
    workflow.current_plan = Some(published);
    workflow.state_revision = workflow
        .state_revision
        .checked_add(1)
        .context("workflow revision overflow")?;
    store.delete_record(STAGING, &staging_key)?;
    crate::agent::failpoint("workflow_revision_staged");
    summary(container)
}

fn artifact_json(
    store: &Store,
    session: &str,
    workflow: &str,
    value: &impl Serialize,
) -> Result<DesignArtifact> {
    artifact(store, session, workflow, &serde_json::to_vec(value)?)
}
