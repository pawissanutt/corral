//! Internal accounting service. No CLI, agent tool, automatic adoption or recovery route
//! invokes it. Import is record-only; even a confirmed import grants no allowance.
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::blob_ref::BlobHash;
use corral_core::{
    RepairHistory, RepairLedger, RepairPolicy, RepairProjection, RepairSources, Store, TaskRef,
};
use serde::{Deserialize, Serialize};

mod authority;
mod chronology;
mod continuation;
pub use authority::{Confirmation, confirm};
pub use chronology::{CommitSource, HistoricalEntry};
pub use continuation::{
    ContinuationConfirmation, ContinuationPreview, ContinuationRequest, confirm_continuation,
    preview_continuation, renew_continuation,
};

pub(crate) fn check_route(store: &Store, task: &impl TaskRef, ledger: &RepairLedger) -> Result<()> {
    if let Some(pin) =
        store.record::<continuation::RoutePin>("repair-continuation-route", task.id())?
    {
        ensure!(pin.version == 1, "unsupported continuation executor pin");
        if ledger
            .active()?
            .renewal
            .as_ref()
            .is_some_and(|r| r.id == pin.renewal)
        {
            let run = task
                .current_run_head()?
                .context("pinned continuation run missing")?;
            ensure!(
                run.execution
                    .as_ref()
                    .is_some_and(|e| e.resolved == pin.executor)
                    && run.agent == pin.executor.profile
                    && run.model == pin.executor.model
                    && run.effort == pin.executor.effort
                    && run.harness.as_deref() == Some(pin.executor.harness.as_str()),
                "continuation executor differs from owner-approved route"
            );
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub id: String,
    pub task: String,
    pub expected_revision: u64,
    pub expected_run: String,
    pub expected_contract: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    pub version: u32,
    pub request: Request,
    pub sources: RepairSources,
    pub lineage: String,
    pub policy: RepairPolicy,
    pub profile: String,
    pub entries: Vec<HistoricalEntry>,
    pub commits: Vec<CommitSource>,
    pub digest: String,
}

pub use corral_core::RepairImportReceipt as Receipt;

fn digest(value: &impl Serialize) -> Result<String> {
    Ok(BlobHash::of(&serde_json::to_vec(value)?).to_string())
}

pub fn preview(ctx: &Ctx, request: &Request) -> Result<Preview> {
    ensure!(
        request.version == 1 && !request.id.trim().is_empty() && request.id.len() <= 128,
        "invalid repair import request"
    );
    ensure!(
        Store::normalize_id(&request.task)? == request.task,
        "noncanonical repair task"
    );
    let store = ctx.store.read_only_snapshot()?;
    let sources = store.repair_sources(&request.task)?;
    ensure!(
        sources.head.header.rev == request.expected_revision,
        "repair source task revision changed"
    );
    let head = store.head(&request.task)?;
    ensure!(
        head.contract_revision()? == request.expected_contract
            && head
                .current_run_head()?
                .is_some_and(|r| r.id == request.expected_run),
        "repair source contract/run changed"
    );
    ensure!(
        head.launch.is_none() && !head.queued && head.status != corral_core::Status::Running,
        "repair import requires no active/prepared writer"
    );
    ensure!(
        head.current_run_head()?
            .is_some_and(|r| r.outcome == Some(corral_core::RunOutcome::Done)),
        "repair import requires a submitted candidate"
    );
    ensure!(
        sources
            .records
            .get("repair-import")
            .is_some_and(Option::is_none)
            && sources
                .records
                .get("repair-import-consumed")
                .is_some_and(Option::is_none),
        "task repair import lineage already consumed"
    );
    chronology::check_existing(&sources)?;
    let (policy, profile, entries, commits) = chronology::reconstruct(&store, &head, &sources)?;
    let lineage = digest(&(
        "corral-legacy-repair-import-v1",
        &sources.store,
        &request.task,
    ))?;
    let mut proposal = Preview {
        version: 1,
        request: request.clone(),
        sources,
        lineage,
        policy,
        profile,
        entries,
        commits,
        digest: String::new(),
    };
    proposal.digest = digest(&proposal)?;
    Ok(proposal)
}

/// Consumes an exact in-process owner confirmation. JSON, PM prose and an `approved_by`
/// field cannot manufacture it. Publication joins established dispatch/task serialization.
pub fn apply(ctx: &Ctx, confirmation: Confirmation) -> Result<Receipt> {
    authority::recheck(&ctx.store, &confirmation)?;
    apply_checked(ctx, confirmation)
}

fn apply_checked(ctx: &Ctx, confirmation: Confirmation) -> Result<Receipt> {
    let proposal = confirmation.preview;
    let id = &proposal.request.task;
    let mut receipt = None;
    ctx.store.dispatch_update_head(id, |store, _task| {
        let _observation = store.try_lock(&format!("repair-observe-{id}"))?.context("repair observation in progress")?;
        let _profile = store.try_lock(&format!("repair-renew-{id}"))?.context("repair renewal in progress")?;
        if let Some(old) = store.record::<Receipt>("repair-import", id)? {
            ensure!(old.version == 1 && old.request == proposal.request.id && old.preview_digest == proposal.digest && old.lineage == proposal.lineage, "repair import request/lineage cannot be reused");
            receipt = Some(old);
            return Ok(());
        }
        let fresh = Ctx { store:store.clone(), ..ctx.clone() };
        ensure!(preview(&fresh, &proposal.request)? == proposal, "repair import preview changed before publication");
        ensure!(store.record_row("repair-import-consumed", id)?.is_none(), "repair import lineage consumed");
        let proof = serde_json::to_vec(&serde_json::json!({"version":1,"preview":proposal,"confirmation":confirmation.provenance}))?;
        let evidence = BlobHash::of(&proof).to_ref();
        let events = proposal.entries.iter().filter_map(|entry| entry.accounting.clone()).collect();
        let history = RepairHistory::projected(proposal.policy.clone(), &proposal.profile,
            RepairProjection { version:1, evidence:evidence.clone(), lineage:proposal.lineage.clone() }, events)?;
        let ledger = RepairLedger::new(history);
        ledger.validate()?;
        let next = Receipt { version:1, request:proposal.request.id.clone(), preview_digest:proposal.digest.clone(),
            lineage:proposal.lineage.clone(), evidence:evidence.clone(), ledger_digest:ledger.digest()? };
        let source_digest = proposal.sources.digest()?;
        store.stage_task_evidence("repair-import-evidence", id, &proof, |blob| serde_json::json!({"version":1,"evidence":blob,"lineage":proposal.lineage,"source_blobs":proposal.sources.verified_blobs,"source_end":proposal.sources.end,"source_digest":source_digest}))?;
        // This is the sole supported replacement of a recognized fail-closed placeholder.
        // Its complete original document remains in the proof; substantive history refuses.
        store.publish_repair_import(id, proposal.sources.records.get(corral_core::REPAIR_RUNTIME).and_then(Option::as_ref), &ledger, &next)?;
        store.log_event(Some(id), "repair_history_imported", serde_json::json!({"receipt":next,"starts":0,"grants":0}))?;
        receipt = Some(next);
        Ok(())
    })?;
    receipt.context("repair import receipt missing")
}

#[cfg(test)]
mod tests;
