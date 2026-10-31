//! Keyed edits of artifact registrations, without hydrating the owning task.
use super::codec::ordinal_key;
use super::head::TaskHead;
use super::head_mut::TaskHeadMut;
use crate::backend::{RowQuery, TaskPart};
use crate::work::Counter;
use anyhow::{Context, Result};

pub(crate) const RUN_ORDINAL: &str = "_run_ordinal";

pub(crate) fn stored_ordinal(doc: &serde_json::Value) -> Result<u64> {
    doc[RUN_ORDINAL]
        .as_str()
        .context("artifact run ordinal missing")?
        .parse()
        .context("invalid artifact run ordinal")
}

pub(crate) fn artifact_indexes(artifact: &crate::Artifact) -> Vec<String> {
    let file = crate::snapshot_name(artifact);
    let original = match file.split_once('-') {
        Some((sha, rest)) if sha.len() == 8 && sha.chars().all(|c| c.is_ascii_hexdigit()) => rest,
        _ => file,
    };
    let mut tokens = vec![
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
    ];
    if artifact.purged_at.is_some() {
        tokens.push(purged_token(&artifact.run));
    }
    tokens
}

pub(crate) fn purged_token(run: &str) -> String {
    format!("run-purged:{}:{run}", run.len())
}

pub(crate) fn digest_token(run: &str, sha: &str) -> String {
    format!("run-sha:{}:{run}:{sha}", run.len())
}

impl TaskHead {
    pub(crate) fn next_artifact_ordinal(&self, run: &str) -> Result<u64> {
        if self.current_run.as_ref().is_some_and(|r| r.id == run) {
            return self
                .summary
                .current_artifacts
                .count
                .checked_add(1)
                .context("artifact ordinal overflow");
        }
        let view = self.pinned()?;
        let row = view
            .records(
                &crate::backend::RecordQuery {
                    area: Some(crate::backend::artifact_counts::AREA.into()),
                    key: Some(format!("{}/{run}", self.id)),
                },
                crate::backend::PageReq::first(1),
            )?
            .items
            .into_iter()
            .next();
        if let Some(row) = &row {
            crate::work::decoded(
                Counter::RecordDecodes,
                u64::try_from(serde_json::to_vec(&row.doc)?.len())?,
                crate::work::Subject::Plain,
            );
        }
        crate::backend::artifact_counts::count(row.as_ref().map(|r| &r.doc))?
            .checked_add(1)
            .context("artifact ordinal overflow")
    }

    fn first_purged_artifact(
        &self,
        run: &str,
    ) -> Result<Option<crate::accepted_proof::ProofFailure>> {
        self.query(
            &RowQuery::part(TaskPart::Artifact).with_index(purged_token(run)),
            1,
            Counter::HistoryRowDecodes,
        )?
        .into_iter()
        .next()
        .map(|row| {
            let ordinal = stored_ordinal(&row.doc)?;
            let artifact: crate::Artifact = serde_json::from_value(row.doc)?;
            Ok(crate::accepted_proof::ProofFailure {
                ordinal,
                path: artifact.path,
                kind: crate::accepted_proof::ProofFailureKind::Purged,
            })
        })
        .transpose()
    }

    fn latest_diff_title(&self, run: &str) -> Result<Option<String>> {
        self.query(
            &RowQuery::part(TaskPart::Artifact)
                .with_index(format!("run-kind:{run}:diff"))
                .newest_first(),
            1,
            Counter::HistoryRowDecodes,
        )?
        .into_iter()
        .next()
        .map(|row| {
            let artifact: crate::Artifact = serde_json::from_value(row.doc)?;
            Ok(artifact.title)
        })
        .transpose()
    }

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
        let ordinal = stored_ordinal(&row.doc)?;
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
        row.doc[RUN_ORDINAL] = format!("{ordinal:020}").into();
        row.index = super::head_api::artifact_indexes(&after);
        self.put(row);
        if self
            .head
            .current_run
            .as_ref()
            .is_some_and(|r| r.id == after.run)
        {
            let overlaid = self.overlaid()?;
            let first_purged = overlaid.first_purged_artifact(&after.run)?;
            let latest_diff = overlaid.latest_diff_title(&after.run)?;
            let facts = &mut self.head.summary.current_artifacts;
            facts.accepted_proof.replace(ordinal, &before, &after);
            facts.accepted_proof.first_purged = first_purged;
            facts.latest_diff = latest_diff;
        }
        Ok(())
    }
}
