//! Evidence-only stale admission reconciliation. No executor work, owner input, grant,
//! driver transition, budget credit or handoff attestation is performed by this service.
use anyhow::{Context, Result, ensure};
use corral_core::{Store, TaskRef, assistant::AssistantContainer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{boundary, effect_evidence as facts};

#[path = "effect_recovery_authority.rs"]
mod authority;
#[path = "effect_journal.rs"]
mod journal;
#[path = "effect_legacy.rs"]
mod legacy;
pub use authority::{Confirmation, confirm};

#[cfg(test)]
#[path = "effect_recovery_tests.rs"]
mod tests;

const RECEIPTS: &str = "assistant-effect-recovery-receipts";
const OUTCOMES: &str = "assistant-effect-reconciliations";
const BLOBS: &str = "assistant-effect-proof-blobs";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub assistant: String,
    pub task: Option<String>,
    pub generation: u32,
    pub admissions: Vec<String>,
    pub request_id: String,
}
impl Request {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.assistant.starts_with("A-") && self.generation > 0,
            "invalid recovery target"
        );
        ensure!(
            !self.request_id.is_empty()
                && self.request_id.len() <= 128
                && self
                    .request_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)),
            "invalid reconciliation request ID"
        );
        ensure!(
            !self.admissions.is_empty()
                && self.admissions.len() <= 128
                && self.admissions.iter().all(|id| !id.is_empty()
                    && id.len() <= 256
                    && !id.chars().any(char::is_control)),
            "select 1 through 128 exact admission IDs"
        );
        let mut ids = self.admissions.clone();
        ids.sort();
        ids.dedup();
        ensure!(
            ids.len() == self.admissions.len(),
            "duplicate selected admission"
        );
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Proof {
    Managed {
        evidence: facts::Evidence,
        record_version: u64,
    },
    LegacyCommand {
        proof: legacy::LegacyProof,
    },
    RetainedLifecycle {
        proof: journal::JournalProof,
    },
}
impl Proof {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Managed { evidence, .. } => evidence.require_terminal(),
            Self::LegacyCommand { proof } => proof.validate(),
            Self::RetainedLifecycle { proof } => proof.validate(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selected {
    pub admission: String,
    pub activity: Option<boundary::Activity>,
    pub observation: Option<Observation>,
    pub proof: Option<Proof>,
    pub proof_sha256: Option<String>,
    pub blocked: Option<String>,
    pub reconciled_by: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blocker {
    pub admission: String,
    pub generation: u32,
    pub tool: String,
    pub reason: String,
    pub observation: Option<Observation>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub key: String,
    pub record_version: u64,
    pub sha256: String,
    /// Diagnostic only. Unknown/mismatched/unsealed bytes are not a terminal proof.
    pub evidence: Option<facts::Evidence>,
}
fn observation(
    store: &Store,
    assistant: &str,
    row: &boundary::Activity,
) -> Result<Option<Observation>> {
    let key = facts::key(assistant, row.generation, &row.id);
    store
        .record_row(facts::AREA, &key)?
        .map(|record| -> Result<Observation> {
            Ok(Observation {
                key,
                record_version: record.version,
                sha256: digest(&record.doc)?,
                evidence: serde_json::from_value(record.doc).ok(),
            })
        })
        .transpose()
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    pub version: u32,
    pub request: Request,
    pub context: Value,
    pub boundary_sha256: String,
    pub boundary_version: Option<u64>,
    pub selected: Vec<Selected>,
    pub retained: Vec<Blocker>,
    pub digest: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProofBlob {
    version: u32,
    blob: corral_core::blob_ref::BlobRef,
    sha256: String,
}

fn prepare_blob(store: &Store, bytes: &[u8]) -> Result<()> {
    let hash = facts::digest(bytes);
    let reference = corral_core::blob_ref::BlobHash::of(bytes).to_ref();
    if store.record_row(BLOBS, &hash)?.is_none() {
        store.put_evidence(
            BLOBS,
            &hash,
            bytes,
            |blob| json!({"version":1,"blob":blob,"sha256":hash}),
        )?;
    }
    let saved: ProofBlob = store
        .record(BLOBS, &hash)?
        .context("retained proof blob record missing")?;
    ensure!(
        saved
            == ProofBlob {
                version: 1,
                blob: reference.clone(),
                sha256: hash
            },
        "immutable proof blob reference differs"
    );
    let artifact = corral_core::assistant_workflow::DesignArtifact {
        blob: reference,
        bytes: bytes.len() as u32,
    };
    ensure!(
        super::publication::load_artifact(store, &artifact)? == bytes,
        "retained proof bytes differ"
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowOutcome {
    pub admission: String,
    pub proof_sha256: String,
    pub proof_blob: String,
    pub receipt: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub version: u32,
    pub request: Request,
    pub payload_sha256: String,
    pub preview_sha256: String,
    pub outcomes: Vec<RowOutcome>,
    pub remaining_count: usize,
    pub remaining_sha256: String,
    pub at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reconciliation {
    version: u32,
    request: Request,
    context: Value,
    confirmation: authority::Provenance,
    outcome: RowOutcome,
    original: boundary::Activity,
    old_boundary_sha256: String,
    new_boundary_sha256: String,
    retained: Vec<Blocker>,
}

fn digest(value: &impl Serialize) -> Result<String> {
    Ok(facts::digest(&serde_json::to_vec(value)?))
}
fn receipt_key(assistant: &str, request: &str) -> String {
    format!("{assistant}/{}", facts::digest(request.as_bytes()))
}

pub fn lookup(store: &Store, assistant: &str, request_id: &str) -> Result<Option<Receipt>> {
    let receipt: Option<Receipt> = store.record(RECEIPTS, &receipt_key(assistant, request_id))?;
    if let Some(receipt) = &receipt {
        ensure!(
            receipt.version == 1
                && receipt.request.assistant == assistant
                && receipt.request.request_id == request_id,
            "unsupported/mismatched recovery receipt"
        );
    }
    Ok(receipt)
}

/// Keyed immutable audit lookup, never a growing session export or a new confirmation.
pub fn outcome(store: &Store, assistant: &str, generation: u32, admission: &str) -> Result<Value> {
    let record: Option<Reconciliation> =
        store.record(OUTCOMES, &facts::key(assistant, generation, admission))?;
    if let Some(record) = &record {
        ensure!(
            record.version == 1
                && record.request.assistant == assistant
                && record.request.generation == generation
                && record.outcome.admission == admission,
            "reconciliation identity/version mismatch"
        );
    }
    Ok(
        json!({"reconciliation":record,"authority":"historical acknowledgement only; no current authority"}),
    )
}

/// No writes or process operations. Unknown selected evidence is reported, never omitted.
pub fn preview(store: &Store, request: &Request) -> Result<Preview> {
    request.validate()?;
    store.inspect_assistant_container(&request.assistant, |container, ended| {
        let _profiles = request
            .task
            .as_ref()
            .map(|_| {
                store
                    .try_lock("profile-records")?
                    .context("repair history writer busy; obtain a fresh preview later")
            })
            .transpose()?;
        build(store, request, container, ended, None)
    })
}

fn context(
    store: &Store,
    request: &Request,
    container: &AssistantContainer,
    ended: bool,
) -> Result<Value> {
    container.validate_protocol()?;
    let source = container
        .state
        .generations
        .iter()
        .find(|g| g.n == request.generation)
        .context("source generation missing")?;
    ensure!(
        source.fenced.is_some(),
        "ordinary/live source must use its generation-authenticated host lifecycle, not recovery"
    );
    let bound = source
        .bound
        .as_ref()
        .context("source bound session missing; no executor identity inferred")?;
    let session = store.get_assistant(&request.assistant)?;
    let task = request
        .task
        .as_deref()
        .map(|id| store.get(id))
        .transpose()?;
    if let Some(task) = &task {
        ensure!(
            task.project.as_deref() == Some(container.state.binding.project.as_str()),
            "task/project mismatch"
        );
    }
    if let Some(index) = &container.workflow {
        use corral_core::assistant_workflow::WorkflowPhase;
        ensure!(
            !matches!(
                index.current.phase,
                WorkflowPhase::HandoffPreparing | WorkflowPhase::HandoffReconciling
            ),
            "competing handoff publication must settle before effect reconciliation"
        );
    }
    let task_pin = task.as_ref().map(|t| -> Result<Value> {
        let fields = serde_json::to_value(t)?;
        let repair = store.repair_ledger(&t.id)?;
        let counters = repair.as_ref().map(|r| r.active()?.history.counters()).transpose()?;
        let profile = store.record_row("profile-repairs", &t.id)?.map(|r| -> Result<Value> {
            Ok(json!({"version":r.version,"sha256":digest(&r.doc)?}))
        }).transpose()?;
        Ok(json!({"id":t.id,"contract":t.contract_revision()?,"snapshot_sha256":digest(t)?,
            "repair_sha256":repair.as_ref().map(digest).transpose()?,"repair_counters":counters,
            "repair_version":store.record_row(corral_core::REPAIR_RUNTIME, &t.id)?.map(|r| r.version),
            "profile_repairs":profile,
            "completion":fields.get("completion"),"cowork":fields.get("cowork"),
            "run":t.current_run().map(|r| &r.id)}))
    }).transpose()?;
    // Full immutable pins cover owner-input/grants, scratch, task driver/run/check/budget and
    // workflow changes without modifying any of them. Task writers share the dispatch lock.
    Ok(
        json!({"store":container.state.binding.store, "project":container.state.binding.project,
        "assistant":request.assistant,"source":{"generation":source.n,"bound":source.bound,"fenced":source.fenced,"assets_sha256":source.assets_sha256},
        "current_binding":container.state.binding,
        "current_host":container.state.current_generation().map(|g| json!({"generation":g.n,"bound":g.bound,"fenced":g.fenced,"assets_sha256":g.assets_sha256})),"ended":ended,
        "source_session":bound.session_id,"workflow":super::summary(container)?,
        "session":{"id":session.id,"project":session.project,"focus":session.focus,"linked_scratch":session.linked_scratch,"scratch":session.scratch,"sha256":digest(&session)?},
        "container_sha256":digest(container)?,
        "container_version":store.record_row("assistant-containers", &request.assistant)?.map(|r| r.version),
        "session_version":store.record_row("assistant-sessions", &request.assistant)?.map(|r| r.version),
        "task":task_pin,
        "authority":"record-only; no drain, input, consent, driver or budget change"}),
    )
}

fn proof(
    store: &Store,
    request: &Request,
    container: &AssistantContainer,
    row: &boundary::Activity,
    staged: Option<&Preview>,
) -> Result<Proof> {
    let key = facts::key(&request.assistant, request.generation, &row.id);
    let canonical = store
        .record_row(facts::AREA, &key)?
        .map(|record| -> Result<_> {
            let evidence: facts::Evidence =
                serde_json::from_value(record.doc).context("unsupported executor evidence")?;
            evidence.validate_target(
                store,
                &request.assistant,
                container,
                request.generation,
                row,
            )?;
            Ok((evidence, record.version))
        })
        .transpose()?;
    if let Some((evidence, version)) = &canonical
        && evidence.sealed
    {
        evidence.require_terminal()?;
        return Ok(Proof::Managed {
            evidence: evidence.clone(),
            record_version: *version,
        });
    }
    if let Some(staged) = staged {
        let selected = staged
            .selected
            .iter()
            .find(|s| s.admission == row.id)
            .context("staged proof missing")?;
        let proof = selected.proof.as_ref().context("staged proof unknown")?;
        ensure!(
            Some(digest(proof)?) == selected.proof_sha256,
            "immutable staged proof changed"
        );
        match proof {
            Proof::RetainedLifecycle { proof } => {
                ensure!(
                    proof.canonical_version == canonical.as_ref().map(|(_, v)| *v),
                    "canonical proof CAS changed"
                );
                if let Some((prefix, _)) = &canonical {
                    journal::require_prefix(prefix, &proof.evidence)?;
                }
            }
            Proof::LegacyCommand { .. } => {
                ensure!(canonical.is_none(), "canonical executor proof appeared")
            }
            _ => anyhow::bail!("staged proof source changed"),
        }
        proof.validate()?;
        return Ok(proof.clone());
    }
    if row.tool == "bash" && canonical.is_none() {
        return Ok(Proof::LegacyCommand {
            proof: legacy::verify(store, &request.assistant, container, row)?,
        });
    }
    journal::verify(store, &request.assistant, container, row, canonical)
        .map(|proof| Proof::RetainedLifecycle { proof })
        .context("resource has no authoritative shutdown identity/proof")
}

fn build(
    store: &Store,
    request: &Request,
    container: &AssistantContainer,
    ended: bool,
    staged: Option<&Preview>,
) -> Result<Preview> {
    let context = context(store, request, container, ended)?;
    let active = boundary::activities(store, &request.assistant)?;
    ensure!(
        active.len() <= 128,
        "admission ledger exceeds supported complete preview bound"
    );
    let mut selected = Vec::new();
    for id in &request.admissions {
        let key = facts::key(&request.assistant, request.generation, id);
        let old: Option<Reconciliation> = store.record(OUTCOMES, &key)?;
        if let Some(old) = old {
            ensure!(
                old.version == 1
                    && old.request.assistant == request.assistant
                    && old.request.generation == request.generation
                    && old.outcome.admission == *id,
                "reconciliation identity/version mismatch"
            );
            selected.push(Selected {
                admission: id.clone(),
                activity: Some(old.original),
                observation: None,
                proof: None,
                proof_sha256: Some(old.outcome.proof_sha256),
                blocked: None,
                reconciled_by: Some(old.outcome.receipt),
            });
            continue;
        }
        let activity = active
            .iter()
            .find(|a| a.id == *id && a.generation == request.generation)
            .cloned();
        let observed = activity
            .as_ref()
            .map(|row| observation(store, &request.assistant, row))
            .transpose()?
            .flatten();
        let result = activity
            .as_ref()
            .context("exact source admission missing")
            .and_then(|row| proof(store, request, container, row, staged));
        selected.push(match result {
            Ok(proof) => Selected {
                admission: id.clone(),
                activity,
                observation: observed,
                proof_sha256: Some(digest(&proof)?),
                proof: Some(proof),
                blocked: None,
                reconciled_by: None,
            },
            Err(error) => Selected {
                admission: id.clone(),
                activity,
                observation: observed,
                proof: None,
                proof_sha256: None,
                blocked: Some(format!("{error:#}")),
                reconciled_by: None,
            },
        });
    }
    let retained = active
        .iter()
        .filter(|row| {
            !selected.iter().any(|s| {
                s.admission == row.id && s.blocked.is_none() && row.generation == request.generation
            })
        })
        .map(|row| -> Result<Blocker> {
            Ok(Blocker {
                admission: row.id.clone(),
                generation: row.generation,
                tool: row.tool.clone(),
                reason: selected
                    .iter()
                    .find(|s| s.admission == row.id)
                    .and_then(|s| s.blocked.clone())
                    .unwrap_or_else(|| {
                        "unselected admission remains outstanding; no executor settlement inferred"
                            .into()
                    }),
                observation: observation(store, &request.assistant, row)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut result = Preview {
        version: 1,
        request: request.clone(),
        context,
        boundary_sha256: digest(&active)?,
        boundary_version: store
            .record_row(boundary::LEDGER, &request.assistant)?
            .map(|r| r.version),
        selected,
        retained,
        digest: String::new(),
    };
    ensure!(
        serde_json::to_vec(&result)?.len() <= 1024 * 1024,
        "complete preview exceeds bound; select fewer admissions (no rows were cleared)"
    );
    result.digest = digest(&result)?;
    Ok(result)
}

/// The only production mutation entry takes an opaque controlling-terminal confirmation.
/// Proof validation/preparation is outside the assistant unit; final pins are rechecked under
/// its established dispatch lock and all row outcomes, receipt and audit publish together.
pub fn apply(store: &Store, confirmation: Confirmation) -> Result<(Receipt, bool)> {
    authority::recheck(store, &confirmation)?;
    apply_confirmed(store, confirmation)
}

fn apply_confirmed(store: &Store, confirmation: Confirmation) -> Result<(Receipt, bool)> {
    let wanted = &confirmation.preview;
    let request = &wanted.request;
    let payload = digest(request)?;
    if let Some(saved) = lookup(store, &request.assistant, &request.request_id)? {
        ensure!(
            saved.payload_sha256 == payload,
            "reconciliation request payload changed"
        );
        return Ok((saved, true));
    }
    let fresh = preview(store, request)?;
    ensure!(
        fresh == *wanted,
        "stale effect recovery preview; inspect a fresh exact proposal"
    );
    ensure!(
        fresh.selected.iter().all(|s| s.blocked.is_none()),
        "unknown selected executors cannot be reconciled"
    );
    for selected in &fresh.selected {
        if let Some(proof) = &selected.proof {
            proof.validate()?;
            let bytes = serde_json::to_vec(proof)?;
            let hash = facts::digest(&bytes);
            ensure!(
                selected.proof_sha256.as_deref() == Some(&hash),
                "proof digest changed"
            );
            prepare_blob(store, &bytes)?;
        }
    }
    // Same writer order as assistant/task publication. Keep BOTH leases through the actual
    // record-unit commit, not just its preparation callback (profile history also has an
    // independent writer). Stage joins this existing unit; it never takes another board lock.
    let _board = store
        .try_lock("dispatch")?
        .context("assistant/task writer busy; no reconciliation applied")?;
    let _profiles = request
        .task
        .as_ref()
        .map(|_| {
            store
                .try_lock("profile-records")?
                .context("repair history writer busy; no reconciliation applied")
        })
        .transpose()?;
    store.records_transaction(|| store.stage_assistant_container(&request.assistant, |container, ended| {
        if let Some(saved) = lookup(store, &request.assistant, &request.request_id)? {
            ensure!(saved.payload_sha256 == payload, "reconciliation request payload changed");
            return Ok((saved, true));
        }
        // Build once more under the same lock as end/resume/start/handoff/task writers. It
        // revalidates retained canonical evidence, rather than trusting serialized preview.
        ensure!(build(store, request, container, ended, Some(&fresh))? == fresh, "stale recovery CAS; no rows changed");
        let mut active = boundary::activities(store, &request.assistant)?;
        let original = active.clone();
        let mut outcomes = Vec::new();
        for selected in &fresh.selected {
            if selected.reconciled_by.is_some() {
                let old: Reconciliation = store.record(OUTCOMES, &facts::key(&request.assistant, request.generation, &selected.admission))?
                    .context("immutable row outcome missing")?;
                ensure!(Some(&old.outcome.proof_sha256) == selected.proof_sha256.as_ref(), "reconciled proof changed");
                outcomes.push(old.outcome); continue;
            }
            let hash = selected.proof_sha256.as_deref().context("missing staged proof digest")?;
            let blob: ProofBlob = store.record(BLOBS, hash)?.context("immutable staged proof reference missing")?;
            ensure!(blob.version == 1 && blob.sha256 == hash && blob.blob.hash().as_str() == hash,
                "immutable staged proof reference changed");
            active.retain(|row| !(row.id == selected.admission && row.generation == request.generation));
            outcomes.push(RowOutcome { admission: selected.admission.clone(), proof_sha256: selected.proof_sha256.clone().context("missing proof")?,
                proof_blob: selected.proof_sha256.clone().context("missing proof blob")?, receipt: request.request_id.clone() });
        }
        if active.len() == original.len() {
            let first = outcomes.first().context("no row outcomes")?;
            let saved = lookup(store, &request.assistant, &first.receipt)?.context("original reconciliation receipt missing")?;
            ensure!(outcomes.iter().all(|o| saved.outcomes.contains(o)), "already-reconciled selection spans receipts; inspect keyed outcomes");
            return Ok((saved, true));
        }
        let new_digest = digest(&active)?;
        let receipt = Receipt { version: 1, request: request.clone(), payload_sha256: payload.clone(), preview_sha256: fresh.digest.clone(),
            outcomes, remaining_count: active.len(), remaining_sha256: new_digest.clone(), at: corral_core::clock::now().to_string() };
        store.save_record(boundary::LEDGER, &request.assistant, &active)?;
        for outcome in &receipt.outcomes {
            if outcome.receipt != request.request_id { continue; }
            let row = original.iter().find(|r| r.id == outcome.admission && r.generation == request.generation).context("source admission changed")?;
            let reconciliation = Reconciliation { version: 1, request: request.clone(), context: fresh.context.clone(), confirmation: confirmation.provenance.clone(),
                outcome: outcome.clone(), original: row.clone(), old_boundary_sha256: fresh.boundary_sha256.clone(),
                new_boundary_sha256: new_digest.clone(), retained: fresh.retained.clone() };
            ensure!(store.insert_record(OUTCOMES, &facts::key(&request.assistant, request.generation, &row.id), &reconciliation)?, "reconciliation row already exists");
        }
        ensure!(store.insert_record(RECEIPTS, &receipt_key(&request.assistant, &request.request_id), &receipt)?, "request receipt already exists");
        store.log_event(None, "assistant_effects_reconciled", json!({"assistant":request.assistant,"generation":request.generation,
            "request":request.request_id,"preview":fresh.digest,"old_boundary":fresh.boundary_sha256,"new_boundary":new_digest,
            "proofs":receipt.outcomes,"retained":fresh.retained,"confirmation":confirmation.provenance}))?;
        Ok((receipt, false))
    }))
}
