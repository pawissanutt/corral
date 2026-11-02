//! Legacy evidence: every legacy file is listed in `legacy_files`. A registered artifact's
//! snapshot is decoded as its metadata describes and stored as its recorded content, owned by
//! the task's `artifact-blobs` record (the owner artifact retention drops). A legacy file whose
//! bytes are a registered artifact's content is a copy of it with no reference of its own, so
//! retention ends every copy; one whose content only purged artifacts named is not revived.
//! Every other kept file is stored as a blob its `legacy_files` row references.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use flate2::read::MultiGzDecoder;

use super::{ArtifactBlob, Blobs, Collected, ImportReport, legacy_file};
use crate::Artifact;
use crate::backend::BlobStore;
use crate::blob::FsBlobStore;
use crate::blob_ref::BlobHash;
use crate::migrate::blockers::LegacyBoard;
use crate::migrate::classify::Disposition;
use crate::migrate::scan::{Classified, Manifest};
use crate::sqlite::SqliteBackend;

pub(super) const ARTIFACT_BLOBS: &str = "artifact-blobs";

pub(crate) struct Registration<'a> {
    pub task: &'a str,
    pub artifact: &'a Artifact,
}

impl Registration<'_> {
    pub(crate) fn name(&self) -> String {
        format!(
            "{} {} artifact {:?} ({})",
            self.task, self.artifact.run, self.artifact.title, self.artifact.path
        )
    }

    fn live(&self) -> bool {
        self.artifact.purged_at.is_none()
    }
}

pub(crate) fn registrations(board: &LegacyBoard) -> BTreeMap<&str, Vec<Registration<'_>>> {
    let mut out: BTreeMap<&str, Vec<Registration<'_>>> = BTreeMap::new();
    for (task, _) in &board.tasks {
        for artifact in &task.task.artifacts {
            out.entry(artifact.path.as_str())
                .or_default()
                .push(Registration {
                    task: &task.task.id,
                    artifact,
                });
        }
    }
    for session in &board.assistants {
        for artifact in &session.artifacts {
            out.entry(artifact.path.as_str())
                .or_default()
                .push(Registration {
                    task: &session.id,
                    artifact,
                });
        }
    }
    out
}

pub(super) fn import(
    corral: &Path,
    classified: &[Classified],
    hashes: &BTreeMap<&str, &str>,
    board: &LegacyBoard,
    blobs: &mut Blobs<'_>,
    collected: &mut Collected,
    report: &mut ImportReport,
) -> Result<()> {
    let registered = registrations(board);
    let all = || registered.values().flatten();
    let live: BTreeSet<&str> = all()
        .filter(|r| r.live())
        .map(|r| r.artifact.sha256.as_str())
        .collect();
    let purged: BTreeSet<&str> = all()
        .filter(|r| !r.live() && !live.contains(r.artifact.sha256.as_str()))
        .map(|r| r.artifact.sha256.as_str())
        .collect();
    for item in classified {
        let path = corral.join(&item.path);
        let meta = std::fs::symlink_metadata(&path)
            .with_context(|| format!("inspecting {}", path.display()))?;
        if item.area.disposition == Disposition::Runtime {
            super::super::workspace::copy_runtime(corral, &item.path)?;
        }
        let sha = hashes.get(item.path.as_str()).copied();
        let kind = item.area.name;
        let copy = meta.is_file()
            && matches!(
                item.area.disposition,
                Disposition::Artifact | Disposition::Evidence
            );
        let row = match (item.area.disposition, registered.get(item.path.as_str())) {
            (Disposition::Task | Disposition::Events, _) => {
                legacy_file(&item.path, kind, meta.len(), sha, None, "imported")
            }
            (Disposition::Cache | Disposition::Tmp | Disposition::Lock, _) => {
                legacy_file(&item.path, kind, meta.len(), sha, None, "not_imported")
            }
            (Disposition::Artifact, Some(regs)) => {
                let disposition = artifact(&path, &item.path, regs, blobs, collected, report)?;
                legacy_file(&item.path, kind, meta.len(), sha, None, disposition)
            }
            _ if copy && sha.is_some_and(|s| live.contains(s)) => {
                legacy_file(&item.path, kind, meta.len(), sha, None, "artifact_copy")
            }
            _ if copy && sha.is_some_and(|s| purged.contains(s)) => legacy_file(
                &item.path,
                kind,
                meta.len(),
                sha,
                None,
                "purged_artifact_copy",
            ),
            _ => {
                let hash = if meta.file_type().is_symlink() {
                    let target = std::fs::read_link(&path)
                        .with_context(|| format!("reading link {}", path.display()))?;
                    blobs.put(&mut target.as_os_str().as_encoded_bytes())?.0
                } else {
                    blobs.put_file(&path)?
                };
                collected.kept(&item.path, hash.clone());
                legacy_file(&item.path, kind, meta.len(), sha, Some(hash), "evidence")
            }
        };
        collected.rows.legacy_files.push(row);
    }
    reconcile(board, report)
}

