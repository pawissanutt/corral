use std::collections::BTreeMap;
use std::fs;
use std::io::Write;

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
    let path = ctx.store.state_dir().join("inbox-send-budget.json");
    let mut ledger: BTreeMap<String, Vec<Slot>> = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("parsing inbox send budget")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(e).context("reading inbox send budget"),
    };
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
    fs::create_dir_all(ctx.store.state_dir()).context("creating budget directory")?;
    let mut tmp = tempfile::NamedTempFile::new_in(ctx.store.state_dir())
        .context("creating send budget temporary file")?;
    tmp.write_all(&serde_json::to_vec(&ledger).context("serializing send budget")?)
        .context("writing send budget")?;
    tmp.persist(&path).context("publishing send budget")?;
    Ok(true)
}
