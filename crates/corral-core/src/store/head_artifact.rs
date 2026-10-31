//! Keyed edits of artifact registrations, without hydrating the owning task.
use super::codec::ordinal_key;
use super::head::TaskHead;
use super::head_mut::TaskHeadMut;
use crate::backend::{RowQuery, TaskPart};
use crate::work::Counter;
use anyhow::{Context, Result};

pub(crate) fn artifact_indexes(artifact: &crate::Artifact) -> Vec<String> {
    let file = crate::snapshot_name(artifact);
    let original = match file.split_once('-') {
        Some((sha, rest)) if sha.len() == 8 && sha.chars().all(|c| c.is_ascii_hexdigit()) => rest,
        _ => file,
    };
    vec![
        format!("run:{}", artifact.run),
        digest_token(&artifact.run, &artifact.sha256),
        format!("run-path:{}:{}", artifact.run, artifact.path),
        format!("run-title:{}:{}", artifact.run, artifact.title),
        format!("run-kind:{}:{}", artifact.run, artifact.kind.as_str()),
        format!("path:{}", artifact.path),
        format!("file:{}:{file}", artifact.run),
        format!("original:{}:{original}", artifact.run),
        format!("title:{}", artifact.title),
        format!("source:{}", artifact.source),
    ]
}

pub(crate) fn digest_token(run: &str, sha: &str) -> String {
    format!("run-sha:{}:{run}:{sha}", run.len())
}

impl TaskHead {
    /// The first registration of this run and content, including purged registrations.
    /// Returns its zero-based task-wide ordinal; names, kinds and retention never change ordering.
    pub fn artifact_registration_by_sha(
        &self,
        run: &str,
        sha: &str,
    ) -> Result<Option<(usize, crate::Artifact)>> {
        self.query(
            &RowQuery::part(TaskPart::Artifact).with_index(digest_token(run, sha)),
            1,
            Counter::HistoryRowDecodes,
        )?
        .into_iter()
        .next()
        .map(|row| {
            let ordinal: usize = row.key.parse().context("artifact row key")?;
            let index = ordinal.checked_sub(1).context("artifact row ordinal 0")?;
            Ok((index, serde_json::from_value(row.doc)?))
        })
        .transpose()
    }
}

impl TaskHeadMut {
    /// Edits a zero-based registration ordinal, maintaining retention and current-run facts.
    pub fn edit_artifact(
        &mut self,
        index: usize,
        edit: impl FnOnce(&mut crate::Artifact) -> Result<()>,
    ) -> Result<()> {
        let key = ordinal_key(index + 1);
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
