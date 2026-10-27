//! Eligible lesson artifacts, ordered independently of append keys.
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use jiff::Timestamp;
use serde_json::Value;

use super::TaskNum;

pub(crate) fn metadata(doc: &Value) -> anyhow::Result<Option<(Timestamp, String)>> {
    // Low-level blob promotion also stores ArtifactRow documents without facade paths.
    if doc.get("path").is_none() {
        return Ok(None);
    }
    let artifact: crate::Artifact = serde_json::from_value(doc.clone())?;
    Ok(matches!(
        artifact.kind,
        crate::ArtifactKind::Log
            | crate::ArtifactKind::Summary
            | crate::ArtifactKind::Report
            | crate::ArtifactKind::Diff
            | crate::ArtifactKind::Evidence
    )
    .then_some((artifact.added, artifact.path)))
}

type Order = (Reverse<Timestamp>, Reverse<String>, String);

#[derive(Debug, Clone, Default)]
pub(crate) struct Artifacts {
    tasks: BTreeMap<TaskNum, BTreeSet<Order>>,
    keys: BTreeMap<(TaskNum, String), Order>,
}

impl Artifacts {
    pub fn put(&mut self, task: TaskNum, key: &str, doc: Option<&Value>) -> anyhow::Result<()> {
        let meta = doc.map(metadata).transpose()?.flatten();
        if let Some(old) = self.keys.remove(&(task, key.to_owned()))
            && let Some(rows) = self.tasks.get_mut(&task)
        {
            rows.remove(&old);
        }
        if let Some((added, path)) = meta {
            let order = (Reverse(added), Reverse(path), key.to_owned());
            self.tasks.entry(task).or_default().insert(order.clone());
            self.keys.insert((task, key.to_owned()), order);
        }
        Ok(())
    }

    pub fn keys(&self, task: TaskNum) -> impl Iterator<Item = &str> {
        self.tasks
            .get(&task)
            .into_iter()
            .flat_map(|rows| rows.iter().map(|(_, _, key)| key.as_str()))
    }
}
