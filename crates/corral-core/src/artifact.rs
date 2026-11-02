use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::backend::{
    BlobQuery, BlobRow, BlobState, Clock, LeaseOp, Operation, PageReq, PutRecords, RecordWrite,
    ScratchKind, ScratchRow, SystemClock,
};
use crate::blob::{FsBlobStore, Gc, GcRun, ProcessIdentity, Putter};
use crate::blob_ref::{BlobHash, BlobRef};
use crate::scratch::{NewScratch, ScratchArea};
use crate::{Artifact, ArtifactKind, Store};

/// How long a reader lease keeps blob GC's horizon behind a read that may outlive the grace.
const READ_LEASE: Duration = Duration::from_secs(3600);

/// The evidence facade (plan T-253 sections 7-8): content-addressed blobs referenced by
/// record documents, reader leases, blob GC and registered scratch directories.
impl Store {
    /// The workspace's content-addressed evidence store, `.corral/blobs`.
    pub fn blob_store(&self) -> Result<FsBlobStore> {
        FsBlobStore::open(self.dir())
    }

    /// Puts `bytes` as a blob and commits record `area`/`key`, whose document `doc` builds
    /// around the blob's reference, consuming the put's reservation in that same write: the
    /// blob is live and referenced once this returns. Returns whether the record is new.
    /// Call it outside a task update; its record write commits on its own.
    pub fn put_evidence(
        &self,
        area: &str,
        key: &str,
        bytes: &[u8],
        doc: impl FnOnce(&BlobRef) -> serde_json::Value,
    ) -> Result<bool> {
        anyhow::ensure!(!self.is_snapshot(), "a snapshot cannot store evidence");
        self.check_runtime()?;
        let blobs = self.blob_store()?;
        let put = Putter::new(self.backend(), &blobs, &SystemClock)?.put(
            &mut &bytes[..],
            &mut |attempt| {
                // Busy only while GC deletes this exact content; it finishes in one pass.
                if attempt > 600 {
                    bail!("blob GC kept {area}/{key} busy for a minute");
                }
                std::thread::sleep(Duration::from_millis(100));
                Ok(())
            },
        )?;
        let before = self.record_row(area, key)?;
        let created = before.is_none();
        let write = RecordWrite {
            area: area.to_owned(),
            key: key.to_owned(),
            expected: before.map(|row| row.version),
            doc: Some(doc(&put.blob)),
            updated: crate::clock::now(),
        };
        self.backend()
            .apply(Operation::PutRecords(PutRecords {
                writes: vec![write],
                events: Vec::new(),
                reservations: vec![put.reservation],
            }))
            .map_err(|error| anyhow!("storing evidence {area}/{key}: {error}"))?;
        Ok(created)
    }

    /// Stage evidence inside an existing task-update callback. Its reference, reservation
    /// and task state publish in that task's single operation; callback error/unwind publishes
    /// neither the reference nor live evidence. The staged bytes remain GC-reservable.
    pub fn stage_task_evidence(
        &self,
        area: &str,
        key: &str,
        bytes: &[u8],
        doc: impl FnOnce(&BlobRef) -> serde_json::Value,
    ) -> Result<bool> {
        anyhow::ensure!(
            self.has_record_unit(),
            "staging evidence requires a task update"
        );
        let put = self.put_gate_blob(bytes)?;
        let created = self.record_row(area, key)?.is_none();
        self.save_record(area, key, &doc(&put.blob))?;
        self.stage_evidence_reservation(put.reservation)?;
        Ok(created)
    }

    /// The artifacts of `task` read as its artifact rows alone, never the full task: only those
    /// of `run` when given (by the rows' run index), in stored order. Each decoded row counts
    /// as a history row decode; use [`Store::artifact_row`] for one artifact.
    pub fn artifact_rows(&self, task: &str, run: Option<&str>) -> Result<Vec<Artifact>> {
        self.artifact_query(
            task,
            run.map(|run| format!("run:{run}")),
            None,
            PageReq::first(256),
        )
    }

    /// The artifact of `task` stored under row key `key` (its 1-based position as
    /// `{n:010}`), read as that one row; `None` when there is none.
    pub fn artifact_row(&self, task: &str, key: &str) -> Result<Option<Artifact>> {
        Ok(self
            .artifact_query(task, None, Some(key.to_owned()), PageReq::first(1))?
            .pop())
    }

