//! On-disk task envelopes deliberately cannot be decoded by legacy flat readers.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::Task;

pub const STORE_SCHEMA_VERSION: u32 = 2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskEnvelope {
    schema_version: u32,
    task: Task,
}

#[derive(Serialize)]
struct TaskRef<'a> {
    schema_version: u32,
    task: &'a Task,
}

pub(crate) fn parse(yaml: &str) -> Result<Task> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(yaml).context("parsing task schema")?;
    if let Some(version) = value.get("schema_version") {
        if version.as_u64() != Some(u64::from(STORE_SCHEMA_VERSION)) {
            bail!("unsupported task schema {version:?}; expected {STORE_SCHEMA_VERSION}");
        }
        let envelope: TaskEnvelope =
            serde_yaml_ng::from_value(value).context("parsing versioned task")?;
        debug_assert_eq!(envelope.schema_version, STORE_SCHEMA_VERSION);
        Ok(envelope.task)
    } else {
        // Read historical records for inspection/recovery. Receipt boundaries separately
        // refuse missing candidate identities; the next save upgrades the whole record.
        serde_yaml_ng::from_value(value).context("parsing legacy task")
    }
}

pub(crate) fn render(task: &Task) -> String {
    serde_yaml_ng::to_string(&TaskRef {
        schema_version: STORE_SCHEMA_VERSION,
        task,
    })
    .unwrap_or_default()
}
