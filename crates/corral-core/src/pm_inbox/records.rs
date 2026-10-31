//! Database authority for workspace PM requests; body and metadata publish together.
use super::{ItemState, NoticeState, PM_INBOX_VERSION, PmInboxItem, refuse, validate_id};
use crate::Store;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
const ITEMS: &str = "pm-inbox-items";
#[derive(Serialize, Deserialize)]
struct ItemRecord {
    item: PmInboxItem,
    body: String,
}

pub(super) fn read(store: &Store, id: &str) -> Result<PmInboxItem> {
    validate_id(id)?;
    let record: ItemRecord = store
        .record(ITEMS, id)?
        .ok_or_else(|| refuse("unknown_item", format!("no PM inbox item {id}")))?;
    decode(id, record)
}
fn decode(id: &str, record: ItemRecord) -> Result<PmInboxItem> {
    let mut item = record.item;
    if item.version != PM_INBOX_VERSION || item.id != id {
        bail!("unsupported or mismatched PM inbox record {id}");
    }
    item.body = record.body;
    Ok(item)
}
pub(super) fn write(store: &Store, item: &PmInboxItem) -> Result<()> {
    validate_id(&item.id)?;
    store.records_transaction(|| {
        store.save_record(
            ITEMS,
            &item.id,
            &ItemRecord {
                item: item.clone(),
                body: item.body.clone(),
            },
        )?;
        for (area, owed) in [
            ("pm-inbox-active", item.pending()),
            (
                "pm-inbox-notices",
                item.notice
                    .as_ref()
                    .is_some_and(|notice| notice.state == NoticeState::Pending),
            ),
            (
                "pm-inbox-wakes",
                item.state == ItemState::Open && item.pm_wake.is_none(),
            ),
        ] {
            if owed {
                store.save_record(area, &item.id, &true)?;
            } else {
                store.delete_record(area, &item.id)?;
            }
        }
        Ok(())
    })
}
pub(super) fn all(store: &Store) -> Result<Vec<PmInboxItem>> {
    let mut items = store
        .record_rows(ITEMS)?
        .into_iter()
        .map(|row| {
            decode(
                &row.key,
                serde_json::from_value(row.doc).context("decoding PM inbox item")?,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    items.sort_by_key(|item| (item.created, item.id[4..].parse::<u64>().unwrap_or(0)));
    Ok(items)
}
pub(super) fn reserve(store: &Store) -> Result<String> {
    let mut next = store
        .record::<u64>("pm-inbox-counter", "current")?
        .unwrap_or(0)
        .checked_add(1)
        .context("PM inbox counter overflow")?;
    while store
        .record::<serde_json::Value>(ITEMS, &format!("PMI-{next}"))?
        .is_some()
    {
        next = next.checked_add(1).context("PM inbox counter overflow")?;
    }
    store.save_record("pm-inbox-counter", "current", &next)?;
    Ok(format!("PMI-{next}"))
}
impl Store {
    /// Output-sized live requests only; terminal history is not decoded.
    pub fn pending_pm_inbox_items(&self) -> Result<Vec<PmInboxItem>> {
        self.pm_inbox_indexed("pm-inbox-active")
    }
    /// Items still owing an author notice or a PM wake, deduplicated in canonical item order.
    pub fn owed_pm_inbox_items(&self) -> Result<Vec<PmInboxItem>> {
        let mut keys = std::collections::BTreeSet::new();
        for area in ["pm-inbox-notices", "pm-inbox-wakes"] {
            keys.extend(self.record_rows(area)?.into_iter().map(|row| row.key));
        }
        let mut items = keys
            .into_iter()
            .map(|id| read(self, &id))
            .collect::<Result<Vec<_>>>()?;
        items.sort_by_key(|item| (item.created, item.id[4..].parse::<u64>().unwrap_or(0)));
        Ok(items)
    }
    fn pm_inbox_indexed(&self, area: &str) -> Result<Vec<PmInboxItem>> {
        let mut items = self
            .record_rows(area)?
            .into_iter()
            .map(|row| read(self, &row.key))
            .collect::<Result<Vec<_>>>()?;
        items.sort_by_key(|item| (item.created, item.id[4..].parse::<u64>().unwrap_or(0)));
        Ok(items)
    }
}