    /// At most `limit` artifact rows of `task`, newest stored first, read as one page, and
    /// whether older rows exist beyond it. A bounded window, never the whole artifact history.
    pub fn artifact_window(&self, task: &str, limit: u32) -> Result<(Vec<Artifact>, bool)> {
        use crate::backend::{RowQuery, TaskNum, TaskPart};
        use crate::work::{Counter, Subject};
        let num = TaskNum(crate::store::id_number(&Self::normalize_id(task)?));
        let query = RowQuery {
            newest_first: true,
            ..RowQuery::part(TaskPart::Artifact)
        };
        let page = self.view()?.rows(num, &query, PageReq::first(limit))?;
        let more = page.next.is_some();
        let rows = page
            .items
            .into_iter()
            .map(|row| {
                let bytes = u64::try_from(serde_json::to_string(&row.doc)?.len())?;
                crate::work::decoded(Counter::HistoryRowDecodes, bytes, Subject::Plain);
                Ok(serde_json::from_value(row.doc)?)
            })
            .collect::<Result<Vec<Artifact>>>()?;
        Ok((rows, more))
    }

    /// The artifacts of `task` stored after row key `after`, in stored order.
    pub fn artifact_rows_after(&self, task: &str, after: &str) -> Result<Vec<Artifact>> {
        let page = PageReq {
            after: Some(after.to_owned()),
            limit: 256,
        };
        self.artifact_query(task, None, None, page)
    }

    fn artifact_query(
        &self,
        task: &str,
        index: Option<String>,
        key: Option<String>,
        mut page: PageReq,
    ) -> Result<Vec<Artifact>> {
        let rows = self.task_rows(
            task,
            crate::backend::TaskPart::Artifact,
            index,
            key,
            &mut page,
        )?;
        rows.into_iter()
            .map(|doc| Ok(serde_json::from_value(doc)?))
            .collect()
    }

    /// Documents of one row family of `task` matching `index`/`key` from `page` on, each
    /// decoded row counted as a history row decode. A single-key read reads one page.
    pub fn task_rows(
        &self,
        task: &str,
        part: crate::backend::TaskPart,
        index: Option<String>,
        key: Option<String>,
        page: &mut PageReq,
    ) -> Result<Vec<serde_json::Value>> {
        use crate::backend::{RowQuery, TaskNum};
        use crate::work::{Counter, Subject};
        let num = TaskNum(crate::store::id_number(&Self::normalize_id(task)?));
        let single = key.is_some();
        let query = RowQuery {
            part,
            key,
            after_key: None,
            prefix: None,
            index,
            current_only: false,
            open_only: false,
            newest_first: false,
        };
        let view = self.view()?;
        let mut docs = Vec::new();
        loop {
            let rows = view.rows(num, &query, page.clone())?;
            for row in rows.items {
                let bytes = u64::try_from(serde_json::to_string(&row.doc)?.len())?;
                crate::work::decoded(Counter::HistoryRowDecodes, bytes, Subject::Plain);
                docs.push(row.doc);
            }
            match rows.next {
                Some(after) if !single => page.after = Some(after),
                _ => return Ok(docs),
            }
        }
    }

    /// The blob row of `hash`, in any state.
    pub fn blob_row(&self, hash: &BlobHash) -> Result<Option<BlobRow>> {
        let query = BlobQuery {
            hash: Some(hash.clone()),
            state: None,
        };
        Ok(self
            .backend()
            .view()?
            .blobs(&query, PageReq::first(1))?
            .items
            .pop())
    }

    /// A verifying reader of a live blob's original bytes; `None` without a live row.
    pub fn blob_reader(&self, hash: &BlobHash) -> Result<Option<Box<dyn Read + Send>>> {
        match self.blob_row(hash)? {
            Some(row) if row.state == BlobState::Live => {
                use crate::backend::BlobStore;
                Ok(Some(Box::new(self.blob_store()?.get(hash)?)))
            }
            _ => Ok(None),
        }
    }

