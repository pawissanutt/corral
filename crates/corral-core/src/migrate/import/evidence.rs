//! Legacy evidence: every legacy file the import keeps is stored as a blob and listed in
//! `legacy_files`; artifacts are stored as their recorded content and referenced as v2
//! artifact blobs are.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, ensure};

use super::{Blobs, Collected, ImportReport, legacy_file};
use crate::backend::BlobStore;
use crate::blob::FsBlobStore;
use crate::blob_ref::BlobHash;
use crate::migrate::blockers::LegacyBoard;
use crate::migrate::classify::Disposition;
use crate::migrate::scan::{Classified, Manifest};
use crate::sqlite::SqliteBackend;

pub(super) const ARTIFACT_BLOBS: &str = "artifact-blobs";

pub(super) fn import(
    corral: &Path,
    classified: &[Classified],
    hashes: &BTreeMap<&str, &str>,
    board: &LegacyBoard,
    blobs: &mut Blobs<'_>,
    collected: &mut Collected,
    report: &mut ImportReport,
) -> Result<()> {
    let mut artifacts: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (task, _) in &board.tasks {
        for artifact in &task.task.artifacts {
            artifacts.insert(
                artifact.path.clone(),
                (task.task.id.clone(), artifact.sha256.clone()),
            );
        }
    }
    for item in classified {
        let path = corral.join(&item.path);
        let meta = std::fs::symlink_metadata(&path)
            .with_context(|| format!("inspecting {}", path.display()))?;
        let sha = hashes.get(item.path.as_str()).copied();
        let kind = item.area.name;
        let row = match item.area.disposition {
            Disposition::Task | Disposition::Events => {
                legacy_file(&item.path, kind, meta.len(), sha, None, "imported")
            }
            Disposition::Cache | Disposition::Tmp | Disposition::Lock => {
                legacy_file(&item.path, kind, meta.len(), sha, None, "not_imported")
            }
            Disposition::Artifact => {
                let hash = blobs.put_file(&path)?;
                if let Some((task, recorded)) = artifacts.get(&item.path) {
                    ensure!(
                        hash.as_str() == recorded,
                        "artifact {} decodes to {hash}, its task records {recorded}",
                        item.path
                    );
                    report
                        .artifact_blobs
                        .push((item.path.clone(), hash.clone()));
                    collected.raw(
                        ARTIFACT_BLOBS,
                        &format!("{task}/{recorded}"),
                        serde_json::json!({"task": task, "sha256": recorded, "blob": hash.to_ref()}),
                    );
                }
                collected.kept(&item.path, hash.clone());
                legacy_file(&item.path, kind, meta.len(), sha, Some(hash), "artifact")
            }
            _ => {
                let hash = if meta.file_type().is_symlink() {
                    let target = std::fs::read_link(&path)
                        .with_context(|| format!("reading link {}", path.display()))?;
                    blobs.put(&mut target.as_os_str().as_encoded_bytes())?
                } else {
                    blobs.put_file(&path)?
                };
                collected.kept(&item.path, hash.clone());
                legacy_file(&item.path, kind, meta.len(), sha, Some(hash), "evidence")
            }
        };
        collected.rows.legacy_files.push(row);
    }
    Ok(())
}

/// Every stored blob decodes to its hash, and every artifact to its recorded sha256.
pub(super) fn verify(
    backend: &SqliteBackend,
    store: &FsBlobStore,
    m0: &Manifest,
    report: &ImportReport,
) -> Result<()> {
    for (path, hash) in &report.artifact_blobs {
        let mut reader = store.get(hash)?;
        std::io::copy(&mut reader, &mut std::io::sink())
            .with_context(|| format!("artifact {path} does not decode to {hash}"))?;
    }
    let rows = backend.legacy_file_rows()?;
    let listed: BTreeMap<&str, Option<&BlobHash>> = rows
        .iter()
        .map(|r| (r.path.as_str(), r.blob.as_ref()))
        .collect();
    for entry in &m0.entries {
        ensure!(
            listed.contains_key(entry.path.as_str()),
            "legacy file {} is not recorded in the imported database",
            entry.path
        );
    }
    for row in rows.iter().filter(|r| r.blob.is_some()) {
        let hash = row.blob.as_ref().context("blob checked above")?;
        let mut reader = store.get(hash)?;
        std::io::copy(&mut reader, &mut std::io::sink())
            .with_context(|| format!("legacy file {} does not decode to {hash}", row.path))?;
    }
    Ok(())
}
