//! Checked indexes over canonical native receipts, never transcript replay authority.
//!
//! Context is bounded UTF-8 public data. Credential/private-provider material is refused,
//! not redacted into a different artifact. Content and manifests are inserted once; loading
//! verifies their bytes and the original source records, including after its generation fences.
use std::collections::BTreeSet;
use std::io::Read;

use anyhow::{Context, Result, anyhow, ensure};
use corral_core::agent::{self, AgentRunState, ContainerRef, ReceiptState, RequestReceipt};
use corral_core::blob_ref::{BlobHash, BlobRef};
use corral_core::paid_episode::PaidBinding;
use corral_core::{Store, Task};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::continuation::{PauseReason, SubjectRevision};
use crate::Ctx;

pub(crate) const CONTEXT_MAX: usize = 256 * 1024;
const CONTEXT_AREA: &str = "continuation-context";
const CHECKPOINT_AREA: &str = "continuation-checkpoints";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceiptRef {
    pub request_id: String,
    pub op: String,
    pub result_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TurnFrontier {
    pub generation: u32,
    pub turn: u64,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContextArtifact {
    pub blob: BlobRef,
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContinuationCheckpoint {
    pub version: u32,
    pub source: PaidBinding,
    pub subject: SubjectRevision,
    pub frontier: TurnFrontier,
    pub context: ContextArtifact,
    pub receipts: Vec<ReceiptRef>,
    pub captured: Timestamp,
}

impl ContinuationCheckpoint {
    pub(crate) fn digest(&self) -> Result<String> {
        let bytes = serde_json::to_vec(self).context("encoding continuation checkpoint")?;
        Ok(BlobHash::of(&bytes).as_str().to_owned())
    }
}

fn settled(state: &AgentRunState) -> std::result::Result<(), PauseReason> {
    // An older generation's unresolved effect is still an unresolved effect of this container.
    if state.receipts.iter().any(|receipt| {
        receipt.effects_pending()
            || receipt.state == ReceiptState::Uncertain
            || receipt.recovery_update_due()
    }) || state.jobs.iter().any(|job| job.needs_reap())
    {
        return Err(PauseReason::UncertainEffect);
    }
    Ok(())
}

fn frontier(
    state: &AgentRunState,
    generation: u32,
) -> std::result::Result<TurnFrontier, PauseReason> {
    settled(state)?;
    let bound = state
        .generation(generation)
        .and_then(|generation| generation.bound.as_ref())
        .filter(|bound| !bound.session_id.is_empty())
        .ok_or(PauseReason::MissingCheckpoint)?;
    let mut latest = None;
    let mut turns = BTreeSet::new();
    for turn in state
        .turns
        .iter()
        .filter(|turn| turn.generation == generation)
    {
        if turn.ended.is_none() {
            return Err(PauseReason::EnteringEffect);
        }
        if !turns.insert(turn.seq) {
            return Err(PauseReason::CheckpointInvalid);
        }
        latest = Some(latest.map_or(turn.seq, |seq: u64| seq.max(turn.seq)));
    }
    Ok(TurnFrontier {
        generation,
        turn: latest.ok_or(PauseReason::MissingCheckpoint)?,
        session_id: bound.session_id.clone(),
    })
}

/// Only the current generation can produce a new checkpoint. Historical loads use the
/// recorded generation instead, since retained PM/assistant containers advance in place.
pub(crate) fn quiescent(
    state: &AgentRunState,
    generation: u32,
) -> std::result::Result<TurnFrontier, PauseReason> {
    if generation == 0 || state.binding.generation != generation {
        return Err(PauseReason::CheckpointInvalid);
    }
    frontier(state, generation)
}

/// A retained API conversation can be idle before its first turn. There is no
/// checkpoint frontier to carry, but unresolved effects still forbid recovery.
pub(crate) fn resumed_quiescent(
    state: &AgentRunState,
    generation: u32,
) -> std::result::Result<(), PauseReason> {
    if state.turns.iter().any(|t| t.generation == generation) {
        return quiescent(state, generation).map(|_| ());
    }
    settled(state)?;
    if generation == 0
        || state.binding.generation != generation
        || state
            .generation(generation)
            .and_then(|g| g.bound.as_ref())
            .is_none_or(|b| b.session_id.is_empty())
    {
        return Err(PauseReason::MissingCheckpoint);
    }
    Ok(())
}

fn refs_for(
    state: &AgentRunState,
    frontier: &TurnFrontier,
) -> std::result::Result<Vec<ReceiptRef>, PauseReason> {
    let mut ids = BTreeSet::new();
    let mut refs = Vec::new();
    for receipt in &state.receipts {
        if !ids.insert(receipt.request_id.as_str()) {
            return Err(PauseReason::CheckpointInvalid);
        }
        if receipt.identity.generation != frontier.generation {
            continue;
        }
        if receipt.request_id.is_empty()
            || receipt.op.is_empty()
            || receipt.identity.role != state.binding.role
            || receipt.identity.store != state.binding.store
            || receipt.identity.project != state.binding.project
            || receipt.identity.session_id != frontier.session_id
            || receipt
                .identity
                .turn
                .is_some_and(|turn| turn > frontier.turn)
        {
            return Err(PauseReason::CheckpointInvalid);
        }
        let result =
            serde_json::to_vec(&receipt.result).map_err(|_| PauseReason::CheckpointInvalid)?;
        if result.len() > agent::RESULT_MAX_BYTES {
            return Err(PauseReason::CheckpointInvalid);
        }
        refs.push(ReceiptRef {
            request_id: receipt.request_id.clone(),
            op: receipt.op.clone(),
            result_sha256: BlobHash::of(&result).as_str().to_owned(),
        });
    }
    refs.sort_unstable_by(|a, b| a.request_id.cmp(&b.request_id));
    Ok(refs)
}

pub(crate) fn receipt_refs(
    state: &AgentRunState,
    generation: u32,
) -> std::result::Result<Vec<ReceiptRef>, PauseReason> {
    refs_for(state, &quiescent(state, generation)?)
}

fn paused(reason: PauseReason) -> anyhow::Error {
    anyhow!("continuation checkpoint refused: {reason:?}")
}

fn source_matches(state: &AgentRunState, source: &PaidBinding) -> Result<()> {
    source
        .binding
        .validate()
        .context("validating checkpoint source binding")?;
    ensure!(
        state.binding.role == source.role(),
        "checkpoint source role changed"
    );
    ensure!(
        state.version == agent::AGENT_RECORD_VERSION
            && source.generation > 0
            && source.generation <= state.binding.generation
            && state
                .generations
                .iter()
                .filter(|generation| generation.n == source.generation)
                .count()
                == 1,
        "checkpoint source generation is invalid"
    );
    let generation = state
        .generation(source.generation)
        .context("checkpoint source generation missing")?;
    ensure!(
        generation.connection.as_ref() == Some(&source.binding),
        "checkpoint source connection changed"
    );
    Ok(())
}

/// Publish the artifact before its manifest. Callers hold their role's continuation claim;
/// a subsequent transfer must still validate the source before fencing it.
pub(crate) fn commit(
    ctx: &Ctx,
    source: &PaidBinding,
    state: &AgentRunState,
    subject: SubjectRevision,
    context: &[u8],
    receipts: Vec<ReceiptRef>,
) -> Result<ContinuationCheckpoint> {
    let frontier = quiescent(state, source.generation).map_err(paused)?;
    source_matches(state, source)?;
    ensure!(
        subject.task.as_deref() == source.container.task(),
        "checkpoint subject does not name its source task"
    );
    ensure!(
        receipts == refs_for(state, &frontier).map_err(paused)?,
        "checkpoint receipts do not exactly cover the source frontier"
    );
    validate_receipt_owners(state, source)?;
    safe_context(context)?;
    let artifact = ContextArtifact {
        blob: BlobHash::of(context).to_ref(),
        bytes: context.len(),
    };
    let checkpoint = ContinuationCheckpoint {
        version: 1,
        source: source.clone(),
        subject,
        frontier,
        context: artifact,
        receipts,
        captured: ctx.store.now(),
    };
    // Refuse stale or fabricated caller snapshots before publishing either record.
    validate_source(&ctx.store, &checkpoint)?;
    let artifact = &checkpoint.context;
    // put_evidence is an upsert facade, so serialize the absent check and never call it
    // on an existing key. Equal content shares its original immutable artifact record.
    let key = artifact.blob.hash().as_str();
    {
        let _claim = ctx
            .store
            .try_lock(&format!("continuation-context-{key}"))
            .context("claiming immutable continuation context")?
            .context("continuation context publication is already in progress")?;
        match ctx
            .store
            .record::<ContextArtifact>(CONTEXT_AREA, key)
            .context("reading continuation context record")?
        {
            Some(existing) => {
                ensure!(existing == *artifact, "continuation context record changed");
                ensure!(
                    read_context(&ctx.store, &existing)? == context,
                    "continuation context bytes changed"
                );
            }
            None => {
                ctx.store
                    .put_evidence(
                        CONTEXT_AREA,
                        key,
                        context,
                        |blob| serde_json::json!({"blob": blob, "bytes": context.len()}),
                    )
                    .context("storing immutable continuation context")?;
            }
        }
    }
    let digest = checkpoint.digest()?;
    if !ctx
        .store
        .insert_record(CHECKPOINT_AREA, &digest, &checkpoint)
        .context("inserting immutable continuation checkpoint")?
    {
        ensure!(
            ctx.store
                .record::<ContinuationCheckpoint>(CHECKPOINT_AREA, &digest)?
                .as_ref()
                == Some(&checkpoint),
            "continuation checkpoint record changed"
        );
    }
    Ok(checkpoint)
}

fn read_context(store: &Store, artifact: &ContextArtifact) -> Result<Vec<u8>> {
    ensure!(
        artifact.bytes > 0 && artifact.bytes <= CONTEXT_MAX,
        "invalid continuation context size"
    );
    store.with_blob_lease(|| {
        let reader = store
            .blob_reader(artifact.blob.hash())
            .context("opening continuation context blob")?
            .context("continuation context blob missing")?;
        let mut bytes = Vec::with_capacity(artifact.bytes);
        reader
            .take(artifact.bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .context("reading continuation context blob")?;
        ensure!(
            bytes.len() == artifact.bytes && BlobHash::of(&bytes) == *artifact.blob.hash(),
            "continuation context bytes changed"
        );
        safe_context(&bytes)?;
        Ok(bytes)
    })
}

/// All failures are CheckpointInvalid, without echoing malformed private data from storage.
pub(crate) fn load(
    store: &Store,
    digest: &str,
    source: &PaidBinding,
    subject: &SubjectRevision,
) -> Result<(ContinuationCheckpoint, Vec<u8>)> {
    load_verified(store, digest, source, subject)
        .map_err(|_| anyhow!("CheckpointInvalid: continuation checkpoint verification failed"))
}

fn load_verified(
    store: &Store,
    digest: &str,
    source: &PaidBinding,
    subject: &SubjectRevision,
) -> Result<(ContinuationCheckpoint, Vec<u8>)> {
    BlobHash::parse(digest)?;
    let snapshot = store
        .read_only_snapshot()
        .context("reading checkpoint snapshot")?;
    let checkpoint = snapshot
        .record::<ContinuationCheckpoint>(CHECKPOINT_AREA, digest)?
        .context("continuation checkpoint missing")?;
    ensure!(
        checkpoint.version == 1
            && checkpoint.digest()? == digest
            && &checkpoint.source == source
            && &checkpoint.subject == subject
            && checkpoint.frontier.generation == source.generation
            && subject.task.as_deref() == source.container.task(),
        "continuation checkpoint identity changed"
    );
    let artifact = snapshot
        .record::<ContextArtifact>(CONTEXT_AREA, checkpoint.context.blob.hash().as_str())?
        .context("continuation context record missing")?;
    ensure!(
        artifact == checkpoint.context,
        "continuation context reference changed"
    );
    validate_source(&snapshot, &checkpoint)?;
    // Use the live store for the GC lease; snapshots cannot acquire leases. Hash and size
    // are checked explicitly even though blob_reader also verifies the content address.
    let bytes = read_context(store, &artifact)?;
    Ok((checkpoint, bytes))
}

fn validate_source(store: &Store, checkpoint: &ContinuationCheckpoint) -> Result<()> {
    match &checkpoint.source.container {
        ContainerRef::Pm => {
            let pm = agent::pm_state::load(store)?.context("checkpoint PM missing")?;
            validate_state(&pm.state, checkpoint)
        }
        ContainerRef::Assistant { session } => {
            let assistant = store
                .get_assistant_container(session)
                .context("reading checkpoint assistant")?;
            validate_state(&assistant.state, checkpoint)
        }
        container => {
            let task = store
                .get(container.task().context("checkpoint task missing")?)
                .context("reading checkpoint task")?;
            validate_subject(&task, checkpoint)?;
            let state = agent::container_state(&task, container)
                .context("checkpoint source container missing")?;
            validate_state(state, checkpoint)
        }
    }
}

fn validate_subject(task: &Task, checkpoint: &ContinuationCheckpoint) -> Result<()> {
    let subject = &checkpoint.subject;
    if let Some(revision) = &subject.spec_revision {
        let current = match checkpoint.source.container {
            ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. } => {
                crate::plan_spec_revision(task)?
            }
            _ => corral_core::store::spec_revision(task)?,
        };
        ensure!(
            current == *revision,
            "checkpoint task specification changed"
        );
    }
    let Some(target) = &subject.target else {
        return Ok(());
    };
    match &checkpoint.source.container {
        ContainerRef::Worker { run, .. } => {
            let run = task
                .runs
                .iter()
                .find(|candidate| candidate.id == *run)
                .context("checkpoint worker missing")?;
            let handoff = run
                .checkpoints
                .last()
                .context("checkpoint worker handoff missing")?;
            ensure!(
                format!(
                    "{}:{}",
                    handoff.checkpoint.input.head, handoff.checkpoint.input.tree
                ) == *target,
                "checkpoint worker handoff changed"
            );
        }
        ContainerRef::Reviewer { run, .. } => {
            let run = task
                .runs
                .iter()
                .find(|candidate| candidate.id == *run)
                .context("checkpoint reviewer parent missing")?;
            ensure!(
                run.review
                    .as_ref()
                    .is_some_and(|review| review.sha == *target),
                "checkpoint review target changed"
            );
        }
        ContainerRef::PlanReviewer { plan, .. } => {
            ensure!(
                plan == target && task.plans.iter().any(|record| record.id == *plan),
                "checkpoint plan target changed"
            );
        }
        ContainerRef::Planner { .. } | ContainerRef::Pm | ContainerRef::Assistant { .. } => {}
    }
    Ok(())
}

fn validate_state(state: &AgentRunState, checkpoint: &ContinuationCheckpoint) -> Result<()> {
    source_matches(state, &checkpoint.source)?;
    ensure!(
        frontier(state, checkpoint.source.generation).map_err(paused)? == checkpoint.frontier,
        "checkpoint source frontier changed"
    );
    ensure!(
        refs_for(state, &checkpoint.frontier).map_err(paused)? == checkpoint.receipts,
        "checkpoint canonical receipt changed"
    );
    validate_receipt_owners(state, &checkpoint.source)
}

fn validate_receipt_owners(state: &AgentRunState, source: &PaidBinding) -> Result<()> {
    for receipt in state
        .receipts
        .iter()
        .filter(|receipt| receipt.identity.generation == source.generation)
    {
        ensure!(
            receipt_owner(receipt, &source.container),
            "checkpoint receipt source changed"
        );
    }
    Ok(())
}

fn receipt_owner(receipt: &RequestReceipt, source: &ContainerRef) -> bool {
    let identity = &receipt.identity;
    let (task, run, review, assistant) = match source {
        ContainerRef::Worker { task, run } | ContainerRef::Planner { task, run } => {
            (Some(task.as_str()), Some(run.as_str()), None, None)
        }
        ContainerRef::Reviewer { task, run, review } => (
            Some(task.as_str()),
            Some(run.as_str()),
            Some(review.as_str()),
            None,
        ),
        ContainerRef::PlanReviewer { task, plan, review } => (
            Some(task.as_str()),
            Some(plan.as_str()),
            Some(review.as_str()),
            None,
        ),
        ContainerRef::Pm => (None, None, None, None),
        ContainerRef::Assistant { session } => (None, None, None, Some(session.as_str())),
    };
    identity.task.as_deref() == task
        && identity.run.as_deref() == run
        && identity.review.as_deref() == review
        && identity.assistant.as_deref() == assistant
}

fn safe_context(bytes: &[u8]) -> Result<()> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= CONTEXT_MAX,
        "unsafe continuation context size"
    );
    let text =
        std::str::from_utf8(bytes).map_err(|_| anyhow!("unsafe continuation context encoding"))?;
    ensure!(
        !text
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t')),
        "unsafe continuation context control bytes"
    );
    safe_text(text)?;
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
        safe_value(&value)?;
    }
    Ok(())
}

