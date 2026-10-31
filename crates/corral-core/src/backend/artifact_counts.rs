//! Backend-maintained counts of canonical registrations per task/run, for bounded appends.
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) const AREA: &str = "artifact-run-counts";

pub(crate) fn changes(before: Option<&Value>, after: Option<&Value>) -> BTreeMap<String, i64> {
    let mut changes = BTreeMap::new();
    for (doc, delta) in [(before, -1), (after, 1)] {
        if let Some(doc) = doc.filter(|d| d.get("path").is_some())
            && let Some(run) = doc["run"].as_str()
        {
            *changes.entry(run.to_owned()).or_default() += delta;
        }
    }
    changes.retain(|_, n| *n != 0);
    changes
}

pub(crate) fn count(doc: Option<&Value>) -> Result<u64> {
    doc.map(|doc| {
        doc.as_str()
            .context("invalid artifact count")?
            .parse()
            .context("parse artifact count")
    })
    .transpose()
    .map(|n| n.unwrap_or(0))
}

pub(crate) fn adjusted(doc: Option<&Value>, delta: i64) -> Result<Value> {
    let count = count(doc)?
        .checked_add_signed(delta)
        .context("artifact count overflow")?;
    Ok(Value::String(format!("{count:020}")))
}
