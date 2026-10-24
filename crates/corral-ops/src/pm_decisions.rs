//! Workspace coordination decisions are not fabricated worker-run obligations.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use corral_core::OperatingPresentation;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::pm_next::{PmAction, PmActionKind};
use crate::pm_workset::PmDecision;

#[derive(Default, Serialize, Deserialize)]
struct Decisions {
    version: u32,
    records: BTreeMap<String, Record>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    decision: PmDecision,
    answer: Option<String>,
}

fn read(ctx: &Ctx) -> Result<Decisions> {
    let path = ctx.store.state_dir().join("pm-decisions.json");
    match std::fs::read(&path) {
        Ok(bytes) => {
            let records: Decisions =
                serde_json::from_slice(&bytes).context("parsing workspace PM decisions")?;
            ensure!(records.version == 1, "unsupported PM decision state");
            Ok(records)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Decisions {
            version: 1,
            ..Default::default()
        }),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn save(ctx: &Ctx, records: &Decisions) -> Result<()> {
    let path = ctx.store.state_dir().join("pm-decisions.json");
    crate::pm_policy::write_atomic(&path, &serde_json::to_vec(records)?)
}

pub(crate) fn workspace_decision(ctx: &Ctx, action: PmAction) -> Result<Option<PmDecision>> {
    if action.kind == PmActionKind::Wait {
        return Ok(None);
    }
    let request_digest = crate::verification::digest(&serde_json::to_vec(&action)?);
    let id = format!("decision-{request_digest}");
    Ok(Some(PmDecision {
        presentation: ctx.store.operating_state()?.presentations.get(&id).cloned(),
        id,
        task: "workspace".into(),
        run: None,
        source: "workspace".into(),
        request_digest,
        action,
    }))
}

fn externally_resolved(ctx: &Ctx, decision: &PmDecision) -> Result<bool> {
    if let Some(id) = decision.source.strip_prefix("portable:") {
        let task = ctx.store.get(&decision.task)?;
        return Ok(task.runs.iter().any(|run| {
            Some(&run.id) == decision.run.as_ref()
                && run.obligations.decisions.iter().any(|record| {
                    record.request.id == id
                        && record.receipt.as_ref().is_some_and(|receipt| {
                            receipt.outcome == corral_core::ReceiptOutcome::Answered
                                && receipt
                                    .answer
                                    .as_deref()
                                    .is_some_and(|answer| !answer.trim().is_empty())
                        })
                })
        }));
    }
    if decision.source.contains(" request ") {
        // A finished or dropped task has no worker left to answer a native question.
        let task = ctx.store.get(&decision.task)?;
        if matches!(
            task.status,
            corral_core::Status::Done | corral_core::Status::Dropped
        ) {
            return Ok(true);
        }
        return crate::pm_questions::source_resolved(&decision.source);
    }
    Ok(false)
}

pub(crate) fn merge_pending(ctx: &Ctx, decisions: &mut Vec<PmDecision>) -> Result<()> {
    let records = read(ctx)?;
    for decision in decisions.iter_mut() {
        if decision.presentation.is_none()
            && let Some(record) = records.records.get(&decision.id)
        {
            decision
                .presentation
                .clone_from(&record.decision.presentation);
        }
    }
    decisions.retain(|decision| {
        records
            .records
            .get(&decision.id)
            .is_none_or(|record| record.answer.is_none())
    });
    for record in records.records.values() {
        if record.answer.is_none()
            && !decisions.iter().any(|d| d.id == record.decision.id)
            && !externally_resolved(ctx, &record.decision)?
        {
            let mut decision = record.decision.clone();
            decision.presentation = ctx
                .store
                .operating_state()?
                .presentations
                .get(&decision.id)
                .cloned()
                .or(decision.presentation);
            decisions.push(decision);
        }
    }
    Ok(())
}

/// Explicit presentation acknowledgement, separate from owner resolution.
/// `confirmed=false` means delivery uncertain, not permission to resend.
pub fn pm_present(
    ctx: &Ctx,
    id: &str,
    delivery: &str,
    confirmed: bool,
    at: Timestamp,
) -> Result<()> {
    ensure!(
        !delivery.trim().is_empty(),
        "presentation needs a delivery identity"
    );
    let receipt = {
        let _board = ctx
            .store
            .try_lock("dispatch")?
            .context("board is being updated")?;
        let mut decision = crate::pm_workset(ctx)?
            .decisions
            .into_iter()
            .find(|decision| decision.id == id)
            .context("decision is stale or no longer pending")?;
        if let Some(previous) = &decision.presentation {
            ensure!(
                previous.delivery == delivery,
                "decision already has a different presentation delivery identity"
            );
            if previous.confirmed || !confirmed {
                return Ok(());
            }
        }
        let receipt = OperatingPresentation {
            request_digest: decision.request_digest.clone(),
            delivery: delivery.to_owned(),
            confirmed,
            at,
        };
        decision.presentation = Some(receipt.clone());
        let mut records = read(ctx)?;
        if let Some(record) = records.records.get_mut(id) {
            ensure!(record.answer.is_none(), "decision is no longer pending");
            record.decision = decision;
        } else {
            records.records.insert(
                id.to_owned(),
                Record {
                    decision,
                    answer: None,
                },
            );
        }
        save(ctx, &records)?;
        receipt
    };
    // This writer takes the board lock itself; never reacquire it recursively.
    ctx.store.record_operating_presentation(id, receipt)
}

/// Record an actual owner answer to an exact workspace coordination decision.
/// Native and portable questions resolve only through their matching response lifecycle.
pub fn pm_resolve(ctx: &Ctx, id: &str, answer: &str) -> Result<()> {
    ensure!(!answer.trim().is_empty(), "owner answer is empty");
    let _board = ctx
        .store
        .try_lock("dispatch")?
        .context("board is being updated")?;
    let mut records = read(ctx)?;
    if !records.records.contains_key(id) {
        let decision = crate::pm_workset(ctx)?
            .decisions
            .into_iter()
            .find(|decision| decision.id == id)
            .context("unknown pending decision")?;
        records.records.insert(
            id.to_owned(),
            Record {
                decision,
                answer: None,
            },
        );
    }
    let record = records
        .records
        .get_mut(id)
        .context("unknown presented decision")?;
    ensure!(
        record.decision.action.kind != PmActionKind::Question,
        "answer this native or portable request through its matching response operation"
    );
    if let Some(previous) = &record.answer {
        ensure!(
            previous == answer,
            "decision already has a different owner answer"
        );
        return Ok(());
    }
    record.answer = Some(answer.to_owned());
    save(ctx, &records)?;
    ctx.store.log_event(
        record_task(&records, id),
        "pm_decision_resolved",
        serde_json::json!({"decision":id}),
    )
}

fn record_task<'a>(records: &'a Decisions, id: &str) -> Option<&'a str> {
    records.records.get(id).and_then(|record| {
        (record.decision.task != "workspace").then_some(record.decision.task.as_str())
    })
}
