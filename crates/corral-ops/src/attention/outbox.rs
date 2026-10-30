//! Lifecycle publication is repeatable; fact identities survive interrupted acknowledgement.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use corral_core::attention::{AttentionFact, AttentionLedger, ConditionKey};
use jiff::Timestamp;
use serde_json::{Value, json};

use crate::Ctx;

pub(super) type Wakes = BTreeMap<Option<String>, BTreeMap<String, Value>>;

pub(super) fn enqueue(
    ledger: &mut AttentionLedger,
    key: &ConditionKey,
    kind: &str,
    suffix: &str,
    detail: Value,
) -> Result<()> {
    anyhow::ensure!(
        ledger.outbox.iter().filter(|fact| fact.key == *key).count() < 4,
        "attention_accounting_backpressure: {key} has four unpublished facts"
    );
    ledger.outbox.push(AttentionFact {
        key: key.clone(),
        fact_id: format!("{key}/{kind}{suffix}"),
        kind: kind.into(),
        detail,
    });
    Ok(())
}

pub(super) fn publish(ctx: &Ctx, id: &str, wakes: &mut Wakes) -> Result<()> {
    let task = ctx.store.get(id)?;
    if task.attention.outbox.is_empty() {
        return Ok(());
    }
    for fact in &task.attention.outbox {
        let mut detail = fact.detail.clone();
        detail["fact_id"] = json!(fact.fact_id);
        ctx.store.log_event(Some(id), &fact.kind, detail)?;
        #[cfg(test)]
        super::handling::crash_at(super::handling::CrashPoint::AfterAppend)?;
        if fact.detail["wake"] == true {
            wakes.entry(task.project.clone()).or_default().insert(
                fact.fact_id.clone(),
                json!({
                    "key": fact.key, "task": id, "class": fact.detail["class"],
                    "reason": fact.detail["reason"], "fact_id": fact.fact_id,
                }),
            );
        }
    }
    #[cfg(test)]
    super::handling::crash_at(super::handling::CrashPoint::BeforeClear)?;
    // Reclaim appended non-wake facts before another transition needs their slots.
    // Wake carriers stay durable until finish appends the complete project digest.
    let acknowledged: BTreeSet<_> = task
        .attention
        .outbox
        .iter()
        .filter(|fact| fact.detail["wake"] != true)
        .map(|fact| fact.fact_id.as_str())
        .collect();
    if !acknowledged.is_empty() {
        ctx.store.update_attention(id, |ledger| {
            ledger
                .outbox
                .retain(|fact| !acknowledged.contains(fact.fact_id.as_str()));
            ledger.records.retain(|key, record| {
                record.resolved_at.is_none() || ledger.outbox.iter().any(|fact| fact.key == *key)
            });
            Ok(())
        })?;
    }
    Ok(())
}

pub(super) fn finish(
    ctx: &Ctx,
    published: &BTreeMap<String, Vec<String>>,
    wakes: Wakes,
    pm_live: Option<bool>,
    now: Timestamp,
) -> Result<()> {
    for (project, entries) in wakes {
        let entries: Vec<_> = entries.into_values().collect();
        let tasks: BTreeSet<_> = entries
            .iter()
            .filter_map(|entry| entry["task"].as_str())
            .collect();
        let only = (tasks.len() == 1).then(|| tasks.first().copied()).flatten();
        let fact_id = crate::verification::digest(&serde_json::to_vec(
            &entries
                .iter()
                .map(|entry| &entry["fact_id"])
                .collect::<Vec<_>>(),
        )?);
        ctx.store.log_event(
            only,
            "attention_due",
            json!({
                "project": project, "tasks": tasks, "count": entries.len(),
                "entries": entries.iter().take(16).collect::<Vec<_>>(),
                "pm_live": pm_live, "at": now, "fact_id": fact_id,
            }),
        )?;
    }
    for (id, facts) in published {
        if facts.is_empty() {
            continue;
        }
        ctx.store.update_attention(id, |ledger| {
            ledger.outbox.retain(|fact| !facts.contains(&fact.fact_id));
            ledger.records.retain(|key, record| {
                record.resolved_at.is_none() || ledger.outbox.iter().any(|fact| fact.key == *key)
            });
            Ok(())
        })?;
    }
    Ok(())
}