    /// Runs `read` under a storage lease, so blob GC's horizon stays before its start
    /// however long it takes; the lease is released whatever `read` returns.
    pub fn with_blob_lease<T>(&self, read: impl FnOnce() -> Result<T>) -> Result<T> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let holder = ProcessIdentity::current()?.owner();
        let id = format!("read-{holder}-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let expires =
            crate::clock::now().checked_add(jiff::SignedDuration::try_from(READ_LEASE)?)?;
        let lease = |op| {
            self.backend()
                .apply(Operation::Lease(op))
                .map_err(|error| anyhow!("storage lease {id}: {error}"))
        };
        lease(LeaseOp::Acquire {
            id: id.clone(),
            holder: holder.clone(),
            expires,
        })?;
        let result = read();
        lease(LeaseOp::Release {
            id: id.clone(),
            holder,
        })?;
        result
    }

    /// One blob GC pass under the `storage-gc` claim with the configured grace period.
    pub fn blob_gc(&self, clock: &dyn Clock) -> Result<GcRun> {
        let blobs = self.blob_store()?;
        Gc::new(self.backend(), &blobs, clock)?
            .grace(self.config.storage.blob_grace())
            .run()
    }

    /// Registers and creates scratch directory `rel` below the validated scratch root.
    pub fn register_scratch(
        &self,
        kind: ScratchKind,
        id: &str,
        task: &str,
        attempt: &str,
        rel: &str,
    ) -> Result<PathBuf> {
        let layout = self.storage_layout()?;
        let new = NewScratch {
            id: id.to_owned(),
            kind,
            task: Some(task.to_owned()),
            subject: None,
            attempt: Some(attempt.to_owned()),
            path: rel.to_owned(),
        };
        ScratchArea::open(self.backend(), &layout)?.register(new, crate::clock::now())
    }

    /// The scratch registration `id`, in any state.
    pub fn scratch_row(&self, id: &str) -> Result<Option<ScratchRow>> {
        let layout = self.storage_layout()?;
        ScratchArea::open(self.backend(), &layout)?.row(id)
    }

    /// Closes open scratch registration `id`, making it eligible for reclaim.
    pub fn close_scratch(&self, id: &str) -> Result<()> {
        let layout = self.storage_layout()?;
        ScratchArea::open(self.backend(), &layout)?.close(id, crate::clock::now())
    }
}

/// Storage lifetime classification, independent of pin protection in operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactTier {
    Keep,
    Normal,
    Transient,
}

impl ArtifactTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Normal => "normal",
            Self::Transient => "transient",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "keep" => Some(Self::Keep),
            "normal" => Some(Self::Normal),
            "transient" => Some(Self::Transient),
            _ => None,
        }
    }
}

impl Artifact {
    /// Explicit classification wins; operations separately protect pinned artifacts.
    pub fn tier(&self) -> ArtifactTier {
        if let Some(tier) = self.tier_override {
            return tier;
        }
        match self.kind {
            ArtifactKind::Report
            | ArtifactKind::Summary
            | ArtifactKind::Plan
            | ArtifactKind::Screenshot => return ArtifactTier::Keep,
            ArtifactKind::Log => return ArtifactTier::Transient,
            ArtifactKind::Diff => return ArtifactTier::Normal,
            ArtifactKind::Evidence | ArtifactKind::Other => {}
        }
        let title = self.title.to_ascii_lowercase();
        let path = std::path::Path::new(&self.path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let source = std::path::Path::new(&self.source)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let names = [&path, &source, &title];
        let has_ext = |extensions: &[&str]| {
            names
                .iter()
                .any(|name| extensions.iter().any(|ext| name.ends_with(ext)))
        };
        if has_ext(&[".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"])
            || (has_ext(&[".md", ".markdown", ".html", ".htm"])
                && names.iter().any(|name| {
                    name.split(|c: char| !c.is_ascii_alphanumeric())
                        .any(|word| {
                            matches!(
                                word,
                                "review"
                                    | "acceptance"
                                    | "report"
                                    | "summary"
                                    | "plan"
                                    | "screenshot"
                            )
                        })
                }))
        {
            ArtifactTier::Keep
        } else if has_ext(&[".log", ".ansi"]) || (self.bytes >= 1_000_000 && has_ext(&[".json"])) {
            ArtifactTier::Transient
        } else {
            ArtifactTier::Normal
        }
    }
}
