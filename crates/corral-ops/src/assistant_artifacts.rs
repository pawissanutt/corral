//! Session-owned snapshots share the task artifact byte store and retention machinery.
use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::{Artifact, ArtifactKind};
use sha2::{Digest, Sha256};

use crate::Ctx;

pub(crate) fn check_owner(session: Option<&str>) -> Result<()> {
    if let Some(caller) = std::env::var_os("CORRAL_ASSISTANT")
        && session.is_none_or(|id| caller != id)
    {
        bail!(
            "assistant_refused: artifacts must belong to the calling session; use corral_owner_action or corral pm inbox post"
        );
    }
    Ok(())
}

/// Snapshot a file on an existing session, deduplicating and reviving as for task artifacts.
pub fn add_session_artifact(
    ctx: &Ctx,
    session: &str,
    path: &Path,
    kind: Option<ArtifactKind>,
    title: Option<&str>,
) -> Result<Artifact> {
    check_owner(Some(session))?;
    corral_core::assistant::validate_id(session)?;
    let _storage = crate::artifact_storage::storage_lock(ctx)?;
    let current = ctx.store.get_assistant(session)?;
    let meta = std::fs::metadata(path).context("inspecting session artifact")?;
    if !meta.is_file() || meta.len() > ctx.store.config.artifacts.max_bytes {
        bail!("artifact must be a regular file within artifacts.max_bytes");
    }
    let bytes = std::fs::read(path).context("reading session artifact")?;
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let name = path
        .file_name()
        .context("artifact has no file name")?
        .to_string_lossy();
    let previous = current.artifacts.iter().find(|a| a.sha256 == sha);
    if let Some(a) = previous.filter(|a| a.purged_at.is_none()) {
        return Ok(a.clone());
    }
    let mut artifact = previous.cloned().unwrap_or_else(|| Artifact {
        run: String::new(),
        kind: kind.unwrap_or_else(|| ArtifactKind::guess(&name)),
        title: title.unwrap_or(&name).into(),
        path: format!("artifacts/assistant/{session}/{}-{name}", &sha[..8]),
        source: std::fs::canonicalize(path)
            .unwrap_or_else(|_| path.into())
            .to_string_lossy()
            .into(),
        sha256: sha,
        bytes: bytes.len() as u64,
        added: corral_core::clock::now(),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    });
    artifact.compressed_bytes = crate::artifact_storage::snapshot(
        ctx,
        &ctx.store.dir().join(&artifact.path),
        &bytes,
        &artifact,
    )?;
    artifact.purged_at = None;
    ctx.store.update_assistant(session, |s| {
        if let Some(old) = s.artifacts.iter_mut().find(|a| a.path == artifact.path) {
            *old = artifact.clone();
        } else {
            if artifact.kind == ArtifactKind::Diff {
                for old in s
                    .artifacts
                    .iter_mut()
                    .filter(|a| a.kind == ArtifactKind::Diff && a.tier_override.is_none())
                {
                    old.tier_override = Some(corral_core::ArtifactTier::Transient);
                }
            }
            s.artifacts.push(artifact.clone());
        }
        Ok(())
    })?;
    ctx.store.log_event(None, "artifact", serde_json::json!({
        "session": session, "kind": artifact.kind.as_str(), "path": artifact.path, "title": artifact.title,
    }))?;
    crate::content_index::snapshot(ctx, &ctx.store.list()?, false)?;
    Ok(artifact)
}

pub(crate) fn update(
    ctx: &Ctx,
    owner: &str,
    path: &str,
    f: impl FnOnce(&mut Artifact),
) -> Result<Artifact> {
    let mut result = None;
    let apply = |artifacts: &mut Vec<Artifact>| -> Result<()> {
        let a = artifacts
            .iter_mut()
            .find(|a| a.path == path)
            .context("artifact disappeared")?;
        f(a);
        result = Some(a.clone());
        Ok(())
    };
    if owner.starts_with("A-") {
        ctx.store
            .update_assistant(owner, |s| apply(&mut s.artifacts))?;
    } else {
        ctx.store.update(owner, |t| apply(&mut t.artifacts))?;
    }
    result.context("artifact update produced no result")
}
