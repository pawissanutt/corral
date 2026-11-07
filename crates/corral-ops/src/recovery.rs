//! Exact, local-owner recovery. A preview is evidence, never permission.
use anyhow::{Context, Result, ensure};
use corral_core::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::Ctx;

mod admission;
mod apply;
pub use admission::recover_resource_input;
pub(crate) use admission::{
    active_journal, admission_attempt, bind_active_journal, require_no_pending,
};
mod authority;
mod journal;
mod landing;
pub(crate) use journal::bind_journal;
pub(crate) use landing::LandingProof;
mod code;
mod continuity;
pub(crate) use code::*;
mod evaluate;
mod policy;
mod receipts;
mod snapshot;
pub(crate) use receipts::{require_manual_receipts, review_needed};
mod reconcile;
pub(crate) use reconcile::reconcile_pending;

pub use apply::{recover_apply, recover_reconcile};
pub use authority::{OwnerConfirmation, recover_confirm, recover_confirm_tui};
pub(crate) use continuity::*;

/// Surface input, including the actual resolution notes whose hash the request binds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInput {
    pub action: RecoveryAction,
    pub exceptions: BTreeSet<RecoveryCheck>,
    #[serde(default)]
    pub notes: String,
}

/// Evaluate without observing configuration changes or persisting a proposal.
pub fn recover_preview(ctx: &Ctx, id: &str, input: &RecoveryInput) -> Result<RecoveryPreview> {
    let mut ctx = ctx.clone();
    ctx.store = Store::open_unobserved(ctx.store.root())?;
    let task = ctx.store.get(id)?;
    evaluate::preview(&ctx, &task, input)
}

pub(crate) fn hash(value: &impl Serialize) -> Result<String> {
    Ok(crate::verification::digest(&serde_json::to_vec(value)?))
}

fn outcome(
    state: RecoveryState,
    detail: impl Into<String>,
    effect: Option<RecoveryEffect>,
) -> RecoveryOutcome {
    RecoveryOutcome {
        state,
        at: corral_core::clock::now(),
        detail: detail.into(),
        actual_effect: effect,
        after: None,
        evidence: Vec::new(),
    }
}

fn reference(record: &RecoveryRecord) -> RecoveryReference {
    RecoveryReference {
        operation: record.id.clone(),
        contract_digest: record.preview.contract_digest.clone(),
    }
}

fn acceptance(record: &RecoveryRecord) -> RecoveryAcceptance {
    RecoveryAcceptance {
        acceptance_mode: AcceptanceMode::OwnerRecovery,
        policy_satisfied: false,
        recovery: Some(reference(record)),
        reason: Some(record.authorization.reason.clone()),
        checks: record.preview.checks.clone(),
    }
}

/// Durable recovery history, retaining original checks and owner-recovery dispositions.
pub fn recover_history(ctx: &Ctx, task: &str) -> Result<Vec<RecoveryRecord>> {
    records(ctx, task)
}

pub(crate) fn records(ctx: &Ctx, task: &str) -> Result<Vec<RecoveryRecord>> {
    use corral_core::backend::{HeadQuery, PageReq, RowQuery, TaskPart};
    let view = ctx.store.view()?;
    let normalized = Store::normalize_id(task)?;
    let Some(head) = view
        .heads(&HeadQuery::Ids(vec![normalized]), PageReq::first(1))?
        .items
        .into_iter()
        .next()
    else {
        anyhow::bail!("unknown recovery task {task}")
    };
    let mut page = PageReq::first(100);
    let mut records = Vec::new();
    loop {
        let rows = view.rows(
            head.header.num,
            &RowQuery::part(TaskPart::Recovery),
            page.clone(),
        )?;
        for row in rows.items {
            let mut doc = row.doc;
            for (field, part) in [("outcomes", "outcome"), ("links", "link")] {
                let mut values = Vec::new();
                let mut children = PageReq::first(100);
                loop {
                    let result = view.rows(
                        head.header.num,
                        &RowQuery::under(TaskPart::RecoveryOutcome, format!("{}/{part}/", row.key)),
                        children.clone(),
                    )?;
                    values.extend(result.items.into_iter().map(|r| r.doc));
                    match result.next {
                        Some(next) => children.after = Some(next),
                        None => break,
                    }
                }
                doc[field] = serde_json::Value::Array(values);
            }
            records.push(serde_json::from_value(doc)?);
        }
        match rows.next {
            Some(next) => page.after = Some(next),
            None => break,
        }
    }
    Ok(records)
}

fn state(record: &RecoveryRecord) -> Result<RecoveryState> {
    record
        .outcomes
        .last()
        .map(|o| o.state)
        .context("recovery has no durable intent")
}

fn current_preview(ctx: &Ctx, preview: &RecoveryPreview, input: &RecoveryInput) -> Result<()> {
    let now = corral_core::clock::now();
    ensure!(
        now < preview.expires
            && preview.expires <= now.checked_add(jiff::SignedDuration::from_secs(600))?,
        "recovery_stale: preview expired or lifetime was extended"
    );
    let mut actual = recover_preview(ctx, &preview.request.target.task, input)?;
    // Sampling time is diagnostic, but every authoritative field must re-derive. Comparing
    // just caller-provided digest strings would let changed effects borrow a valid digest.
    actual.expires = preview.expires;
    ensure!(
        actual == *preview,
        "recovery_stale: target, policy, checks or operation changed; preview again"
    );
    ensure!(
        actual.remaining_blockers.is_empty(),
        "recovery blocked: {:?}",
        actual.remaining_blockers
    );
    Ok(())
}

#[cfg(test)]
#[path = "../tests/recovery/mod.rs"]
mod tests;
