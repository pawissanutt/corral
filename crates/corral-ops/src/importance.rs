//! Artifact importance (T-060): ordering, one-line excerpts and reviewed marks on top of the stored tiers
//! (`Artifact::tier`, which honours an explicit `tier_override`). Every list of artifacts uses the same order.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use anyhow::{Context, Result};
use corral_core::{Artifact, ArtifactKind};

use crate::Ctx;

/// Snapshots larger than this get no excerpt (their original size, before any compression).
const EXCERPT_MAX_BYTES: u64 = 4 * 1024 * 1024;

fn kind_rank(a: &Artifact) -> u8 {
    match a.kind {
        ArtifactKind::Report => 0,
        ArtifactKind::Summary => 1,
        _ => 2,
    }
}

/// Importance order: tier (keep, normal, transient), then report before summary before the rest, then newest
/// first.
pub fn cmp_importance(a: &Artifact, b: &Artifact) -> Ordering {
    a.tier()
        .cmp(&b.tier())
        .then(kind_rank(a).cmp(&kind_rank(b)))
        .then(b.added.cmp(&a.added))
}

/// Sorts artifacts into importance order (stable).
pub fn order_artifacts(arts: &mut [Artifact]) {
    arts.sort_by(cmp_importance);
}

/// Index of the artifact the review screen opens first: the newest report, else the newest summary.
pub fn lead_artifact(arts: &[Artifact]) -> Option<usize> {
    arts.iter()
        .enumerate()
        .filter(|(_, a)| matches!(a.kind, ArtifactKind::Report | ArtifactKind::Summary))
        .min_by(|(_, a), (_, b)| cmp_importance(a, b))
        .map(|(i, _)| i)
}

/// First heading, "verdict"/"result" line or first non-empty line of a text artifact, at most 80 chars.
/// The snapshot is decoded through `artifact_read`, so compressed content reads as text; None for images,
/// binaries, purged contents and unreadable files.
pub fn excerpt(ctx: &Ctx, a: &Artifact) -> Option<String> {
    if a.purged_at.is_some() || a.bytes > EXCERPT_MAX_BYTES {
        return None;
    }
    let bytes = crate::artifact_read(ctx, a).ok()?;
    let head = &bytes[..bytes.len().min(16 * 1024)];
    if head.contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(head);
    let pick = |f: &dyn Fn(&str) -> bool| {
        text.lines()
            .map(str::trim)
            .find(|l| f(l))
            .map(str::to_string)
    };
    let line = pick(&|l| l.starts_with('#'))
        .or_else(|| {
            pick(&|l| {
                let low = l.to_ascii_lowercase();
                low.starts_with("verdict") || low.starts_with("result")
            })
        })
        .or_else(|| pick(&|l| !l.is_empty()))?;
    let line = line.trim_start_matches('#').trim();
    Some(line.chars().take(80).collect())
}

fn reviewed_path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.store
        .dir()
        .join("state")
        .join("reviewed-artifacts.json")
}

/// Snapshot paths the reviewer has marked reviewed.
pub fn reviewed_set(ctx: &Ctx) -> BTreeSet<String> {
    std::fs::read_to_string(reviewed_path(ctx))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Toggles the reviewed mark of `a`; returns the new state (true = reviewed).
pub fn toggle_reviewed(ctx: &Ctx, a: &Artifact) -> Result<bool> {
    let mut set = reviewed_set(ctx);
    let now = set.insert(a.path.clone());
    if !now {
        set.remove(&a.path);
    }
    let path = reviewed_path(ctx);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(&set)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
    Ok(now)
}

#[cfg(test)]
#[path = "importance_tests.rs"]
mod tests;
