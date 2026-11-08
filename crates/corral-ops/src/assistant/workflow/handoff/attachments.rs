//! Publish selected source context as ordinary, task-scoped, readable artifacts.
//! Blob references alone are not a continuation interface for a fresh planner/worker.
use super::{Prepared, content};
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::{Artifact, ArtifactKind};
use serde::Serialize;

#[derive(Serialize)]
struct Part {
    path: String,
    sha256: String,
    offset: usize,
    bytes: usize,
}
#[derive(Serialize)]
struct Entry {
    label: String,
    sha256: String,
    bytes: usize,
    parts: Vec<Part>,
}

pub(super) fn publish(ctx: &Ctx, task: &str, prepared: &Prepared) -> Result<()> {
    let package = prepared.package();
    let package_bytes = serde_json::to_vec_pretty(package)?;
    let refs: Vec<_> = std::iter::once(("original-request".to_string(), &package.original_request))
        .chain(package.plan.iter().flat_map(|p| {
            std::iter::once(("current-plan".into(), &p.artifact))
                .chain(
                    p.assumptions
                        .iter()
                        .map(|a| ("ready-assumptions".into(), a)),
                )
                .chain(p.execution_scope.iter().map(|a| ("ready-scope".into(), a)))
        }))
        .chain(
            package
                .decisions
                .iter()
                .enumerate()
                .map(|(i, a)| (format!("decision-{i}"), a)),
        )
        .chain(
            package
                .reviews
                .iter()
                .enumerate()
                .map(|(i, a)| (format!("review-{i}"), a)),
        )
        .chain(
            package
                .execution
                .iter()
                .map(|a| ("execution-evidence".into(), a)),
        )
        .collect();
    let total = refs.iter().try_fold(
        package_bytes
            .len()
            .checked_add(package.context.bytes)
            .context("context size overflow")?,
        |n, (_, a)| {
            n.checked_add(a.bytes as usize)
                .context("context size overflow")
        },
    )?;
    ensure!(
        total <= content::TOTAL_MAX,
        "handoff_context_oversized: selected continuation context exceeds 64 MiB"
    );
    let mut inputs = vec![
        ("package".into(), package_bytes),
        ("context".into(), package.context.load(&ctx.store)?),
    ];
    for (label, reference) in refs {
        inputs.push((label, super::load_artifact(&ctx.store, reference)?));
    }
    let cap = usize::try_from(ctx.store.config_layers()?.config()?.artifacts.max_bytes)?;
    ensure!(
        cap >= 4,
        "handoff_artifact_limit: context needs an artifact byte cap of at least four"
    );
    let count: usize = inputs.iter().map(|(_, b)| b.len().div_ceil(cap)).sum();
    ensure!(
        count <= 128,
        "handoff_artifact_limit: selected context exceeds 128 artifact parts; raise the normal artifact limit or narrow the explicit context"
    );
    let stable = prepared.id().replace('/', "-");
    let _storage = crate::artifact_storage::storage_lock(ctx)?;
    ctx.store.update(task, |task| {
        let mut entries = Vec::new();
        for (label, bytes) in &inputs {
            let text = std::str::from_utf8(bytes).context("handoff_context: selected continuation text is not UTF-8")?;
            let mut parts = Vec::new();
            let mut offset = 0;
            while offset < bytes.len() {
                let mut end = offset.saturating_add(cap).min(bytes.len());
                while !text.is_char_boundary(end) { end -= 1; }
                let chunk = &bytes[offset..end];
                let part = attach(ctx, task, &stable, &format!("{label}-{}",parts.len()), chunk)?;
                parts.push(Part {path:part.path, sha256:part.sha256, offset, bytes:chunk.len()});
                offset = end;
            }
            entries.push(Entry { label:label.clone(), sha256:corral_core::blob_ref::BlobHash::of(bytes).to_string(), bytes:bytes.len(), parts });
        }
        let index = serde_json::to_vec_pretty(&serde_json::json!({
            "version":1,"package":prepared.id(),"entries":entries,
            "code":"Code snapshot metadata is in package; code layers are preserved in the verified checkpoint, not claimed loaded or approved by these context artifacts."
        }))?;
        ensure!(index.len() <= cap, "handoff_artifact_limit: archive index exceeds the normal artifact byte cap");
        attach(ctx, task, &stable, "source-index", &index)?;
        Ok(())
    })?;
    Ok(())
}

fn attach(
    ctx: &Ctx,
    task: &mut corral_core::Task,
    stable: &str,
    label: &str,
    bytes: &[u8],
) -> Result<Artifact> {
    let sha256 = corral_core::blob_ref::BlobHash::of(bytes).to_string();
    let path = format!(
        "artifacts/{}/R0/{}-cowork-{stable}-{label}.json",
        task.id,
        &sha256[..8]
    );
    if let Some(existing) = task.artifacts.iter().find(|a| a.path == path) {
        ensure!(
            existing.sha256 == sha256 && existing.purged_at.is_none(),
            "handoff_missing: archived continuation context was purged"
        );
        crate::artifact_storage::artifact_read_confined(ctx, existing)
            .context("handoff_missing: archived continuation context unavailable")?;
        return Ok(existing.clone());
    }
    crate::artifact_storage::store_blob(ctx, &task.id, &sha256, bytes)?;
    let artifact = Artifact {
        run: "R0".into(),
        kind: ArtifactKind::Evidence,
        title: format!("Co-work selected source {label}"),
        path,
        source: format!("assistant-handoff://{stable}/{label}"),
        sha256,
        bytes: u64::try_from(bytes.len())?,
        added: corral_core::clock::now(),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    };
    task.artifacts.push(artifact.clone());
    crate::artifact_storage::record_keys(ctx, task, task.artifacts.len() - 1, true)?;
    Ok(artifact)
}
