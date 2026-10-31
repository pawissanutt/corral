//! Explicit cold review of the selected final diff when its worktree is gone.
use std::io::Read;

use anyhow::{Context, Result, ensure};
use corral_core::{Artifact, ArtifactKind};
use corral_git::DiffStat;

use crate::Ctx;

pub(super) fn read(ctx: &Ctx, artifacts: &[Artifact]) -> Result<(DiffStat, String)> {
    let text = match artifacts
        .iter()
        .rev()
        .find(|a| a.kind == ArtifactKind::Diff)
    {
        Some(artifact) => {
            // Preserve the complete diff-stat contract, reading the canonical body (or
            // imported gzip), not a presumed mirror. Declared bytes plus one bound decoding.
            let mut text = String::new();
            let mut reader =
                crate::artifact_reader(ctx, artifact)?.take(artifact.bytes.saturating_add(1));
            reader
                .read_to_string(&mut text)
                .context("reading final diff snapshot")?;
            ensure!(
                u64::try_from(text.len())? == artifact.bytes,
                "final diff snapshot length differs from declared content"
            );
            text
        }
        None => String::new(),
    };
    Ok((DiffStat::from_diff(&text), text))
}
