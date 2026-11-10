//! A separate, once-only owner approval for one continuation of an imported task.
//! It remains staged under the existing dispatch hold; this is NOT custody or launch.
use super::*;
use corral_core::blob_ref::BlobRef;
use corral_core::{RepairCounters, RepairRenewal, ResolvedProfile};
use std::io::{BufRead, IsTerminal, Read, Write};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationRequest {
    pub version: u32,
    pub id: String,
    pub task: String,
    pub previous_digest: String,
    pub import_lineage: String,
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationPreview {
    pub version: u32,
    pub request: ContinuationRequest,
    pub revision: u64,
    pub contract: String,
    pub run: String,
    pub imported: Receipt,
    pub consumed: RepairCounters,
    pub executor: ResolvedProfile,
    pub digest: String,
}

#[derive(Debug)]
pub struct ContinuationConfirmation {
    preview: ContinuationPreview,
    reason: String,
    at: jiff::Timestamp,
    terminal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Consumed {
    version: u32,
    request: String,
    preview: String,
    lineage: String,
    approval: BlobRef,
    renewal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RoutePin {
    pub version: u32,
    pub renewal: String,
    pub executor: ResolvedProfile,
}

pub fn preview_continuation(
    ctx: &Ctx,
    request: &ContinuationRequest,
) -> Result<ContinuationPreview> {
    ensure!(
        request.version == 1
            && !request.id.trim().is_empty()
            && request.id.len() <= 128
            && Store::normalize_id(&request.task)? == request.task,
        "invalid continuation request"
    );
    let store = ctx.store.read_only_snapshot()?;
    ensure!(
        store
            .record_row("repair-renewal-consumed", &request.task)?
            .is_none(),
        "import continuation approval lineage already consumed"
    );
    let imported: Receipt = store
        .record("repair-import", &request.task)?
        .context("exact repair import missing")?;
    ensure!(
        imported.version == 1 && imported.lineage == request.import_lineage,
        "import lineage differs"
    );
    let consumed: Receipt = store
        .record("repair-import-consumed", &request.task)?
        .context("import consumption index missing")?;
    ensure!(
        imported == consumed,
        "import lineage index contradicts receipt"
    );
    let ledger = store
        .repair_ledger(&request.task)?
        .context("imported history missing")?;
    ensure!(
        ledger.allowances.len() == 1
            && ledger.digest()? == request.previous_digest
            && ledger
                .active()?
                .history
                .projection()
                .is_some_and(|p| p.lineage == imported.lineage && p.evidence == imported.evidence),
        "continuation requires the exact unrenewed imported history"
    );
    let counts = ledger.task_counters()?;
    ensure!(
        counts.initial_implementations == 1
            && matches!(
                ledger.transition()?,
                corral_core::RepairTransition::Owner { .. }
            )
            && counts.pm_diagnoses == 0
            && !ledger.unresolved()?.is_empty(),
        "original normal allowance must be exhausted with unresolved findings"
    );
    let head = store.head(&request.task)?;
    ensure!(
        head.dispatch_hold
            && !head.queued
            && head.launch.is_none()
            && head.status != corral_core::Status::Running,
        "continuation must stay staged under dispatch hold with no prepared writer"
    );
    let run = head
        .current_run_head()?
        .context("continuation run missing")?;
    ensure!(
        run.outcome == Some(corral_core::RunOutcome::Done),
        "continuation requires a submitted candidate"
    );
    let config = store.config_layers()?.config()?;
    config.check_profile_eligibility(
        &request.profile,
        corral_core::ProfileRole::Implementation,
        head.project.as_deref(),
        false,
    )?;
    let executor = config.resolve_profile(
        &corral_core::ProfileChoice {
            profile: Some(request.profile.clone()),
            ..Default::default()
        },
        None,
        None,
    )?;
    // The import's typed blob remains verified authority for history, NOT renewal consent.
    let mut proof = Vec::new();
    store
        .blob_reader(imported.evidence.hash())?
        .context("import proof missing")?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut proof)
        .context("verifying import proof")?;
    ensure!(
        proof.len() <= 8 * 1024 * 1024 && BlobHash::of(&proof) == *imported.evidence.hash(),
        "invalid/oversized import proof"
    );
    let mut proposal = ContinuationPreview {
        version: 1,
        request: request.clone(),
        revision: head.rev,
        contract: head.contract_revision()?,
        run: run.id.clone(),
        imported,
        consumed: counts,
        executor,
        digest: String::new(),
    };
    proposal.digest = digest(&proposal)?;
    Ok(proposal)
}

pub fn confirm_continuation(
    ctx: &Ctx,
    proposal: &ContinuationPreview,
    reason: &str,
) -> Result<ContinuationConfirmation> {
    crate::recovery::authority::require_owner_store(&ctx.store)?;
    ensure!(
        !reason.trim().is_empty() && reason.len() <= 4096 && std::io::stdin().is_terminal(),
        "genuine interactive continuation approval required"
    );
    ensure!(
        preview_continuation(ctx, &proposal.request)? == *proposal,
        "continuation preview stale"
    );
    #[cfg(unix)]
    {
        let mut tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("opening owner continuation terminal")?;
        ensure!(tty.is_terminal(), "owner controlling terminal required");
        let challenge = format!("one repair {} {}", proposal.request.task, proposal.digest);
        writeln!(tty,"One corrective continuation, NOT a fresh initial implementation.\n{}\nGrant: current=1 stronger=0 overall=1 diagnosis=0. Dispatch hold stays set.\nReason: {reason}\nType `{challenge}`:",serde_json::to_string_pretty(proposal)?).context("displaying exact continuation approval")?;
        tty.flush().context("flushing continuation challenge")?;
        let mut answer = String::new();
        std::io::BufReader::new(&tty)
            .take(1024)
            .read_line(&mut answer)
            .context("reading genuine continuation owner input")?;
        ensure!(
            answer.ends_with('\n') && answer.trim_end() == challenge,
            "continuation cancelled"
        );
        crate::recovery::authority::require_owner_store(&ctx.store)?;
        ensure!(
            preview_continuation(ctx, &proposal.request)? == *proposal,
            "continuation changed during confirmation"
        );
        Ok(ContinuationConfirmation {
            preview: proposal.clone(),
            reason: reason.trim().into(),
            at: ctx.store.now(),
            terminal: "/dev/tty".into(),
        })
    }
    #[cfg(not(unix))]
    anyhow::bail!("continuation confirmation requires a supported unmanaged terminal")
}

pub fn renew_continuation(
    ctx: &Ctx,
    confirmation: ContinuationConfirmation,
) -> Result<RepairLedger> {
    crate::recovery::authority::require_owner_store(&ctx.store)?;
    renew_checked(ctx, confirmation)
}

fn renew_checked(ctx: &Ctx, confirmation: ContinuationConfirmation) -> Result<RepairLedger> {
    let proposal = &confirmation.preview;
    let id = &proposal.request.task;
    ctx.store.dispatch_update_head(id, |store,_| {
        let _observation = store.try_lock(&format!("repair-observe-{id}"))?.context("repair observation in progress")?;
        if let Some(old) = store.record::<Consumed>("repair-renewal-consumed",id)? {
            ensure!(old.version == 1 && old.request == proposal.request.id && old.preview == proposal.digest && old.lineage == proposal.imported.lineage, "continuation approval lineage cannot be reused");
            return Ok(());
        }
        let fresh = Ctx { store:store.clone(), ..ctx.clone() };
        ensure!(preview_continuation(&fresh,&proposal.request)? == *proposal, "continuation changed before CAS publication");
        let renewal_id = format!("import-continuation:{}",proposal.imported.lineage);
        let proof = serde_json::to_vec(&serde_json::json!({"version":1,"preview":proposal,"reason":confirmation.reason,"at":confirmation.at,"terminal":confirmation.terminal}))?;
        let approval = BlobHash::of(&proof).to_ref();
        store.stage_task_evidence("repair-renewal-evidence",id,&proof,|blob| serde_json::json!({"version":1,"approval":blob,"lineage":proposal.imported.lineage}))?;
        store.renew_repair_allowance(id, RepairRenewal { id:renewal_id.clone(), previous_digest:proposal.request.previous_digest.clone(), reason:confirmation.reason.clone(), approved_by:"owner".into(), at:confirmation.at },
            RepairPolicy::V1 { current_profile_limit:1,stronger_profile_limit:0,overall_limit:1,pm_diagnosis_allowance:0 }, &proposal.executor.profile,None)?;
        store.save_record("repair-continuation-route",id,&RoutePin { version:1, renewal:renewal_id.clone(), executor:proposal.executor.clone() })?;
        store.save_record("repair-renewal-consumed",id,&Consumed { version:1,request:proposal.request.id.clone(),preview:proposal.digest.clone(),lineage:proposal.imported.lineage.clone(),approval,renewal:renewal_id })
    })?;
    ctx.store
        .repair_ledger(id)?
        .context("renewed continuation ledger missing")
}

#[cfg(test)]
pub(super) fn fixture(preview: ContinuationPreview) -> ContinuationConfirmation {
    ContinuationConfirmation {
        preview,
        reason: "isolated continuation fixture".into(),
        at: "2026-11-01T00:00:00Z".parse().unwrap(),
        terminal: "test-only".into(),
    }
}

#[cfg(test)]
pub(super) fn renew_fixture(
    ctx: &Ctx,
    confirmation: ContinuationConfirmation,
) -> Result<RepairLedger> {
    renew_checked(ctx, confirmation)
}
