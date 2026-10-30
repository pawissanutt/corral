//! Keyed edits of artifact registrations, without hydrating the owning task.
use super::codec::seq;
use super::head_mut::TaskHeadMut;
use crate::backend::{RowQuery, TaskPart};
use crate::work::Counter;
use anyhow::{Context, Result};

impl TaskHeadMut {
    /// Edits a zero-based registration ordinal, maintaining retention and current-run facts.
    pub fn edit_artifact(
        &mut self,
        index: usize,
        edit: impl FnOnce(&mut crate::Artifact) -> Result<()>,
    ) -> Result<()> {
        let key = seq(index + 1);
        let mut row = self
            .rows_now(
                &RowQuery::key(TaskPart::Artifact, key),
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .next()
            .context("artifact ordinal missing")?;
        let before: crate::Artifact = serde_json::from_value(row.doc.clone())?;
        let mut after = before.clone();
        edit(&mut after)?;
        anyhow::ensure!(
            before.run == after.run && before.path == after.path && before.sha256 == after.sha256,
            "artifact identity changed"
        );
        let kept = |a: &crate::Artifact| {
            u64::from(a.tier() == crate::ArtifactTier::Keep && a.purged_at.is_none())
        };
        self.head.summary.kept_artifacts =
            self.head.summary.kept_artifacts - kept(&before) + kept(&after);
        row.doc = serde_json::to_value(&after)?;
        row.index = super::head_api::artifact_indexes(&after);
        self.put(row);
        if self
            .head
            .current_run
            .as_ref()
            .is_some_and(|r| r.id == after.run)
        {
            let mut facts = crate::container_facts::ArtifactFacts::default();
            for artifact in self.overlaid()?.artifacts_of(&after.run)? {
                facts.fold(&artifact);
            }
            self.head.summary.current_artifacts = facts;
        }
        Ok(())
    }
}
