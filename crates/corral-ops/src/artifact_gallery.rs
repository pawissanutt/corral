//! Bounded read-only artifact catalog for the native Pi artifact browser.
//!
//! One newest-first page of the task's artifact rows plus the bound run's evidence links.
//! Provenance the store does not record (a candidate for an artifact without evidence, a
//! comparison order, freshness) is never derived here; the panel shows it as unknown.
use crate::Ctx;
use anyhow::{Result, bail, ensure};
use corral_core::{Artifact, ObservationStatus, PinRef};
use serde::Serialize;

const MAX_ARTIFACTS: u32 = 100;
const MAX_LINKS: usize = 8;
const MAX_RESPONSE_BYTES: usize = 128 * 1024;

#[cfg(test)]
#[path = "artifact_gallery_tests.rs"]
mod tests;

#[derive(Debug, Serialize)]
pub struct EvidenceLink {
    pub criterion: String,
    pub key: String,
    pub status: ObservationStatus,
    pub captured: String,
    /// The candidate HEAD corral computed when the observation was registered.
    pub head: String,
    /// Whether the hash recorded with the observation equals this artifact's registered hash;
    /// `None` when the observation carries no hash for this reference.
    pub hash_match: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct Entry {
    /// Stable artifact reference accepted by `corral artifact show|serve`.
    pub reference: String,
    pub run: String,
    pub kind: String,
    pub title: String,
    /// Snapshot path relative to the store directory.
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub added: String,
    pub compressed: bool,
    pub purged: bool,
    /// Bytes were readable at read: a legacy snapshot file or a live blob (not purged).
    pub available: bool,
    /// An uncompressed legacy snapshot file holding the original bytes, when one exists.
    /// Blob-backed content is encoded; read it through `corral artifact show`.
    pub raw: Option<std::path::PathBuf>,
    /// Evidence of the bound run naming this snapshot. Other runs' evidence is not read.
    pub evidence: Vec<EvidenceLink>,
    pub evidence_omitted: usize,
}

#[derive(Debug, Serialize)]
pub struct Gallery {
    pub version: u32,
    pub task: String,
    pub run: String,
    pub store: std::path::PathBuf,
    /// The bound worktree's HEAD at read; `None` when unavailable. Not a freshness verdict.
    pub head: Option<String>,
    pub artifacts: Vec<Entry>,
    /// Older artifact rows exist beyond this bounded page.
    pub more: bool,
}

fn excerpt(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut value: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        value.push('…');
    }
    value
}

/// One head read, one bounded artifact page and a stat per listed snapshot. No task hydration.
pub fn inspect(ctx: &Ctx, id: &str, run_id: &str) -> Result<Gallery> {
    let root = ctx.store.head(id)?;
    let Some(run) = root.run_head(run_id)? else {
        bail!("artifacts_run_missing");
    };
    let (rows, more) = ctx.store.artifact_window(&root.task.id, MAX_ARTIFACTS)?;
    let artifacts = rows
        .iter()
        .map(|a| entry(ctx, &root.task.id, a, &run))
        .collect::<Result<_>>()?;
    let gallery = Gallery {
        version: 1,
        task: root.task.id.clone(),
        run: run.id.clone(),
        head: corral_git::rev_sha(&run.worktree, "HEAD").ok(),
        store: ctx.store.dir().to_path_buf(),
        artifacts,
        more,
    };
    ensure!(
        serde_json::to_vec(&gallery)?.len() <= MAX_RESPONSE_BYTES,
        "artifacts_response_limit"
    );
    Ok(gallery)
}

/// Whether the bytes are readable, and the raw legacy file when the original bytes sit in one.
fn content(ctx: &Ctx, a: &Artifact) -> Result<(bool, Option<std::path::PathBuf>)> {
    if a.purged_at.is_some() {
        return Ok((false, None));
    }
    let legacy = ctx.store.dir().join(&a.path);
    if std::fs::symlink_metadata(&legacy).is_ok_and(|m| m.is_file()) {
        return Ok((true, a.compressed_bytes.is_none().then_some(legacy)));
    }
    let Ok(hash) = corral_core::blob_ref::BlobHash::parse(&a.sha256) else {
        return Ok((false, None));
    };
    let live = ctx
        .store
        .blob_row(&hash)?
        .is_some_and(|row| row.state == corral_core::backend::BlobState::Live);
    let path = ctx.store.blob_store()?.path_of(&hash);
    Ok((
        live && std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()),
        None,
    ))
}

fn entry(ctx: &Ctx, task: &str, a: &Artifact, run: &corral_core::RunHead) -> Result<Entry> {
    let mut links: Vec<EvidenceLink> = Vec::new();
    let mut omitted = 0;
    if a.run == run.id {
        for record in run.completion.evidence.iter().rev() {
            let observation = &record.observation;
            let Some(index) = observation.artifacts.iter().position(|p| *p == a.path) else {
                continue;
            };
            if links.len() == MAX_LINKS {
                omitted += 1;
                continue;
            }
            links.push(EvidenceLink {
                criterion: excerpt(&observation.criterion, 200),
                key: excerpt(&observation.key, 200),
                status: observation.status,
                captured: observation.captured.to_string(),
                head: record.identity.head.clone(),
                hash_match: record.artifact_hashes.get(index).map(|h| *h == a.sha256),
            });
        }
    }
    let (available, raw) = content(ctx, a)?;
    Ok(Entry {
        reference: PinRef::of_artifact(task, a).to_string(),
        run: a.run.clone(),
        kind: a.kind.as_str().to_owned(),
        title: excerpt(&a.title, 300),
        path: a.path.clone(),
        sha256: a.sha256.clone(),
        bytes: a.bytes,
        added: a.added.to_string(),
        compressed: a.compressed_bytes.is_some(),
        purged: a.purged_at.is_some(),
        available,
        raw,
        evidence: links,
        evidence_omitted: omitted,
    })
}
