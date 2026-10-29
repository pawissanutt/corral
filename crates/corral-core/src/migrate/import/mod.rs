//! A4-A5's import into the candidate database: blobs first, then tasks, events and records as
//! `ImportBatch` operations, then the rows `ImportBatch` has no field for; and the checks that
//! gate `verified`.

mod events;
mod evidence;
mod inbox;
mod records;
mod tasks;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use super::Migrate;
use super::blockers::LegacyBoard;
use super::scan::{Manifest, classify_all};
use crate::backend::{ImportBatch, ImportedBlob, Operation, StoreBackend};
use crate::blob::FsBlobStore;
use crate::blob_ref::BlobHash;
use crate::sqlite::SqliteBackend;
use crate::sqlite::migrate::{LegacyFileRow, MigrationRows};

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    pub tasks: u64,
    pub events: u64,
    pub malformed_events: u64,
    pub records: BTreeMap<String, u64>,
    pub claims: u64,
    pub authorizations: u64,
    pub legacy_files: u64,
    pub blobs: u64,
    /// Blobs whose content an earlier apply had already stored.
    pub blobs_reused: u64,
    pub blob_raw_bytes: u64,
    pub blob_stored_bytes: u64,
    /// Content hashes of the artifacts, checked against their recorded sha256.
    #[serde(skip)]
    pub(crate) artifact_blobs: Vec<(String, BlobHash)>,
}

/// Blobs the import stored, by hash, ready for one `ImportBatch`.
pub(crate) struct Blobs<'a> {
    store: &'a FsBlobStore,
    rows: BTreeMap<BlobHash, ImportedBlob>,
    reused: u64,
}

impl<'a> Blobs<'a> {
    fn new(store: &'a FsBlobStore) -> Self {
        Self {
            store,
            rows: BTreeMap::new(),
            reused: 0,
        }
    }

    /// Stores `content` (deduplicated by hash) and returns its hash.
    pub(crate) fn put(&mut self, content: &mut dyn std::io::Read) -> Result<BlobHash> {
        use crate::backend::BlobStore;
        let staged = self.store.stage(content)?;
        let hash = staged.hash.clone();
        let existed = self.store.path_of(&hash).is_file();
        let row = ImportedBlob {
            hash: hash.clone(),
            raw_bytes: staged.raw_bytes,
            stored_bytes: staged.stored_bytes,
            codec: staged.codec.clone(),
        };
        self.store.put_staged(staged)?;
        if self.rows.insert(hash.clone(), row).is_none() && existed {
            self.reused += 1;
        }
        Ok(hash)
    }

    pub(crate) fn put_file(&mut self, path: &Path) -> Result<BlobHash> {
        let mut file =
            std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        self.put(&mut file)
            .with_context(|| format!("storing {}", path.display()))
    }
}

/// What the evidence and record passes produce besides blobs: records by `(area, key)` (a key
/// written twice keeps one record), the rows `ImportBatch` cannot carry, and each kept legacy
/// file's blob.
#[derive(Default)]
pub(crate) struct Collected {
    records: BTreeMap<(String, String), serde_json::Value>,
    pub(crate) rows: MigrationRows,
    blobs: BTreeMap<String, BlobHash>,
}

impl Collected {
    pub(crate) fn raw(&mut self, area: &str, key: &str, doc: serde_json::Value) {
        self.records.insert((area.to_owned(), key.to_owned()), doc);
    }

    pub(crate) fn kept(&mut self, path: &str, blob: BlobHash) {
        self.blobs.insert(path.to_owned(), blob);
    }

    pub(crate) fn blob_of(&self, path: &str) -> Result<BlobHash> {
        self.blobs
            .get(path)
            .cloned()
            .with_context(|| format!("{path} was not stored as a blob"))
    }

