use std::collections::BTreeMap;

use anyhow::{Context, Result};
use corral_core::inbox::Inbox;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::SendBudget;
use crate::Ctx;

#[derive(Serialize, Deserialize)]
struct Slot {
    key: String,
    at: Timestamp,
}

pub(super) fn reserve(
    ctx: &Ctx,
    inbox: &Inbox<'_>,
    adapter: &str,
    budget: Option<&SendBudget>,
    now: Timestamp,
) -> Result<bool> {
    let Some(budget) = budget else {
        return Ok(true);
    };
    // A separate global ledger serializes admission across recipients and processes.
    let Some(_lock) = ctx.store.try_lock("inbox-send-budget")? else {
        return Ok(false);
    };
    let mut ledger: BTreeMap<String, Vec<Slot>> = ctx
        .store
        .record("inbox-send-budget", "current")?
        .unwrap_or_default();
    let slots = ledger.entry(adapter.into()).or_default();
    let interval = i128::try_from(budget.per_recipient_interval.as_nanos())
        .context("send budget interval overflow")?;
    slots.retain(|slot| {
        now.as_nanosecond() - slot.at.as_nanosecond() < interval.max(60_000_000_000)
    });
    let recent = slots
        .iter()
        .filter(|slot| now.as_nanosecond() - slot.at.as_nanosecond() < 60_000_000_000)
        .count();
    if recent >= usize::try_from(budget.per_minute_total).context("send budget count overflow")?
        || slots.iter().any(|slot| {
            slot.key == inbox.key() && now.as_nanosecond() - slot.at.as_nanosecond() < interval
        })
    {
        return Ok(false);
    }
    slots.push(Slot {
        key: inbox.key().into(),
        at: now,
    });
    ctx.store
        .save_record("inbox-send-budget", "current", &ledger)?;
    Ok(true)
}