fn safe_value(value: &serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                let key = key.to_ascii_lowercase().replace(['-', '_'], "");
                // PM native context contains an operating-grant digest, not an HTTP
                // authorization header. Keep that canonical identity without accepting
                // arbitrary authorization strings or authentication objects.
                if key == "authorization" {
                    ensure!(
                        fields.contains_key("mode")
                            && fields.contains_key("revision")
                            && fields.contains_key("authority")
                            && (value.is_null()
                                || value.as_str().is_some_and(|id| BlobHash::parse(id).is_ok())),
                        "unsafe continuation context authorization"
                    );
                    continue;
                }
                ensure!(
                    !matches!(
                        key.as_str(),
                        "apikey"
                            | "accesstoken"
                            | "refreshtoken"
                            | "idtoken"
                            | "clientsecret"
                            | "password"
                            | "credentials"
                            | "cookie"
                            | "setcookie"
                            | "awssecretaccesskey"
                            | "awssessiontoken"
                            | "hostcap"
                            | "ctlcap"
                            | "bindnonce"
                            | "raw"
                            | "rawsdk"
                            | "sdkdump"
                            | "sdkresponse"
                            | "sdkerror"
                            | "sdksessionid"
                            | "rawresponse"
                            | "rawrequest"
                            | "rawevent"
                            | "parenttooluseid"
                            | "requestheaders"
                            | "responseheaders"
                            | "privatereasoning"
                            | "reasoning"
                            | "thinking"
                            | "redactedthinking"
                            | "encryptedcontent"
                            | "signature"
                            | "responsehandle"
                            | "previousresponseid"
                            | "responseid"
                    ),
                    "unsafe continuation context private field"
                );
                if key == "type" {
                    ensure!(
                        !value.as_str().is_some_and(|kind| matches!(
                            kind,
                            "thinking"
                                | "redacted_thinking"
                                | "reasoning"
                                | "reasoning_item"
                                | "input_json_delta"
                                | "content_block_delta"
                                | "message_delta"
                        )),
                        "unsafe continuation context provider stream"
                    );
                }
                safe_value(value)?;
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                safe_value(value)?;
            }
        }
        serde_json::Value::String(text) => safe_text(text)?,
        _ => {}
    }
    Ok(())
}

