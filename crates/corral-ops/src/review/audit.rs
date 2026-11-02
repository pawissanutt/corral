//! Completion-audit evidence publication shared by ordinary and receipted completion.
use super::*;

pub(crate) fn audit_artifact(
    ctx: &Ctx,
    id: &str,
    run: &str,
    audit: &corral_core::CompletionAudit,
    named: bool,
) -> Result<()> {
    let file = ctx
        .store
        .scratch_tmp()?
        .join(format!("{id}-{run}-completion-audit.json"));
    std::fs::write(&file, serde_json::to_vec_pretty(audit)?)
        .with_context(|| format!("writing {}", file.display()))?;
    let (kind, title) = (Some(ArtifactKind::Evidence), Some("Completion audit"));
    if named {
        crate::artifacts::add_artifact_to_run(ctx, id, run, &file, kind, title).map(drop)
    } else {
        add_artifact(ctx, id, &file, kind, title).map(drop)
    }
}
