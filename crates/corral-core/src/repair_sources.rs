//! Bounded, snapshot-consistent canonical inputs for explicit repair-history import.
//! This reader never adopts policy, edits history, or infers credit from run counts.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;

use crate::backend::{
    EventQuery, HistoryRow, PageReq, RowQuery, StoredEvent, StoredRecord, TaskHeadDoc, TaskNum,
    TaskPart,
};
use crate::blob_ref::{BlobHash, BlobRef};
use crate::{STORE_SCHEMA_VERSION, Store};

const MAX_ROWS: usize = 1024;
const MAX_EVENTS: u32 = 512;
const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairSources {
    pub version: u32,
    pub schema: u32,
    pub store: String,
    pub head: TaskHeadDoc,
    /// Rows of every accounting-relevant family, including explicit empty families.
    pub rows: BTreeMap<String, Vec<HistoryRow>>,
    pub events: Vec<StoredEvent>,
    /// `None` is a pinned absence, not zero consumed allowance.
    pub records: BTreeMap<String, Option<StoredRecord>>,
    /// Explicit traversal end. A page continuation or malformed event refuses the read.
    pub end: RepairSourceEnd,
    pub verified_blobs: Vec<BlobRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairSourceEnd {
    pub revision: u64,
    pub last_event: u64,
    pub event_count: usize,
    pub row_count: usize,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairImportReceipt {
    pub version: u32,
    pub request: String,
    pub preview_digest: String,
    pub lineage: String,
    pub evidence: BlobRef,
    pub ledger_digest: String,
}

impl RepairSources {
    pub fn digest(&self) -> Result<String> {
        Ok(BlobHash::of(&serde_json::to_vec(self)?).to_string())
    }
}

impl Store {
    /// Low-level publication only; ops must obtain its exact in-process owner confirmation.
    /// The evidence reservation, task CAS, receipt and consumed index share this task unit.
    pub fn publish_repair_import(
        &self,
        task: &str,
        expected: Option<&StoredRecord>,
        ledger: &crate::RepairLedger,
        receipt: &RepairImportReceipt,
    ) -> Result<()> {
        ensure!(
            self.has_record_unit(),
            "repair import requires a task transaction"
        );
        ledger.validate()?;
        ensure!(
            ledger.version == 2
                && ledger.allowances.len() == 1
                && ledger.active()?.diagnosis.is_none()
                && ledger.active()?.renewal.is_none(),
            "import must be one historical allowance"
        );
        let history = &ledger.active()?.history;
        let projection = history
            .projection()
            .context("historical projection evidence missing")?;
        ensure!(
            history.context().is_none()
                && history.counters()?.initial_implementations == 1
                && matches!(ledger.transition()?, crate::RepairTransition::Owner { .. }),
            "import grants no new coding allowance"
        );
        ensure!(
            receipt.version == 1
                && !receipt.request.trim().is_empty()
                && receipt.lineage == projection.lineage
                && receipt.evidence == projection.evidence
                && receipt.ledger_digest == ledger.digest()?,
            "repair import receipt does not bind history"
        );
        BlobHash::parse(&receipt.preview_digest)?;
        let previous = self.record_row(crate::REPAIR_RUNTIME, task)?;
        ensure!(
            previous.as_ref() == expected,
            "repair import ledger CAS changed"
        );
        if let Some(previous) = previous {
            let old: crate::RepairLedger = serde_json::from_value(previous.doc)?;
            let active = old.active()?;
            ensure!(
                old.allowances.len() == 1
                    && active.diagnosis.is_none()
                    && active.renewal.is_none()
                    && active.history.context().is_none()
                    && active.history.projection().is_none()
                    && active.history.coding_blocker() == Some(crate::LEGACY_REPAIR_BLOCKER)
                    && active.history.counters()?
                        == crate::RepairCounters {
                            initial_implementations: 1,
                            ..Default::default()
                        },
                "cannot overwrite substantive repair history"
            );
        }
        ensure!(
            self.record_row("repair-import", task)?.is_none()
                && self.record_row("repair-import-consumed", task)?.is_none(),
            "repair import lineage already consumed"
        );
        self.save_record(crate::REPAIR_RUNTIME, task, ledger)?;
        self.save_record("repair-import", task, receipt)?;
        self.save_record("repair-import-consumed", task, receipt)
    }

    pub fn repair_sources(&self, task: &str) -> Result<RepairSources> {
        let task = Self::normalize_id(task)?;
        let snapshot = self.read_only_snapshot()?;
        let view = snapshot.view()?;
        let num = TaskNum(crate::store::id_number(&task));
        let head = view.head(num)?.context("repair source task missing")?;
        let mut rows = BTreeMap::new();
        let mut total = 0;
        for part in [
            TaskPart::Run,
            TaskPart::RunSwitch,
            TaskPart::GateAttempt,
            TaskPart::MemberGateHistory,
            TaskPart::ReviewRun,
            TaskPart::ReviewEvidence,
            TaskPart::Finding,
            TaskPart::Prompt,
            TaskPart::Artifact,
            TaskPart::AgentRecord,
            TaskPart::PmReceipt,
            TaskPart::ScopeRevision,
        ] {
            let found = view.rows(
                num,
                &RowQuery::part(part),
                PageReq::first((MAX_ROWS + 1) as u32),
            )?;
            total += found.items.len();
            ensure!(
                found.next.is_none() && total <= MAX_ROWS,
                "repair source row traversal exceeds bound"
            );
            rows.insert(
                serde_json::to_value(part)?
                    .as_str()
                    .context("row family")?
                    .to_owned(),
                found.items,
            );
        }
        let found = view.events(
            &EventQuery {
                task: Some(task),
                ..Default::default()
            },
            PageReq::first(MAX_EVENTS + 1),
        )?;
        ensure!(
            found.next.is_none() && found.items.len() <= MAX_EVENTS as usize,
            "repair source event traversal exceeds bound"
        );
        ensure!(
            found.items.iter().all(|e| !e.malformed),
            "malformed canonical repair event"
        );
        let mut records = BTreeMap::new();
        for area in [
            crate::REPAIR_RUNTIME,
            "profile-repairs",
            "completion-repair-cycle",
            "repair-continuation",
            "gate-triage",
            "repair-import",
            "repair-import-consumed",
        ] {
            records.insert(area.into(), snapshot.record_row(area, &head.header.id)?);
        }
        let end = RepairSourceEnd {
            revision: head.header.rev,
            last_event: found.items.last().map_or(0, |e| e.seq.0),
            event_count: found.items.len(),
            row_count: total,
            complete: true,
        };
        let mut source = RepairSources {
            version: 1,
            schema: STORE_SCHEMA_VERSION,
            store: crate::agent::paths::store_id(self),
            head,
            rows,
            events: found.items,
            records,
            end,
            verified_blobs: Vec::new(),
        };
        let bytes = serde_json::to_vec(&source)?;
        ensure!(
            bytes.len() <= MAX_BYTES,
            "repair source byte budget exceeded"
        );
        let mut remaining = MAX_BYTES - bytes.len();
        for hash in crate::blob_ref::scan_doc(&source)? {
            let mut reader = snapshot
                .blob_reader(&hash)?
                .context("canonical repair evidence blob missing")?
                .take((remaining + 1) as u64);
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .context("verifying canonical repair evidence hash")?;
            ensure!(
                bytes.len() <= remaining,
                "repair evidence blobs exceed total bound"
            );
            remaining -= bytes.len();
            ensure!(
                BlobHash::of(&bytes) == hash,
                "canonical repair evidence hash mismatch"
            );
            source.verified_blobs.push(hash.to_ref());
        }
        view.ensure_fresh()?;
        Ok(source)
    }
}