fn safe_text(text: &str) -> Result<()> {
    // Same credential families as setup_discovery::contains_secret, but require token
    // boundaries so ordinary words such as "task-" are not mistaken for an API key.
    let lower = text.to_ascii_lowercase();
    let token_secret = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '-' | '_' | '.'))
        .any(|token| {
            [
                "sk-",
                "sk_",
                "ghp_",
                "github_pat_",
                "xoxb-",
                "xoxp-",
                "aiza",
                "eyj",
            ]
            .iter()
            .any(|prefix| token.starts_with(prefix))
        });
    // Also inspect assignments in markdown, malformed JSON and nested JSON strings.
    // Parsing only the outer JSON would otherwise admit a credential in a quoted dump.
    let mut remaining = lower.as_str();
    while let Some(start) = remaining.find(|ch: char| ch.is_ascii_alphanumeric() || ch == '_') {
        remaining = &remaining[start..];
        let end = remaining
            .find(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '_' | '-'))
            .unwrap_or(remaining.len());
        let key = &remaining[..end];
        let rest = &remaining[end..];
        let private = matches!(
            key,
            "api_key"
                | "api-key"
                | "apikey"
                | "access_token"
                | "access-token"
                | "accesstoken"
                | "refresh_token"
                | "refresh-token"
                | "refreshtoken"
                | "id_token"
                | "idtoken"
                | "client_secret"
                | "client-secret"
                | "clientsecret"
                | "password"
                | "credentials"
                | "host_cap"
                | "hostcap"
                | "ctl_cap"
                | "ctlcap"
                | "bind_nonce"
                | "bindnonce"
                | "raw_sdk"
                | "raw-sdk"
                | "rawsdk"
                | "sdk_dump"
                | "sdkdump"
                | "private_reasoning"
                | "privatereasoning"
                | "thinking"
                | "encrypted_content"
                | "previous_response_id"
                | "previousresponseid"
                | "response_handle"
                | "responsehandle"
        );
        ensure!(
            !private
                || !rest
                    .trim_start_matches(['\"', '\''])
                    .trim_start()
                    .starts_with([':', '=']),
            "unsafe continuation context private assignment"
        );
        remaining = rest;
    }
    ensure!(
        !token_secret
            && ![
                "bearer ",
                "-----begin private key",
                "-----begin rsa private key",
                "-----begin openssh private key",
                "api_key=",
                "api-key=",
                "apikey=",
                "access_token=",
                "refresh_token=",
                "client_secret=",
                "password=",
                "aws_secret_access_key",
                "aws_session_token",
                "authorization:",
                "raw_sdk",
                "raw sdk dump",
                "sdk_dump",
                "private_reasoning",
                "<thinking>",
                "<analysis>",
            ]
            .iter()
            .any(|marker| lower.contains(marker)),
        "unsafe continuation context secret or provider-private material"
    );
    Ok(())
}