/// Decodes a registered snapshot as its metadata describes (gzip when `compressed_bytes` is
/// set), checks the decoded content against every live registration and records it as each
/// task's artifact blob. A snapshot only purged registrations name keeps no content.
fn artifact(
    path: &Path,
    rel: &str,
    regs: &[Registration<'_>],
    blobs: &mut Blobs<'_>,
    collected: &mut Collected,
    report: &mut ImportReport,
) -> Result<&'static str> {
    let Some(first) = regs.iter().find(|r| r.live()) else {
        return Ok("artifact_purged");
    };
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut reader: Box<dyn Read> = if first.artifact.compressed_bytes.is_some() {
        Box::new(MultiGzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let (hash, bytes) = blobs
        .put(&mut reader)
        .with_context(|| format!("decoding {}", first.name()))?;
    for reg in regs.iter().filter(|r| r.live()) {
        ensure!(
            hash.as_str() == reg.artifact.sha256 && bytes == reg.artifact.bytes,
            "{} decodes to {hash} ({bytes} bytes); its task records {} ({} bytes)",
            reg.name(),
            reg.artifact.sha256,
            reg.artifact.bytes
        );
        report.artifact_blobs.push(ArtifactBlob {
            task: reg.task.to_owned(),
            path: rel.to_owned(),
            hash: hash.clone(),
        });
        collected.raw(
            ARTIFACT_BLOBS,
            &format!("{}/{}", reg.task, reg.artifact.sha256),
            serde_json::json!({"task": reg.task, "sha256": reg.artifact.sha256, "blob": hash.to_ref()}),
        );
    }
    Ok("artifact")
}

/// Every live registration was imported from its own snapshot.
fn reconcile(board: &LegacyBoard, report: &ImportReport) -> Result<()> {
    let imported: BTreeSet<(&str, &str)> = report
        .artifact_blobs
        .iter()
        .map(|a| (a.task.as_str(), a.path.as_str()))
        .collect();
    let missing: Vec<String> = registrations(board)
        .values()
        .flatten()
        .filter(|r| r.live() && !imported.contains(&(r.task, r.artifact.path.as_str())))
        .map(Registration::name)
        .collect();
    ensure!(
        missing.is_empty(),
        "{} registered artifacts have no snapshot to import: {}",
        missing.len(),
        missing.join("; ")
    );
    Ok(())
}

/// Every live registration is imported and decodes to its recorded sha256, and every stored
/// legacy file decodes to its hash.
pub(super) fn verify(
    corral: &Path,
    backend: &SqliteBackend,
    store: &FsBlobStore,
    board: &LegacyBoard,
    m0: &Manifest,
    report: &ImportReport,
) -> Result<()> {
    reconcile(board, report)?;
    for blob in &report.artifact_blobs {
        let mut reader = store.get(&blob.hash)?;
        std::io::copy(&mut reader, &mut std::io::sink()).with_context(|| {
            format!(
                "{} artifact {} does not decode to {}",
                blob.task, blob.path, blob.hash
            )
        })?;
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
    for row in rows.iter().filter(|r| r.kind == "role-sessions") {
        let path = super::super::workspace::runtime_path(corral, &row.path)?;
        ensure!(
            Some(super::super::scan::sha256_file(&path)?) == row.sha256,
            "runtime copy differs: {}",
            row.path
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
