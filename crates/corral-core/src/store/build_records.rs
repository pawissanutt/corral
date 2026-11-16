//! Build sessions use an isolated record area, never the task family. The record,
//! revision and outcome events commit in one normal version-checked store operation.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Store;
use crate::build_record::{BuildRecord, BuildRecordSummary, validate_id};

const AREA: &str = "build-records";

#[derive(Serialize, Deserialize)]
struct StoredBuildRecord {
    record: BuildRecord,
    revision: u64,
    source_hash: String,
}

impl StoredBuildRecord {
    fn validate(&self, id: &str) -> Result<()> {
        ensure!(self.record.id == id, "mismatched build record {id}");
        ensure!(self.revision > 0, "invalid revision for build record {id}");
        self.record
            .validate()
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("validating stored build record {id}"))
    }
}

impl Store {
    /// Validates and upserts the wire document. Identical input bytes are a no-op;
    /// identity fields are immutable. The return value is `(id, revision)`.
    pub fn publish_build_record(&self, record_json: &str) -> Result<(String, u64)> {
        let record: BuildRecord =
            serde_json::from_str(record_json).context("decoding build record JSON")?;
        record
            .validate()
            .map_err(anyhow::Error::msg)
            .context("validating build record")?;
        let source_hash = format!("{:x}", Sha256::digest(record_json.as_bytes()));
        let id = record.id.clone();
        let _lock = self.lock(&format!("build-record-{id}"))?;
        self.records_transaction(|| {
            let before: Option<StoredBuildRecord> = self
                .record(AREA, &id)
                .with_context(|| format!("reading build record {id}"))?;
            let revision = if let Some(stored) = &before {
                stored.validate(&id)?;
                ensure!(
                    stored.record.created == record.created,
                    "build record {id} cannot change created"
                );
                ensure!(
                    stored.record.kind == record.kind,
                    "build record {id} cannot change kind"
                );
                ensure!(
                    stored.record.repo_root == record.repo_root,
                    "build record {id} cannot change repo_root"
                );
                if stored.source_hash == source_hash {
                    return Ok((id, stored.revision));
                }
                stored
                    .revision
                    .checked_add(1)
                    .with_context(|| format!("revision overflow for build record {id}"))?
            } else {
                1
            };
            if before.is_none() {
                self.log_event(
                    None,
                    "build_recorded",
                    serde_json::json!({
                        "id": record.id,
                        "kind": record.kind,
                        "project": record.project,
                        "title": record.title,
                    }),
                )
                .context("logging build_recorded event")?;
            } else if before
                .as_ref()
                .is_some_and(|stored| stored.record.outcome.state != record.outcome.state)
            {
                self.log_event(
                    None,
                    "build_outcome",
                    serde_json::json!({
                        "id": record.id,
                        "kind": record.kind,
                        "project": record.project,
                        "title": record.title,
                        "state": record.outcome.state,
                        "head": record.outcome.head,
                        "landed": record.outcome.landed,
                        "override_reason": record.outcome.override_reason,
                    }),
                )
                .context("logging build_outcome event")?;
            }
            self.save_record(
                AREA,
                &id,
                &StoredBuildRecord {
                    record,
                    revision,
                    source_hash,
                },
            )
            .with_context(|| format!("saving build record {id}"))?;
            Ok((id, revision))
        })
        .context("publishing build record")
    }

    pub fn get_build_record(&self, id: &str) -> Result<BuildRecord> {
        validate_id(id).map_err(anyhow::Error::msg)?;
        let stored: StoredBuildRecord = self
            .record(AREA, id)
            .with_context(|| format!("reading build record {id}"))?
            .with_context(|| format!("build record {id} not found"))?;
        stored.validate(id)?;
        Ok(stored.record)
    }

    /// Newest updated instant first, with an ID tie-break for deterministic reports.
    pub fn list_build_records(&self) -> Result<Vec<BuildRecordSummary>> {
        let rows = self.record_rows(AREA).context("listing build records")?;
        let mut summaries = Vec::with_capacity(rows.len());
        for row in rows {
            let stored: StoredBuildRecord = serde_json::from_value(row.doc)
                .with_context(|| format!("decoding build record {}", row.key))?;
            stored.validate(&row.key)?;
            let updated = stored
                .record
                .updated
                .parse::<jiff::Timestamp>()
                .with_context(|| format!("parsing build record {} updated", row.key))?;
            summaries.push((updated, stored.record.summary(stored.revision)));
        }
        summaries
            .sort_by(|(a_time, a), (b_time, b)| b_time.cmp(a_time).then_with(|| a.id.cmp(&b.id)));
        Ok(summaries.into_iter().map(|(_, summary)| summary).collect())
    }
}