    fn take_records(&mut self, at: jiff::Timestamp) -> Vec<crate::backend::StoredRecord> {
        std::mem::take(&mut self.records)
            .into_iter()
            .map(|((area, key), doc)| crate::backend::StoredRecord {
                area,
                key,
                version: 1,
                doc,
                updated: at,
            })
            .collect()
    }
}

fn apply(backend: &SqliteBackend, batch: ImportBatch, what: &str) -> Result<()> {
    backend
        .apply(Operation::ImportBatch(batch))
        .map(drop)
        .map_err(crate::store::apply_error)
        .with_context(|| format!("importing {what}"))
}

/// Imports the legacy tree under `m`'s store into `backend`, whose blobs go to `store`.
pub(crate) fn run(
    m: &Migrate,
    backend: &SqliteBackend,
    store: &FsBlobStore,
    m0: &Manifest,
) -> Result<ImportReport> {
    let corral = m.corral_dir();
    let board = LegacyBoard::read(corral)?;
    let lines = events::read(corral)?;
    let (classified, _) = classify_all(m0);
    let mut blobs = Blobs::new(store);
    let mut collected = Collected::default();
    let mut report = ImportReport::default();
    let hashes: BTreeMap<&str, &str> = m0
        .entries
        .iter()
        .filter_map(|e| e.sha256.as_deref().map(|h| (e.path.as_str(), h)))
        .collect();
    evidence::import(
        corral,
        &classified,
        &hashes,
        &board,
        &mut blobs,
        &mut collected,
        &mut report,
    )?;
    records::import(corral, &classified, &mut collected, &mut report)?;
    inbox::import(corral, &board, &classified, &mut collected)?;
    report.blobs = blobs.rows.len() as u64;
    report.blobs_reused = blobs.reused;
    report.blob_raw_bytes = blobs.rows.values().map(|b| b.raw_bytes).sum();
    report.blob_stored_bytes = blobs.rows.values().map(|b| b.stored_bytes).sum();
    let at = jiff::Timestamp::now();
    apply(
        backend,
        ImportBatch {
            blobs: blobs.rows.into_values().collect(),
            tasks: Vec::new(),
            events: Vec::new(),
            records: Vec::new(),
        },
        "blobs",
    )?;
    m.hit("B3")?;
    tasks::import(backend, &board, &lines, &mut report)?;
    events::import(backend, &lines, &mut report)?;
    let records = collected.take_records(at);
    report.records = records.iter().fold(BTreeMap::new(), |mut counts, record| {
        let area = record
            .area
            .split('/')
            .next()
            .unwrap_or(&record.area)
            .to_owned();
        *counts.entry(area).or_default() += 1;
        counts
    });
    apply(
        backend,
        ImportBatch {
            blobs: Vec::new(),
            tasks: Vec::new(),
            events: Vec::new(),
            records,
        },
        "records",
    )?;
    report.claims = collected.rows.claims.len() as u64;
    report.authorizations = collected.rows.authorizations.len() as u64;
    report.legacy_files = collected.rows.legacy_files.len() as u64;
    backend.import_migration_rows(&collected.rows, at)?;
    Ok(report)
}

/// The import checks that gate `verified`.
pub(crate) fn verify(
    corral: &Path,
    backend: &SqliteBackend,
    store: &FsBlobStore,
    m0: &Manifest,
    report: &ImportReport,
) -> Result<()> {
    let board = LegacyBoard::read(corral)?;
    tasks::verify(backend, &board)?;
    events::verify(corral, backend)?;
    evidence::verify(backend, store, m0, report)
}

pub(crate) fn legacy_file(
    path: &str,
    kind: &str,
    bytes: u64,
    sha256: Option<&str>,
    blob: Option<BlobHash>,
    disposition: &str,
) -> LegacyFileRow {
    LegacyFileRow {
        path: path.to_owned(),
        kind: kind.to_owned(),
        bytes,
        sha256: sha256.map(str::to_owned),
        blob,
        disposition: disposition.to_owned(),
    }
}
