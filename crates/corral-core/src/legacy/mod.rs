//! The legacy file format of a task, read-only (plan T-253 sections 12 and 16): Markdown with
//! YAML frontmatter, schema 2 (flat or versioned) or schema 3 (with recovery evidence). The
//! database store never reads or writes task files; `corral edit`, `corral export tasks` and
//! the migration import use this format as text.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{RecoveryRecord, Status, Task, TaskRecord};

/// The schema of a legacy task file with recovery evidence.
pub const RECOVERY_SCHEMA_VERSION: u32 = 3;
/// The schema of an ordinary legacy task file.
pub const BASE_SCHEMA_VERSION: u32 = 2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskEnvelope {
    schema_version: u32,
    task: Task,
    #[serde(default)]
    recoveries: Vec<RecoveryRecord>,
}

#[derive(Serialize)]
struct EnvelopeRef<'a> {
    schema_version: u32,
    task: &'a Task,
    #[serde(skip_serializing_if = "<[RecoveryRecord]>::is_empty")]
    recoveries: &'a [RecoveryRecord],
}

fn parse_envelope(yaml: &str) -> Result<TaskRecord> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(yaml).context("parsing task schema")?;
    if let Some(version) = value.get("schema_version") {
        ensure!(
            matches!(version.as_u64(), Some(2 | 3)),
            "unsupported task schema {version:?}; expected 2 or {RECOVERY_SCHEMA_VERSION}"
        );
        ensure!(
            version.as_u64() == Some(u64::from(RECOVERY_SCHEMA_VERSION))
                || value.get("recoveries").is_none(),
            "recovery records require task schema 3"
        );
        ensure!(
            value
                .get("task")
                .and_then(|t| t.get("recoveries"))
                .is_none(),
            "recovery records belong in the schema-3 envelope"
        );
        let envelope: TaskEnvelope =
            serde_yaml_ng::from_value(value).context("parsing versioned task")?;
        debug_assert!(matches!(envelope.schema_version, 2 | 3));
        let record = TaskRecord {
            task: envelope.task,
            recoveries: envelope.recoveries,
        };
        validate(&record)?;
        Ok(record)
    } else {
        ensure!(
            value.get("recoveries").is_none(),
            "legacy tasks cannot contain recovery records"
        );
        Ok(TaskRecord {
            task: serde_yaml_ng::from_value(value).context("parsing legacy task")?,
            recoveries: Vec::new(),
        })
    }
}

fn validate(record: &TaskRecord) -> Result<()> {
    let mut ids = BTreeSet::new();
    for recovery in &record.recoveries {
        recovery.validate()?;
        ensure!(
            recovery.preview.request.target.task == record.task.id,
            "recovery target does not match canonical task"
        );
        ensure!(ids.insert(&recovery.id), "duplicate recovery identity");
    }
    Ok(())
}

fn render_envelope(record: &TaskRecord, version: u32) -> Result<String> {
    validate(record)?;
    ensure!(
        matches!(version, 2 | 3),
        "unsupported task schema {version}"
    );
    ensure!(
        version == 3 || record.recoveries.is_empty(),
        "recovery records require task schema 3"
    );
    serde_yaml_ng::to_string(&EnvelopeRef {
        schema_version: version,
        task: &record.task,
        recoveries: &record.recoveries,
    })
    .context("serializing task envelope")
}

/// Parses a task file's text. Frontmatter is between a first line "---" and the next line "---".
pub fn parse_task(text: &str, path: &Path) -> Result<Task> {
    Ok(parse_task_record(text, path)?.task)
}

/// Parses the complete task envelope without losing recovery evidence.
///
/// # Errors
/// Refuses malformed frontmatter, unsupported schemas and invalid recovery records.
pub fn parse_task_record(text: &str, path: &Path) -> Result<crate::TaskRecord> {
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        bail!("{}: missing frontmatter start", path.display());
    }
    let mut yaml = String::new();
    let mut consumed = first.len();
    let mut closed = false;
    for line in lines {
        consumed += line.len();
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
    }
    if !closed {
        bail!("{}: unterminated frontmatter", path.display());
    }
    let mut record = parse_envelope(&yaml)
        .with_context(|| format!("parsing frontmatter of {}", path.display()))?;
    let task = &mut record.task;
    if task.origin.is_none() && task.status == Status::Idea {
        task.origin = Some(crate::Origin::Owner);
    }
    task.body = text[consumed..].to_string();
    task.path = path.to_path_buf();
    Ok(record)
}

/// Renders a task to file text: "---\n<yaml>---\n<body>" (body kept verbatim; ends with a newline).
pub fn render_task(task: &Task) -> String {
    render_record(
        &crate::TaskRecord {
            task: task.clone(),
            recoveries: Vec::new(),
        },
        BASE_SCHEMA_VERSION,
    )
    .unwrap_or_default()
}

/// Renders a complete record; recovery-bearing records always use schema 3.
///
/// # Errors
/// Refuses invalid recovery evidence or serialization failures.
pub fn render_task_record(record: &crate::TaskRecord) -> Result<String> {
    render_record(record, RECOVERY_SCHEMA_VERSION)
}

pub(crate) fn render_record(record: &crate::TaskRecord, version: u32) -> Result<String> {
    let yaml = render_envelope(record, version)?;
    let mut out = String::from("---\n");
    out.push_str(&yaml);
    if !yaml.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n");
    out.push_str(&record.task.body);
    if version == BASE_SCHEMA_VERSION
        && !record.task.body.is_empty()
        && !record.task.body.ends_with('\n')
    {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
