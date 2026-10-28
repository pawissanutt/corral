//! Versioned documents behind the store facade. Reads share the caller's view and see
//! callback-local writes; writes join the task's unit of work when one is open.

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};

use crate::Store;
use crate::backend::{PageReq, RecordQuery, RecordWrite, StoredRecord};

mod area;
pub use area::RecordArea;

impl Store {
    pub fn record_row(&self, area: &str, key: &str) -> Result<Option<StoredRecord>> {
        let view = self.record_view()?;
        view.ensure_fresh()?;
        if let Some(staged) = self.staged_record(area, key) {
            if let Some(row) = &staged {
                count(row);
            }
            return Ok(staged);
        }
        if let Some(held) = self.held_record(area, key) {
            if let Some(row) = &held {
                count(row);
            }
            return Ok(held);
        }
        let row = view
            .records(
                &RecordQuery {
                    area: Some(area.to_owned()),
                    key: Some(key.to_owned()),
                },
                PageReq::first(1),
            )?
            .items
            .pop();
        if let Some(row) = &row {
            count(row);
        }
        self.hold_record(area, key, &row);
        Ok(row)
    }

    pub fn record<T: DeserializeOwned>(&self, area: &str, key: &str) -> Result<Option<T>> {
        self.record_row(area, key)?
            .map(|row| {
                serde_json::from_value(row.doc)
                    .with_context(|| format!("decoding record {area}/{key}"))
            })
            .transpose()
    }

    /// Output-sized reads of one area. Hot readers use a key or an unfinished index area.
    pub fn record_rows(&self, area: &str) -> Result<Vec<StoredRecord>> {
        let view = self.record_view()?;
        let mut page = PageReq::first(256);
        let mut rows = std::collections::BTreeMap::new();
        loop {
            let found = view.records(
                &RecordQuery {
                    area: Some(area.to_owned()),
                    key: None,
                },
                page.clone(),
            )?;
            for row in found.items {
                count(&row);
                self.hold_record(area, &row.key, &Some(row.clone()));
                rows.insert(row.key.clone(), row);
            }
            match found.next {
                Some(after) => page.after = Some(after),
                None => break,
            }
        }
        for write in self.staged_record_writes(area) {
            if let Some(row) = staged_row(&write) {
                rows.insert(write.key, row);
            } else {
                rows.remove(&write.key);
            }
        }
        Ok(rows.into_values().collect())
    }

    pub fn save_record(&self, area: &str, key: &str, value: &impl Serialize) -> Result<()> {
        anyhow::ensure!(
            !self.is_snapshot(),
            "a read-only snapshot cannot write records"
        );
        let before = self.record_row(area, key)?;
        let doc = serde_json::to_value(value).context("encoding record")?;
        if before.as_ref().is_some_and(|old| old.doc == doc) {
            return Ok(());
        }
        self.put_records(vec![RecordWrite {
            area: area.to_owned(),
            key: key.to_owned(),
            expected: before.map(|row| row.version),
            doc: Some(doc),
            updated: crate::clock::now(),
        }])
    }

    /// Insert-if-absent with the same version precondition as every other record write.
    /// The caller can read the winning row after a false result.
    pub fn insert_record(&self, area: &str, key: &str, value: &impl Serialize) -> Result<bool> {
        anyhow::ensure!(
            !self.is_snapshot(),
            "a read-only snapshot cannot insert records"
        );
        if self.record_row(area, key)?.is_some() {
            return Ok(false);
        }
        let write = RecordWrite {
            area: area.to_owned(),
            key: key.to_owned(),
            expected: None,
            doc: Some(serde_json::to_value(value).context("encoding record")?),
            updated: crate::clock::now(),
        };
        self.insert_record_write(write)
    }

    pub fn delete_record(&self, area: &str, key: &str) -> Result<()> {
        anyhow::ensure!(
            !self.is_snapshot(),
            "a read-only snapshot cannot delete records"
        );
        let Some(before) = self.record_row(area, key)? else {
            return Ok(());
        };
        self.put_records(vec![RecordWrite {
            area: area.to_owned(),
            key: key.to_owned(),
            expected: Some(before.version),
            doc: None,
            updated: crate::clock::now(),
        }])
    }
}

pub(crate) fn staged_row(write: &RecordWrite) -> Option<StoredRecord> {
    write.doc.as_ref().map(|doc| StoredRecord {
        area: write.area.clone(),
        key: write.key.clone(),
        version: write.expected.unwrap_or(0) + 1,
        doc: doc.clone(),
        updated: write.updated,
    })
}

fn count(row: &StoredRecord) {
    if crate::work::in_hot_scope() {
        crate::work::decoded(
            crate::work::Counter::RecordDecodes,
            serde_json::to_string(&row.doc)
                .map_or(0, |text| u64::try_from(text.len()).unwrap_or(u64::MAX)),
            crate::work::Subject::Plain,
        );
    }
}
