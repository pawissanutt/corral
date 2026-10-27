//! Maintained reviewed-Keep counts, keyed by task and snapshot path.
use std::collections::{BTreeMap, BTreeSet};

use super::super::TaskNum;

pub(crate) fn paths(doc: &serde_json::Value) -> anyhow::Result<BTreeSet<String>> {
    crate::records::RecordArea::new("reviewed-artifacts").decode_document(doc.clone())
}

pub(crate) fn kept_path(doc: &serde_json::Value) -> anyhow::Result<Option<String>> {
    // The low-level blob promotion contract uses ArtifactRow, not a facade registration.
    if doc.get("path").is_none() {
        return Ok(None);
    }
    let artifact: crate::Artifact = serde_json::from_value(doc.clone())?;
    Ok(
        (artifact.purged_at.is_none() && artifact.tier() == crate::ArtifactTier::Keep)
            .then_some(artifact.path),
    )
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Reviews {
    paths: BTreeSet<String>,
    kept: BTreeMap<String, BTreeSet<(TaskNum, String)>>,
    counts: BTreeMap<TaskNum, u64>,
}

impl Reviews {
    pub fn count(&self, task: TaskNum) -> u64 {
        self.counts.get(&task).copied().unwrap_or(0)
    }

    pub fn replace(&mut self, paths: BTreeSet<String>) {
        for path in self.paths.symmetric_difference(&paths) {
            let add = paths.contains(path);
            for (task, _) in self.kept.get(path).into_iter().flatten() {
                let n = self.counts.entry(*task).or_default();
                *n = if add { *n + 1 } else { n.saturating_sub(1) };
            }
        }
        self.paths = paths;
    }

    pub fn remove(&mut self, task: TaskNum, key: &str, path: &str) {
        if let Some(kept) = self.kept.get_mut(path)
            && kept.remove(&(task, key.to_owned()))
        {
            if self.paths.contains(path) {
                let n = self.counts.entry(task).or_default();
                *n = n.saturating_sub(1);
            }
            if kept.is_empty() {
                self.kept.remove(path);
            }
        }
    }

    pub fn insert(&mut self, task: TaskNum, key: &str, path: String) {
        if self
            .kept
            .entry(path.clone())
            .or_default()
            .insert((task, key.to_owned()))
            && self.paths.contains(&path)
        {
            *self.counts.entry(task).or_default() += 1;
        }
    }
}
